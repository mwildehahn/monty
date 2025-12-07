use std::collections::hash_map::Entry;

use ahash::{AHashMap, AHashSet};

use crate::args::ArgExprs;
use crate::callable::Callable;
use crate::exceptions::{ExcType, ExceptionRaise, SimpleException};
use crate::expressions::{Expr, ExprLoc, Identifier, Literal, NameScope, Node};
use crate::function::Function;
use crate::operators::{CmpOperator, Operator};
use crate::parse::ParseNode;
use crate::parse_error::ParseError;

/// Result of the prepare phase, containing everything needed to execute code.
///
/// This struct holds the outputs of name resolution and AST transformation:
/// - The initial namespace with placeholder values for each variable
/// - A mapping from variable names to their namespace indices (for ref-count testing)
/// - The transformed AST nodes ready for execution
pub(crate) struct PrepareResult<'c> {
    /// Number of items in the namespace (at module level, this IS the global namespace)
    pub namespace_size: usize,
    /// Maps variable names to their indices in the namespace.
    /// Used for ref-count testing to look up variables by name.
    /// Only available when the `ref-counting` feature is enabled.
    #[cfg(feature = "ref-counting")]
    pub name_map: AHashMap<String, usize>,
    /// The prepared AST nodes with all names resolved to namespace indices.
    pub nodes: Vec<Node<'c>>,
}

/// Prepares parsed nodes for execution by resolving names and building the initial namespace.
///
/// The namespace will be converted to runtime Objects when execution begins and the heap is available.
/// At module level, the local namespace IS the global namespace.
pub(crate) fn prepare<'c>(
    nodes: Vec<ParseNode<'c>>,
    input_names: &[&str],
) -> Result<PrepareResult<'c>, ParseError<'c>> {
    let mut p = Prepare::new_module(nodes.len(), input_names);
    let prepared_nodes = p.prepare_nodes(nodes)?;
    Ok(PrepareResult {
        namespace_size: p.namespace_size,
        #[cfg(feature = "ref-counting")]
        name_map: p.name_map,
        nodes: prepared_nodes,
    })
}

/// State machine for the preparation phase that transforms parsed AST nodes into an executable form.
///
/// This struct maintains the mapping between variable names and their namespace indices,
/// builds the initial namespace with Literals (pre-runtime), and handles scope resolution.
/// The preparation phase is crucial for converting string-based name lookups into efficient
/// integer-indexed namespace access during execution.
///
/// For functions, this struct also tracks:
/// - Which variables are declared `global` (should resolve to module namespace)
/// - Which variables are assigned locally (determines local vs global scope)
/// - Reference to the global name map for resolving global variable references
struct Prepare {
    /// Maps variable names to their indices in this scope's namespace vector
    name_map: AHashMap<String, usize>,
    /// Number of items in the namespace
    pub namespace_size: usize,
    /// Root frame is the outer frame of the script, e.g. the "global" scope.
    /// When true, the last expression in a block is implicitly returned.
    root_frame: bool,
    /// Whether this is the module-level scope.
    /// At module level, all variables are global and `global` keyword is a no-op.
    is_module_scope: bool,
    /// Names declared as `global` in this scope.
    /// These names will resolve to the global namespace instead of local.
    global_names: AHashSet<String>,
    /// Names that are assigned in this scope (from first-pass scan).
    /// Used in functions to determine if a variable is local (assigned) or global (only read).
    assigned_names: AHashSet<String>,
    /// Names that have been assigned so far during the second pass (in order).
    /// Used to produce the correct error message for `global x` when x was assigned before.
    names_assigned_in_order: AHashSet<String>,
    /// Copy of the module-level global name map.
    /// Used by functions to resolve global variable references.
    /// None at module level (not needed since all names are global there).
    global_name_map: Option<AHashMap<String, usize>>,
}

impl Prepare {
    /// Creates a new Prepare instance for module-level code.
    ///
    /// At module level, all variables are global. The `global` keyword is a no-op
    /// since all variables are already in the global namespace.
    ///
    /// # Arguments
    /// * `capacity` - Expected number of nodes, used to preallocate the name map
    /// * `input_names` - Names that should be pre-registered in the namespace (e.g., external variables)
    fn new_module(capacity: usize, input_names: &[&str]) -> Self {
        let mut name_map = AHashMap::with_capacity(capacity);
        for (index, name) in input_names.iter().enumerate() {
            name_map.insert((*name).to_string(), index);
        }
        let namespace_size = name_map.len();
        Self {
            name_map,
            namespace_size,
            root_frame: true,
            is_module_scope: true,
            global_names: AHashSet::new(),
            assigned_names: AHashSet::new(),
            names_assigned_in_order: AHashSet::new(),
            global_name_map: None,
        }
    }

    /// Creates a new Prepare instance for function-level code.
    ///
    /// # Arguments
    /// * `capacity` - Expected number of nodes
    /// * `params` - Function parameter names (pre-registered in namespace)
    /// * `assigned_names` - Names that are assigned in this function (from first-pass scan)
    /// * `global_names` - Names declared as `global` in this function
    /// * `global_name_map` - Copy of the module-level name map for global resolution
    fn new_function(
        capacity: usize,
        params: &[&str],
        assigned_names: AHashSet<String>,
        global_names: AHashSet<String>,
        global_name_map: AHashMap<String, usize>,
    ) -> Self {
        let mut name_map = AHashMap::with_capacity(capacity);
        for (index, name) in params.iter().enumerate() {
            name_map.insert((*name).to_string(), index);
        }
        let namespace_size = name_map.len();
        Self {
            name_map,
            namespace_size,
            root_frame: false,
            is_module_scope: false,
            global_names,
            assigned_names,
            names_assigned_in_order: AHashSet::new(),
            global_name_map: Some(global_name_map),
        }
    }

    /// Recursively prepares a sequence of AST nodes by resolving names and transforming expressions.
    ///
    /// This method processes each node type differently:
    /// - Resolves variable names to namespace indices
    /// - Transforms function calls from identifier-based to builtin type-based
    /// - Handles special cases like implicit returns in root frames
    /// - Validates that names used in attribute calls are already defined
    ///
    /// # Returns
    /// A vector of prepared nodes ready for execution
    fn prepare_nodes<'c>(&mut self, nodes: Vec<ParseNode<'c>>) -> Result<Vec<Node<'c>>, ParseError<'c>> {
        let nodes_len = nodes.len();
        let mut new_nodes = Vec::with_capacity(nodes_len);
        for (index, node) in nodes.into_iter().enumerate() {
            match node {
                ParseNode::Pass => (),
                ParseNode::Expr(expr) => {
                    let expr = self.prepare_expression(expr)?;
                    // In the root frame (global scope), the last expression is implicitly returned
                    // if it's not None. This matches Python REPL behavior where the last expression
                    // value is displayed/returned.
                    if self.root_frame && index == nodes_len - 1 && !expr.expr.is_none() {
                        new_nodes.push(Node::Return(expr));
                    } else {
                        new_nodes.push(Node::Expr(expr));
                    }
                }
                ParseNode::Return(expr) => {
                    let expr = self.prepare_expression(expr)?;
                    new_nodes.push(Node::Return(expr));
                }
                ParseNode::ReturnNone => new_nodes.push(Node::ReturnNone),
                ParseNode::Raise(exc) => {
                    let expr = match exc {
                        Some(expr) => {
                            match expr.expr {
                                // Handle raising an exception type constant without instantiation,
                                // e.g. `raise TypeError`. This is transformed into a call: `raise TypeError()`
                                // so the exception is properly instantiated before being raised.
                                Expr::Callable(Callable::ExcType(exc_type)) => {
                                    let call_expr = Expr::Call {
                                        callable: Callable::ExcType(exc_type),
                                        args: ArgExprs::Zero,
                                    };
                                    Some(ExprLoc::new(expr.position, call_expr))
                                }
                                // Handle raising a builtin constant (unlikely but consistent)
                                Expr::Callable(Callable::Builtin(builtin)) => {
                                    let call_expr = Expr::Call {
                                        callable: Callable::Builtin(builtin),
                                        args: ArgExprs::Zero,
                                    };
                                    Some(ExprLoc::new(expr.position, call_expr))
                                }
                                Expr::Name(id) => {
                                    // Handle raising a variable - could be an exception type or instance.
                                    // The runtime will determine whether to call it (type) or raise it directly (instance).
                                    let position = id.position;
                                    let (resolved_id, is_new) = self.get_id(id);
                                    if is_new {
                                        let exc: ExceptionRaise =
                                            SimpleException::new(ExcType::NameError, Some(resolved_id.name.into()))
                                                .into();
                                        return Err(exc.into());
                                    }
                                    Some(ExprLoc::new(position, Expr::Name(resolved_id)))
                                }
                                _ => Some(self.prepare_expression(expr)?),
                            }
                        }
                        None => None,
                    };
                    new_nodes.push(Node::Raise(expr));
                }
                ParseNode::Assert { test, msg } => {
                    let test = self.prepare_expression(test)?;
                    let msg = match msg {
                        Some(m) => Some(self.prepare_expression(m)?),
                        None => None,
                    };
                    new_nodes.push(Node::Assert { test, msg });
                }
                ParseNode::Assign { target, object } => {
                    let object = self.prepare_expression(object)?;
                    // Track that this name was assigned before we call get_id
                    self.names_assigned_in_order.insert(target.name.to_string());
                    let (target, _) = self.get_id(target);
                    new_nodes.push(Node::Assign { target, object });
                }
                ParseNode::OpAssign { target, op, object } => {
                    // Track that this name was assigned
                    self.names_assigned_in_order.insert(target.name.to_string());
                    let target = self.get_id(target).0;
                    let object = self.prepare_expression(object)?;
                    new_nodes.push(Node::OpAssign { target, op, object });
                }
                ParseNode::SubscriptAssign { target, index, value } => {
                    // SubscriptAssign doesn't assign to the target itself, just modifies it
                    let target = self.get_id(target).0;
                    let index = self.prepare_expression(index)?;
                    let value = self.prepare_expression(value)?;
                    new_nodes.push(Node::SubscriptAssign { target, index, value });
                }
                ParseNode::For {
                    target,
                    iter,
                    body,
                    or_else,
                } => {
                    // Track that the loop variable is assigned
                    self.names_assigned_in_order.insert(target.name.to_string());
                    new_nodes.push(Node::For {
                        target: self.get_id(target).0,
                        iter: self.prepare_expression(iter)?,
                        body: self.prepare_nodes(body)?,
                        or_else: self.prepare_nodes(or_else)?,
                    });
                }
                ParseNode::If { test, body, or_else } => {
                    let test = self.prepare_expression(test)?;
                    let body = self.prepare_nodes(body)?;
                    let or_else = self.prepare_nodes(or_else)?;
                    new_nodes.push(Node::If { test, body, or_else });
                }
                ParseNode::FunctionDef { name, params, body } => {
                    let func_node = self.prepare_function_def(name, params, body)?;
                    new_nodes.push(func_node);
                }
                ParseNode::Global(names) => {
                    // At module level, `global` is a no-op since all variables are already global.
                    // In functions, the global declarations are already collected in the first pass
                    // (see prepare_function_def), so this is also a no-op at this point.
                    // The actual effect happens in get_id where we check global_names.
                    if !self.is_module_scope {
                        // Validate that names weren't already used/assigned before `global` declaration
                        for name in names {
                            let name_str = name.to_string();
                            if self.names_assigned_in_order.contains(&name_str) {
                                // Name was assigned before the global declaration
                                let exc: ExceptionRaise = ExcType::syntax_error_assigned_before_global(name).into();
                                return Err(exc.into());
                            } else if self.name_map.contains_key(&name_str) {
                                // Name was used (but not assigned) before the global declaration
                                let exc: ExceptionRaise = ExcType::syntax_error_used_before_global(name).into();
                                return Err(exc.into());
                            }
                        }
                    }
                    // Global statements don't produce any runtime nodes
                }
            }
        }
        Ok(new_nodes)
    }

    /// Prepares an expression by resolving names, transforming calls, and applying optimizations.
    ///
    /// Key transformations performed:
    /// - Name lookups are resolved to namespace indices via `get_id`
    /// - Function calls are resolved from identifiers to builtin types
    /// - Attribute calls validate that the object is already defined (not a new name)
    /// - Lists and tuples are recursively prepared
    /// - Modulo equality patterns like `x % n == k` (constant right-hand side) are optimized to
    ///   `CmpOperator::ModEq`
    ///
    /// # Errors
    /// Returns a NameError if an attribute call references an undefined variable
    fn prepare_expression<'c>(&mut self, loc_expr: ExprLoc<'c>) -> Result<ExprLoc<'c>, ParseError<'c>> {
        let ExprLoc { position, expr } = loc_expr;
        let expr = match expr {
            Expr::Literal(object) => Expr::Literal(object),
            Expr::Callable(callable) => Expr::Callable(callable),
            Expr::Name(name) => Expr::Name(self.get_id(name).0),
            Expr::Op { left, op, right } => Expr::Op {
                left: Box::new(self.prepare_expression(*left)?),
                op,
                right: Box::new(self.prepare_expression(*right)?),
            },
            Expr::CmpOp { left, op, right } => Expr::CmpOp {
                left: Box::new(self.prepare_expression(*left)?),
                op,
                right: Box::new(self.prepare_expression(*right)?),
            },
            Expr::Call { callable, mut args } => {
                // Prepare the arguments
                args.prepare_args(|expr| self.prepare_expression(expr))?;
                // For Name callables, resolve the identifier in the namespace
                let callable = match callable {
                    Callable::Name(ident) => {
                        let (resolved_ident, is_new) = self.get_id(ident);
                        // Unlike regular name lookups, calling requires the name to already exist.
                        // Calling an undefined variable should fail at prepare-time, not runtime.
                        if is_new {
                            let exc: ExceptionRaise =
                                SimpleException::new(ExcType::NameError, Some(resolved_ident.name.to_owned().into()))
                                    .into();
                            return Err(exc.into());
                        }
                        Callable::Name(resolved_ident)
                    }
                    // Builtins and ExcTypes are already resolved at parse time
                    other => other,
                };
                Expr::Call { callable, args }
            }
            Expr::AttrCall { object, attr, mut args } => {
                let (object, is_new) = self.get_id(object);
                // Unlike regular name lookups, attribute calls require the object to already exist.
                // Calling a method on an undefined variable should fail at prepare-time, not runtime.
                // Example: `undefined_var.method()` should raise NameError here.
                if is_new {
                    let exc: ExceptionRaise =
                        SimpleException::new(ExcType::NameError, Some(object.name.to_owned().into())).into();
                    return Err(exc.into());
                }
                args.prepare_args(|expr| self.prepare_expression(expr))?;
                Expr::AttrCall { object, attr, args }
            }
            Expr::List(elements) => {
                let expressions = elements
                    .into_iter()
                    .map(|e| self.prepare_expression(e))
                    .collect::<Result<_, ParseError<'c>>>()?;
                Expr::List(expressions)
            }
            Expr::Tuple(elements) => {
                let expressions = elements
                    .into_iter()
                    .map(|e| self.prepare_expression(e))
                    .collect::<Result<_, ParseError<'c>>>()?;
                Expr::Tuple(expressions)
            }
            Expr::Subscript { object, index } => Expr::Subscript {
                object: Box::new(self.prepare_expression(*object)?),
                index: Box::new(self.prepare_expression(*index)?),
            },
            Expr::Dict(pairs) => {
                let prepared_pairs = pairs
                    .into_iter()
                    .map(|(k, v)| Ok((self.prepare_expression(k)?, self.prepare_expression(v)?)))
                    .collect::<Result<_, ParseError<'c>>>()?;
                Expr::Dict(prepared_pairs)
            }
            Expr::Not(operand) => Expr::Not(Box::new(self.prepare_expression(*operand)?)),
        };

        // Optimization: Transform `(x % n) == value` with any constant right-hand side into a
        // specialized ModEq operator.
        // This is a common pattern in competitive programming (e.g., FizzBuzz checks like `i % 3 == 0`)
        // and can be executed more efficiently with a single modulo operation + comparison
        // instead of separate modulo, then equality check.
        if let Expr::CmpOp { left, op, right } = &expr {
            if op == &CmpOperator::Eq {
                if let Expr::Literal(Literal::Int(value)) = right.expr {
                    if let Expr::Op {
                        left: left2,
                        op,
                        right: right2,
                    } = &left.expr
                    {
                        if op == &Operator::Mod {
                            let new_expr = Expr::CmpOp {
                                left: left2.clone(),
                                op: CmpOperator::ModEq(value),
                                right: right2.clone(),
                            };
                            return Ok(ExprLoc {
                                position: left.position,
                                expr: new_expr,
                            });
                        }
                    }
                }
            }
        }

        Ok(ExprLoc { position, expr })
    }

    /// Prepares a function definition using a two-pass approach for correct scope resolution.
    ///
    /// Pass 1: Scan the function body to collect:
    /// - Names declared as `global`
    /// - Names that are assigned (these are local unless declared global)
    ///
    /// Pass 2: Prepare the function body with the scope information from pass 1.
    fn prepare_function_def<'c>(
        &mut self,
        name: Identifier<'c>,
        params: Vec<&'c str>,
        body: Vec<ParseNode<'c>>,
    ) -> Result<Node<'c>, ParseError<'c>> {
        // Register the function name in the current scope
        let (name, _) = self.get_id(name);

        // Pass 1: Collect scope information from the function body
        let (global_names, assigned_names) = collect_function_scope_info(&body);

        // Get the global name map to pass to the function preparer
        // At module level, use our own name_map; otherwise use the inherited global_name_map
        let global_name_map = if self.is_module_scope {
            self.name_map.clone()
        } else {
            self.global_name_map.clone().unwrap_or_default()
        };

        // Pass 2: Create child preparer for function body with scope info
        let mut prepare = Prepare::new_function(body.len(), &params, assigned_names, global_names, global_name_map);

        // Prepare the function body
        let prepared_body = prepare.prepare_nodes(body)?;

        // Return the final FunctionDef node
        Ok(Node::FunctionDef(Function::new(
            name,
            params,
            prepared_body,
            prepare.namespace_size,
        )))
    }

    /// Resolves an identifier to its namespace index and scope, creating a new entry if needed.
    ///
    /// This is the core name resolution mechanism with scope-aware resolution:
    ///
    /// **At module level:** All names go to the local namespace (which IS the global namespace).
    ///
    /// **In functions:**
    /// - If name is declared `global` → resolve to global namespace
    /// - If name is assigned in this function → resolve to local namespace
    /// - If name exists in global namespace (read-only access) → resolve to global namespace
    /// - Otherwise → resolve to local namespace (will be NameError at runtime)
    ///
    /// # Returns
    /// A tuple of (resolved Identifier with id and scope set, whether this is a new local name).
    fn get_id<'c>(&mut self, ident: Identifier<'c>) -> (Identifier<'c>, bool) {
        let name_str = ident.name.to_owned();

        // At module level, all names are local (which is also the global namespace)
        if self.is_module_scope {
            let (id, is_new) = match self.name_map.entry(name_str) {
                Entry::Occupied(e) => (*e.get(), false),
                Entry::Vacant(e) => {
                    let id = self.namespace_size;
                    self.namespace_size += 1;
                    e.insert(id);
                    (id, true)
                }
            };
            return (
                Identifier::new_with_scope(ident.name, ident.position, id, NameScope::Local),
                is_new,
            );
        }

        // In a function: determine scope based on global_names, assigned_names, global_name_map

        // 1. Check if declared `global`
        if self.global_names.contains(&name_str) {
            if let Some(ref global_map) = self.global_name_map {
                if let Some(&global_id) = global_map.get(&name_str) {
                    // Name exists in global namespace
                    return (
                        Identifier::new_with_scope(ident.name, ident.position, global_id, NameScope::Global),
                        false,
                    );
                }
            }
            // Declared global but doesn't exist yet - it will be created when assigned
            // For now, we still need a global index. We'll use a placeholder approach:
            // allocate in global namespace (this is a simplification - in real Python,
            // the global would be created at module level when first assigned)
            // For our implementation, we'll resolve to global but the variable won't exist until assigned.
            // Return a "new" global - but we can't modify global_name_map here.
            // For simplicity, we'll resolve to local with Global scope - runtime will handle the lookup.
            let (id, is_new) = match self.name_map.entry(name_str) {
                Entry::Occupied(e) => (*e.get(), false),
                Entry::Vacant(e) => {
                    let id = self.namespace_size;
                    self.namespace_size += 1;
                    e.insert(id);
                    (id, true)
                }
            };
            // Mark as Global scope - runtime will need to handle this specially
            return (
                Identifier::new_with_scope(ident.name, ident.position, id, NameScope::Global),
                is_new,
            );
        }

        // 2. Check if assigned in this function (local variable)
        if self.assigned_names.contains(&name_str) {
            let (id, is_new) = match self.name_map.entry(name_str) {
                Entry::Occupied(e) => (*e.get(), false),
                Entry::Vacant(e) => {
                    let id = self.namespace_size;
                    self.namespace_size += 1;
                    e.insert(id);
                    (id, true)
                }
            };
            return (
                Identifier::new_with_scope(ident.name, ident.position, id, NameScope::Local),
                is_new,
            );
        }

        // 3. Check if exists in global namespace (implicit global read)
        if let Some(ref global_map) = self.global_name_map {
            if let Some(&global_id) = global_map.get(&name_str) {
                return (
                    Identifier::new_with_scope(ident.name, ident.position, global_id, NameScope::Global),
                    false,
                );
            }
        }

        // 4. Name not found anywhere - resolve to local (will be NameError at runtime)
        let (id, is_new) = match self.name_map.entry(name_str) {
            Entry::Occupied(e) => (*e.get(), false),
            Entry::Vacant(e) => {
                let id = self.namespace_size;
                self.namespace_size += 1;
                e.insert(id);
                (id, true)
            }
        };
        (
            Identifier::new_with_scope(ident.name, ident.position, id, NameScope::Local),
            is_new,
        )
    }
}

/// Scans a function body to collect scope information (first pass of two-pass preparation).
///
/// This function recursively walks the AST to find:
/// - Names declared as `global` (from Global statements)
/// - Names that are assigned (from Assign, OpAssign, For targets, etc.)
///
/// This information is used to determine whether each name reference should resolve
/// to the local namespace or the global namespace.
///
/// # Returns
/// A tuple of (global_names, assigned_names) as HashSets.
fn collect_function_scope_info(nodes: &[ParseNode<'_>]) -> (AHashSet<String>, AHashSet<String>) {
    let mut global_names = AHashSet::new();
    let mut assigned_names = AHashSet::new();

    for node in nodes {
        collect_scope_info_from_node(node, &mut global_names, &mut assigned_names);
    }

    (global_names, assigned_names)
}

/// Helper to collect scope info from a single node.
fn collect_scope_info_from_node(
    node: &ParseNode<'_>,
    global_names: &mut AHashSet<String>,
    assigned_names: &mut AHashSet<String>,
) {
    match node {
        ParseNode::Global(names) => {
            for name in names {
                global_names.insert((*name).to_string());
            }
        }
        ParseNode::Assign { target, .. } => {
            assigned_names.insert(target.name.to_string());
        }
        ParseNode::OpAssign { target, .. } => {
            assigned_names.insert(target.name.to_string());
        }
        ParseNode::SubscriptAssign { .. } => {
            // Subscript assignment doesn't create a new name, it modifies existing container
        }
        ParseNode::For {
            target, body, or_else, ..
        } => {
            // For loop target is assigned
            assigned_names.insert(target.name.to_string());
            // Recurse into body and else
            for n in body {
                collect_scope_info_from_node(n, global_names, assigned_names);
            }
            for n in or_else {
                collect_scope_info_from_node(n, global_names, assigned_names);
            }
        }
        ParseNode::If { body, or_else, .. } => {
            // Recurse into branches
            for n in body {
                collect_scope_info_from_node(n, global_names, assigned_names);
            }
            for n in or_else {
                collect_scope_info_from_node(n, global_names, assigned_names);
            }
        }
        ParseNode::FunctionDef { name, .. } => {
            // Function definition creates a local binding for the function name
            // But we don't recurse into the function body - that's a separate scope
            assigned_names.insert(name.name.to_string());
        }
        // These don't create new names
        ParseNode::Pass
        | ParseNode::Expr(_)
        | ParseNode::Return(_)
        | ParseNode::ReturnNone
        | ParseNode::Raise(_)
        | ParseNode::Assert { .. } => {}
    }
}
