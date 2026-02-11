//! Python `datetime.timedelta` implementation.
//!
//! The type stores durations in CPython-compatible normalized form:
//! `(days, seconds, microseconds)` where:
//! - `-999999999 <= days <= 999999999`
//! - `0 <= seconds < 86400`
//! - `0 <= microseconds < 1_000_000`
//!
//! This normalization keeps arithmetic and comparisons deterministic while also
//! matching Python's string and repr formatting behavior.

use std::fmt::Write;

use ahash::AHashSet;

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

/// Python `datetime.timedelta` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct TimeDelta {
    /// Signed day component.
    pub days: i32,
    /// Seconds within the day.
    pub seconds: i32,
    /// Microseconds within the second.
    pub microseconds: i32,
}

impl TimeDelta {
    /// Creates a normalized timedelta value.
    pub fn new(days: i32, seconds: i32, microseconds: i32) -> RunResult<Self> {
        if !(MIN_TIMEDELTA_DAYS..=MAX_TIMEDELTA_DAYS).contains(&days) {
            return Err(SimpleException::new_msg(
                ExcType::OverflowError,
                format!("days={days}; must have magnitude <= 999999999"),
            )
            .into());
        }
        if !(0..86_400).contains(&seconds) || !(0..1_000_000).contains(&microseconds) {
            return Err(
                SimpleException::new_msg(ExcType::ValueError, "timedelta normalized fields out of range").into(),
            );
        }
        Ok(Self {
            days,
            seconds,
            microseconds,
        })
    }

    /// Returns the duration as total microseconds.
    #[must_use]
    pub fn total_microseconds(self) -> i128 {
        i128::from(self.days) * DAY_MICROSECONDS + i128::from(self.seconds) * 1_000_000 + i128::from(self.microseconds)
    }

    /// Returns the duration as total whole seconds plus fractional microseconds.
    #[must_use]
    pub fn total_seconds(self) -> f64 {
        self.total_microseconds() as f64 / 1_000_000.0
    }

    /// Returns total seconds only when exact (no microseconds), otherwise `None`.
    #[must_use]
    pub fn exact_total_seconds(self) -> Option<i128> {
        if self.microseconds == 0 {
            Some(i128::from(self.days) * DAY_SECONDS + i128::from(self.seconds))
        } else {
            None
        }
    }

    /// Builds a normalized timedelta from an arbitrary microsecond count.
    pub fn from_total_microseconds(total_microseconds: i128) -> RunResult<Self> {
        let days = total_microseconds.div_euclid(DAY_MICROSECONDS);
        let rem = total_microseconds.rem_euclid(DAY_MICROSECONDS);
        let seconds = rem / 1_000_000;
        let micros = rem % 1_000_000;

        if !(i128::from(MIN_TIMEDELTA_DAYS)..=i128::from(MAX_TIMEDELTA_DAYS)).contains(&days) {
            return Err(SimpleException::new_msg(
                ExcType::OverflowError,
                format!("days={days}; must have magnitude <= 999999999"),
            )
            .into());
        }

        Self::new(
            i32::try_from(days).expect("days validated to fit i32"),
            i32::try_from(seconds).expect("seconds are bounded by one day"),
            i32::try_from(micros).expect("microseconds are bounded by one second"),
        )
    }

    /// Creates a `timedelta` from constructor arguments.
    ///
    /// Supports positional `(days, seconds, microseconds)` and keyword arguments
    /// `days`, `seconds`, `microseconds`, `milliseconds`, `minutes`, `hours`, `weeks`.
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
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

        let delta = Self::from_total_microseconds(total_microseconds)?;
        Ok(Value::Ref(heap.allocate(HeapData::TimeDelta(delta))?))
    }
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
        Ok(self == other)
    }

    fn py_cmp(
        &self,
        other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> Result<Option<std::cmp::Ordering>, ResourceError> {
        Ok(self.total_microseconds().partial_cmp(&other.total_microseconds()))
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {}

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        self.days != 0 || self.seconds != 0 || self.microseconds != 0
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        _heap: &Heap<impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> std::fmt::Result {
        if self.days == 0 && self.seconds == 0 && self.microseconds == 0 {
            return f.write_str("datetime.timedelta(0)");
        }
        f.write_str("datetime.timedelta(")?;
        let mut first = true;
        if self.days != 0 {
            write!(f, "days={}", self.days)?;
            first = false;
        }
        if self.seconds != 0 {
            if !first {
                f.write_str(", ")?;
            }
            write!(f, "seconds={}", self.seconds)?;
            first = false;
        }
        if self.microseconds != 0 {
            if !first {
                f.write_str(", ")?;
            }
            write!(f, "microseconds={}", self.microseconds)?;
        }
        f.write_char(')')
    }

    fn py_str(
        &self,
        _heap: &Heap<impl ResourceTracker>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> std::borrow::Cow<'static, str> {
        let hours = self.seconds / 3600;
        let minutes = (self.seconds % 3600) / 60;
        let seconds = self.seconds % 60;
        let time = if self.microseconds == 0 {
            format!("{hours}:{minutes:02}:{seconds:02}")
        } else {
            format!("{hours}:{minutes:02}:{seconds:02}.{:06}", self.microseconds)
        };

        if self.days == 0 {
            return std::borrow::Cow::Owned(time);
        }

        let day_word = if self.days.abs() == 1 { "day" } else { "days" };
        std::borrow::Cow::Owned(format!("{} {day_word}, {time}", self.days))
    }

    fn py_add(
        &self,
        other: &Self,
        heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> Result<Option<Value>, ResourceError> {
        let total = self.total_microseconds() + other.total_microseconds();
        let Ok(result) = Self::from_total_microseconds(total) else {
            return Ok(None);
        };
        Ok(Some(Value::Ref(heap.allocate(HeapData::TimeDelta(result))?)))
    }

    fn py_sub(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>) -> Result<Option<Value>, ResourceError> {
        let total = self.total_microseconds() - other.total_microseconds();
        let Ok(result) = Self::from_total_microseconds(total) else {
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
            return Ok(Value::Float(self.total_seconds()));
        }
        Err(ExcType::attribute_error(self.py_type(heap), attr.as_str(interns)))
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}
