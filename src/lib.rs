mod args;
mod builtins;
mod callable;
mod evaluate;
pub mod exceptions;
mod expressions;
mod function;
mod heap;
mod namespace;
mod object;
mod operators;
mod parse;
mod parse_error;
mod prepare;
mod run;
mod value;
mod values;

#[cfg(feature = "ref-counting")]
use ahash::AHashMap;

use crate::exceptions::InternalRunError;
pub use crate::exceptions::RunError;
use crate::expressions::Node;
use crate::heap::Heap;
use crate::namespace::Namespaces;
pub use crate::object::{InvalidInputError, PyObject};
use crate::parse::parse;
pub use crate::parse_error::ParseError;
use crate::prepare::prepare;
use crate::run::RunFrame;
use crate::value::Value;

/// Main executor that compiles and runs Python code.
///
/// The executor stores the compiled AST and initial namespace as literals (not runtime
/// values). When `run()` is called, literals are converted to heap-allocated runtime
/// values, ensuring proper reference counting from the start of execution.
///
/// When the `ref-counting` feature is enabled, `run_ref_counts()` can be used to
/// execute code and retrieve reference count data for testing purposes.
#[derive(Debug)]
pub struct Executor<'c> {
    namespace_size: usize,
    /// Maps variable names to their indices in the namespace. Used for ref-count testing.
    #[cfg(feature = "ref-counting")]
    name_map: AHashMap<String, usize>,
    nodes: Vec<Node<'c>>,
}

impl<'c> Executor<'c> {
    pub fn new(code: &'c str, filename: &'c str, input_names: &[&str]) -> Result<Self, ParseError<'c>> {
        let nodes = parse(code, filename)?;
        let prepared = prepare(nodes, input_names)?;
        Ok(Self {
            namespace_size: prepared.namespace_size,
            #[cfg(feature = "ref-counting")]
            name_map: prepared.name_map,
            nodes: prepared.nodes,
        })
    }

    /// Executes the code with the given input values.
    ///
    /// The heap is created fresh for each run, ensuring no state leaks between
    /// executions.
    ///
    /// # Arguments
    /// * `inputs` - Values to fill the first N slots of the namespace (e.g., function parameters)
    pub fn run(&self, inputs: Vec<PyObject>) -> Result<PyObject, RunError<'c>> {
        let mut heap = Heap::new(self.namespace_size);
        let mut namespaces = self.prepare_namespaces(inputs, &mut heap)?;

        let frame = RunFrame::new();
        let result = frame.execute(&mut namespaces, &mut heap, &self.nodes);

        // Clean up the global namespace before returning (only needed with dec-ref-check)
        #[cfg(feature = "dec-ref-check")]
        namespaces.drop_global_with_heap(&mut heap);

        result.map(|frame_exit| PyObject::new(frame_exit, &mut heap))
    }

    /// Executes the code and returns both the result and reference count data.
    ///
    /// This is used for testing reference counting behavior. Returns:
    /// - The execution result (`Exit`)
    /// - Reference count data as a tuple of:
    ///   - A map from variable names to their reference counts (only for heap-allocated values)
    ///   - The number of unique heap value IDs referenced by variables
    ///   - The total number of live heap values
    ///
    /// For strict matching validation, compare unique_refs_count with heap_entry_count.
    /// If they're equal, all heap values are accounted for by named variables.
    ///
    /// Only available when the `ref-counting` feature is enabled.
    #[cfg(feature = "ref-counting")]
    pub fn run_ref_counts(&self, inputs: Vec<PyObject>) -> RunRefCountsResult<'c> {
        use crate::value::Value;
        use std::collections::HashSet;

        let mut heap = Heap::new(self.namespace_size);
        let mut namespaces = self.prepare_namespaces(inputs, &mut heap)?;

        let frame = RunFrame::new();
        let result = frame.execute(&mut namespaces, &mut heap, &self.nodes);

        // Compute ref counts before consuming the heap
        let final_namespace = namespaces.into_global();
        let mut counts = AHashMap::new();
        let mut unique_ids = HashSet::new();

        for (name, &index) in &self.name_map {
            if let Some(Value::Ref(id)) = final_namespace.get(index) {
                counts.insert(name.clone(), heap.get_refcount(*id));
                unique_ids.insert(*id);
            }
        }
        let ref_count_data: RefCountSnapshot = (counts, unique_ids.len(), heap.entry_count());

        // Clean up the namespace after reading ref counts but before moving the heap
        for obj in final_namespace {
            obj.drop_with_heap(&mut heap);
        }

        let python_value = result.map(|frame_exit| PyObject::new(frame_exit, &mut heap))?;

        Ok((python_value, ref_count_data))
    }

    /// Prepares the namespace namespaces for execution.
    ///
    /// Converts each `PyObject` input to a `Value`, allocating on the heap if needed.
    /// Returns the prepared Namespaces or an error if there are too many inputs or invalid input types.
    fn prepare_namespaces<'e>(
        &self,
        inputs: Vec<PyObject>,
        heap: &mut Heap<'c, 'e>,
    ) -> Result<Namespaces<'c, 'e>, InternalRunError> {
        let Some(extra) = self.namespace_size.checked_sub(inputs.len()) else {
            return Err(InternalRunError::Error(
                format!("input length should be <= {}", self.namespace_size).into(),
            ));
        };
        // Convert each PyObject to a Value, propagating any invalid input errors
        let mut namespace: Vec<Value<'c, 'e>> = inputs
            .into_iter()
            .map(|pv| pv.to_value(heap))
            .collect::<Result<_, _>>()
            .map_err(|e| InternalRunError::Error(e.to_string().into()))?;
        if extra > 0 {
            namespace.extend((0..extra).map(|_| Value::Undefined));
        }
        Ok(Namespaces::new(namespace))
    }
}

/// parse code and show the parsed AST, mostly for testing
pub fn parse_show(code: &str, filename: &str) -> Result<String, String> {
    match parse(code, filename) {
        Ok(ast) => Ok(format!("{ast:#?}")),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(feature = "ref-counting")]
/// Aggregated reference counting statistics returned by `Executor::run_ref_counts`.
type RefCountSnapshot = (AHashMap<String, usize>, usize, usize);

#[cfg(feature = "ref-counting")]
/// Result type used by `Executor::run_ref_counts`.
type RunRefCountsResult<'c> = Result<(PyObject, RefCountSnapshot), RunError<'c>>;
