//! Binary and in-place operation helpers for the VM.

use super::VM;
use crate::{
    defer_drop,
    exception_private::{ExcType, RunError, SimpleException},
    heap::{HeapData, HeapGuard},
    resource::ResourceTracker,
    types::{PyTrait, datetime, timedelta},
    value::{BitwiseOp, Value},
};

impl<T: ResourceTracker> VM<'_, '_, T> {
    /// Binary addition with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths to avoid
    /// overhead on the success path (99%+ of operations).
    pub(super) fn binary_add(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        match lhs.py_add(rhs, this.heap, this.interns) {
            Ok(Some(v)) => {
                this.push(v);
                Ok(())
            }
            Ok(None) => {
                if let Some(err) = datetime_arithmetic_overflow_add(lhs, rhs, this.heap) {
                    return Err(err);
                }
                let lhs_type = lhs.py_type(this.heap);
                let rhs_type = rhs.py_type(this.heap);
                Err(ExcType::binary_type_error("+", lhs_type, rhs_type))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Binary subtraction with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    pub(super) fn binary_sub(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        match lhs.py_sub(rhs, this.heap) {
            Ok(Some(v)) => {
                this.push(v);
                Ok(())
            }
            Ok(None) => {
                if let (Some(lhs_aware), Some(rhs_aware)) =
                    (datetime_awareness(lhs, this.heap), datetime_awareness(rhs, this.heap))
                    && lhs_aware != rhs_aware
                {
                    return Err(ExcType::datetime_subtract_naive_aware_error());
                }
                if let Some(err) = datetime_arithmetic_overflow_sub(lhs, rhs, this.heap) {
                    return Err(err);
                }
                let lhs_type = lhs.py_type(this.heap);
                let rhs_type = rhs.py_type(this.heap);
                Err(ExcType::binary_type_error("-", lhs_type, rhs_type))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Binary multiplication with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    pub(super) fn binary_mult(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        match lhs.py_mult(rhs, this.heap, this.interns) {
            Ok(Some(v)) => {
                this.push(v);
                Ok(())
            }
            Ok(None) => {
                let lhs_type = lhs.py_type(this.heap);
                let rhs_type = rhs.py_type(this.heap);
                Err(ExcType::binary_type_error("*", lhs_type, rhs_type))
            }
            Err(e) => Err(e),
        }
    }

    /// Binary division with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    pub(super) fn binary_div(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        match lhs.py_div(rhs, this.heap, this.interns) {
            Ok(Some(v)) => {
                this.push(v);
                Ok(())
            }
            Ok(None) => {
                let lhs_type = lhs.py_type(this.heap);
                let rhs_type = rhs.py_type(this.heap);
                Err(ExcType::binary_type_error("/", lhs_type, rhs_type))
            }
            Err(e) => Err(e),
        }
    }

    /// Binary floor division with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    pub(super) fn binary_floordiv(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        match lhs.py_floordiv(rhs, this.heap) {
            Ok(Some(v)) => {
                this.push(v);
                Ok(())
            }
            Ok(None) => {
                let lhs_type = lhs.py_type(this.heap);
                let rhs_type = rhs.py_type(this.heap);
                Err(ExcType::binary_type_error("//", lhs_type, rhs_type))
            }
            Err(e) => Err(e),
        }
    }

    /// Binary modulo with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    pub(super) fn binary_mod(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        match lhs.py_mod(rhs, this.heap) {
            Ok(Some(v)) => {
                this.push(v);
                Ok(())
            }
            Ok(None) => {
                let lhs_type = lhs.py_type(this.heap);
                let rhs_type = rhs.py_type(this.heap);
                Err(ExcType::binary_type_error("%", lhs_type, rhs_type))
            }
            Err(e) => Err(e),
        }
    }

    /// Binary power with proper refcount handling.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    #[inline(never)]
    pub(super) fn binary_pow(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        match lhs.py_pow(rhs, this.heap) {
            Ok(Some(v)) => {
                this.push(v);
                Ok(())
            }
            Ok(None) => {
                let lhs_type = lhs.py_type(this.heap);
                let rhs_type = rhs.py_type(this.heap);
                Err(ExcType::binary_type_error("** or pow()", lhs_type, rhs_type))
            }
            Err(e) => Err(e),
        }
    }

    /// Binary bitwise operation on integers.
    ///
    /// Pops two values, performs the bitwise operation, and pushes the result.
    pub(super) fn binary_bitwise(&mut self, op: BitwiseOp) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        let lhs = this.pop();
        defer_drop!(lhs, this);

        let result = lhs.py_bitwise(rhs, op, this.heap)?;
        this.push(result);
        Ok(())
    }

    /// In-place addition (uses py_iadd for mutable containers, falls back to py_add).
    ///
    /// For mutable types like lists, `py_iadd` mutates in place and returns true.
    /// For immutable types, we fall back to regular addition.
    ///
    /// Uses lazy type capture: only calls `py_type()` in error paths.
    ///
    /// Note: Cannot use `defer_drop!` for `lhs` here because on successful in-place
    /// operation, we need to push `lhs` back onto the stack rather than drop it.
    pub(super) fn inplace_add(&mut self) -> Result<(), RunError> {
        let this = self;

        let rhs = this.pop();
        defer_drop!(rhs, this);
        // Use HeapGuard because inplace addition will push lhs back on the stack if successful
        let mut lhs_guard = HeapGuard::new(this.pop(), this);
        let (lhs, this) = lhs_guard.as_parts_mut();

        // Try in-place operation first (for mutable types like lists)
        if lhs.py_iadd(rhs.clone_with_heap(this.heap), this.heap, lhs.ref_id(), this.interns)? {
            // In-place operation succeeded - push lhs back
            let (lhs, this) = lhs_guard.into_parts();
            this.push(lhs);
            return Ok(());
        }

        // Next try regular addition
        if let Some(v) = lhs.py_add(rhs, this.heap, this.interns)? {
            this.push(v);
            return Ok(());
        }

        let lhs_type = lhs.py_type(this.heap);
        let rhs_type = rhs.py_type(this.heap);
        Err(ExcType::binary_type_error("+=", lhs_type, rhs_type))
    }

    /// Binary matrix multiplication (`@` operator).
    ///
    /// Currently not implemented - returns a `NotImplementedError`.
    /// Matrix multiplication requires numpy-like array types which Monty doesn't support.
    pub(super) fn binary_matmul(&mut self) -> Result<(), RunError> {
        let rhs = self.pop();
        let lhs = self.pop();
        lhs.drop_with_heap(self.heap);
        rhs.drop_with_heap(self.heap);
        Err(ExcType::not_implemented("matrix multiplication (@) is not supported").into())
    }
}

/// Returns datetime awareness (`true` for aware, `false` for naive) for datetime values.
///
/// Returns `None` when the value is not a datetime reference.
fn datetime_awareness(value: &crate::value::Value, heap: &crate::heap::Heap<impl ResourceTracker>) -> Option<bool> {
    let crate::value::Value::Ref(id) = value else {
        return None;
    };
    match heap.get(*id) {
        HeapData::DateTime(dt) => Some(datetime::is_aware(dt)),
        _ => None,
    }
}

/// Returns an `OverflowError` when datetime/date/timedelta addition failed due to bounds.
fn datetime_arithmetic_overflow_add(
    lhs: &Value,
    rhs: &Value,
    heap: &crate::heap::Heap<impl ResourceTracker>,
) -> Option<RunError> {
    if (is_date(lhs, heap) && is_timedelta(rhs, heap))
        || (is_datetime(lhs, heap) && is_timedelta(rhs, heap))
        || (is_timedelta(lhs, heap) && is_date(rhs, heap))
        || (is_timedelta(lhs, heap) && is_datetime(rhs, heap))
    {
        return Some(date_value_out_of_range_error());
    }

    if let (Some(lhs_delta), Some(rhs_delta)) = (as_timedelta(lhs, heap), as_timedelta(rhs, heap)) {
        return timedelta_overflow_error(lhs_delta, rhs_delta, true);
    }

    None
}

/// Returns an `OverflowError` when datetime/date/timedelta subtraction failed due to bounds.
fn datetime_arithmetic_overflow_sub(
    lhs: &Value,
    rhs: &Value,
    heap: &crate::heap::Heap<impl ResourceTracker>,
) -> Option<RunError> {
    if (is_date(lhs, heap) && is_timedelta(rhs, heap)) || (is_datetime(lhs, heap) && is_timedelta(rhs, heap)) {
        return Some(date_value_out_of_range_error());
    }

    if let (Some(lhs_delta), Some(rhs_delta)) = (as_timedelta(lhs, heap), as_timedelta(rhs, heap)) {
        return timedelta_overflow_error(lhs_delta, rhs_delta, false);
    }

    None
}

fn date_value_out_of_range_error() -> RunError {
    SimpleException::new_msg(ExcType::OverflowError, "date value out of range").into()
}

fn timedelta_overflow_error(
    lhs: &crate::types::TimeDelta,
    rhs: &crate::types::TimeDelta,
    add: bool,
) -> Option<RunError> {
    const DAY_MICROSECONDS: i128 = 86_400_000_000;

    let lhs_micros = timedelta::total_microseconds(lhs);
    let rhs_micros = timedelta::total_microseconds(rhs);
    let total_micros = if add {
        lhs_micros + rhs_micros
    } else {
        lhs_micros - rhs_micros
    };
    let days = total_micros.div_euclid(DAY_MICROSECONDS);
    if !(i128::from(timedelta::MIN_TIMEDELTA_DAYS)..=i128::from(timedelta::MAX_TIMEDELTA_DAYS)).contains(&days) {
        return Some(
            SimpleException::new_msg(
                ExcType::OverflowError,
                format!("days={days}; must have magnitude <= 999999999"),
            )
            .into(),
        );
    }
    None
}

fn is_date(value: &Value, heap: &crate::heap::Heap<impl ResourceTracker>) -> bool {
    matches!(value, Value::Ref(id) if matches!(heap.get(*id), HeapData::Date(_)))
}

fn is_datetime(value: &Value, heap: &crate::heap::Heap<impl ResourceTracker>) -> bool {
    matches!(value, Value::Ref(id) if matches!(heap.get(*id), HeapData::DateTime(_)))
}

fn is_timedelta(value: &Value, heap: &crate::heap::Heap<impl ResourceTracker>) -> bool {
    matches!(value, Value::Ref(id) if matches!(heap.get(*id), HeapData::TimeDelta(_)))
}

fn as_timedelta<'a>(
    value: &Value,
    heap: &'a crate::heap::Heap<impl ResourceTracker>,
) -> Option<&'a crate::types::TimeDelta> {
    let Value::Ref(id) = value else {
        return None;
    };
    let HeapData::TimeDelta(delta) = heap.get(*id) else {
        return None;
    };
    Some(delta)
}
