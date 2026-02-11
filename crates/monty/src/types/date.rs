//! Python `datetime.date` implementation.
//!
//! Dates are stored as proleptic Gregorian ordinals (`1 == 0001-01-01`) which
//! keeps arithmetic and comparisons simple and deterministic.

use std::fmt::Write;

use ahash::AHashSet;
use chrono::{Datelike, NaiveDate};

use crate::{
    args::ArgValues,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, RunResult, SimpleException},
    heap::{Heap, HeapData, HeapId},
    intern::Interns,
    os::OsFunction,
    resource::{DepthGuard, ResourceError, ResourceTracker},
    types::{AttrCallResult, PyTrait, TimeDelta, Type},
    value::Value,
};

/// Python `datetime.date` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct Date {
    /// Proleptic Gregorian ordinal (`1 == 0001-01-01`).
    pub ordinal: i32,
}

impl Date {
    /// Creates a date from validated civil components.
    pub fn from_ymd(year: i32, month: i32, day: i32) -> RunResult<Self> {
        if !(1..=9999).contains(&year) {
            return Err(SimpleException::new_msg(ExcType::ValueError, format!("year {year} is out of range")).into());
        }
        if !(1..=12).contains(&month) {
            return Err(SimpleException::new_msg(ExcType::ValueError, "month must be in 1..12").into());
        }
        let month_u32 = u32::try_from(month).expect("month already validated");
        let day_u32 = u32::try_from(day).ok();
        let Some(day_u32) = day_u32 else {
            return Err(SimpleException::new_msg(ExcType::ValueError, "day is out of range for month").into());
        };
        let Some(date) = NaiveDate::from_ymd_opt(year, month_u32, day_u32) else {
            return Err(SimpleException::new_msg(ExcType::ValueError, "day is out of range for month").into());
        };
        Ok(Self {
            ordinal: date.num_days_from_ce(),
        })
    }

    /// Creates a date from an ordinal value.
    pub fn from_ordinal(ordinal: i32) -> RunResult<Self> {
        let Some(date) = NaiveDate::from_num_days_from_ce_opt(ordinal) else {
            return Err(SimpleException::new_msg(ExcType::OverflowError, "date value out of range").into());
        };
        if !(1..=9999).contains(&date.year()) {
            return Err(SimpleException::new_msg(ExcType::OverflowError, "date value out of range").into());
        }
        Ok(Self { ordinal })
    }

    /// Returns civil components `(year, month, day)`.
    #[must_use]
    pub fn to_ymd(self) -> (i32, u32, u32) {
        let date = NaiveDate::from_num_days_from_ce_opt(self.ordinal).expect("stored ordinal is always valid");
        (date.year(), date.month(), date.day())
    }

    /// Creates a date from local wall-clock microseconds since Unix epoch.
    pub fn from_local_unix_micros(local_unix_micros: i64) -> RunResult<Self> {
        let seconds = local_unix_micros.div_euclid(1_000_000);
        let micros = local_unix_micros.rem_euclid(1_000_000);
        let micros_u32 = u32::try_from(micros).expect("rem_euclid keeps micros in range");
        let nanos = micros_u32 * 1_000;
        let Some(dt) = chrono::DateTime::from_timestamp(seconds, nanos) else {
            return Err(SimpleException::new_msg(ExcType::OverflowError, "date value out of range").into());
        };
        let date = dt.date_naive();
        Self::from_ymd(
            date.year(),
            i32::try_from(date.month()).expect("month fits i32"),
            i32::try_from(date.day()).expect("day fits i32"),
        )
    }

    /// Constructor for `date(year, month, day)`.
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
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

        let date = Self::from_ymd(year, month, day)?;
        Ok(Value::Ref(heap.allocate(HeapData::Date(date))?))
    }

    /// Classmethod implementation for `date.today()`.
    ///
    /// This issues the shared `datetime.now` OS callback. The VM resume path uses the
    /// encoded internal mode argument to convert the callback payload into a `date`.
    pub fn class_today(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<AttrCallResult> {
        args.check_zero_args("date.today", heap)?;
        Ok(AttrCallResult::OsCall(
            OsFunction::DateTimeNow,
            ArgValues::One(Value::Int(0)),
        ))
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
        Ok(self.ordinal == other.ordinal)
    }

    fn py_cmp(
        &self,
        other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> Result<Option<std::cmp::Ordering>, ResourceError> {
        Ok(self.ordinal.partial_cmp(&other.ordinal))
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
        let (year, month, day) = self.to_ymd();
        write!(f, "datetime.date({year}, {month}, {day})")
    }

    fn py_str(
        &self,
        _heap: &Heap<impl ResourceTracker>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> std::borrow::Cow<'static, str> {
        let (year, month, day) = self.to_ymd();
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
        let diff_days = i64::from(self.ordinal) - i64::from(other.ordinal);
        let Ok(delta) = TimeDelta::from_total_microseconds(i128::from(diff_days) * 86_400_000_000) else {
            return Ok(None);
        };
        Ok(Some(Value::Ref(heap.allocate(HeapData::TimeDelta(delta))?)))
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

impl Date {
    /// `date + timedelta` helper with the correct operand type.
    pub fn py_add(
        self,
        delta: &TimeDelta,
        heap: &mut Heap<impl ResourceTracker>,
        _interns: &Interns,
    ) -> Result<Option<Value>, ResourceError> {
        let new_ordinal = self.ordinal.saturating_add(delta.days);
        match Self::from_ordinal(new_ordinal) {
            Ok(date) => Ok(Some(Value::Ref(heap.allocate(HeapData::Date(date))?))),
            Err(_) => Ok(None),
        }
    }

    /// `date - timedelta` helper.
    pub fn py_sub(
        self,
        delta: &TimeDelta,
        heap: &mut Heap<impl ResourceTracker>,
    ) -> Result<Option<Value>, ResourceError> {
        let new_ordinal = self.ordinal.saturating_sub(delta.days);
        match Self::from_ordinal(new_ordinal) {
            Ok(date) => Ok(Some(Value::Ref(heap.allocate(HeapData::Date(date))?))),
            Err(_) => Ok(None),
        }
    }

    /// `date - date` helper.
    pub fn py_sub_date(
        self,
        other: Self,
        heap: &mut Heap<impl ResourceTracker>,
    ) -> Result<Option<Value>, ResourceError> {
        let diff_days = i64::from(self.ordinal) - i64::from(other.ordinal);
        let Ok(delta) = TimeDelta::from_total_microseconds(i128::from(diff_days) * 86_400_000_000) else {
            return Ok(None);
        };
        Ok(Some(Value::Ref(heap.allocate(HeapData::TimeDelta(delta))?)))
    }
}
