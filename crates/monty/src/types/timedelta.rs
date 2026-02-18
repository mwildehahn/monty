//! Python `datetime.timedelta` implementation.
//!
//! Monty stores timedeltas using `chrono::TimeDelta`, while preserving CPython's
//! normalized `(days, seconds, microseconds)` semantics for constructors, arithmetic,
//! and formatting.

use std::fmt::Write;

use ahash::AHashSet;
use chrono::TimeDelta as ChronoTimeDelta;

use crate::{
    args::ArgValues,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData, HeapId},
    intern::Interns,
    resource::{DepthGuard, ResourceError, ResourceTracker},
    types::{PyTrait, Type},
    value::{EitherStr, Value},
};

/// Minimum allowed day magnitude for `timedelta`.
pub(crate) const MIN_TIMEDELTA_DAYS: i32 = -999_999_999;
/// Maximum allowed day magnitude for `timedelta`.
pub(crate) const MAX_TIMEDELTA_DAYS: i32 = 999_999_999;

const DAY_SECONDS: i128 = 86_400;
const DAY_MICROSECONDS: i128 = DAY_SECONDS * 1_000_000;

/// `datetime.timedelta` storage backed by `chrono::TimeDelta`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct TimeDelta(pub(crate) ChronoTimeDelta);

/// Creates a normalized timedelta value from CPython components.
pub(crate) fn new(days: i32, seconds: i32, microseconds: i32) -> RunResult<TimeDelta> {
    if !(MIN_TIMEDELTA_DAYS..=MAX_TIMEDELTA_DAYS).contains(&days) {
        return Err(SimpleException::new_msg(
            ExcType::OverflowError,
            format!("days={days}; must have magnitude <= 999999999"),
        )
        .into());
    }
    if !(0..86_400).contains(&seconds) || !(0..1_000_000).contains(&microseconds) {
        return Err(SimpleException::new_msg(ExcType::ValueError, "timedelta normalized fields out of range").into());
    }
    let total_microseconds =
        i128::from(days) * DAY_MICROSECONDS + i128::from(seconds) * 1_000_000 + i128::from(microseconds);
    from_total_microseconds(total_microseconds)
}

/// Returns CPython normalized `(days, seconds, microseconds)` components.
#[must_use]
pub(crate) fn components(delta: &TimeDelta) -> (i32, i32, i32) {
    let total_microseconds = total_microseconds(delta);
    let days = total_microseconds.div_euclid(DAY_MICROSECONDS);
    let rem = total_microseconds.rem_euclid(DAY_MICROSECONDS);
    let seconds = rem / 1_000_000;
    let micros = rem % 1_000_000;
    (
        i32::try_from(days).expect("chrono day range fits CPython i32 day bounds"),
        i32::try_from(seconds).expect("seconds are bounded by one day"),
        i32::try_from(micros).expect("microseconds are bounded by one second"),
    )
}

/// Returns the duration as total microseconds.
#[must_use]
pub(crate) fn total_microseconds(delta: &TimeDelta) -> i128 {
    // `subsec_nanos` can be negative for negative durations; summing both parts
    // yields an exact signed duration as long as we keep microsecond precision.
    let seconds = i128::from(delta.0.num_seconds());
    let microseconds = i128::from(delta.0.subsec_nanos() / 1_000);
    seconds * 1_000_000 + microseconds
}

/// Returns the duration as total whole seconds plus fractional microseconds.
#[must_use]
pub(crate) fn total_seconds(delta: &TimeDelta) -> f64 {
    total_microseconds(delta) as f64 / 1_000_000.0
}

/// Returns total seconds only when exact (no microseconds), otherwise `None`.
#[must_use]
pub(crate) fn exact_total_seconds(delta: &TimeDelta) -> Option<i128> {
    let (days, seconds, microseconds) = components(delta);
    if microseconds == 0 {
        Some(i128::from(days) * DAY_SECONDS + i128::from(seconds))
    } else {
        None
    }
}

/// Exposes the underlying chrono duration for datetime/date arithmetic.
#[must_use]
pub(crate) fn chrono_delta(delta: &TimeDelta) -> ChronoTimeDelta {
    delta.0
}

/// Converts a chrono duration to Monty's bounded timedelta.
pub(crate) fn from_chrono(delta: ChronoTimeDelta) -> RunResult<TimeDelta> {
    from_total_microseconds(i128::from(delta.num_seconds()) * 1_000_000 + i128::from(delta.subsec_nanos() / 1_000))
}

/// Builds a normalized timedelta from an arbitrary microsecond count.
pub(crate) fn from_total_microseconds(total_microseconds: i128) -> RunResult<TimeDelta> {
    let days = total_microseconds.div_euclid(DAY_MICROSECONDS);
    if !(i128::from(MIN_TIMEDELTA_DAYS)..=i128::from(MAX_TIMEDELTA_DAYS)).contains(&days) {
        return Err(SimpleException::new_msg(
            ExcType::OverflowError,
            format!("days={days}; must have magnitude <= 999999999"),
        )
        .into());
    }

    let seconds = total_microseconds.div_euclid(1_000_000);
    let micros = total_microseconds.rem_euclid(1_000_000);

    let seconds = i64::try_from(seconds)
        .map_err(|_| SimpleException::new_msg(ExcType::OverflowError, "timedelta value out of range"))?;
    let nanos =
        u32::try_from(micros * 1_000).expect("microsecond remainder is in 0..1_000_000 and fits u32 nanoseconds");

    let delta = ChronoTimeDelta::new(seconds, nanos)
        .ok_or_else(|| SimpleException::new_msg(ExcType::OverflowError, "timedelta value out of range"))?;
    Ok(TimeDelta(delta))
}

/// Creates a `timedelta` from constructor arguments.
///
/// Supports positional `(days, seconds, microseconds)` and keyword arguments
/// `days`, `seconds`, `microseconds`, `milliseconds`, `minutes`, `hours`, `weeks`.
pub(crate) fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (pos, kwargs) = args.into_parts();
    defer_drop_mut!(pos, heap);
    let kwargs = kwargs.into_iter();
    defer_drop_mut!(kwargs, heap);

    let mut days = 0_i128;
    let mut seconds = 0_i128;
    let mut microseconds = 0_i128;
    let mut milliseconds = 0_i128;
    let mut minutes = 0_i128;
    let mut hours = 0_i128;
    let mut weeks = 0_i128;

    let mut seen_days = false;
    let mut seen_seconds = false;
    let mut seen_microseconds = false;

    for (index, arg) in pos.by_ref().enumerate() {
        defer_drop!(arg, heap);
        match index {
            0 => {
                days = value_to_i128(arg, heap)?;
                seen_days = true;
            }
            1 => {
                seconds = value_to_i128(arg, heap)?;
                seen_seconds = true;
            }
            2 => {
                microseconds = value_to_i128(arg, heap)?;
                seen_microseconds = true;
            }
            _ => return Err(ExcType::type_error_at_most("timedelta", 3, index + 1)),
        }
    }

    for (key, value) in kwargs {
        defer_drop!(key, heap);
        defer_drop!(value, heap);
        let Some(key_name) = key.as_either_str(heap) else {
            return Err(ExcType::type_error_kwargs_nonstring_key());
        };
        let key_name = key_name.as_str(interns);
        let parsed = value_to_i128(value, heap)?;

        match key_name {
            "days" => {
                if seen_days {
                    return Err(ExcType::type_error_multiple_values("timedelta", "days"));
                }
                days = parsed;
                seen_days = true;
            }
            "seconds" => {
                if seen_seconds {
                    return Err(ExcType::type_error_multiple_values("timedelta", "seconds"));
                }
                seconds = parsed;
                seen_seconds = true;
            }
            "microseconds" => {
                if seen_microseconds {
                    return Err(ExcType::type_error_multiple_values("timedelta", "microseconds"));
                }
                microseconds = parsed;
                seen_microseconds = true;
            }
            "milliseconds" => milliseconds = parsed,
            "minutes" => minutes = parsed,
            "hours" => hours = parsed,
            "weeks" => weeks = parsed,
            _ => return Err(ExcType::type_error_unexpected_keyword("timedelta", key_name)),
        }
    }

    let total_microseconds = checked_component(weeks, 7 * DAY_MICROSECONDS)?
        + checked_component(days, DAY_MICROSECONDS)?
        + checked_component(hours, 3_600_000_000)?
        + checked_component(minutes, 60_000_000)?
        + checked_component(seconds, 1_000_000)?
        + checked_component(milliseconds, 1_000)?
        + microseconds;

    let delta = from_total_microseconds(total_microseconds)?;
    Ok(Value::Ref(heap.allocate(HeapData::TimeDelta(delta))?))
}

fn checked_component(value: i128, unit_microseconds: i128) -> RunResult<i128> {
    value.checked_mul(unit_microseconds).ok_or_else(|| {
        SimpleException::new_msg(ExcType::OverflowError, "timedelta argument overflow while normalizing").into()
    })
}

fn value_to_i128(value: &Value, heap: &Heap<impl ResourceTracker>) -> RunResult<i128> {
    if let Value::Bool(b) = value {
        return Ok(i128::from(i64::from(*b)));
    }
    Ok(i128::from(value.as_int(heap)?))
}

impl PyTrait for TimeDelta {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::TimeDelta
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        None
    }

    fn py_eq(
        &self,
        other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> Result<bool, ResourceError> {
        Ok(total_microseconds(self) == total_microseconds(other))
    }

    fn py_cmp(
        &self,
        other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> Result<Option<std::cmp::Ordering>, ResourceError> {
        Ok(total_microseconds(self).partial_cmp(&total_microseconds(other)))
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {}

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        total_microseconds(self) != 0
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        _heap: &Heap<impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> std::fmt::Result {
        let (days, seconds, microseconds) = components(self);
        if days == 0 && seconds == 0 && microseconds == 0 {
            return f.write_str("datetime.timedelta(0)");
        }
        f.write_str("datetime.timedelta(")?;
        let mut first = true;
        if days != 0 {
            write!(f, "days={days}")?;
            first = false;
        }
        if seconds != 0 {
            if !first {
                f.write_str(", ")?;
            }
            write!(f, "seconds={seconds}")?;
            first = false;
        }
        if microseconds != 0 {
            if !first {
                f.write_str(", ")?;
            }
            write!(f, "microseconds={microseconds}")?;
        }
        f.write_char(')')
    }

    fn py_str(
        &self,
        _heap: &Heap<impl ResourceTracker>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> std::borrow::Cow<'static, str> {
        let (days, seconds, microseconds) = components(self);
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        let second = seconds % 60;
        let time = if microseconds == 0 {
            format!("{hours}:{minutes:02}:{second:02}")
        } else {
            format!("{hours}:{minutes:02}:{second:02}.{microseconds:06}")
        };

        if days == 0 {
            return std::borrow::Cow::Owned(time);
        }

        let day_word = if days.abs() == 1 { "day" } else { "days" };
        std::borrow::Cow::Owned(format!("{days} {day_word}, {time}"))
    }

    fn py_add(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> Result<Option<Value>, ResourceError> {
        let Some(total) = total_microseconds(self).checked_add(total_microseconds(other)) else {
            return Ok(None);
        };
        let Ok(result) = from_total_microseconds(total) else {
            return Ok(None);
        };
        Ok(Some(Value::Ref(heap.allocate(HeapData::TimeDelta(result))?)))
    }

    fn py_sub(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>) -> Result<Option<Value>, ResourceError> {
        let Some(total) = total_microseconds(self).checked_sub(total_microseconds(other)) else {
            return Ok(None);
        };
        let Ok(result) = from_total_microseconds(total) else {
            return Ok(None);
        };
        Ok(Some(Value::Ref(heap.allocate(HeapData::TimeDelta(result))?)))
    }

    fn py_call_attr(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        if attr.as_str(interns) == "total_seconds" {
            args.check_zero_args("timedelta.total_seconds", heap)?;
            return Ok(Value::Float(total_seconds(self)));
        }
        Err(ExcType::attribute_error(self.py_type(heap), attr.as_str(interns)))
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}
