use std::{borrow::Cow, fmt};

use crate::{
    args::ArgValues,
    exceptions::{ExcType, SimpleException, StackFrame},
    expressions::{FrameExit, Identifier, Node},
    heap::Heap,
    namespace::Namespaces,
    run::{RunFrame, RunResult},
    value::{heap_tagged_id, Value},
    values::str::string_repr,
};

/// Stores a function definition.
///
/// Contains everything needed to execute a user-defined function: the body AST,
/// initial namespace layout, and captured closure cells. Functions are stored
/// on the heap and referenced via HeapId.
///
/// # Future: `nonlocal` Support
///
/// When `nonlocal` is implemented, this struct will need to store the enclosing
/// function's namespace index to support accessing variables from enclosing scopes.
///
/// TODO: Add enclosing_ns_idx field for nonlocal support
#[derive(Debug, Clone)]
pub(crate) struct Function<'c> {
    /// The function name (used for error messages and repr).
    pub name: Identifier<'c>,
    /// The function parameters (used for error message).
    pub params: Vec<&'c str>,
    /// The prepared function body AST nodes.
    pub body: Vec<Node<'c>>,
    /// Size of the initial namespace
    pub namespace_size: usize,
    // /// References to shared cells for captured variables.
    // /// Each HeapId points to a HeapData::Cell on the heap.
    // pub closure_cells: Vec<HeapId>,
}

impl fmt::Display for Function<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name.name)
    }
}

impl<'c> Function<'c> {
    /// Create a new function definition.
    pub fn new(name: Identifier<'c>, params: Vec<&'c str>, body: Vec<Node<'c>>, namespace_size: usize) -> Self {
        Self {
            name,
            params,
            body,
            namespace_size,
        }
    }

    /// Calls this function with the given arguments.
    ///
    /// # Arguments
    /// * `namespaces` - The namespace namespaces for managing all namespaces
    /// * `heap` - The heap for allocating objects
    /// * `args` - The arguments to pass to the function
    ///
    /// # Future: `nonlocal` Support
    ///
    /// TODO: Add enclosing_idx parameter for nested function calls
    pub fn call<'e>(
        &'e self,
        namespaces: &mut Namespaces<'c, 'e>,
        heap: &mut Heap<'c, 'e>,
        args: ArgValues<'c, 'e>,
    ) -> RunResult<'c, Value<'c, 'e>>
    where
        'c: 'e,
    {
        let mut namespace = Vec::with_capacity(self.namespace_size);
        args.inject_into_namespace(&mut namespace);
        if namespace.len() == self.params.len() {
            // Fill remaining slots with Undefined for local variables
            namespace.resize_with(self.namespace_size, || Value::Undefined);

            // Create a new local namespace for this function call
            let local_idx = namespaces.push(namespace);

            // Create stack frame for error tracebacks
            let parent_frame = StackFrame::new(&self.name.position, self.name.name, None);

            // Execute the function body in a new frame
            let frame = RunFrame::new_for_function(local_idx, self.name.name, Some(parent_frame));

            let result = frame.execute(namespaces, heap, &self.body);

            // Clean up the function's namespace (properly decrementing ref counts)
            namespaces.pop_with_heap(heap);

            match result {
                Ok(FrameExit::Return(obj)) => Ok(obj),
                Err(e) => Err(e),
            }
        } else {
            // Wrong number of arguments - return error without creating namespace
            let msg = if let Some(missing_count) = self.params.len().checked_sub(namespace.len()) {
                let mut msg = format!(
                    "{}() missing {} required positional argument{}: ",
                    self.name.name,
                    missing_count,
                    if missing_count == 1 { "" } else { "s" }
                );
                let mut missing_names: Vec<_> = self
                    .params
                    .iter()
                    .skip(namespace.len())
                    .map(|param| string_repr(param))
                    .collect();
                let last = missing_names.pop().unwrap();
                if !missing_names.is_empty() {
                    msg.push_str(&missing_names.join(", "));
                    msg.push_str(", and ");
                }
                msg.push_str(&last);
                msg
            } else {
                format!(
                    "{}() takes {} positional argument{} but {} {} given",
                    self.name.name,
                    self.params.len(),
                    if self.params.len() == 1 { "" } else { "s" },
                    namespace.len(),
                    if namespace.len() == 1 { "was" } else { "were" }
                )
            };
            Err(SimpleException::new(ExcType::TypeError, Some(msg.into()))
                .with_position(self.name.position)
                .into())
        }
    }

    pub fn py_repr(&self) -> Cow<'_, str> {
        format!("<function '{}' at 0x{:x}>", self, self.id()).into()
    }

    pub fn id(&self) -> usize {
        heap_tagged_id(self.name.heap_id())
    }
}
