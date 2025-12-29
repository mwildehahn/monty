use crate::args::ArgValues;
use crate::evaluate::{EvalResult, EvaluateExpr};
use crate::exception::{exc_err_static, exc_fmt, ExcType, RawStackFrame, RunError, SimpleException};
use crate::expressions::{ExprLoc, Identifier, NameScope, Node};
use crate::for_iterator::ForIterator;
use crate::heap::{Heap, HeapData};
use crate::intern::{FunctionId, Interns, StringId, MODULE_STRING_ID};
use crate::io::PrintWriter;
use crate::namespace::{NamespaceId, Namespaces, GLOBAL_NS_IDX};
use crate::operators::Operator;
use crate::parse::CodeRange;
use crate::resource::ResourceTracker;
use crate::snapshot::{AbstractSnapshotTracker, ClauseState, FrameExit};
use crate::types::PyTrait;
use crate::value::Value;

/// Result type for runtime operations.
pub type RunResult<T> = Result<T, RunError>;

/// Represents an execution frame with an index into Namespaces.
///
/// At module level, `local_idx == GLOBAL_NS_IDX` (same namespace).
/// In functions, `local_idx` points to the function's local namespace.
/// Global variables always use `GLOBAL_NS_IDX` (0) directly.
///
/// # Closure Support
///
/// Cell variables (for closures) are stored directly in the namespace as
/// `Value::Ref(cell_id)` pointing to a `HeapData::Cell`. Both captured cells
/// (from enclosing scopes) and owned cells (for variables captured by nested
/// functions) are injected into the namespace at function call time.
///
/// When accessing a variable with `NameScope::Cell`, we look up the namespace
/// slot to get the `Value::Ref(cell_id)`, then read/write through that cell.
#[derive(Debug)]
pub struct RunFrame<'i, P: AbstractSnapshotTracker, W: PrintWriter> {
    /// Index of this frame's local namespace in Namespaces.
    local_idx: NamespaceId,
    /// The name of the current frame (function name or "<module>").
    /// Uses string id to lookup
    name: StringId,
    /// reference to interns
    interns: &'i Interns,
    /// reference to position tracker
    snapshot_tracker: &'i mut P,
    /// Writer for print output
    print: &'i mut W,
}

/// Extracts a value from `EvalResult`, returning early with `FrameExit::ExternalCall` if
/// an external call is pending.
///
/// Similar to `return_ext_call!` from evaluate.rs, but returns `Ok(Some(FrameExit::ExternalCall(...)))`
/// which is the appropriate return type for `execute_node` and related methods.
macro_rules! frame_ext_call {
    ($expr:expr) => {
        match $expr {
            EvalResult::Value(value) => value,
            EvalResult::ExternalCall(ext_call) => return Ok(Some(FrameExit::ExternalCall(ext_call))),
        }
    };
}

impl<'i, P: AbstractSnapshotTracker, W: PrintWriter> RunFrame<'i, P, W> {
    /// Creates a new frame for module-level execution.
    ///
    /// At module level, `local_idx` is `GLOBAL_NS_IDX` (0).
    pub fn module_frame(interns: &'i Interns, snapshot_tracker: &'i mut P, print: &'i mut W) -> Self {
        Self {
            local_idx: GLOBAL_NS_IDX,
            name: MODULE_STRING_ID,
            interns,
            snapshot_tracker,
            print,
        }
    }

    /// Creates a new frame for function execution.
    ///
    /// The function's local namespace is at `local_idx`. Global variables
    /// always use `GLOBAL_NS_IDX` directly.
    ///
    /// Cell variables (for closures) are already injected into the namespace
    /// by Function::call or Function::call_with_cells before this frame is created.
    ///
    /// # Arguments
    /// * `local_idx` - Index of the function's local namespace in Namespaces
    /// * `name` - The function name StringId (for error messages)
    /// * `snapshot_tracker` - Tracker for the current position in the code
    /// * `print` - Writer for print output
    pub fn function_frame(
        local_idx: NamespaceId,
        name: StringId,
        interns: &'i Interns,
        snapshot_tracker: &'i mut P,
        print: &'i mut W,
    ) -> Self {
        Self {
            local_idx,
            name,
            interns,
            snapshot_tracker,
            print,
        }
    }

    /// Executes all nodes in sequence, returning when a frame exit (return/yield) occurs.
    ///
    /// This will use `PositionTracker` to manage where in the block to resume execution.
    ///
    /// # Arguments
    /// * `namespaces` - The namespace stack
    /// * `heap` - The heap for allocations
    /// * `nodes` - The AST nodes to execute
    pub fn execute(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        nodes: &[Node],
    ) -> RunResult<Option<FrameExit>> {
        // The first position must be an Index - it tells us where to start in this block
        let position = self.snapshot_tracker.next();
        let start_index = position.index;
        let mut clause_state = position.clause_state;

        // execute from start_index
        for (i, node) in nodes.iter().enumerate().skip(start_index) {
            // External calls are returned as Ok(Some(FrameExit::ExternalCall(...))) from execute_node
            let exit_frame = self.execute_node(namespaces, heap, node, clause_state)?;
            if let Some(exit) = exit_frame {
                // Set the index of the node to execute on resume
                // we will have called set_skip() already if we need to skip the current node
                self.snapshot_tracker.record(i);
                return Ok(Some(exit));
            }
            clause_state = None;

            // if enabled, clear return values after executing each node
            if P::clear_return_values() {
                namespaces.clear_ext_return_values(heap);
            }
        }
        Ok(None)
    }

    /// Executes a single node, returning exit info with positions if execution should stop.
    ///
    /// Returns `Some(exit)` if the node caused a yield/return, where:
    /// - `exit` is the FrameExit (Yield or Return)
    /// - `positions` is the position stack within this node (empty for simple yields/returns)
    fn execute_node(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        node: &Node,
        clause_state: Option<ClauseState>,
    ) -> RunResult<Option<FrameExit>> {
        // Check time limit at statement boundaries
        heap.tracker().check_time().map_err(|e| {
            let frame = node.position().map(|pos| self.stack_frame(pos));
            RunError::UncatchableExc(e.into_exception(frame))
        })?;

        // Trigger garbage collection if scheduler says it's time.
        // GC runs at statement boundaries because:
        // 1. This is a natural pause point where we have access to GC roots
        // 2. The namespace state is stable (not mid-expression evaluation)
        // Note: GC won't run during long-running single expressions (e.g., large list
        // comprehensions). This is acceptable because most Python code is structured
        // as multiple statements, and resource limits (time, memory) still apply.
        if heap.tracker().should_gc() {
            heap.collect_garbage(|| namespaces.iter_heap_ids());
        }

        match node {
            Node::Expr(expr) => {
                match EvaluateExpr::new(namespaces, self.local_idx, heap, self.interns, self.print)
                    .evaluate_discard(expr)
                {
                    Ok(EvalResult::Value(())) => {}
                    Ok(EvalResult::ExternalCall(ext_call)) => return Ok(Some(FrameExit::ExternalCall(ext_call))),
                    Err(mut e) => {
                        add_frame_info(self.name, expr.position, &mut e);
                        return Err(e);
                    }
                }
            }
            Node::Return(expr) => {
                return self.execute_expr(namespaces, heap, expr).map(|result| match result {
                    EvalResult::Value(value) => Some(FrameExit::Return(value)),
                    EvalResult::ExternalCall(ext_call) => Some(FrameExit::ExternalCall(ext_call)),
                })
            }
            Node::ReturnNone => return Ok(Some(FrameExit::Return(Value::None))),
            Node::Raise(exc) => {
                if let Some(exit) = self.raise(namespaces, heap, exc.as_ref())? {
                    return Ok(Some(exit));
                }
            }
            Node::Assert { test, msg } => {
                if let Some(exit) = self.assert_(namespaces, heap, test, msg.as_ref())? {
                    return Ok(Some(exit));
                }
            }
            Node::Assign { target, object } => {
                if let Some(exit) = self.assign(namespaces, heap, target, object)? {
                    return Ok(Some(exit));
                }
            }
            Node::OpAssign { target, op, object } => {
                if let Some(exit) = self.op_assign(namespaces, heap, target, op, object)? {
                    return Ok(Some(exit));
                }
            }
            Node::SubscriptAssign { target, index, value } => {
                if let Some(exit) = self.subscript_assign(namespaces, heap, target, index, value)? {
                    return Ok(Some(exit));
                }
            }
            Node::For {
                target,
                iter,
                body,
                or_else,
            } => {
                if let Some(exit_frame) = self.for_(namespaces, heap, clause_state, target, iter, body, or_else)? {
                    return Ok(Some(exit_frame));
                }
            }
            Node::If { test, body, or_else } => {
                if let Some(exit_frame) = self.if_(namespaces, heap, clause_state, test, body, or_else)? {
                    return Ok(Some(exit_frame));
                }
            }
            Node::FunctionDef(function_id) => self.define_function(namespaces, heap, *function_id)?,
        }
        Ok(None)
    }

    /// Evaluates an expression and returns a Value.
    fn execute_expr(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        expr: &ExprLoc,
    ) -> RunResult<EvalResult<Value>> {
        match EvaluateExpr::new(namespaces, self.local_idx, heap, self.interns, self.print).evaluate_use(expr) {
            Ok(value) => Ok(value),
            Err(mut e) => {
                add_frame_info(self.name, expr.position, &mut e);
                Err(e)
            }
        }
    }

    fn execute_expr_bool(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        expr: &ExprLoc,
    ) -> RunResult<EvalResult<bool>> {
        match EvaluateExpr::new(namespaces, self.local_idx, heap, self.interns, self.print).evaluate_bool(expr) {
            Ok(value) => Ok(value),
            Err(mut e) => {
                add_frame_info(self.name, expr.position, &mut e);
                Err(e)
            }
        }
    }

    /// Executes a raise statement.
    ///
    /// Handles:
    /// * Exception instance (Value::Exc) - raise directly
    /// * Exception type (Value::Callable with ExcType) - instantiate then raise
    /// * Anything else - TypeError
    fn raise(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        op_exc_expr: Option<&ExprLoc>,
    ) -> RunResult<Option<FrameExit>> {
        if let Some(exc_expr) = op_exc_expr {
            let value = frame_ext_call!(self.execute_expr(namespaces, heap, exc_expr)?);
            match &value {
                Value::Exc(_) => {
                    // Match on the reference then use into_exc() due to issues with destructuring Value
                    let exc = value.into_exc();
                    // Use raise_frame so traceback won't show caret for raise statement
                    return Err(exc.with_frame(self.raise_frame(exc_expr.position)).into());
                }
                Value::Builtin(builtin) => {
                    // Callable is inline - call it to get the exception
                    let builtin = *builtin;
                    let result = builtin.call(heap, ArgValues::Empty, self.interns, self.print)?;
                    if matches!(&result, Value::Exc(_)) {
                        // No need to drop value - Callable is Copy and doesn't need cleanup
                        let exc = result.into_exc();
                        // Use raise_frame so traceback won't show caret for raise statement
                        return Err(exc.with_frame(self.raise_frame(exc_expr.position)).into());
                    }
                }
                _ => {}
            }
            value.drop_with_heap(heap);
            exc_err_static!(ExcType::TypeError; "exceptions must derive from BaseException")
        } else {
            Err(RunError::internal("plain raise not yet supported"))
        }
    }

    /// Executes an assert statement by evaluating the test expression and raising
    /// `AssertionError` if the test is falsy.
    ///
    /// If a message expression is provided, it is evaluated and used as the exception message.
    fn assert_(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        test: &ExprLoc,
        msg: Option<&ExprLoc>,
    ) -> RunResult<Option<FrameExit>> {
        let ok = frame_ext_call!(self.execute_expr_bool(namespaces, heap, test)?);
        if !ok {
            let msg = if let Some(msg_expr) = msg {
                let msg_value = frame_ext_call!(self.execute_expr(namespaces, heap, msg_expr)?);
                Some(msg_value.py_str(heap, self.interns).to_string())
            } else {
                None
            };
            return Err(SimpleException::new(ExcType::AssertionError, msg)
                .with_frame(self.stack_frame(test.position))
                .into());
        }
        Ok(None)
    }

    fn assign(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        target: &Identifier,
        expr: &ExprLoc,
    ) -> RunResult<Option<FrameExit>> {
        let new_value = frame_ext_call!(self.execute_expr(namespaces, heap, expr)?);

        // Determine which namespace to use
        let ns_idx = match target.scope {
            NameScope::Global => GLOBAL_NS_IDX,
            _ => self.local_idx, // Local and Cell both use local namespace
        };

        if target.scope == NameScope::Cell {
            // Cell assignment - look up cell HeapId from namespace slot, then write through it
            let namespace = namespaces.get_mut(ns_idx);
            let Value::Ref(cell_id) = namespace.get(target.namespace_id()) else {
                panic!("Cell variable slot doesn't contain a cell reference - prepare-time bug")
            };
            heap.set_cell_value(*cell_id, new_value);
        } else {
            // Direct assignment to namespace slot (Local or Global)
            let namespace = namespaces.get_mut(ns_idx);
            let old_value = std::mem::replace(namespace.get_mut(target.namespace_id()), new_value);
            old_value.drop_with_heap(heap);
        }
        Ok(None)
    }

    fn op_assign(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        target: &Identifier,
        op: &Operator,
        expr: &ExprLoc,
    ) -> RunResult<Option<FrameExit>> {
        let rhs = frame_ext_call!(self.execute_expr(namespaces, heap, expr)?);
        // Capture rhs type before it's consumed
        let rhs_type = rhs.py_type(Some(heap));

        // Cell variables need special handling - read through cell, modify, write back
        let err_target_type = if target.scope == NameScope::Cell {
            let namespace = namespaces.get_mut(self.local_idx);
            let Value::Ref(cell_id) = namespace.get(target.namespace_id()) else {
                panic!("Cell variable slot doesn't contain a cell reference - prepare-time bug")
            };
            let mut cell_value = heap.get_cell_value(*cell_id);
            // Capture type before potential drop
            let cell_value_type = cell_value.py_type(Some(heap));
            let result: RunResult<Option<Value>> = match op {
                // In-place add has special optimization for mutable types
                Operator::Add => {
                    let ok = cell_value.py_iadd(rhs, heap, None, self.interns)?;
                    if ok {
                        Ok(Some(cell_value))
                    } else {
                        Ok(None)
                    }
                }
                // For other operators, use binary op + replace
                Operator::Mult => {
                    let new_val = cell_value.py_mult(&rhs, heap, self.interns)?;
                    rhs.drop_with_heap(heap);
                    cell_value.drop_with_heap(heap);
                    Ok(new_val)
                }
                Operator::Div => {
                    let new_val = cell_value.py_div(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    cell_value.drop_with_heap(heap);
                    Ok(new_val)
                }
                Operator::FloorDiv => {
                    let new_val = cell_value.py_floordiv(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    cell_value.drop_with_heap(heap);
                    Ok(new_val)
                }
                Operator::Pow => {
                    let new_val = cell_value.py_pow(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    cell_value.drop_with_heap(heap);
                    Ok(new_val)
                }
                Operator::Sub => {
                    let new_val = cell_value.py_sub(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    cell_value.drop_with_heap(heap);
                    Ok(new_val)
                }
                Operator::Mod => {
                    let new_val = cell_value.py_mod(&rhs);
                    rhs.drop_with_heap(heap);
                    cell_value.drop_with_heap(heap);
                    Ok(new_val)
                }
                _ => return Err(RunError::internal("assign operator not yet implemented")),
            };
            match result? {
                Some(new_value) => {
                    heap.set_cell_value(*cell_id, new_value);
                    None
                }
                None => Some(cell_value_type),
            }
        } else {
            // Direct access for Local/Global scopes
            let target_val = namespaces.get_var_mut(self.local_idx, target, self.interns)?;
            let target_type = target_val.py_type(Some(heap));
            let result: RunResult<Option<()>> = match op {
                // In-place add has special optimization for mutable types
                Operator::Add => {
                    let ok = target_val.py_iadd(rhs, heap, None, self.interns)?;
                    if ok {
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                // For other operators, use binary op + replace
                Operator::Mult => {
                    let new_val = target_val.py_mult(&rhs, heap, self.interns)?;
                    rhs.drop_with_heap(heap);
                    if let Some(v) = new_val {
                        let old = std::mem::replace(target_val, v);
                        old.drop_with_heap(heap);
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                Operator::Div => {
                    let new_val = target_val.py_div(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    if let Some(v) = new_val {
                        let old = std::mem::replace(target_val, v);
                        old.drop_with_heap(heap);
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                Operator::FloorDiv => {
                    let new_val = target_val.py_floordiv(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    if let Some(v) = new_val {
                        let old = std::mem::replace(target_val, v);
                        old.drop_with_heap(heap);
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                Operator::Pow => {
                    let new_val = target_val.py_pow(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    if let Some(v) = new_val {
                        let old = std::mem::replace(target_val, v);
                        old.drop_with_heap(heap);
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                Operator::Sub => {
                    let new_val = target_val.py_sub(&rhs, heap)?;
                    rhs.drop_with_heap(heap);
                    if let Some(v) = new_val {
                        let old = std::mem::replace(target_val, v);
                        old.drop_with_heap(heap);
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                Operator::Mod => {
                    let new_val = target_val.py_mod(&rhs);
                    rhs.drop_with_heap(heap);
                    if let Some(v) = new_val {
                        let old = std::mem::replace(target_val, v);
                        old.drop_with_heap(heap);
                        Ok(Some(()))
                    } else {
                        Ok(None)
                    }
                }
                _ => return Err(RunError::internal("assign operator not yet implemented")),
            };
            match result? {
                Some(()) => None,
                None => Some(target_type),
            }
        };

        if let Some(target_type) = err_target_type {
            let e = SimpleException::augmented_assign_type_error(op, target_type, rhs_type);
            Err(e.with_frame(self.stack_frame(expr.position)).into())
        } else {
            Ok(None)
        }
    }

    fn subscript_assign(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        target: &Identifier,
        index: &ExprLoc,
        value: &ExprLoc,
    ) -> RunResult<Option<FrameExit>> {
        let key = frame_ext_call!(self.execute_expr(namespaces, heap, index)?);
        let val = frame_ext_call!(self.execute_expr(namespaces, heap, value)?);
        let target_val = namespaces.get_var_mut(self.local_idx, target, self.interns)?;
        if let Value::Ref(id) = target_val {
            let id = *id;
            heap.with_entry_mut(id, |heap, data| data.py_setitem(key, val, heap, self.interns))?;
            Ok(None)
        } else {
            let e = exc_fmt!(ExcType::TypeError; "'{}' object does not support item assignment", target_val.py_type(Some(heap)));
            Err(e.with_frame(self.stack_frame(index.position)).into())
        }
    }

    /// Executes a for loop, propagating any `FrameExit` (yield/return) from the body.
    ///
    /// Returns `Some(FrameExit)` if a yield or explicit return occurred in the body,
    /// `None` if the loop completed normally.
    ///
    /// Supports iteration over: Range, List, Tuple, Dict (keys), Str (chars), Bytes (ints).
    /// Uses `ForIterator` for unified iteration with index-based state for resumption.
    #[allow(clippy::too_many_arguments)]
    fn for_(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        clause_state: Option<ClauseState>,
        target: &Identifier,
        iter: &ExprLoc,
        body: &[Node],
        _or_else: &[Node],
    ) -> RunResult<Option<FrameExit>> {
        // Get the iterator from the snapshot state if it
        let mut for_iter = if let Some(ClauseState::For(for_iter)) = clause_state {
            for_iter
        } else {
            let iter_value = frame_ext_call!(self.execute_expr(namespaces, heap, iter)?);
            // Create ForIterator from value
            let for_iter = ForIterator::new(iter_value, heap, self.interns)?;

            // Same as below, clear ext_return_values after evaluating the loop value but before entering the body.
            // This ensures that when we resume with ClauseState::For (which skips re-evaluating
            // the condition), there are no stale return values from the condition evaluation.
            if P::clear_return_values() {
                namespaces.clear_ext_return_values(heap);
            }
            for_iter
        };

        let namespace_id = target.namespace_id();
        loop {
            let value = match for_iter.for_next(heap, self.interns) {
                Ok(Some(v)) => v,
                Ok(None) => break, // Iteration complete
                Err(e) => {
                    for_iter.drop_with_heap(heap);
                    // Add frame info for errors from for_next (e.g., set/dict mutation during iteration)
                    return Err(e.set_frame(self.stack_frame(iter.position)));
                }
            };

            // For loop target is always local scope - must drop old value properly
            let namespace = namespaces.get_mut(self.local_idx);
            let old_value = std::mem::replace(namespace.get_mut(namespace_id), value);
            old_value.drop_with_heap(heap);

            match self.execute(namespaces, heap, body) {
                Ok(Some(exit)) => {
                    // Decrement iterator so on resume for_next() returns the same value.
                    // The loop variable is already set, but we need the iterator at the
                    // correct position for potential re-iteration after the body completes.
                    for_iter.decr();
                    self.snapshot_tracker.set_clause_state(ClauseState::For(for_iter));
                    return Ok(Some(exit));
                }
                Ok(None) => {
                    // for_next() already advanced, continue to next iteration
                }
                Err(e) => {
                    for_iter.drop_with_heap(heap);
                    return Err(e);
                }
            }
        }

        // Drop the original iterable value after loop completes
        for_iter.drop_with_heap(heap);
        Ok(None)
    }

    /// Executes an if statement.
    ///
    /// Evaluates the test condition and executes the appropriate branch.
    /// Tracks return value consumption for proper resumption with external calls.
    fn if_(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        clause_state: Option<ClauseState>,
        test: &ExprLoc,
        body: &[Node],
        or_else: &[Node],
    ) -> RunResult<Option<FrameExit>> {
        let is_true = if let Some(ClauseState::If(resume_test)) = clause_state {
            resume_test
        } else {
            let test = frame_ext_call!(self.execute_expr_bool(namespaces, heap, test)?);
            // Clear ext_return_values after evaluating the condition but before entering the body.
            // This ensures that when we resume with ClauseState::If (which skips re-evaluating
            // the condition), there are no stale return values from the condition evaluation.
            // Only clear when actually evaluating a real condition (not using ClauseState::If).
            if P::clear_return_values() {
                namespaces.clear_ext_return_values(heap);
            }
            test
        };
        if is_true {
            if let Some(frame_exit) = self.execute(namespaces, heap, body)? {
                self.snapshot_tracker.set_clause_state(ClauseState::If(true));
                return Ok(Some(frame_exit));
            }
        } else if let Some(frame_exit) = self.execute(namespaces, heap, or_else)? {
            self.snapshot_tracker.set_clause_state(ClauseState::If(false));
            return Ok(Some(frame_exit));
        }
        Ok(None)
    }

    /// Defines a function (or closure) by storing it in the namespace.
    ///
    /// If the function has free_var_enclosing_slots (captures variables from enclosing scope),
    /// this captures the cells from the enclosing namespace and stores a Closure.
    /// If the function has default values, they are evaluated at definition time and stored.
    /// Otherwise, it stores a simple Function reference.
    ///
    /// # Cell Sharing
    ///
    /// Closures share cells with their enclosing scope. The cell HeapIds are
    /// looked up from the enclosing namespace slots specified in free_var_enclosing_slots.
    /// This ensures modifications through `nonlocal` are visible to both scopes.
    fn define_function(
        &mut self,
        namespaces: &mut Namespaces,
        heap: &mut Heap<impl ResourceTracker>,
        function_id: FunctionId,
    ) -> RunResult<()> {
        let function = self.interns.get_function(function_id);

        // Evaluate default expressions at definition time
        // These are evaluated in the enclosing scope (not the function's own scope)
        let defaults = if function.has_defaults() {
            let mut defaults = Vec::with_capacity(function.default_exprs.len());
            for expr in &function.default_exprs {
                match self.execute_expr(namespaces, heap, expr) {
                    Ok(EvalResult::Value(value)) => defaults.push(value),
                    Ok(EvalResult::ExternalCall(_)) => {
                        // External calls in default expressions are not supported
                        for value in defaults.drain(..) {
                            value.drop_with_heap(heap);
                        }
                        return Err(ExcType::not_implemented(
                            "external function calls in default parameter expressions",
                        )
                        .into());
                    }
                    Err(err) => {
                        for value in defaults.drain(..) {
                            value.drop_with_heap(heap);
                        }
                        return Err(err);
                    }
                }
            }
            defaults
        } else {
            Vec::new()
        };

        let new_value = if function.is_closure() {
            // This function captures variables from enclosing scopes.
            // Look up the cell HeapIds from the enclosing namespace.
            let enclosing_namespace = namespaces.get(self.local_idx);
            let mut captured_cells = Vec::with_capacity(function.free_var_enclosing_slots.len());

            for &enclosing_slot in &function.free_var_enclosing_slots {
                // The enclosing namespace slot contains Value::Ref(cell_id)
                let Value::Ref(cell_id) = enclosing_namespace.get(enclosing_slot) else {
                    panic!("Expected cell in enclosing namespace slot {enclosing_slot:?} - prepare-time bug")
                };

                // Increment the cell's refcount since this closure now holds a reference
                heap.inc_ref(*cell_id);
                captured_cells.push(*cell_id);
            }

            Value::Ref(heap.allocate(HeapData::Closure(function_id, captured_cells, defaults))?)
        } else if !defaults.is_empty() {
            // Non-closure function with defaults needs heap allocation
            Value::Ref(heap.allocate(HeapData::FunctionDefaults(function_id, defaults))?)
        } else {
            // Simple function without captures or defaults
            Value::Function(function_id)
        };

        let namespace = namespaces.get_mut(self.local_idx);
        let old_value = std::mem::replace(namespace.get_mut(function.name.namespace_id()), new_value);
        // Drop the old value properly (dec_ref for Refs, no-op for others)
        old_value.drop_with_heap(heap);
        Ok(())
    }

    fn stack_frame(&self, position: CodeRange) -> RawStackFrame {
        // Create frame without parent - the parent chain is built up by add_frame_info()
        // as the error propagates through the call stack
        RawStackFrame::new(position, self.name, None)
    }

    /// Creates a stack frame for a raise statement (no caret shown in traceback).
    fn raise_frame(&self, position: CodeRange) -> RawStackFrame {
        RawStackFrame::from_raise(position, self.name)
    }
}

/// Adds the caller's frame to an error as it propagates up the call stack.
///
/// This builds the traceback chain by appending each caller's frame information
/// to the exception, so the full call stack is visible when the error is displayed.
fn add_frame_info(name: StringId, position: CodeRange, error: &mut RunError) {
    match error {
        RunError::Exc(ref mut exc) | RunError::UncatchableExc(ref mut exc) => {
            exc.add_caller_frame(position, name);
        }
        RunError::Internal(_) => {}
    }
}
