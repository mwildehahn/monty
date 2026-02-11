//! Implementation of the `datetime` module.
//!
//! This module exposes a minimal phase-1 surface:
//! - `date`
//! - `datetime`
//! - `timedelta`
//! - `timezone`
//!
//! Behavior for constructors, arithmetic, and classmethods is implemented by the
//! corresponding runtime types.

use crate::{
    builtins::Builtins,
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings},
    resource::{ResourceError, ResourceTracker},
    types::{Module, Type},
    value::Value,
};

/// Creates the `datetime` module and allocates it on the heap.
///
/// Returns a `HeapId` pointing to the newly allocated module.
///
/// # Panics
///
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Datetime);

    module.set_attr(
        StaticStrings::Date,
        Value::Builtin(Builtins::Type(Type::Date)),
        heap,
        interns,
    );
    module.set_attr(
        StaticStrings::Datetime,
        Value::Builtin(Builtins::Type(Type::DateTime)),
        heap,
        interns,
    );
    module.set_attr(
        StaticStrings::Timedelta,
        Value::Builtin(Builtins::Type(Type::TimeDelta)),
        heap,
        interns,
    );
    module.set_attr(
        StaticStrings::Timezone,
        Value::Builtin(Builtins::Type(Type::TimeZone)),
        heap,
        interns,
    );

    heap.allocate(HeapData::Module(module))
}
