//! Python `datetime.datetime` implementation.
//!
//! The phase-1 model supports:
//! - naive datetimes (civil time only)
//! - fixed-offset aware datetimes via `datetime.timezone`
//! - arithmetic with `timedelta`
//! - aware/naive comparison rules matching CPython semantics for equality and ordering.

use std::{borrow::Cow, fmt::Write};

use ahash::AHashSet;
use chrono::{Datelike, NaiveDate, Timelike};

use crate::{
    args::ArgValues,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings, StringId},
    os::OsFunction,
    resource::{DepthGuard, ResourceError, ResourceTracker},
    types::{AttrCallResult, PyTrait, TimeDelta, TimeZone, Type},
    value::Value,
};

const MICROS_PER_SECOND: i64 = 1_000_000;

/// Python `datetime.datetime` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct DateTime {
    /// Naive: civil microseconds since Unix epoch.
    /// Aware: UTC microseconds since Unix epoch.
    pub micros: i64,
    /// None for naive, fixed offset seconds for aware.
    pub offset_seconds: Option<i32>,
}

impl DateTime {
    /// Creates a datetime from civil components and optional fixed offset.
    #[expect(clippy::too_many_arguments)]
    pub fn from_components(
        year: i32,
        month: i32,
        day: i32,
        hour: i32,
        minute: i32,
        second: i32,
        microsecond: i32,
        tzinfo: Option<TimeZone>,
    ) -> RunResult<Self> {
        if !(1..=9999).contains(&year) {
            return Err(SimpleException::new_msg(ExcType::ValueError, format!("year {year} is out of range")).into());
        }
        if !(1..=12).contains(&month) {
            return Err(SimpleException::new_msg(ExcType::ValueError, "month must be in 1..12").into());
        }
        if !(0..=23).contains(&hour) {
            return Err(SimpleException::new_msg(ExcType::ValueError, "hour must be in 0..23").into());
        }
        if !(0..=59).contains(&minute) {
            return Err(SimpleException::new_msg(ExcType::ValueError, "minute must be in 0..59").into());
        }
        if !(0..=59).contains(&second) {
            return Err(SimpleException::new_msg(ExcType::ValueError, "second must be in 0..59").into());
        }
        if !(0..=999_999).contains(&microsecond) {
            return Err(SimpleException::new_msg(ExcType::ValueError, "microsecond must be in 0..999999").into());
        }

        let Some(date) = NaiveDate::from_ymd_opt(
            year,
            u32::try_from(month).expect("month validated"),
            u32::try_from(day)
                .map_err(|_| SimpleException::new_msg(ExcType::ValueError, "day is out of range for month"))?,
        ) else {
            return Err(SimpleException::new_msg(ExcType::ValueError, "day is out of range for month").into());
        };
        let Some(dt) = date.and_hms_micro_opt(
            u32::try_from(hour).expect("hour validated"),
            u32::try_from(minute).expect("minute validated"),
            u32::try_from(second).expect("second validated"),
            u32::try_from(microsecond).expect("microsecond validated"),
        ) else {
            return Err(SimpleException::new_msg(ExcType::ValueError, "invalid datetime components").into());
        };
        let civil_micros = dt.and_utc().timestamp_micros();

        if let Some(tz) = tzinfo {
            let offset_micros = i64::from(tz.offset_seconds) * MICROS_PER_SECOND;
            let Some(utc_micros) = civil_micros.checked_sub(offset_micros) else {
                return Err(SimpleException::new_msg(ExcType::OverflowError, "date value out of range").into());
            };
            let value = Self {
                micros: utc_micros,
                offset_seconds: Some(tz.offset_seconds),
            };
            if value.civil_components().is_none() {
                return Err(SimpleException::new_msg(ExcType::OverflowError, "date value out of range").into());
            }
            Ok(value)
        } else {
            Ok(Self {
                micros: civil_micros,
                offset_seconds: None,
            })
        }
    }

    /// Creates a datetime from callback payload.
    pub fn from_now_payload(
        timestamp_utc: f64,
        local_offset_seconds: i32,
        tzinfo: Option<TimeZone>,
    ) -> RunResult<Self> {
        if !timestamp_utc.is_finite() {
            return Err(
                SimpleException::new_msg(ExcType::TypeError, "datetime.now payload timestamp must be finite").into(),
            );
        }
        let micros_f = timestamp_utc * 1_000_000.0;
        if micros_f < i64::MIN as f64 || micros_f > i64::MAX as f64 {
            return Err(SimpleException::new_msg(ExcType::OverflowError, "timestamp out of range").into());
        }
        let utc_micros = rounded_f64_to_i64(micros_f.round());

        if let Some(tz) = tzinfo {
            let value = Self {
                micros: utc_micros,
                offset_seconds: Some(tz.offset_seconds),
            };
            if value.civil_components().is_none() {
                return Err(SimpleException::new_msg(ExcType::OverflowError, "date value out of range").into());
            }
            Ok(value)
        } else {
            let local_delta = i64::from(local_offset_seconds) * MICROS_PER_SECOND;
            let Some(local_micros) = utc_micros.checked_add(local_delta) else {
                return Err(SimpleException::new_msg(ExcType::OverflowError, "date value out of range").into());
            };
            let value = Self {
                micros: local_micros,
                offset_seconds: None,
            };
            if value.civil_components().is_none() {
                return Err(SimpleException::new_msg(ExcType::OverflowError, "date value out of range").into());
            }
            Ok(value)
        }
    }

    /// Returns true when this is an aware datetime.
    #[must_use]
    pub fn is_aware(self) -> bool {
        self.offset_seconds.is_some()
    }

    /// Returns local civil microseconds (aware values apply fixed offset).
    fn civil_micros(self) -> Option<i64> {
        match self.offset_seconds {
            Some(offset) => self.micros.checked_add(i64::from(offset) * MICROS_PER_SECOND),
            None => Some(self.micros),
        }
    }

    /// Returns `(year, month, day, hour, minute, second, microsecond)` in local civil time.
    fn civil_components(self) -> Option<(i32, u32, u32, u32, u32, u32, u32)> {
        let civil_micros = self.civil_micros()?;
        let seconds = civil_micros.div_euclid(MICROS_PER_SECOND);
        let micros = civil_micros.rem_euclid(MICROS_PER_SECOND);
        let nanos = u32::try_from(micros).ok()?.saturating_mul(1_000);
        let dt = chrono::DateTime::from_timestamp(seconds, nanos)?;
        let naive = dt.naive_utc();
        if !(1..=9999).contains(&naive.year()) {
            return None;
        }
        Some((
            naive.year(),
            naive.month(),
            naive.day(),
            naive.hour(),
            naive.minute(),
            naive.second(),
            naive.and_utc().timestamp_subsec_micros(),
        ))
    }

    /// Returns civil components in compact integer widths for object conversion.
    #[must_use]
    pub fn to_components(self) -> Option<(i32, u8, u8, u8, u8, u8, u32)> {
        let (year, month, day, hour, minute, second, microsecond) = self.civil_components()?;
        Some((
            year,
            u8::try_from(month).ok()?,
            u8::try_from(day).ok()?,
            u8::try_from(hour).ok()?,
            u8::try_from(minute).ok()?,
            u8::try_from(second).ok()?,
            microsecond,
        ))
    }

    /// Constructor for `datetime(...)`.
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
        let (pos, kwargs) = args.into_parts();
        defer_drop_mut!(pos, heap);
        let kwargs = kwargs.into_iter();
        defer_drop_mut!(kwargs, heap);

        let mut year: Option<i32> = None;
        let mut month: Option<i32> = None;
        let mut day: Option<i32> = None;
        let mut hour: i32 = 0;
        let mut minute: i32 = 0;
        let mut second: i32 = 0;
        let mut microsecond: i32 = 0;
        let mut tzinfo: Option<TimeZone> = None;

        for (index, arg) in pos.by_ref().enumerate() {
            defer_drop!(arg, heap);
            match index {
                0 => year = Some(value_to_i32(arg, heap)?),
                1 => month = Some(value_to_i32(arg, heap)?),
                2 => day = Some(value_to_i32(arg, heap)?),
                3 => hour = value_to_i32(arg, heap)?,
                4 => minute = value_to_i32(arg, heap)?,
                5 => second = value_to_i32(arg, heap)?,
                6 => microsecond = value_to_i32(arg, heap)?,
                7 => tzinfo = tzinfo_from_value(arg, heap)?,
                _ => return Err(ExcType::type_error_at_most("datetime", 8, index + 1)),
            }
        }

        for (key, value) in kwargs {
            defer_drop!(key, heap);
            defer_drop!(value, heap);
            let Some(key_name) = key.as_either_str(heap) else {
                return Err(ExcType::type_error_kwargs_nonstring_key());
            };
            let key_name = key_name.as_str(interns);
            match key_name {
                "year" => {
                    if year.is_some() {
                        return Err(ExcType::type_error_multiple_values("datetime", "year"));
                    }
                    year = Some(value_to_i32(value, heap)?);
                }
                "month" => {
                    if month.is_some() {
                        return Err(ExcType::type_error_multiple_values("datetime", "month"));
                    }
                    month = Some(value_to_i32(value, heap)?);
                }
                "day" => {
                    if day.is_some() {
                        return Err(ExcType::type_error_multiple_values("datetime", "day"));
                    }
                    day = Some(value_to_i32(value, heap)?);
                }
                "hour" => hour = value_to_i32(value, heap)?,
                "minute" => minute = value_to_i32(value, heap)?,
                "second" => second = value_to_i32(value, heap)?,
                "microsecond" => microsecond = value_to_i32(value, heap)?,
                "tzinfo" => tzinfo = tzinfo_from_value(value, heap)?,
                _ => return Err(ExcType::type_error_unexpected_keyword("datetime", key_name)),
            }
        }

        let Some(year) = year else {
            return Err(ExcType::type_error_missing_positional_with_names(
                "datetime",
                &["year", "month", "day"],
            ));
        };
        let Some(month) = month else {
            return Err(ExcType::type_error_missing_positional_with_names(
                "datetime",
                &["month", "day"],
            ));
        };
        let Some(day) = day else {
            return Err(ExcType::type_error_missing_positional_with_names("datetime", &["day"]));
        };

        let dt = Self::from_components(year, month, day, hour, minute, second, microsecond, tzinfo)?;
        Ok(Value::Ref(heap.allocate(HeapData::DateTime(dt))?))
    }

    /// Classmethod implementation for `datetime.now(tz=None)`.
    pub fn class_now(
        heap: &mut Heap<impl ResourceTracker>,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<AttrCallResult> {
        let (pos, kwargs) = args.into_parts();
        defer_drop_mut!(pos, heap);
        let kwargs = kwargs.into_iter();
        defer_drop_mut!(kwargs, heap);

        let mut tzinfo: Option<TimeZone> = None;
        let mut seen_tz = false;

        for (index, arg) in pos.by_ref().enumerate() {
            defer_drop!(arg, heap);
            match index {
                0 => {
                    tzinfo = tzinfo_from_value(arg, heap)?;
                    seen_tz = true;
                }
                _ => return Err(ExcType::type_error_at_most("datetime.now", 1, index + 1)),
            }
        }

        for (key, value) in kwargs {
            defer_drop!(key, heap);
            defer_drop!(value, heap);
            let Some(key_name) = key.as_either_str(heap) else {
                return Err(ExcType::type_error_kwargs_nonstring_key());
            };
            let key_name = key_name.as_str(interns);
            if key_name != "tz" {
                return Err(ExcType::type_error_unexpected_keyword("datetime.now", key_name));
            }
            if seen_tz {
                return Err(ExcType::type_error_multiple_values("datetime.now", "tz"));
            }
            tzinfo = tzinfo_from_value(value, heap)?;
            seen_tz = true;
        }

        // Internal-only mode encoding:
        // 1 => datetime.now(tz=None), 2 => datetime.now(tz=<fixed offset>)
        let os_args = match tzinfo {
            Some(tz) => ArgValues::Two(Value::Int(2), Value::Int(i64::from(tz.offset_seconds))),
            None => ArgValues::One(Value::Int(1)),
        };
        Ok(AttrCallResult::OsCall(OsFunction::DateTimeNow, os_args))
    }

    /// `datetime + timedelta`
    pub fn py_add(
        &self,
        delta: &TimeDelta,
        heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> Result<Option<Value>, ResourceError> {
        let delta_micros = delta.total_microseconds();
        let Ok(delta_micros_i64) = i64::try_from(delta_micros) else {
            return Ok(None);
        };
        let Some(next_micros) = self.micros.checked_add(delta_micros_i64) else {
            return Ok(None);
        };
        let value = Self {
            micros: next_micros,
            offset_seconds: self.offset_seconds,
        };
        if value.civil_components().is_none() {
            return Ok(None);
        }
        Ok(Some(Value::Ref(heap.allocate(HeapData::DateTime(value))?)))
    }

    /// `datetime - timedelta`
    pub fn py_sub_timedelta(
        &self,
        delta: &TimeDelta,
        heap: &mut Heap<impl ResourceTracker>,
    ) -> Result<Option<Value>, ResourceError> {
        let delta_micros = delta.total_microseconds();
        let Ok(delta_micros_i64) = i64::try_from(delta_micros) else {
            return Ok(None);
        };
        let Some(next_micros) = self.micros.checked_sub(delta_micros_i64) else {
            return Ok(None);
        };
        let value = Self {
            micros: next_micros,
            offset_seconds: self.offset_seconds,
        };
        if value.civil_components().is_none() {
            return Ok(None);
        }
        Ok(Some(Value::Ref(heap.allocate(HeapData::DateTime(value))?)))
    }
}

fn value_to_i32(value: &Value, heap: &Heap<impl ResourceTracker>) -> RunResult<i32> {
    let int_value = if let Value::Bool(b) = value {
        i64::from(*b)
    } else {
        value.as_int(heap)?
    };
    i32::try_from(int_value)
        .map_err(|_| SimpleException::new_msg(ExcType::OverflowError, "signed integer is greater than maximum").into())
}

fn tzinfo_from_value(value: &Value, heap: &Heap<impl ResourceTracker>) -> RunResult<Option<TimeZone>> {
    match value {
        Value::None => Ok(None),
        Value::Ref(id) => match heap.get(*id) {
            HeapData::TimeZone(tz) => Ok(Some(tz.clone())),
            _ => Err(ExcType::type_error(format!(
                "tzinfo argument must be None or datetime.timezone, not {}",
                value.py_type(heap)
            ))),
        },
        _ => Err(ExcType::type_error(format!(
            "tzinfo argument must be None or datetime.timezone, not {}",
            value.py_type(heap)
        ))),
    }
}

/// Converts a rounded finite `f64` to `i64`.
///
/// Callers must validate bounds before invoking this helper.
#[expect(
    clippy::cast_possible_truncation,
    reason = "callers check finite i64 bounds before casting"
)]
fn rounded_f64_to_i64(value: f64) -> i64 {
    value as i64
}

impl PyTrait for DateTime {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::DateTime
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
        if self.is_aware() != other.is_aware() {
            return Ok(false);
        }
        Ok(self.micros == other.micros)
    }

    fn py_cmp(
        &self,
        other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> Result<Option<std::cmp::Ordering>, ResourceError> {
        if self.is_aware() != other.is_aware() {
            return Ok(None);
        }
        Ok(self.micros.partial_cmp(&other.micros))
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
        let Some((year, month, day, hour, minute, second, microsecond)) = self.civil_components() else {
            return f.write_str("datetime.datetime(<out of range>)");
        };

        write!(f, "datetime.datetime({year}, {month}, {day}, {hour}, {minute}")?;
        if second != 0 || microsecond != 0 {
            write!(f, ", {second}")?;
        }
        if microsecond != 0 {
            write!(f, ", {microsecond}")?;
        }
        if let Some(offset) = self.offset_seconds {
            if offset == 0 {
                f.write_str(", tzinfo=datetime.timezone.utc")?;
            } else {
                write!(f, ", tzinfo=datetime.timezone(datetime.timedelta(seconds={offset}))")?;
            }
        }
        f.write_char(')')
    }

    fn py_str(
        &self,
        _heap: &Heap<impl ResourceTracker>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> Cow<'static, str> {
        let Some((year, month, day, hour, minute, second, microsecond)) = self.civil_components() else {
            return Cow::Borrowed("<out of range>");
        };
        let mut s = format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}");
        if microsecond != 0 {
            write!(s, ".{microsecond:06}").expect("writing to String cannot fail");
        }
        if let Some(offset) = self.offset_seconds {
            let sign = if offset >= 0 { '+' } else { '-' };
            let abs = offset.abs();
            let off_h = abs / 3600;
            let off_m = (abs % 3600) / 60;
            write!(s, "{sign}{off_h:02}:{off_m:02}").expect("writing to String cannot fail");
        }
        Cow::Owned(s)
    }

    fn py_add(
        &self,
        other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> Result<Option<Value>, ResourceError> {
        let _ = other;
        Ok(None)
    }

    fn py_sub(&self, other: &Self, heap: &mut Heap<impl ResourceTracker>) -> Result<Option<Value>, ResourceError> {
        if self.is_aware() != other.is_aware() {
            return Ok(None);
        }
        let diff = i128::from(self.micros) - i128::from(other.micros);
        let Ok(delta) = TimeDelta::from_total_microseconds(diff) else {
            return Ok(None);
        };
        Ok(Some(Value::Ref(heap.allocate(HeapData::TimeDelta(delta))?)))
    }

    fn py_getattr(
        &self,
        attr_id: StringId,
        heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> RunResult<Option<AttrCallResult>> {
        if attr_id == StaticStrings::Tzinfo {
            if let Some(offset) = self.offset_seconds {
                let tz = TimeZone::new(offset, None)?;
                return Ok(Some(AttrCallResult::Value(Value::Ref(
                    heap.allocate(HeapData::TimeZone(tz))?,
                ))));
            }
            return Ok(Some(AttrCallResult::Value(Value::None)));
        }
        Ok(None)
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}
