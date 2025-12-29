use std::fmt::Debug;

use crate::{evaluate::ExternalCall, for_iterator::ForIterator, value::Value};

/// Result of executing a frame - return, yield, or external function call.
///
/// When a frame encounters a `return` statement, it produces `Return(value)`.
/// When a frame encounters a `yield` statement, it produces `Yield(value)` to
/// pause execution and return control to the caller.
/// When a frame encounters a call to an external function, it produces
/// `FunctionCall` to pause execution and let the host provide the return value.
#[derive(Debug)]
pub enum FrameExit {
    /// Normal return from a function or end of module execution.
    Return(Value),
    /// External function call pauses execution.
    ///
    /// The host must provide the return value to resume execution. The arguments
    /// have already been evaluated and converted to `Value`.
    ExternalCall(ExternalCall),
}

pub trait AbstractSnapshotTracker: Clone + Debug {
    /// Get the next position to execute from
    fn next(&mut self) -> CodePosition;

    /// When suspending execution, set the position to resume from
    fn record(&mut self, index: usize);

    /// When leaving an if statement or for loop, set the position to resume from
    fn set_clause_state(&mut self, clause_state: ClauseState);

    /// Whether to clear return values, this is only necessary when position is being tracked
    fn clear_return_values() -> bool;
}

#[derive(Debug, Clone)]
pub struct NoSnapshotTracker;

impl AbstractSnapshotTracker for NoSnapshotTracker {
    fn next(&mut self) -> CodePosition {
        CodePosition::default()
    }

    fn record(&mut self, _index: usize) {}

    fn set_clause_state(&mut self, _clause_state: ClauseState) {}

    fn clear_return_values() -> bool {
        false
    }
}

#[derive(Debug, Clone, Default)]
pub struct SnapshotTracker {
    /// stack of positions, note this is reversed (last value is the outermost position)
    /// as we push the outermost position last and pop it first
    stack: Vec<CodePosition>,
    clause_state: Option<ClauseState>,
}

impl SnapshotTracker {
    pub fn new(stack: Vec<CodePosition>) -> Self {
        SnapshotTracker {
            stack,
            clause_state: None,
        }
    }

    pub fn into_stack(self) -> Vec<CodePosition> {
        self.stack
    }
}

impl AbstractSnapshotTracker for SnapshotTracker {
    fn next(&mut self) -> CodePosition {
        self.stack.pop().unwrap_or_default()
    }

    fn record(&mut self, index: usize) {
        self.stack.push(CodePosition {
            index,
            clause_state: self.clause_state.take(),
        });
    }

    fn set_clause_state(&mut self, clause_state: ClauseState) {
        self.clause_state = Some(clause_state);
    }

    fn clear_return_values() -> bool {
        true
    }
}

/// Represents a position within nested control flow for snapshotting and code resumption.
#[derive(Debug, Clone, Default)]
pub(crate) struct CodePosition {
    /// Index of the next node to execute within the node array
    pub index: usize,
    /// indicates how to resume within the nested control flow if relevant
    pub clause_state: Option<ClauseState>,
}

/// State for resuming execution within control flow structures.
///
/// When execution suspends inside a control flow structure (if/for), this records
/// which branch was taken so we can skip re-evaluating the condition on resume.
#[derive(Debug, Clone)]
pub(crate) enum ClauseState {
    /// When resuming within the if statement,
    /// whether the condition was met - true to resume the if branch and false to resume the else branch
    If(bool),
    /// When resuming within a for loop, `ForIterator` holds the value and the index of the next element
    /// for iteration.
    For(ForIterator),
}
