use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use strum::Display;

use crate::args::ArgValues;
use crate::callable::Callable;
use crate::exceptions::{exc_err_fmt, ExcType, SimpleException};
use crate::function::Function;
use crate::heap::HeapData;
use crate::heap::{Heap, HeapId};
use crate::run::RunResult;
use crate::values::bytes::bytes_repr;
use crate::values::str::string_repr;
use crate::values::PyTrait;

/// Primary value type representing Python objects at runtime.
///
/// This enum uses a hybrid design: small immediate values (Int, Bool, None) are stored
/// inline, while heap-allocated values (List, Str, Dict, etc.) are stored in the arena
/// and referenced via `Ref(HeapId)`.
///
/// NOTE: `Clone` is intentionally NOT derived. Use `clone_with_heap()` for heap values
/// or `clone_immediate()` for immediate values only. Direct cloning via `.clone()` would
/// bypass reference counting and cause memory leaks.
#[derive(Debug)]
pub enum Value<'c, 'e> {
    // Immediate values (stored inline, no heap allocation)
    Undefined,
    Ellipsis,
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Range(i64),
    InternString(&'e str),
    InternBytes(&'e [u8]),
    /// Exception instance (e.g., result of `ValueError('msg')`).
    Exc(SimpleException<'c>),
    /// Callables, nested enum to make calling easier, allow private until Value is privates
    #[allow(private_interfaces)]
    Callable(Callable<'c>),
    /// A function defined in the module
    #[allow(private_interfaces)]
    Function(&'e Function<'c>),

    // Heap-allocated values (stored in arena)
    Ref(HeapId),

    /// Sentinel value indicating this Value was properly cleaned up via `drop_with_heap`.
    /// Only exists when `dec-ref-check` feature is enabled. Used to verify reference counting
    /// correctness - if a `Ref` variant is dropped without calling `drop_with_heap`, the
    /// Drop impl will panic.
    #[cfg(feature = "dec-ref-check")]
    Dereferenced,
}

/// Drop implementation that panics if a `Ref` variant is dropped without calling `drop_with_heap`.
/// This helps catch reference counting bugs during development/testing.
/// Only enabled when the `dec-ref-check` feature is active.
#[cfg(feature = "dec-ref-check")]
impl Drop for Value<'_, '_> {
    fn drop(&mut self) {
        if let Value::Ref(id) = self {
            panic!("Value::Ref({id}) dropped without calling drop_with_heap() - this is a reference counting bug");
        }
    }
}

impl From<bool> for Value<'_, '_> {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl<'c, 'e> PyTrait<'c, 'e> for Value<'c, 'e> {
    fn py_type(&self, heap: &Heap<'c, 'e>) -> &'static str {
        match self {
            Self::Undefined => "undefined",
            Self::Ellipsis => "ellipsis",
            Self::None => "NoneType",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Range(_) => "range",
            Self::InternString(_) => "str",
            Self::InternBytes(_) => "bytes",
            Self::Exc(e) => e.type_str(),
            Self::Callable(c) => c.py_type(heap),
            Self::Function(_) => "function",
            Self::Ref(id) => heap.get(*id).py_type(heap),
            #[cfg(feature = "dec-ref-check")]
            Self::Dereferenced => panic!("Cannot access Dereferenced object"),
        }
    }

    fn py_len(&self, heap: &Heap<'c, 'e>) -> Option<usize> {
        match self {
            Self::InternString(s) => Some(s.len()),
            Self::InternBytes(b) => Some(b.len()),
            Self::Ref(id) => heap.get(*id).py_len(heap),
            _ => None,
        }
    }

    fn py_eq(&self, other: &Self, heap: &mut Heap<'c, 'e>) -> bool {
        match (self, other) {
            (Self::Undefined, _) => false,
            (_, Self::Undefined) => false,
            (Self::Int(v1), Self::Int(v2)) => v1 == v2,
            (Self::Range(v1), Self::Range(v2)) => v1 == v2,
            (Self::Bool(v1), Self::Bool(v2)) => v1 == v2,
            (Self::Bool(v1), Self::Int(v2)) => i64::from(*v1) == *v2,
            (Self::Int(v1), Self::Bool(v2)) => *v1 == i64::from(*v2),
            (Self::None, Self::None) => true,

            (Self::InternString(s1), Self::InternString(s2)) => s1 == s2,
            // for strings we need to account for the fact they might be either interned or not
            (Self::InternString(s1), Self::Ref(id2)) => {
                if let HeapData::Str(s2) = heap.get(*id2) {
                    *s1 == s2.as_str()
                } else {
                    false
                }
            }
            (Self::Ref(id1), Self::InternString(s2)) => {
                if let HeapData::Str(s1) = heap.get(*id1) {
                    s1.as_str() == *s2
                } else {
                    false
                }
            }

            (Self::InternBytes(b1), Self::InternBytes(b2)) => b1 == b2,
            // same for bytes
            (Self::InternBytes(b1), Self::Ref(id2)) => {
                if let HeapData::Bytes(b2) = heap.get(*id2) {
                    *b1 == b2.as_slice()
                } else {
                    false
                }
            }
            (Self::Ref(id1), Self::InternBytes(b2)) => {
                if let HeapData::Bytes(b1) = heap.get(*id1) {
                    b1.as_slice() == *b2
                } else {
                    false
                }
            }

            (Self::Ref(id1), Self::Ref(id2)) => {
                if *id1 == *id2 {
                    return true;
                }
                // Need to use with_two for proper borrow management
                heap.with_two(*id1, *id2, |heap, left, right| left.py_eq(right, heap))
            }
            _ => false,
        }
    }

    #[allow(clippy::only_used_in_recursion)]
    fn py_cmp(&self, other: &Self, heap: &mut Heap<'c, 'e>) -> Option<Ordering> {
        match (self, other) {
            (Self::Int(s), Self::Int(o)) => s.partial_cmp(o),
            (Self::Float(s), Self::Float(o)) => s.partial_cmp(o),
            (Self::Int(s), Self::Float(o)) => (*s as f64).partial_cmp(o),
            (Self::Float(s), Self::Int(o)) => s.partial_cmp(&(*o as f64)),
            (Self::Bool(s), _) => Self::Int(i64::from(*s)).py_cmp(other, heap),
            (_, Self::Bool(s)) => self.py_cmp(&Self::Int(i64::from(*s)), heap),
            (Self::InternString(s1), Self::InternString(s2)) => s1.partial_cmp(s2),
            (Self::InternBytes(b1), Self::InternBytes(b2)) => b1.partial_cmp(b2),
            // Ref comparison requires heap context, not supported in PartialOrd
            (Self::Ref(_), Self::Ref(_)) => None,
            _ => None,
        }
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        if let Value::Ref(id) = self {
            stack.push(*id);
            // Mark as Dereferenced to prevent Drop panic
            #[cfg(feature = "dec-ref-check")]
            self.dec_ref_forget();
        }
    }

    fn py_bool(&self, heap: &Heap<'c, 'e>) -> bool {
        match self {
            Self::Undefined => false,
            Self::Ellipsis => true,
            Self::None => false,
            Self::Bool(b) => *b,
            Self::Int(v) => *v != 0,
            Self::Float(f) => *f != 0.0,
            Self::Range(v) => *v != 0,
            Self::Exc(_) => true,
            Self::Callable(_) => true,
            Self::Function(_) => true,
            Self::InternString(s) => !s.is_empty(),
            Self::InternBytes(b) => !b.is_empty(),
            Self::Ref(id) => heap.get(*id).py_bool(heap),
            #[cfg(feature = "dec-ref-check")]
            Self::Dereferenced => panic!("Cannot access Dereferenced object"),
        }
    }

    fn py_repr<'a>(&'a self, heap: &'a Heap<'c, 'e>) -> Cow<'a, str> {
        match self {
            Self::Undefined => "Undefined".into(),
            Self::Ellipsis => "Ellipsis".into(),
            Self::None => "None".into(),
            Self::Bool(true) => "True".into(),
            Self::Bool(false) => "False".into(),
            Self::Int(v) => format!("{v}").into(),
            Self::Float(v) => {
                let s = v.to_string();
                if s.contains('.') {
                    s.into()
                } else {
                    format!("{s}.0").into()
                }
            }
            Self::Range(size) => format!("0:{size}").into(),
            Self::Exc(exc) => format!("{exc}").into(),
            Self::Callable(c) => c.py_repr(heap),
            Self::Function(f) => f.py_repr(),
            Self::InternString(s) => string_repr(s).into(),
            Self::InternBytes(b) => bytes_repr(b).into(),
            Self::Ref(id) => heap.get(*id).py_repr(heap),
            #[cfg(feature = "dec-ref-check")]
            Self::Dereferenced => panic!("Cannot access Dereferenced object"),
        }
    }

    fn py_str<'a>(&'a self, heap: &'a Heap<'c, 'e>) -> Cow<'a, str> {
        match self {
            Self::InternString(s) => (*s).into(),
            Self::Ref(id) => heap.get(*id).py_str(heap),
            _ => self.py_repr(heap),
        }
    }

    fn py_add(&self, other: &Self, heap: &mut Heap<'c, 'e>) -> Option<Value<'c, 'e>> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(Value::Int(v1 + v2)),
            (Self::Float(v1), Self::Float(v2)) => Some(Value::Float(v1 + v2)),
            (Self::Ref(id1), Self::Ref(id2)) => heap.with_two(*id1, *id2, |heap, left, right| left.py_add(right, heap)),
            (Self::InternString(s1), Self::InternString(s2)) => {
                let concat = format!("{s1}{s2}");
                Some(Value::Ref(heap.allocate(HeapData::Str(concat.into()))))
            }
            // for strings we need to account for the fact they might be either interned or not
            (Self::InternString(s1), Self::Ref(id2)) => {
                if let HeapData::Str(s2) = heap.get(*id2) {
                    let concat = format!("{}{}", s1, s2.as_str());
                    Some(Value::Ref(heap.allocate(HeapData::Str(concat.into()))))
                } else {
                    None
                }
            }
            (Self::Ref(id1), Self::InternString(s2)) => {
                if let HeapData::Str(s1) = heap.get(*id1) {
                    let concat = format!("{}{}", s1.as_str(), s2);
                    Some(Value::Ref(heap.allocate(HeapData::Str(concat.into()))))
                } else {
                    None
                }
            }
            // same for bytes
            (Self::InternBytes(b1), Self::InternBytes(b2)) => {
                let mut b = Vec::with_capacity(b1.len() + b2.len());
                b.extend_from_slice(b1);
                b.extend_from_slice(b2);
                Some(Value::Ref(heap.allocate(HeapData::Bytes(b.into()))))
            }
            (Self::InternBytes(b1), Self::Ref(id2)) => {
                if let HeapData::Bytes(b2) = heap.get(*id2) {
                    let mut b = Vec::with_capacity(b1.len() + b2.len());
                    b.extend_from_slice(b1);
                    b.extend_from_slice(b2);
                    Some(Value::Ref(heap.allocate(HeapData::Bytes(b.into()))))
                } else {
                    None
                }
            }
            (Self::Ref(id1), Self::InternBytes(b2)) => {
                if let HeapData::Bytes(b1) = heap.get(*id1) {
                    let mut b = Vec::with_capacity(b1.len() + b2.len());
                    b.extend_from_slice(b1);
                    b.extend_from_slice(b2);
                    Some(Value::Ref(heap.allocate(HeapData::Bytes(b.into()))))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn py_sub(&self, other: &Self, _heap: &mut Heap<'c, 'e>) -> Option<Self> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(Value::Int(v1 - v2)),
            _ => None,
        }
    }

    fn py_mod(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(Value::Int(v1 % v2)),
            (Self::Float(v1), Self::Float(v2)) => Some(Value::Float(v1 % v2)),
            (Self::Float(v1), Self::Int(v2)) => Some(Value::Float(v1 % (*v2 as f64))),
            (Self::Int(v1), Self::Float(v2)) => Some(Value::Float((*v1 as f64) % v2)),
            _ => None,
        }
    }

    fn py_mod_eq(&self, other: &Self, right_value: i64) -> Option<bool> {
        match (self, other) {
            (Self::Int(v1), Self::Int(v2)) => Some(v1 % v2 == right_value),
            (Self::Float(v1), Self::Float(v2)) => Some(v1 % v2 == right_value as f64),
            (Self::Float(v1), Self::Int(v2)) => Some(v1 % (*v2 as f64) == right_value as f64),
            (Self::Int(v1), Self::Float(v2)) => Some((*v1 as f64) % v2 == right_value as f64),
            _ => None,
        }
    }

    fn py_iadd(&mut self, other: Self, heap: &mut Heap<'c, 'e>, _self_id: Option<HeapId>) -> bool {
        match (&self, &other) {
            (Self::Int(v1), Self::Int(v2)) => {
                *self = Value::Int(*v1 + v2);
                true
            }
            (Self::Float(v1), Self::Float(v2)) => {
                *self = Value::Float(*v1 + *v2);
                true
            }
            (Self::InternString(s1), Self::InternString(s2)) => {
                let concat = format!("{s1}{s2}");
                *self = Value::Ref(heap.allocate(HeapData::Str(concat.into())));
                true
            }
            (Self::InternString(s1), Self::Ref(id2)) => {
                if let HeapData::Str(s2) = heap.get(*id2) {
                    let concat = format!("{}{}", s1, s2.as_str());
                    *self = Value::Ref(heap.allocate(HeapData::Str(concat.into())));
                    true
                } else {
                    false
                }
            }
            (Self::Ref(id1), Self::InternString(s2)) => {
                if let HeapData::Str(s1) = heap.get_mut(*id1) {
                    s1.as_string_mut().push_str(s2);
                    true
                } else {
                    false
                }
            }
            // same for bytes
            (Self::InternBytes(b1), Self::InternBytes(b2)) => {
                let mut b = Vec::with_capacity(b1.len() + b2.len());
                b.extend_from_slice(b1);
                b.extend_from_slice(b2);
                *self = Value::Ref(heap.allocate(HeapData::Bytes(b.into())));
                true
            }
            (Self::InternBytes(b1), Self::Ref(id2)) => {
                if let HeapData::Bytes(b2) = heap.get(*id2) {
                    let mut b = Vec::with_capacity(b1.len() + b2.len());
                    b.extend_from_slice(b1);
                    b.extend_from_slice(b2);
                    *self = Value::Ref(heap.allocate(HeapData::Bytes(b.into())));
                    true
                } else {
                    false
                }
            }
            (Self::Ref(id1), Self::InternBytes(b2)) => {
                if let HeapData::Bytes(b1) = heap.get_mut(*id1) {
                    b1.as_vec_mut().extend_from_slice(b2);
                    true
                } else {
                    false
                }
            }
            (Self::Ref(id), Self::Ref(_)) => {
                heap.with_entry_mut(*id, |heap, data| data.py_iadd(other, heap, Some(*id)))
            }
            _ => false,
        }
    }

    fn py_getitem(&self, key: &Self, heap: &mut Heap<'c, 'e>) -> RunResult<'c, Self> {
        match self {
            Value::Ref(id) => {
                // Need to take entry out to allow mutable heap access
                let id = *id;
                heap.with_entry_mut(id, |heap, data| data.py_getitem(key, heap))
            }
            _ => Err(ExcType::type_error_not_sub(self.py_type(heap))),
        }
    }

    // fn py_call(&self, heap: &mut Heap<'c, 'e>, args: ArgValues<'c, 'e>) -> Option<RunResult<'c, Self>> {
    //     match self {
    //         Self::Callable(c) => Some(c.call(heap, args)),
    //         _ => None,
    //     }
    // }
}

impl<'c, 'e> Value<'c, 'e> {
    /// Returns a stable, unique identifier for this value.
    ///
    /// Should match Python's `id()` function conceptually.
    ///
    /// For immediate values (Int, Float, Range, Exc, Callable), this computes a deterministic ID
    /// based on the value's hash, avoiding heap allocation. This means `id(5) == id(5)` will
    /// return True (unlike CPython for large integers outside the interning range).
    ///
    /// Singletons (None, True, False, etc.) return IDs from a dedicated tagged range.
    /// Interned strings/bytes use their pointer + length for stable identity.
    /// Heap-allocated values (Ref) reuse their `HeapId` inside the heap-tagged range.
    pub fn id(&self) -> usize {
        match self {
            // Singletons have fixed tagged IDs
            Self::Undefined => singleton_id(SingletonSlot::Undefined),
            Self::Ellipsis => singleton_id(SingletonSlot::Ellipsis),
            Self::None => singleton_id(SingletonSlot::None),
            Self::Bool(b) => {
                if *b {
                    singleton_id(SingletonSlot::True)
                } else {
                    singleton_id(SingletonSlot::False)
                }
            }
            Self::Function(f) => f.id(),
            Self::InternString(s) => {
                interned_id_from_parts(s.as_ptr() as usize, s.len(), INTERN_STR_ID_TAG, INTERN_STR_ID_MASK)
            }
            Self::InternBytes(b) => {
                interned_id_from_parts(b.as_ptr() as usize, b.len(), INTERN_BYTES_ID_TAG, INTERN_BYTES_ID_MASK)
            }
            // Already heap-allocated, return id within a dedicated tag range
            Self::Ref(id) => heap_tagged_id(*id),
            // Value-based IDs for immediate types (no heap allocation!)
            Self::Int(v) => int_value_id(*v),
            Self::Float(v) => float_value_id(*v),
            Self::Range(v) => range_value_id(*v),
            Self::Exc(e) => exc_value_id(e),
            Self::Callable(c) => callable_value_id(c),
            #[cfg(feature = "dec-ref-check")]
            Self::Dereferenced => panic!("Cannot get id of Dereferenced object"),
        }
    }

    /// Equivalent of Python's `is` operator.
    ///
    /// Compares value identity by comparing their IDs.
    pub fn is(&self, other: &Self) -> bool {
        self.id() == other.id()
    }

    /// Computes the hash value for this value, used for dict keys.
    ///
    /// Returns Some(hash) for hashable types (immediate values and immutable heap types).
    /// Returns None for unhashable types (list, dict).
    ///
    /// For heap-allocated values (Ref variant), this computes the hash lazily
    /// on first use and caches it for subsequent calls.
    pub fn py_hash_u64(&self, heap: &mut Heap<'c, 'e>) -> Option<u64> {
        match self {
            // Immediate values can be hashed directly
            Self::Undefined => Some(0),
            Self::Ellipsis => Some(1),
            Self::None => Some(2),
            Self::Bool(b) => {
                let mut hasher = DefaultHasher::new();
                b.hash(&mut hasher);
                Some(hasher.finish())
            }
            Self::Int(i) => {
                let mut hasher = DefaultHasher::new();
                i.hash(&mut hasher);
                Some(hasher.finish())
            }
            Self::Float(f) => {
                let mut hasher = DefaultHasher::new();
                // Hash the bit representation of float for consistency
                f.to_bits().hash(&mut hasher);
                Some(hasher.finish())
            }
            Self::Range(r) => {
                let mut hasher = DefaultHasher::new();
                r.hash(&mut hasher);
                Some(hasher.finish())
            }
            Self::Exc(e) => {
                // Exceptions are rarely used as dict keys, but we provide a hash
                // based on the exception type and argument for proper distribution
                Some(e.py_hash())
            }
            Self::Callable(c) => {
                // Builtins have a fixed identity, hash based on variant discriminant
                let mut hasher = DefaultHasher::new();
                std::mem::discriminant(c).hash(&mut hasher);
                Some(hasher.finish())
            }
            Self::Function(f) => {
                let mut hasher = DefaultHasher::new();
                // TODO, this is NOT proper hashing, we should somehow hash the function properly
                f.name.name.hash(&mut hasher);
                Some(hasher.finish())
            }
            Self::InternString(s) => {
                let mut hasher = DefaultHasher::new();
                s.hash(&mut hasher);
                Some(hasher.finish())
            }
            Self::InternBytes(b) => {
                let mut hasher = DefaultHasher::new();
                b.hash(&mut hasher);
                Some(hasher.finish())
            }
            // For heap-allocated values, compute hash lazily and cache it
            Self::Ref(id) => heap.get_or_compute_hash(*id),
            #[cfg(feature = "dec-ref-check")]
            Self::Dereferenced => panic!("Cannot access Dereferenced object"),
        }
    }

    /// TODO maybe replace with TryFrom
    pub fn as_int(&self) -> RunResult<'static, i64> {
        match self {
            Self::Int(i) => Ok(*i),
            // TODO use self.type
            _ => exc_err_fmt!(ExcType::TypeError; "'{self:?}' object cannot be interpreted as an integer"),
        }
    }

    /// Calls an attribute method on this value (e.g., list.append()).
    ///
    /// This method requires heap access to work with heap-allocated values and
    /// to generate accurate error messages.
    pub fn call_attr(
        &mut self,
        heap: &mut Heap<'c, 'e>,
        attr: &Attr,
        args: ArgValues<'c, 'e>,
    ) -> RunResult<'c, Value<'c, 'e>> {
        if let Self::Ref(id) = self {
            heap.call_attr(*id, attr, args)
        } else {
            Err(ExcType::attribute_error(self.py_type(heap), attr))
        }
    }

    /// Clones an value with proper heap reference counting.
    ///
    /// For immediate values (Int, Bool, None, etc.), this performs a simple copy.
    /// For heap-allocated values (Ref variant), this increments the reference count
    /// and returns a new reference to the same heap value.
    ///
    /// # Important
    /// This method MUST be used instead of the derived `Clone` implementation to ensure
    /// proper reference counting. Using `.clone()` directly will bypass reference counting
    /// and cause memory leaks or double-frees.
    #[must_use]
    pub fn clone_with_heap(&self, heap: &mut Heap<'c, 'e>) -> Self {
        match self {
            Self::Ref(id) => {
                heap.inc_ref(*id);
                Self::Ref(*id)
            }
            // Immediate values can be copied without heap interaction
            other => other.clone_immediate(),
        }
    }

    /// Drops an value, decrementing its heap reference count if applicable.
    ///
    /// For immediate values, this is a no-op. For heap-allocated values (Ref variant),
    /// this decrements the reference count and frees the value (and any children) when
    /// the count reaches zero.
    ///
    /// # Important
    /// This method MUST be called before overwriting a namespace slot or discarding
    /// a value to prevent memory leaks.
    ///
    /// With `dec-ref-check` enabled, `Ref` variants are replaced with `Dereferenced` and
    /// the original is forgotten to prevent the Drop impl from panicking. Non-Ref variants
    /// are left unchanged since they don't trigger the Drop panic.
    #[allow(unused_mut)]
    pub fn drop_with_heap(mut self, heap: &mut Heap<'c, 'e>) {
        #[cfg(feature = "dec-ref-check")]
        {
            let old = std::mem::replace(&mut self, Value::Dereferenced);
            if let Self::Ref(id) = &old {
                heap.dec_ref(*id);
                std::mem::forget(old);
            }
        }
        #[cfg(not(feature = "dec-ref-check"))]
        if let Self::Ref(id) = self {
            heap.dec_ref(id);
        }
    }

    /// Internal helper for copying immediate values without heap interaction.
    ///
    /// This method should only be called by `clone_with_heap()` for immediate values.
    /// Attempting to clone a Ref variant will panic.
    fn clone_immediate(&self) -> Self {
        match self {
            Self::Undefined => Self::Undefined,
            Self::Ellipsis => Self::Ellipsis,
            Self::None => Self::None,
            Self::Bool(b) => Self::Bool(*b),
            Self::Int(v) => Self::Int(*v),
            Self::Float(v) => Self::Float(*v),
            Self::Range(v) => Self::Range(*v),
            Self::Exc(e) => Self::Exc(e.clone()),
            Self::Callable(c) => Self::Callable(c.clone()),
            Self::Function(f) => Self::Function(f),
            Self::InternString(s) => Self::InternString(s),
            Self::InternBytes(b) => Self::InternBytes(b),
            Self::Ref(_) => panic!("Ref clones must go through clone_with_heap to maintain refcounts"),
            #[cfg(feature = "dec-ref-check")]
            Self::Dereferenced => panic!("Cannot clone Dereferenced object"),
        }
    }

    /// Creates a shallow copy of this Value without incrementing reference counts.
    ///
    /// IMPORTANT: For Ref variants, this copies the ValueId but does NOT increment
    /// the reference count. The caller MUST call `heap.inc_ref()` separately for any
    /// Ref variants to maintain correct reference counting.
    ///
    /// This is useful when you need to copy Objects from a borrowed heap context
    /// and will increment refcounts in a separate step.
    pub(crate) fn copy_for_extend(&self) -> Self {
        match self {
            Self::Undefined => Self::Undefined,
            Self::Ellipsis => Self::Ellipsis,
            Self::None => Self::None,
            Self::Bool(b) => Self::Bool(*b),
            Self::Int(v) => Self::Int(*v),
            Self::Float(v) => Self::Float(*v),
            Self::Range(v) => Self::Range(*v),
            Self::Exc(e) => Self::Exc(e.clone()),
            Self::Callable(c) => Self::Callable(c.clone()),
            Self::Function(f) => Self::Function(f),
            Self::InternString(s) => Self::InternString(s),
            Self::InternBytes(b) => Self::InternBytes(b),
            Self::Ref(id) => Self::Ref(*id), // Caller must increment refcount!
            #[cfg(feature = "dec-ref-check")]
            Self::Dereferenced => panic!("Cannot copy Dereferenced object"),
        }
    }

    #[cfg(feature = "dec-ref-check")]
    pub fn into_exc(self) -> SimpleException<'c> {
        if let Self::Exc(exc) = &self {
            // SAFETY: We're reading the exc out and then forgetting the value shell.
            // This is safe because:
            // 1. exc is a valid reference to the SimpleException inside value
            // 2. We immediately forget value, so Drop won't run and won't try to
            //    access the now-moved exc
            // 3. It's only used with the `dref-check` feature enabled for testing
            let exc_owned = unsafe { std::ptr::read(exc) };
            std::mem::forget(self);
            exc_owned
        } else {
            panic!("Cannot convert non-exception value into exception")
        }
    }

    #[cfg(not(feature = "dec-ref-check"))]
    pub fn into_exc(self) -> SimpleException<'c> {
        if let Self::Exc(e) = self {
            e
        } else {
            panic!("Cannot convert non-exception value into exception")
        }
    }

    /// Mark as Dereferenced to prevent Drop panic
    ///
    /// This should be called from `py_dec_ref_ids` methods only
    #[cfg(feature = "dec-ref-check")]
    pub fn dec_ref_forget(&mut self) {
        let old = std::mem::replace(self, Value::Dereferenced);
        std::mem::forget(old);
    }
}

/// Attribute names for method calls on container types (list, dict).
///
/// Uses strum `Display` derive with lowercase serialization.
/// The `Other(String)` variant is a fallback for unknown/dynamic attribute names.
#[derive(Debug, Clone, Display)]
#[strum(serialize_all = "lowercase")]
pub enum Attr {
    Append,
    Insert,
    Get,
    Keys,
    Values,
    Items,
    Pop,
    /// Fallback for unknown attribute names. Displays as the contained string.
    #[strum(default)]
    Other(String),
}

impl From<String> for Attr {
    fn from(name: String) -> Self {
        match name.as_str() {
            "append" => Self::Append,
            "insert" => Self::Insert,
            "get" => Self::Get,
            "keys" => Self::Keys,
            "values" => Self::Values,
            "items" => Self::Items,
            "pop" => Self::Pop,
            _ => Self::Other(name),
        }
    }
}

/// High-bit tag reserved for literal singletons (None, Ellipsis, booleans).
const SINGLETON_ID_TAG: usize = 1usize << (usize::BITS - 1);
/// High-bit tag reserved for interned string `id()` values.
const INTERN_STR_ID_TAG: usize = 1usize << (usize::BITS - 2);
/// High-bit tag reserved for interned bytes `id()` values to avoid colliding with any other space.
const INTERN_BYTES_ID_TAG: usize = 1usize << (usize::BITS - 3);
/// High-bit tag reserved for heap-backed `HeapId`s.
const HEAP_ID_TAG: usize = 1usize << (usize::BITS - 4);

/// Mask that keeps pointer-derived bits below the bytes tag bit.
const INTERN_BYTES_ID_MASK: usize = INTERN_BYTES_ID_TAG - 1;
/// Mask that keeps pointer-derived bits below the string tag bit.
const INTERN_STR_ID_MASK: usize = INTERN_STR_ID_TAG - 1;
/// Mask that keeps per-singleton offsets below the singleton tag bit.
const SINGLETON_ID_MASK: usize = SINGLETON_ID_TAG - 1;
/// Mask that keeps heap value IDs below the heap tag bit.
const HEAP_ID_MASK: usize = HEAP_ID_TAG - 1;

/// High-bit tag for Int value-based IDs (no heap allocation needed).
const INT_ID_TAG: usize = 1usize << (usize::BITS - 5);
/// High-bit tag for Float value-based IDs.
const FLOAT_ID_TAG: usize = 1usize << (usize::BITS - 6);
/// High-bit tag for Range value-based IDs.
const RANGE_ID_TAG: usize = 1usize << (usize::BITS - 7);
/// High-bit tag for Exc (exception) value-based IDs.
const EXC_ID_TAG: usize = 1usize << (usize::BITS - 8);
/// High-bit tag for Callable value-based IDs.
const CALLABLE_ID_TAG: usize = 1usize << (usize::BITS - 9);

/// Masks for value-based ID tags (keep bits below the tag bit).
const INT_ID_MASK: usize = INT_ID_TAG - 1;
const FLOAT_ID_MASK: usize = FLOAT_ID_TAG - 1;
const RANGE_ID_MASK: usize = RANGE_ID_TAG - 1;
const EXC_ID_MASK: usize = EXC_ID_TAG - 1;
const CALLABLE_ID_MASK: usize = CALLABLE_ID_TAG - 1;

/// Rotate distance used when folding slice length into the pointer identity.
const INTERN_LEN_ROTATE: u32 = usize::BITS / 2;

/// Mixes a slice pointer and its length into a deterministic identity
/// that lives in a reserved numeric range controlled by `tag`.
///
/// This lets us use literal namespaces addresses for stable `id()` values without
/// ever overlapping the sequential heap `HeapId` space.
#[inline]
fn interned_id_from_parts(ptr: usize, len: usize, tag: usize, mask: usize) -> usize {
    let mixed = (ptr ^ len.rotate_left(INTERN_LEN_ROTATE)) & mask;
    tag | mixed
}

/// Enumerates singleton literal slots so we can issue stable `id()` values without heap allocation.
#[repr(usize)]
#[derive(Copy, Clone)]
enum SingletonSlot {
    Undefined = 0,
    Ellipsis = 1,
    None = 2,
    False = 3,
    True = 4,
}

/// Returns the fully tagged `id()` value for the requested singleton literal.
#[inline]
const fn singleton_id(slot: SingletonSlot) -> usize {
    SINGLETON_ID_TAG | ((slot as usize) & SINGLETON_ID_MASK)
}

/// Converts a heap `HeapId` into its tagged `id()` value, ensuring it never collides with other spaces.
#[inline]
pub fn heap_tagged_id(heap_id: HeapId) -> usize {
    HEAP_ID_TAG | (heap_id & HEAP_ID_MASK)
}

/// Computes a deterministic ID for an i64 integer value.
/// Uses the value's hash combined with a type tag to ensure uniqueness across types.
#[inline]
fn int_value_id(value: i64) -> usize {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    INT_ID_TAG | (hasher.finish() as usize & INT_ID_MASK)
}

/// Computes a deterministic ID for an f64 float value.
/// Uses the bit representation's hash for consistency (handles NaN, infinities, etc.).
#[inline]
fn float_value_id(value: f64) -> usize {
    let mut hasher = DefaultHasher::new();
    value.to_bits().hash(&mut hasher);
    FLOAT_ID_TAG | (hasher.finish() as usize & FLOAT_ID_MASK)
}

/// Computes a deterministic ID for a Range value.
#[inline]
fn range_value_id(value: i64) -> usize {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    RANGE_ID_TAG | (hasher.finish() as usize & RANGE_ID_MASK)
}

/// Computes a deterministic ID for an exception based on its hash.
#[inline]
fn exc_value_id(exc: &SimpleException<'_>) -> usize {
    let hash = exc.py_hash() as usize;
    EXC_ID_TAG | (hash & EXC_ID_MASK)
}

/// Computes a deterministic ID for a Callable based on its discriminant.
#[inline]
fn callable_value_id(c: &Callable<'_>) -> usize {
    let mut hasher = DefaultHasher::new();
    std::mem::discriminant(c).hash(&mut hasher);
    CALLABLE_ID_TAG | (hasher.finish() as usize & CALLABLE_ID_MASK)
}
