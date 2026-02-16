//! Python `datetime.datetime` implementation.
//!
//! Monty stores datetimes with `speedate::DateTime` and layers CPython-compatible
//! constructor rules, aware/naive comparison semantics, and arithmetic on top.

use std::{borrow::Cow, fmt::Write};

use ahash::AHashSet;
use serde::{Deserialize, Serialize};
use speedate::{DateTimeConfigBuilder, Time as SpeedTime, TimestampUnit};

use crate::{
    args::ArgValues,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings, StringId},
    os::OsFunction,
    resource::{DepthGuard, ResourceError, ResourceTracker},
    types::{AttrCallResult, PyTrait, TimeDelta, TimeZone, Type, date, timedelta, timezone},
    value::Value,
};

const MICROS_PER_SECOND: i64 = 1_000_000;
const DATE_OUT_OF_RANGE: &str = "date value out of range";

/// `datetime.datetime` storage backed directly by `speedate`.
pub(crate) type DateTime = speedate::DateTime;

/// Creates a datetime from civil components and optional fixed offset.
#[expect(clippy::too_many_arguments)]
pub(crate) fn from_components(
    year: i32,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
    second: i32,
    microsecond: i32,
    tzinfo: Option<TimeZone>,
) -> RunResult<DateTime> {
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

    let date_value = date::from_ymd(year, month, day)?;
    let offset_seconds = tzinfo.map(|tz| tz.offset_seconds);
    let datetime = DateTime {
        date: date_value,
        time: SpeedTime {
            hour: u8::try_from(hour).expect("hour validated to 0..=23"),
            minute: u8::try_from(minute).expect("minute validated to 0..=59"),
            second: u8::try_from(second).expect("second validated to 0..=59"),
            microsecond: u32::try_from(microsecond).expect("microsecond validated to 0..=999_999"),
            tz_offset: offset_seconds,
        },
    };

    if let Some(offset_seconds) = offset_seconds {
        let Some(utc_micros) = utc_micros(&datetime) else {
            return Err(SimpleException::new_msg(ExcType::OverflowError, DATE_OUT_OF_RANGE).into());
        };
        if from_utc_micros_with_offset(utc_micros, offset_seconds).is_none() {
            return Err(SimpleException::new_msg(ExcType::OverflowError, DATE_OUT_OF_RANGE).into());
        }
    }

    Ok(datetime)
}

/// Creates a datetime from callback payload.
pub(crate) fn from_now_payload(
    timestamp_utc: f64,
    local_offset_seconds: i32,
    tzinfo: Option<TimeZone>,
) -> RunResult<DateTime> {
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
        return from_utc_micros_with_offset(utc_micros, tz.offset_seconds)
            .ok_or_else(|| SimpleException::new_msg(ExcType::OverflowError, DATE_OUT_OF_RANGE).into());
    }

    let local_offset_micros = checked_offset_micros(local_offset_seconds)
        .ok_or_else(|| SimpleException::new_msg(ExcType::OverflowError, DATE_OUT_OF_RANGE))?;
    let local_micros = utc_micros
        .checked_add(local_offset_micros)
        .ok_or_else(|| SimpleException::new_msg(ExcType::OverflowError, DATE_OUT_OF_RANGE))?;
    from_local_unix_micros(local_micros)
        .ok_or_else(|| SimpleException::new_msg(ExcType::OverflowError, DATE_OUT_OF_RANGE).into())
}

/// Returns true when this is an aware datetime.
#[must_use]
pub(crate) fn is_aware(datetime: &DateTime) -> bool {
    datetime.time.tz_offset.is_some()
}

/// Returns the fixed offset seconds for aware datetimes.
#[must_use]
pub(crate) fn offset_seconds(datetime: &DateTime) -> Option<i32> {
    datetime.time.tz_offset
}

/// Returns civil components in compact integer widths for object conversion.
#[must_use]
pub(crate) fn to_components(datetime: &DateTime) -> Option<(i32, u8, u8, u8, u8, u8, u32)> {
    if datetime.date.year == 0 {
        return None;
    }
    Some((
        i32::from(datetime.date.year),
        datetime.date.month,
        datetime.date.day,
        datetime.time.hour,
        datetime.time.minute,
        datetime.time.second,
        datetime.time.microsecond,
    ))
}

/// Constructor for `datetime(...)`.
pub(crate) fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
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

    let dt = from_components(year, month, day, hour, minute, second, microsecond, tzinfo)?;
    Ok(Value::Ref(heap.allocate(HeapData::DateTime(dt))?))
}

/// Classmethod implementation for `datetime.now(tz=None)`.
pub(crate) fn class_now(
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
pub(crate) fn py_add(
    datetime: &DateTime,
    delta: &TimeDelta,
    heap: &mut Heap<impl ResourceTracker>,
    _interns: &Interns,
) -> Result<Option<Value>, ResourceError> {
    let delta_micros = timedelta::total_microseconds(delta);
    let Ok(delta_micros_i64) = i64::try_from(delta_micros) else {
        return Ok(None);
    };

    let next = if let Some(offset) = datetime.time.tz_offset {
        let Some(utc_micros) = utc_micros(datetime) else {
            return Ok(None);
        };
        let Some(next_utc_micros) = utc_micros.checked_add(delta_micros_i64) else {
            return Ok(None);
        };
        from_utc_micros_with_offset(next_utc_micros, offset)
    } else {
        let Some(local_unix_micros) = local_micros(datetime) else {
            return Ok(None);
        };
        let Some(next_local_unix_micros) = local_unix_micros.checked_add(delta_micros_i64) else {
            return Ok(None);
        };
        from_local_unix_micros(next_local_unix_micros)
    };

    let Some(next) = next else {
        return Ok(None);
    };
    Ok(Some(Value::Ref(heap.allocate(HeapData::DateTime(next))?)))
}

/// `datetime - timedelta`
pub(crate) fn py_sub_timedelta(
    datetime: &DateTime,
    delta: &TimeDelta,
    heap: &mut Heap<impl ResourceTracker>,
) -> Result<Option<Value>, ResourceError> {
    let delta_micros = timedelta::total_microseconds(delta);
    let Ok(delta_micros_i64) = i64::try_from(delta_micros) else {
        return Ok(None);
    };

    let next = if let Some(offset) = datetime.time.tz_offset {
        let Some(utc_micros) = utc_micros(datetime) else {
            return Ok(None);
        };
        let Some(next_utc_micros) = utc_micros.checked_sub(delta_micros_i64) else {
            return Ok(None);
        };
        from_utc_micros_with_offset(next_utc_micros, offset)
    } else {
        let Some(local_unix_micros) = local_micros(datetime) else {
            return Ok(None);
        };
        let Some(next_local_unix_micros) = local_unix_micros.checked_sub(delta_micros_i64) else {
            return Ok(None);
        };
        from_local_unix_micros(next_local_unix_micros)
    };

    let Some(next) = next else {
        return Ok(None);
    };
    Ok(Some(Value::Ref(heap.allocate(HeapData::DateTime(next))?)))
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

/// Returns local civil microseconds since Unix epoch.
#[must_use]
pub(crate) fn local_micros(datetime: &DateTime) -> Option<i64> {
    if datetime.date.year == 0 {
        return None;
    }
    let second = datetime.timestamp();
    let base = second.checked_mul(MICROS_PER_SECOND)?;
    base.checked_add(i64::from(datetime.time.microsecond))
}

/// Returns UTC microseconds since Unix epoch for aware datetimes.
#[must_use]
pub(crate) fn utc_micros(datetime: &DateTime) -> Option<i64> {
    let local_unix_micros = local_micros(datetime)?;
    match datetime.time.tz_offset {
        Some(offset_seconds) => {
            let offset_micros = checked_offset_micros(offset_seconds)?;
            local_unix_micros.checked_sub(offset_micros)
        }
        None => Some(local_unix_micros),
    }
}

fn from_local_unix_micros(local_unix_micros: i64) -> Option<DateTime> {
    let second = local_unix_micros.div_euclid(MICROS_PER_SECOND);
    let microsecond = local_unix_micros.rem_euclid(MICROS_PER_SECOND);
    let config = DateTimeConfigBuilder::new()
        .timestamp_unit(TimestampUnit::Second)
        .build();
    let datetime = DateTime::from_timestamp_with_config(second, u32::try_from(microsecond).ok()?, &config).ok()?;
    if datetime.date.year == 0 {
        return None;
    }
    Some(datetime)
}

fn from_utc_micros_with_offset(utc_micros: i64, offset_seconds: i32) -> Option<DateTime> {
    let offset_micros = checked_offset_micros(offset_seconds)?;
    let local_unix_micros = utc_micros.checked_add(offset_micros)?;
    let mut datetime = from_local_unix_micros(local_unix_micros)?;
    datetime.time.tz_offset = Some(offset_seconds);
    Some(datetime)
}

fn checked_offset_micros(offset_seconds: i32) -> Option<i64> {
    i64::from(offset_seconds).checked_mul(MICROS_PER_SECOND)
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
        if is_aware(self) != is_aware(other) {
            return Ok(false);
        }
        if is_aware(self) {
            return Ok(utc_micros(self) == utc_micros(other));
        }
        Ok(local_micros(self) == local_micros(other))
    }

    fn py_cmp(
        &self,
        other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> Result<Option<std::cmp::Ordering>, ResourceError> {
        if is_aware(self) != is_aware(other) {
            return Ok(None);
        }
        if is_aware(self) {
            return Ok(utc_micros(self).partial_cmp(&utc_micros(other)));
        }
        Ok(local_micros(self).partial_cmp(&local_micros(other)))
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
        let Some((year, month, day, hour, minute, second, microsecond)) = to_components(self) else {
            return f.write_str("datetime.datetime(<out of range>)");
        };

        write!(f, "datetime.datetime({year}, {month}, {day}, {hour}, {minute}")?;
        if second != 0 || microsecond != 0 {
            write!(f, ", {second}")?;
        }
        if microsecond != 0 {
            write!(f, ", {microsecond}")?;
        }
        if let Some(offset) = offset_seconds(self) {
            if offset == 0 {
                f.write_str(", tzinfo=datetime.timezone.utc")?;
            } else {
                let timedelta_repr = timezone::format_offset_timedelta_repr(offset);
                write!(f, ", tzinfo=datetime.timezone({timedelta_repr})")?;
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
        let Some((year, month, day, hour, minute, second, microsecond)) = to_components(self) else {
            return Cow::Borrowed("<out of range>");
        };
        let mut s = format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}");
        if microsecond != 0 {
            write!(s, ".{microsecond:06}").expect("writing to String cannot fail");
        }
        if let Some(offset) = offset_seconds(self) {
            s.push_str(&timezone::format_offset_hms(offset));
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
        if is_aware(self) != is_aware(other) {
            return Ok(None);
        }
        let diff = if is_aware(self) {
            let Some(lhs_utc_micros) = utc_micros(self) else {
                return Ok(None);
            };
            let Some(rhs_utc_micros) = utc_micros(other) else {
                return Ok(None);
            };
            i128::from(lhs_utc_micros) - i128::from(rhs_utc_micros)
        } else {
            let Some(lhs_local_micros) = local_micros(self) else {
                return Ok(None);
            };
            let Some(rhs_local_micros) = local_micros(other) else {
                return Ok(None);
            };
            i128::from(lhs_local_micros) - i128::from(rhs_local_micros)
        };
        let Ok(delta) = timedelta::from_total_microseconds(diff) else {
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
            if let Some(offset) = offset_seconds(self) {
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

#[derive(Serialize, Deserialize)]
struct LegacyDateTime {
    micros: i64,
    offset_seconds: Option<i32>,
}

/// Serde adapter that preserves the previous `(micros, offset_seconds)` snapshot format.
pub(crate) mod serde_speedate_datetime {
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _, ser::Error as _};

    use super::{
        DATE_OUT_OF_RANGE, DateTime, LegacyDateTime, from_local_unix_micros, from_utc_micros_with_offset, is_aware,
        local_micros, offset_seconds, utc_micros,
    };

    /// Serializes a `speedate::DateTime` using the previous internal micros format.
    pub(crate) fn serialize<S>(datetime: &DateTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let micros = if is_aware(datetime) {
            utc_micros(datetime)
        } else {
            local_micros(datetime)
        }
        .ok_or_else(|| S::Error::custom(DATE_OUT_OF_RANGE))?;
        LegacyDateTime {
            micros,
            offset_seconds: offset_seconds(datetime),
        }
        .serialize(serializer)
    }

    /// Deserializes a `speedate::DateTime` from the previous internal micros format.
    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<DateTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        let legacy = LegacyDateTime::deserialize(deserializer)?;
        let datetime = match legacy.offset_seconds {
            Some(offset_seconds) => from_utc_micros_with_offset(legacy.micros, offset_seconds),
            None => from_local_unix_micros(legacy.micros),
        };
        datetime.ok_or_else(|| D::Error::custom(DATE_OUT_OF_RANGE))
    }
}
