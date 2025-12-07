use std::borrow::Cow;
use std::fmt::Write;

use indexmap::IndexMap;

use crate::args::ArgValues;
use crate::exceptions::ExcType;
use crate::heap::{Heap, HeapData, HeapId};
use crate::run::RunResult;
use crate::value::{Attr, Value};
use crate::values::PyTrait;

/// Python dict type, wrapping an IndexMap to preserve insertion order.
///
/// This type provides Python dict semantics including dynamic key-value namespaces,
/// reference counting for heap values, and standard dict methods like get, keys,
/// values, items, and pop.
///
/// # Storage Strategy
/// Uses `IndexMap<u64, Vec<(Value<'c, 'e>, Value<'c, 'e>)>>` to preserve insertion order (matching
/// Python 3.7+ behavior). The key is the hash of the dict key. The Vec handles hash
/// collisions by storing multiple (key, value) pairs with the same hash, allowing
/// proper equality checking for collisions.
///
/// # Reference Counting
/// When values are added via `set()`, their reference counts are incremented.
/// When using `from_pairs()`, ownership is transferred without incrementing refcounts
/// (caller must ensure values' refcounts account for the dict's reference).
#[derive(Debug, Default)]
pub struct Dict<'c, 'e> {
    /// Maps hash -> list of (key, value) pairs with that hash
    /// The Vec handles hash collisions. IndexMap preserves insertion order.
    map: IndexMap<u64, Vec<(Value<'c, 'e>, Value<'c, 'e>)>>,
}

impl<'c, 'e> Dict<'c, 'e> {
    /// Creates a new empty dict.
    #[must_use]
    pub fn new() -> Self {
        Self { map: IndexMap::new() }
    }

    pub fn as_index_map(&self) -> &IndexMap<u64, Vec<(Value<'c, 'e>, Value<'c, 'e>)>> {
        &self.map
    }

    /// Creates a dict from a vector of (key, value) pairs.
    ///
    /// Assumes the caller is transferring ownership of all keys and values in the pairs.
    /// Does NOT increment reference counts since ownership is being transferred.
    /// Returns Err if any key is unhashable (e.g., list, dict).
    pub fn from_pairs(pairs: Vec<(Value<'c, 'e>, Value<'c, 'e>)>, heap: &mut Heap<'c, 'e>) -> RunResult<'c, Self> {
        let mut dict = Self::new();
        let mut pairs_iter = pairs.into_iter();
        for (key, value) in pairs_iter.by_ref() {
            if let Err(err) = dict.set_transfer_ownership(key, value, heap) {
                for (k, v) in pairs_iter {
                    k.drop_with_heap(heap);
                    v.drop_with_heap(heap);
                }
                dict.drop_all_entries(heap);
                return Err(err);
            }
        }
        Ok(dict)
    }

    /// Internal method to set a key-value pair without incrementing refcounts.
    ///
    /// Used when ownership is being transferred (e.g., from_pairs) rather than shared.
    /// The caller must ensure the values' refcounts already account for this dict's reference.
    fn set_transfer_ownership(
        &mut self,
        key: Value<'c, 'e>,
        value: Value<'c, 'e>,
        heap: &mut Heap<'c, 'e>,
    ) -> RunResult<'c, Option<Value<'c, 'e>>> {
        let Some(hash) = key.py_hash_u64(heap) else {
            // Key is unhashable - clean up before returning error
            let err = ExcType::type_error_unhashable(key.py_type(heap));
            key.drop_with_heap(heap);
            value.drop_with_heap(heap);
            return Err(err);
        };

        let bucket = self.map.entry(hash).or_default();

        // Check if key already exists in bucket
        for (i, (k, _v)) in bucket.iter().enumerate() {
            if k.py_eq(&key, heap) {
                // Key exists, replace in place to preserve insertion order
                // Note: we don't decrement old value's refcount since this is a transfer
                // and we don't increment new value's refcount either
                let (_old_key, old_value) = std::mem::replace(&mut bucket[i], (key, value));
                return Ok(Some(old_value));
            }
        }

        // Key doesn't exist, add new pair
        bucket.push((key, value));
        Ok(None)
    }

    fn drop_all_entries(&mut self, heap: &mut Heap<'c, 'e>) {
        for bucket in self.map.values_mut() {
            for (key, value) in bucket.drain(..) {
                key.drop_with_heap(heap);
                value.drop_with_heap(heap);
            }
        }
    }

    /// Gets a value from the dict by key.
    ///
    /// Returns Ok(Some(value)) if key exists, Ok(None) if key doesn't exist.
    /// Returns Err if key is unhashable.
    pub fn get(&self, key: &Value<'c, 'e>, heap: &mut Heap<'c, 'e>) -> RunResult<'c, Option<&Value<'c, 'e>>> {
        let hash = key
            .py_hash_u64(heap)
            .ok_or_else(|| ExcType::type_error_unhashable(key.py_type(heap)))?;
        if let Some(bucket) = self.map.get(&hash) {
            for (k, v) in bucket {
                if k.py_eq(key, heap) {
                    return Ok(Some(v));
                }
            }
        }
        Ok(None)
    }

    /// Sets a key-value pair in the dict.
    ///
    /// The caller transfers ownership of `key` and `value` to the dict. Their refcounts
    /// are NOT incremented here - the caller is responsible for ensuring the refcounts
    /// were already incremented (e.g., via `clone_with_heap` or `evaluate_use`).
    ///
    /// If the key already exists, replaces the old value and returns it (caller now
    /// owns the old value and is responsible for its refcount).
    /// Returns Err if key is unhashable.
    pub fn set(
        &mut self,
        key: Value<'c, 'e>,
        value: Value<'c, 'e>,
        heap: &mut Heap<'c, 'e>,
    ) -> RunResult<'c, Option<Value<'c, 'e>>> {
        let hash = key
            .py_hash_u64(heap)
            .ok_or_else(|| ExcType::type_error_unhashable(key.py_type(heap)))?;

        let bucket = self.map.entry(hash).or_default();

        // Check if key already exists in bucket
        for (i, (k, _v)) in bucket.iter().enumerate() {
            if k.py_eq(&key, heap) {
                // Key exists, replace in place to preserve insertion order within the bucket
                let (old_key, old_value) = std::mem::replace(&mut bucket[i], (key, value));

                // Decrement refcount for old key (we're discarding it)
                old_key.drop_with_heap(heap);
                // Transfer ownership of old_value to caller (no clone needed)
                return Ok(Some(old_value));
            }
        }

        // Key doesn't exist, add new pair (ownership transfer)
        bucket.push((key, value));
        Ok(None)
    }

    /// Removes and returns a key-value pair from the dict.
    ///
    /// Returns Ok(Some((key, value))) if key exists, Ok(None) if key doesn't exist.
    /// Returns Err if key is unhashable.
    ///
    /// Reference counting: does not decrement refcounts for removed key and value;
    /// caller assumes ownership and is responsible for managing their refcounts.
    pub fn pop(
        &mut self,
        key: &Value<'c, 'e>,
        heap: &mut Heap<'c, 'e>,
    ) -> RunResult<'c, Option<(Value<'c, 'e>, Value<'c, 'e>)>> {
        let hash = key
            .py_hash_u64(heap)
            .ok_or_else(|| ExcType::type_error_unhashable(key.py_type(heap)))?;

        if let Some(bucket) = self.map.get_mut(&hash) {
            for (i, (k, _v)) in bucket.iter().enumerate() {
                if k.py_eq(key, heap) {
                    let (old_key, old_value) = bucket.swap_remove(i);
                    if bucket.is_empty() {
                        self.map.shift_remove(&hash);
                    }
                    // Don't decrement refcounts - caller now owns the values
                    return Ok(Some((old_key, old_value)));
                }
            }
        }
        Ok(None)
    }

    /// Returns a vector of all keys in the dict with proper reference counting.
    ///
    /// Each key's reference count is incremented since the returned vector
    /// now holds additional references to these values.
    #[must_use]
    pub fn keys(&self, heap: &mut Heap<'c, 'e>) -> Vec<Value<'c, 'e>> {
        let mut result = Vec::new();
        for bucket in self.map.values() {
            for (k, _v) in bucket {
                result.push(k.clone_with_heap(heap));
            }
        }
        result
    }

    /// Returns a vector of all values in the dict with proper reference counting.
    ///
    /// Each value's reference count is incremented since the returned vector
    /// now holds additional references to these values.
    #[must_use]
    pub fn values(&self, heap: &mut Heap<'c, 'e>) -> Vec<Value<'c, 'e>> {
        let mut result = Vec::new();
        for bucket in self.map.values() {
            for (_k, v) in bucket {
                result.push(v.clone_with_heap(heap));
            }
        }
        result
    }

    /// Returns a vector of all (key, value) pairs in the dict with proper reference counting.
    ///
    /// Each key and value's reference count is incremented since the returned vector
    /// now holds additional references to these values.
    #[must_use]
    pub fn items(&self, heap: &mut Heap<'c, 'e>) -> Vec<(Value<'c, 'e>, Value<'c, 'e>)> {
        let mut result = Vec::new();
        for bucket in self.map.values() {
            for (k, v) in bucket {
                result.push((k.clone_with_heap(heap), v.clone_with_heap(heap)));
            }
        }
        result
    }

    /// Returns the number of key-value pairs in the dict.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.values().map(Vec::len).sum()
    }

    /// Returns true if the dict is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Creates a deep clone of this dict with proper reference counting.
    ///
    /// All heap-allocated keys and values have their reference counts
    /// incremented. This should be used instead of `.clone()` which would
    /// bypass reference counting.
    #[must_use]
    pub fn clone_with_heap(&self, heap: &mut Heap<'c, 'e>) -> Self {
        let mut new_map = IndexMap::new();
        for (hash, bucket) in &self.map {
            let new_bucket: Vec<(Value<'c, 'e>, Value<'c, 'e>)> = bucket
                .iter()
                .map(|(k, v)| (k.clone_with_heap(heap), v.clone_with_heap(heap)))
                .collect();
            new_map.insert(*hash, new_bucket);
        }
        Self { map: new_map }
    }
}

impl<'c, 'e> PyTrait<'c, 'e> for Dict<'c, 'e> {
    fn py_type(&self, _heap: &Heap<'c, 'e>) -> &'static str {
        "dict"
    }

    fn py_len(&self, _heap: &Heap<'c, 'e>) -> Option<usize> {
        Some(self.len())
    }

    fn py_eq(&self, other: &Self, heap: &mut Heap<'c, 'e>) -> bool {
        if self.len() != other.len() {
            return false;
        }

        // Check that all keys in self exist in other with equal values
        for bucket in self.map.values() {
            for (k, v) in bucket {
                match other.get(k, heap) {
                    Ok(Some(other_v)) => {
                        if !v.py_eq(other_v, heap) {
                            return false;
                        }
                    }
                    _ => return false,
                }
            }
        }
        true
    }

    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        for bucket in self.map.values_mut() {
            for (key, value) in bucket {
                if let Value::Ref(id) = key {
                    stack.push(*id);
                    #[cfg(feature = "dec-ref-check")]
                    key.dec_ref_forget();
                }
                if let Value::Ref(id) = value {
                    stack.push(*id);
                    #[cfg(feature = "dec-ref-check")]
                    value.dec_ref_forget();
                }
            }
        }
    }

    fn py_bool(&self, _heap: &Heap<'c, 'e>) -> bool {
        !self.is_empty()
    }

    fn py_repr<'a>(&'a self, heap: &'a Heap<'c, 'e>) -> Cow<'a, str> {
        if self.is_empty() {
            return Cow::Borrowed("{}");
        }

        let mut s = String::from("{");
        let mut first = true;
        for bucket in self.map.values() {
            for (k, v) in bucket {
                if !first {
                    s.push_str(", ");
                }
                first = false;
                let key_repr = k.py_repr(heap);
                let val_repr = v.py_repr(heap);
                let _ = write!(s, "{key_repr}: {val_repr}");
            }
        }
        s.push('}');
        Cow::Owned(s)
    }

    fn py_getitem(&self, key: &Value<'c, 'e>, heap: &mut Heap<'c, 'e>) -> RunResult<'c, Value<'c, 'e>> {
        // Use copy_for_extend to avoid borrow conflict, then increment refcount
        let result = self.get(key, heap)?.map(Value::copy_for_extend);
        match result {
            Some(value) => {
                if let Value::Ref(id) = &value {
                    heap.inc_ref(*id);
                }
                Ok(value)
            }
            None => Err(ExcType::key_error(key, heap)),
        }
    }

    fn py_setitem(&mut self, key: Value<'c, 'e>, value: Value<'c, 'e>, heap: &mut Heap<'c, 'e>) -> RunResult<'c, ()> {
        // Drop the old value if one was replaced
        if let Some(old_value) = self.set(key, value, heap)? {
            old_value.drop_with_heap(heap);
        }
        Ok(())
    }

    fn py_call_attr(
        &mut self,
        heap: &mut Heap<'c, 'e>,
        attr: &Attr,
        args: ArgValues<'c, 'e>,
    ) -> RunResult<'c, Value<'c, 'e>> {
        match attr {
            Attr::Get => {
                let (key, opt_default) = args.get_one_two_args("get")?;
                // Use copy_for_extend to avoid borrow conflict, then increment refcount
                let result = match self.get(&key, heap) {
                    Ok(value) => value.map(Value::copy_for_extend),
                    Err(err) => {
                        key.drop_with_heap(heap);
                        if let Some(default) = opt_default {
                            default.drop_with_heap(heap);
                        }
                        return Err(err);
                    }
                };

                // Clean up the key since we're done with it
                key.drop_with_heap(heap);

                match result {
                    Some(value) => {
                        if let Value::Ref(id) = &value {
                            heap.inc_ref(*id);
                        }
                        // Clean up unused default if present
                        if let Some(default) = opt_default {
                            default.drop_with_heap(heap);
                        }
                        Ok(value)
                    }
                    None => {
                        // Return default if provided (transfer ownership), else None
                        if let Some(default) = opt_default {
                            Ok(default)
                        } else {
                            Ok(Value::None)
                        }
                    }
                }
            }
            Attr::Keys => {
                args.check_zero_args("dict.keys")?;
                // keys() now handles refcount incrementing
                let keys = self.keys(heap);
                let list_id = heap.allocate(HeapData::List(crate::values::List::new(keys)));
                Ok(Value::Ref(list_id))
            }
            Attr::Values => {
                args.check_zero_args("dict.values")?;
                // values() now handles refcount incrementing
                let values = self.values(heap);
                let list_id = heap.allocate(HeapData::List(crate::values::List::new(values)));
                Ok(Value::Ref(list_id))
            }
            Attr::Items => {
                args.check_zero_args("dict.items")?;
                // items() now handles refcount incrementing for the returned values
                let items = self.items(heap);
                // Convert to list of tuples
                let mut tuples = Vec::new();
                for (k, v) in items {
                    let tuple_id = heap.allocate(HeapData::Tuple(crate::values::Tuple::from_vec(vec![k, v])));
                    tuples.push(Value::Ref(tuple_id));
                }
                let list_id = heap.allocate(HeapData::List(crate::values::List::new(tuples)));
                Ok(Value::Ref(list_id))
            }
            Attr::Pop => {
                let (key, opt_default) = args.get_one_two_args("pop")?;
                // If key is unhashable, CPython returns the default when provided, else TypeError.
                if key.py_hash_u64(heap).is_none() {
                    return if let Some(default) = opt_default {
                        key.drop_with_heap(heap);
                        Ok(default)
                    } else {
                        let err = ExcType::type_error_unhashable(key.py_type(heap));
                        key.drop_with_heap(heap);
                        Err(err)
                    };
                }
                let pop_result = match self.pop(&key, heap) {
                    Ok(result) => result,
                    Err(err) => {
                        key.drop_with_heap(heap);
                        if let Some(default) = opt_default {
                            default.drop_with_heap(heap);
                        }
                        return Err(err);
                    }
                };
                match pop_result {
                    Some((k, v)) => {
                        // Decrement key refcount since we're not returning it
                        k.drop_with_heap(heap);
                        // Clean up the argument key and unused default
                        key.drop_with_heap(heap);
                        if let Some(default) = opt_default {
                            default.drop_with_heap(heap);
                        }
                        Ok(v)
                    }
                    None => {
                        // Return default if provided (transfer ownership), else KeyError
                        if let Some(default) = opt_default {
                            // Clean up the argument key
                            key.drop_with_heap(heap);
                            Ok(default)
                        } else {
                            // Create error before dropping key (key_error needs to read key's repr)
                            let err = ExcType::key_error(&key, heap);
                            key.drop_with_heap(heap);
                            Err(err)
                        }
                    }
                }
            }
            // Catch-all for unsupported attributes (including list methods like Append, Insert)
            _ => Err(ExcType::attribute_error("dict", attr)),
        }
    }
}
