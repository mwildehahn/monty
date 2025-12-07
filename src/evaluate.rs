use std::cmp::Ordering;

use crate::args::{ArgExprs, ArgValues};
use crate::exceptions::{internal_err, InternalRunError, SimpleException};
use crate::expressions::{Expr, ExprLoc, Identifier};
use crate::heap::{Heap, HeapData};
use crate::namespace::Namespaces;
use crate::operators::{CmpOperator, Operator};
use crate::run::RunResult;
use crate::value::{Attr, Value};
use crate::values::{Dict, List, PyTrait};

/// Evaluates an expression node and returns a value.
///
/// # Arguments
/// * `namespaces` - The namespace namespaces containing all namespaces
/// * `local_idx` - Index of the local namespace in namespaces
/// * `heap` - The heap for allocating objects
/// * `expr_loc` - The expression to evaluate
pub(crate) fn evaluate_use<'c, 'e>(
    namespaces: &mut Namespaces<'c, 'e>,
    local_idx: usize,
    heap: &mut Heap<'c, 'e>,
    expr_loc: &'e ExprLoc<'c>,
) -> RunResult<'c, Value<'c, 'e>> {
    match &expr_loc.expr {
        Expr::Literal(literal) => Ok(literal.to_value()),
        Expr::Callable(callable) => Ok(callable.to_value()),
        Expr::Name(ident) => namespaces
            .get_var_mut(local_idx, ident)
            .map(|object| object.clone_with_heap(heap)),
        Expr::Call { callable, args } => {
            let args = evaluate_args(namespaces, local_idx, heap, args)?;
            callable.call(namespaces, local_idx, heap, args)
        }
        Expr::AttrCall { object, attr, args } => Ok(attr_call(namespaces, local_idx, heap, object, attr, args)?),
        Expr::Op { left, op, right } => match op {
            // Handle boolean operators with short-circuit evaluation.
            // These return the actual operand value, not a boolean.
            Operator::And => eval_and(namespaces, local_idx, heap, left, right),
            Operator::Or => eval_or(namespaces, local_idx, heap, left, right),
            _ => eval_op(namespaces, local_idx, heap, left, op, right),
        },
        Expr::CmpOp { left, op, right } => Ok(cmp_op(namespaces, local_idx, heap, left, op, right)?.into()),
        Expr::List(elements) => {
            let values = elements
                .iter()
                .map(|e| evaluate_use(namespaces, local_idx, heap, e))
                .collect::<RunResult<_>>()?;
            let heap_id = heap.allocate(HeapData::List(List::new(values)));
            Ok(Value::Ref(heap_id))
        }
        Expr::Tuple(elements) => {
            let values = elements
                .iter()
                .map(|e| evaluate_use(namespaces, local_idx, heap, e))
                .collect::<RunResult<_>>()?;
            let heap_id = heap.allocate(HeapData::Tuple(values));
            Ok(Value::Ref(heap_id))
        }
        Expr::Subscript { object, index } => {
            let obj = evaluate_use(namespaces, local_idx, heap, object)?;
            let key = evaluate_use(namespaces, local_idx, heap, index)?;
            let result = obj.py_getitem(&key, heap);
            // Drop temporary references to object and key
            obj.drop_with_heap(heap);
            key.drop_with_heap(heap);
            result
        }
        Expr::Dict(pairs) => {
            let mut eval_pairs = Vec::new();
            for (key_expr, value_expr) in pairs {
                let key = evaluate_use(namespaces, local_idx, heap, key_expr)?;
                let value = evaluate_use(namespaces, local_idx, heap, value_expr)?;
                eval_pairs.push((key, value));
            }
            let dict = Dict::from_pairs(eval_pairs, heap)?;
            let dict_id = heap.allocate(HeapData::Dict(dict));
            Ok(Value::Ref(dict_id))
        }
        Expr::Not(operand) => {
            let val = evaluate_use(namespaces, local_idx, heap, operand)?;
            let result = !val.py_bool(heap);
            val.drop_with_heap(heap);
            Ok(Value::Bool(result))
        }
    }
}

/// Evaluates an expression node and discard the returned value.
///
/// # Arguments
/// * `namespaces` - The namespace namespaces containing all namespaces
/// * `local_idx` - Index of the local namespace in namespaces
/// * `heap` - The heap for allocating objects
/// * `expr_loc` - The expression to evaluate
pub(crate) fn evaluate_discard<'c, 'e>(
    namespaces: &mut Namespaces<'c, 'e>,
    local_idx: usize,
    heap: &mut Heap<'c, 'e>,
    expr_loc: &'e ExprLoc<'c>,
) -> RunResult<'c, ()> {
    match &expr_loc.expr {
        // TODO, is this right for callable?
        Expr::Literal(_) | Expr::Callable(_) => Ok(()),
        Expr::Name(ident) => namespaces.get_var_mut(local_idx, ident).map(|_| ()),
        Expr::Call { callable, args } => {
            let args = evaluate_args(namespaces, local_idx, heap, args)?;
            let result = callable.call(namespaces, local_idx, heap, args)?;
            result.drop_with_heap(heap);
            Ok(())
        }
        Expr::AttrCall { object, attr, args } => {
            let result = attr_call(namespaces, local_idx, heap, object, attr, args)?;
            result.drop_with_heap(heap);
            Ok(())
        }
        Expr::Op { left, op, right } => {
            // Handle and/or with short-circuit evaluation
            let result = match op {
                Operator::And => eval_and(namespaces, local_idx, heap, left, right)?,
                Operator::Or => eval_or(namespaces, local_idx, heap, left, right)?,
                _ => eval_op(namespaces, local_idx, heap, left, op, right)?,
            };
            result.drop_with_heap(heap);
            Ok(())
        }
        Expr::CmpOp { left, op, right } => cmp_op(namespaces, local_idx, heap, left, op, right).map(|_| ()),
        Expr::List(elements) => {
            for el in elements {
                evaluate_discard(namespaces, local_idx, heap, el)?;
            }
            Ok(())
        }
        Expr::Tuple(elements) => {
            for el in elements {
                evaluate_discard(namespaces, local_idx, heap, el)?;
            }
            Ok(())
        }
        Expr::Subscript { object, index } => {
            evaluate_discard(namespaces, local_idx, heap, object)?;
            evaluate_discard(namespaces, local_idx, heap, index)?;
            Ok(())
        }
        Expr::Dict(pairs) => {
            for (key_expr, value_expr) in pairs {
                evaluate_discard(namespaces, local_idx, heap, key_expr)?;
                evaluate_discard(namespaces, local_idx, heap, value_expr)?;
            }
            Ok(())
        }
        Expr::Not(operand) => {
            evaluate_discard(namespaces, local_idx, heap, operand)?;
            Ok(())
        }
    }
}

/// Specialized helper for truthiness checks; shares implementation with `evaluate`.
pub(crate) fn evaluate_bool<'c, 'e>(
    namespaces: &mut Namespaces<'c, 'e>,
    local_idx: usize,
    heap: &mut Heap<'c, 'e>,
    expr_loc: &'e ExprLoc<'c>,
) -> RunResult<'c, bool> {
    match &expr_loc.expr {
        Expr::CmpOp { left, op, right } => cmp_op(namespaces, local_idx, heap, left, op, right),
        // Optimize `not` to avoid creating intermediate Value::Bool
        Expr::Not(operand) => {
            let val = evaluate_use(namespaces, local_idx, heap, operand)?;
            let result = !val.py_bool(heap);
            val.drop_with_heap(heap);
            Ok(result)
        }
        // Optimize `and`/`or` with short-circuit and direct boolean conversion
        Expr::Op { left, op, right } if matches!(op, Operator::And | Operator::Or) => {
            let result = match op {
                Operator::And => eval_and(namespaces, local_idx, heap, left, right)?,
                Operator::Or => eval_or(namespaces, local_idx, heap, left, right)?,
                _ => unreachable!(),
            };
            let bool_result = result.py_bool(heap);
            result.drop_with_heap(heap);
            Ok(bool_result)
        }
        _ => {
            let obj = evaluate_use(namespaces, local_idx, heap, expr_loc)?;
            let result = obj.py_bool(heap);
            // Drop temporary reference
            obj.drop_with_heap(heap);
            Ok(result)
        }
    }
}

/// Evaluates a binary operator expression (`+, -, %`, etc.).
fn eval_op<'c, 'e>(
    namespaces: &mut Namespaces<'c, 'e>,
    local_idx: usize,
    heap: &mut Heap<'c, 'e>,
    left: &'e ExprLoc<'c>,
    op: &Operator,
    right: &'e ExprLoc<'c>,
) -> RunResult<'c, Value<'c, 'e>> {
    let lhs = evaluate_use(namespaces, local_idx, heap, left)?;
    let rhs = evaluate_use(namespaces, local_idx, heap, right)?;
    let op_result: Option<Value> = match op {
        Operator::Add => lhs.py_add(&rhs, heap),
        Operator::Sub => lhs.py_sub(&rhs, heap),
        Operator::Mod => lhs.py_mod(&rhs),
        _ => {
            // Drop temporary references before early return
            lhs.drop_with_heap(heap);
            rhs.drop_with_heap(heap);
            return internal_err!(InternalRunError::TodoError; "Operator {op:?} not yet implemented");
        }
    };
    if let Some(object) = op_result {
        // Drop temporary references to operands now that the operation is complete
        lhs.drop_with_heap(heap);
        rhs.drop_with_heap(heap);
        Ok(object)
    } else {
        let left_type = lhs.py_type(heap);
        let right_type = rhs.py_type(heap);
        lhs.drop_with_heap(heap);
        rhs.drop_with_heap(heap);
        SimpleException::operand_type_error(left, op, right, left_type, right_type)
    }
}

/// Evaluates the `and` operator with short-circuit evaluation.
///
/// Python's `and` operator returns the first falsy operand, or the last operand if all are truthy.
/// For example: `5 and 3` returns `3`, while `0 and 3` returns `0`.
fn eval_and<'c, 'e>(
    namespaces: &mut Namespaces<'c, 'e>,
    local_idx: usize,
    heap: &mut Heap<'c, 'e>,
    left: &'e ExprLoc<'c>,
    right: &'e ExprLoc<'c>,
) -> RunResult<'c, Value<'c, 'e>> {
    let left_val = evaluate_use(namespaces, local_idx, heap, left)?;
    if left_val.py_bool(heap) {
        // Left is truthy, drop it and return right
        left_val.drop_with_heap(heap);
        evaluate_use(namespaces, local_idx, heap, right)
    } else {
        // Short-circuit: return left if falsy
        Ok(left_val)
    }
}

/// Evaluates the `or` operator with short-circuit evaluation.
///
/// Python's `or` operator returns the first truthy operand, or the last operand if all are falsy.
/// For example: `5 or 3` returns `5`, while `0 or 3` returns `3`.
fn eval_or<'c, 'e>(
    namespaces: &mut Namespaces<'c, 'e>,
    local_idx: usize,
    heap: &mut Heap<'c, 'e>,
    left: &'e ExprLoc<'c>,
    right: &'e ExprLoc<'c>,
) -> RunResult<'c, Value<'c, 'e>> {
    let left_val = evaluate_use(namespaces, local_idx, heap, left)?;
    if left_val.py_bool(heap) {
        // Short-circuit: return left if truthy
        Ok(left_val)
    } else {
        // Left is falsy, drop it and return right
        left_val.drop_with_heap(heap);
        evaluate_use(namespaces, local_idx, heap, right)
    }
}

/// Evaluates comparison operators, reusing `evaluate` so heap semantics remain consistent.
fn cmp_op<'c, 'e>(
    namespaces: &mut Namespaces<'c, 'e>,
    local_idx: usize,
    heap: &mut Heap<'c, 'e>,
    left: &'e ExprLoc<'c>,
    op: &CmpOperator,
    right: &'e ExprLoc<'c>,
) -> RunResult<'c, bool> {
    let lhs = evaluate_use(namespaces, local_idx, heap, left)?;
    let rhs = evaluate_use(namespaces, local_idx, heap, right)?;

    let result = match op {
        CmpOperator::Eq => Some(lhs.py_eq(&rhs, heap)),
        CmpOperator::NotEq => Some(!lhs.py_eq(&rhs, heap)),
        CmpOperator::Gt => lhs.py_cmp(&rhs, heap).map(Ordering::is_gt),
        CmpOperator::GtE => lhs.py_cmp(&rhs, heap).map(Ordering::is_ge),
        CmpOperator::Lt => lhs.py_cmp(&rhs, heap).map(Ordering::is_lt),
        CmpOperator::LtE => lhs.py_cmp(&rhs, heap).map(Ordering::is_le),
        CmpOperator::Is => Some(lhs.is(&rhs)),
        CmpOperator::IsNot => Some(!lhs.is(&rhs)),
        CmpOperator::ModEq(v) => lhs.py_mod_eq(&rhs, *v),
        _ => None,
    };

    if let Some(v) = result {
        lhs.drop_with_heap(heap);
        rhs.drop_with_heap(heap);
        Ok(v)
    } else {
        let left_type = lhs.py_type(heap);
        let right_type = rhs.py_type(heap);
        lhs.drop_with_heap(heap);
        rhs.drop_with_heap(heap);
        SimpleException::cmp_type_error(left, op, right, left_type, right_type)
    }
}

/// Handles attribute method calls like `list.append`, again threading the heap for safety.
fn attr_call<'c, 'e>(
    namespaces: &mut Namespaces<'c, 'e>,
    local_idx: usize,
    heap: &mut Heap<'c, 'e>,
    object_ident: &Identifier<'c>,
    attr: &Attr,
    args: &'e ArgExprs<'c>,
) -> RunResult<'c, Value<'c, 'e>> {
    // Evaluate arguments first to avoid borrow conflicts
    let args = evaluate_args(namespaces, local_idx, heap, args)?;

    let object = namespaces.get_var_mut(local_idx, object_ident)?;
    object.call_attr(heap, attr, args)
}

/// Evaluates function arguments into an Args, optimized for common argument counts.
fn evaluate_args<'c, 'e>(
    namespaces: &mut Namespaces<'c, 'e>,
    local_idx: usize,
    heap: &mut Heap<'c, 'e>,
    args_expr: &'e ArgExprs<'c>,
) -> RunResult<'c, ArgValues<'c, 'e>> {
    match args_expr {
        ArgExprs::Zero => Ok(ArgValues::Zero),
        ArgExprs::One(arg) => evaluate_use(namespaces, local_idx, heap, arg).map(ArgValues::One),
        ArgExprs::Two(arg1, arg2) => {
            let arg0 = evaluate_use(namespaces, local_idx, heap, arg1)?;
            let arg1 = evaluate_use(namespaces, local_idx, heap, arg2)?;
            Ok(ArgValues::Two(arg0, arg1))
        }
        ArgExprs::Args(args) => args
            .iter()
            .map(|a| evaluate_use(namespaces, local_idx, heap, a))
            .collect::<RunResult<_>>()
            .map(ArgValues::Many),
        _ => todo!("Implement evaluation for kwargs"),
    }
}
