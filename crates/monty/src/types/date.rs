//! Python `datetime.date` implementation.
//!
//! Monty stores dates with `chrono::NaiveDate` and keeps CPython-compatible
//! constructor validation and arithmetic behavior.

use std::fmt::Write;

use ahash::AHashSet;
use chrono::{DateTime, Datelike, NaiveDate, Utc};

use crate::{
    args::ArgValues,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData, HeapId},
    intern::Interns,
    os::OsFunction,
    resource::{DepthGuard, ResourceError, ResourceTracker},
    types::{AttrCallResult, PyTrait, TimeDelta, Type, timedelta},
    value::Value,
};

/// `datetime.date` storage backed by `chrono::NaiveDate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct Date(pub(crate) NaiveDate);

/// Creates a date from validated civil components.
pub(crate) fn from_ymd(year: i32, month: i32, day: i32) -> RunResult<Date> {
    if !(1..=9999).contains(&year) {
        return Err(SimpleException::new_msg(ExcType::ValueError, format!("year {year} is out of range")).into());
    }
    if !(1..=12).contains(&month) {
        return Err(SimpleException::new_msg(ExcType::ValueError, "month must be in 1..12").into());
    }
    let Ok(month) = u32::try_from(month) else {
        return Err(SimpleException::new_msg(ExcType::ValueError, "month must be in 1..12").into());
    };
    let Ok(day) = u32::try_from(day) else {
        return Err(SimpleException::new_msg(ExcType::ValueError, "day is out of range for month").into());
    };

    let Some(date) = NaiveDate::from_ymd_opt(year, month, day) else {
        return Err(SimpleException::new_msg(ExcType::ValueError, "day is out of range for month").into());
    };
    Ok(Date(date))
}

/// Creates a date from a proleptic Gregorian ordinal value.
pub(crate) fn from_ordinal(ordinal: i32) -> RunResult<Date> {
    let Some(date) = NaiveDate::from_num_days_from_ce_opt(ordinal) else {
        return Err(SimpleException::new_msg(ExcType::OverflowError, "date value out of range").into());
    };
    if !(1..=9999).contains(&date.year()) {
        return Err(SimpleException::new_msg(ExcType::OverflowError, "date value out of range").into());
    }
    Ok(Date(date))
}

/// Returns the proleptic Gregorian ordinal (`1 == 0001-01-01`) for a date.
#[must_use]
pub(crate) fn to_ordinal(date: Date) -> i32 {
    date.0.num_days_from_ce()
}

/// Returns civil components `(year, month, day)`.
#[must_use]
pub(crate) fn to_ymd(date: Date) -> (i32, u32, u32) {
    (date.0.year(), date.0.month(), date.0.day())
}

/// Creates a date from local wall-clock microseconds since Unix epoch.
pub(crate) fn from_local_unix_micros(local_unix_micros: i64) -> RunResult<Date> {
    let Some(datetime) = DateTime::<Utc>::from_timestamp_micros(local_unix_micros) else {
        return Err(SimpleException::new_msg(ExcType::OverflowError, "date value out of range").into());
    };
    let date = datetime.date_naive();
    if !(1..=9999).contains(&date.year()) {
        return Err(SimpleException::new_msg(ExcType::OverflowError, "date value out of range").into());
    }
    Ok(Date(date))
}

/// Constructor for `date(year, month, day)`.
pub(crate) fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
    let (pos, kwargs) = args.into_parts();
    defer_drop_mut!(pos, heap);
    let kwargs = kwargs.into_iter();
    defer_drop_mut!(kwargs, heap);

    let mut year: Option<i32> = None;
    let mut month: Option<i32> = None;
    let mut day: Option<i32> = None;

    for (index, arg) in pos.by_ref().enumerate() {
        defer_drop!(arg, heap);
        match index {
            0 => year = Some(value_to_i32(arg, heap)?),
            1 => month = Some(value_to_i32(arg, heap)?),
            2 => day = Some(value_to_i32(arg, heap)?),
            _ => return Err(ExcType::type_error_at_most("date", 3, index + 1)),
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
                    return Err(ExcType::type_error_multiple_values("date", "year"));
                }
                year = Some(value_to_i32(value, heap)?);
            }
            "month" => {
                if month.is_some() {
                    return Err(ExcType::type_error_multiple_values("date", "month"));
                }
                month = Some(value_to_i32(value, heap)?);
            }
            "day" => {
                if day.is_some() {
                    return Err(ExcType::type_error_multiple_values("date", "day"));
                }
                day = Some(value_to_i32(value, heap)?);
            }
            _ => return Err(ExcType::type_error_unexpected_keyword("date", key_name)),
        }
    }

    let Some(year) = year else {
        return Err(ExcType::type_error_missing_positional_with_names(
            "date",
            &["year", "month", "day"],
        ));
    };
    let Some(month) = month else {
        return Err(ExcType::type_error_missing_positional_with_names(
            "date",
            &["month", "day"],
        ));
    };
    let Some(day) = day else {
        return Err(ExcType::type_error_missing_positional_with_names("date", &["day"]));
    };

    let date = from_ymd(year, month, day)?;
    Ok(Value::Ref(heap.allocate(HeapData::Date(date))?))
}

/// Classmethod implementation for `date.today()`.
///
/// This issues the shared `datetime.now` OS callback. The VM resume path uses the
/// encoded internal mode argument to convert the callback payload into a `date`.
pub(crate) fn class_today(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<AttrCallResult> {
    args.check_zero_args("date.today", heap)?;
    Ok(AttrCallResult::OsCall(
        OsFunction::DateTimeNow,
        ArgValues::One(Value::Int(0)),
    ))
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

impl PyTrait for Date {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::Date
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
        Ok(self.partial_cmp(other))
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
        let (year, month, day) = to_ymd(*self);
        write!(f, "datetime.date({year}, {month}, {day})")
    }

    fn py_str(
        &self,
        _heap: &Heap<impl ResourceTracker>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> std::borrow::Cow<'static, str> {
        let (year, month, day) = to_ymd(*self);
        std::borrow::Cow::Owned(format!("{year:04}-{month:02}-{day:02}"))
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
        // `date - date` returns timedelta days difference.
        let diff_days = i64::from(to_ordinal(*self)) - i64::from(to_ordinal(*other));
        let Ok(delta) = timedelta::from_total_microseconds(i128::from(diff_days) * 86_400_000_000) else {
            return Ok(None);
        };
        Ok(Some(Value::Ref(heap.allocate(HeapData::TimeDelta(delta))?)))
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

/// `date + timedelta` helper with the correct operand type.
pub(crate) fn py_add(
    date: Date,
    delta: &TimeDelta,
    heap: &mut Heap<impl ResourceTracker>,
    _interns: &Interns,
) -> Result<Option<Value>, ResourceError> {
    let (days, _, _) = timedelta::components(delta);
    let new_ordinal = i64::from(to_ordinal(date)).checked_add(i64::from(days));
    let Some(new_ordinal) = new_ordinal else {
        return Ok(None);
    };
    let Ok(new_ordinal) = i32::try_from(new_ordinal) else {
        return Ok(None);
    };
    match from_ordinal(new_ordinal) {
        Ok(value) => Ok(Some(Value::Ref(heap.allocate(HeapData::Date(value))?))),
        Err(_) => Ok(None),
    }
}

/// `date - timedelta` helper.
pub(crate) fn py_sub_timedelta(
    date: Date,
    delta: &TimeDelta,
    heap: &mut Heap<impl ResourceTracker>,
) -> Result<Option<Value>, ResourceError> {
    let (days, _, _) = timedelta::components(delta);
    let new_ordinal = i64::from(to_ordinal(date)).checked_sub(i64::from(days));
    let Some(new_ordinal) = new_ordinal else {
        return Ok(None);
    };
    let Ok(new_ordinal) = i32::try_from(new_ordinal) else {
        return Ok(None);
    };
    match from_ordinal(new_ordinal) {
        Ok(value) => Ok(Some(Value::Ref(heap.allocate(HeapData::Date(value))?))),
        Err(_) => Ok(None),
    }
}

/// `date - date` helper.
pub(crate) fn py_sub_date(
    date: Date,
    other: Date,
    heap: &mut Heap<impl ResourceTracker>,
) -> Result<Option<Value>, ResourceError> {
    let diff_days = i64::from(to_ordinal(date)) - i64::from(to_ordinal(other));
    let Ok(delta) = timedelta::from_total_microseconds(i128::from(diff_days) * 86_400_000_000) else {
        return Ok(None);
    };
    Ok(Some(Value::Ref(heap.allocate(HeapData::TimeDelta(delta))?)))
}
