//! Python `datetime.timezone` implementation for fixed-offset zones.
//!
//! Phase 1 intentionally supports only fixed offsets (no DST or IANA database).

use std::{borrow::Cow, fmt::Write};

use ahash::AHashSet;

use crate::{
    args::ArgValues,
    defer_drop,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData, HeapId},
    intern::Interns,
    resource::{DepthGuard, ResourceError, ResourceTracker},
    types::{PyTrait, TimeDelta, Type, timedelta},
    value::Value,
};

/// Minimum allowed timezone offset in seconds: -23:59.
pub(crate) const MIN_TIMEZONE_OFFSET_SECONDS: i32 = -86_399;
/// Maximum allowed timezone offset in seconds: +23:59.
pub(crate) const MAX_TIMEZONE_OFFSET_SECONDS: i32 = 86_399;

/// Python `datetime.timezone` value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct TimeZone {
    /// Fixed offset in seconds from UTC.
    pub offset_seconds: i32,
    /// Optional display name.
    pub name: Option<String>,
}

impl TimeZone {
    /// Creates a new fixed-offset timezone after validating CPython-compatible bounds.
    pub fn new(offset_seconds: i32, name: Option<String>) -> RunResult<Self> {
        if !(MIN_TIMEZONE_OFFSET_SECONDS..=MAX_TIMEZONE_OFFSET_SECONDS).contains(&offset_seconds) {
            return Err(SimpleException::new_msg(
                ExcType::ValueError,
                format!(
                    "offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24), not datetime.timedelta(seconds={offset_seconds})"
                ),
            )
            .into());
        }
        Ok(Self { offset_seconds, name })
    }

    /// Returns the canonical UTC timezone singleton value.
    #[must_use]
    pub fn utc() -> Self {
        Self {
            offset_seconds: 0,
            name: None,
        }
    }

    /// Parses timezone constructor arguments.
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
        let (offset_arg, name_arg) = args.get_one_two_args("timezone", heap)?;
        defer_drop!(offset_arg, heap);

        let offset_seconds = extract_offset_seconds(offset_arg, heap)?;
        let name = if let Some(name_arg) = name_arg {
            defer_drop!(name_arg, heap);
            extract_name(name_arg, heap, interns)?
        } else {
            None
        };

        let tz = Self::new(offset_seconds, name)?;
        Ok(Value::Ref(heap.allocate(HeapData::TimeZone(tz))?))
    }

    /// Formats offset as `+HH:MM` or `-HH:MM`.
    #[must_use]
    pub fn format_utc_offset(&self) -> String {
        let sign = if self.offset_seconds >= 0 { '+' } else { '-' };
        let abs = self.offset_seconds.abs();
        let hours = abs / 3600;
        let minutes = (abs % 3600) / 60;
        format!("{sign}{hours:02}:{minutes:02}")
    }
}

fn extract_offset_seconds(offset_arg: &Value, heap: &Heap<impl ResourceTracker>) -> RunResult<i32> {
    let Value::Ref(offset_id) = offset_arg else {
        return Err(ExcType::type_error(format!(
            "timezone() argument 1 must be datetime.timedelta, not {}",
            offset_arg.py_type(heap)
        )));
    };
    let HeapData::TimeDelta(delta) = heap.get(*offset_id) else {
        return Err(ExcType::type_error(format!(
            "timezone() argument 1 must be datetime.timedelta, not {}",
            offset_arg.py_type(heap)
        )));
    };

    let Some(total_seconds) = timedelta::exact_total_seconds(delta) else {
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            "offset must be a timedelta representing a whole number of seconds",
        )
        .into());
    };

    if !(i128::from(MIN_TIMEZONE_OFFSET_SECONDS)..=i128::from(MAX_TIMEZONE_OFFSET_SECONDS)).contains(&total_seconds) {
        let timedelta_repr = format_timedelta_repr(delta);
        return Err(SimpleException::new_msg(
            ExcType::ValueError,
            format!(
                "offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24), not {timedelta_repr}"
            ),
        )
        .into());
    }

    i32::try_from(total_seconds)
        .map_err(|_| SimpleException::new_msg(ExcType::ValueError, "timezone offset out of range").into())
}

fn format_timedelta_repr(delta: &TimeDelta) -> String {
    let (days, seconds, microseconds) = timedelta::components(delta);
    if days == 0 && seconds == 0 && microseconds == 0 {
        return "datetime.timedelta(0)".to_owned();
    }

    let mut repr = String::from("datetime.timedelta(");
    let mut first = true;
    if days != 0 {
        write!(repr, "days={days}").expect("writing to String cannot fail");
        first = false;
    }
    if seconds != 0 {
        if !first {
            repr.push_str(", ");
        }
        write!(repr, "seconds={seconds}").expect("writing to String cannot fail");
        first = false;
    }
    if microseconds != 0 {
        if !first {
            repr.push_str(", ");
        }
        write!(repr, "microseconds={microseconds}").expect("writing to String cannot fail");
    }
    repr.push(')');
    repr
}

fn extract_name(name_arg: &Value, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> RunResult<Option<String>> {
    match name_arg {
        Value::None => Ok(None),
        Value::InternString(id) => Ok(Some(interns.get_str(*id).to_owned())),
        Value::Ref(id) => match heap.get(*id) {
            HeapData::Str(s) => Ok(Some(s.as_str().to_owned())),
            _ => Err(ExcType::type_error(format!(
                "timezone() argument 2 must be str, not {}",
                name_arg.py_type(heap)
            ))),
        },
        _ => Err(ExcType::type_error(format!(
            "timezone() argument 2 must be str, not {}",
            name_arg.py_type(heap)
        ))),
    }
}

impl PyTrait for TimeZone {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::TimeZone
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
        Ok(self.offset_seconds == other.offset_seconds && self.name == other.name)
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {}

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        _heap: &Heap<impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> std::fmt::Result {
        if self.offset_seconds == 0 && self.name.is_none() {
            return f.write_str("datetime.timezone.utc");
        }

        write!(
            f,
            "datetime.timezone(datetime.timedelta(seconds={})",
            self.offset_seconds
        )?;
        if let Some(name) = &self.name {
            write!(f, ", {name:?}")?;
        }
        f.write_char(')')?;
        Ok(())
    }

    fn py_str(
        &self,
        _heap: &Heap<impl ResourceTracker>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> Cow<'static, str> {
        if let Some(name) = &self.name {
            return Cow::Owned(name.clone());
        }
        if self.offset_seconds == 0 {
            return Cow::Borrowed("UTC");
        }
        Cow::Owned(format!("UTC{}", self.format_utc_offset()))
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>() + self.name.as_ref().map_or(0, String::len)
    }
}
