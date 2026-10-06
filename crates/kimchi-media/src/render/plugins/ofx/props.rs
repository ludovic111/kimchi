//! Property sets (`OfxPropertySetHandle`) and the property suite.
//!
//! Every object a plugin sees (the host, effects, clips, images, parameters, the arguments of an
//! action) carries a property set: named, typed, multi-dimensional values. The host fills each
//! set with the properties the spec lists for that object and their defaults; a plugin may also
//! set properties the host didn't list (they're created, typed by the setter). Reading a
//! property that doesn't exist answers `kOfxStatErrUnknown`, as the spec asks.

use std::collections::BTreeMap;
use std::ffi::{CStr, CString, c_char, c_double, c_int, c_void};
use std::sync::Mutex;

use super::ffi::*;

const MAGIC: u64 = u64::from_le_bytes(*b"kimchiPS");

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(Vec<c_int>),
    Double(Vec<c_double>),
    Str(Vec<CString>),
    Pointer(Vec<usize>),
}

impl Value {
    fn len(&self) -> usize {
        match self {
            Value::Int(v) => v.len(),
            Value::Double(v) => v.len(),
            Value::Str(v) => v.len(),
            Value::Pointer(v) => v.len(),
        }
    }
}

/// A property set. Its address is the handle given to plugins, so it lives in a `Box`.
pub struct PropertySet {
    magic: u64,
    values: Mutex<BTreeMap<String, Value>>,
}

impl std::fmt::Debug for PropertySet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.lock().iter()).finish()
    }
}

fn cstring(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

impl PropertySet {
    pub fn new() -> Box<Self> {
        Box::new(Self { magic: MAGIC, values: Mutex::new(BTreeMap::new()) })
    }

    pub fn handle(&self) -> OfxPropertySetHandle {
        self as *const Self as OfxPropertySetHandle
    }

    /// The set behind a handle a plugin passed back, if it is one of ours.
    ///
    /// # Safety
    /// `handle` must be null or point at memory that is readable for a `PropertySet`'s size
    /// (any handle kimchi gave out is).
    pub unsafe fn from_handle<'a>(handle: OfxPropertySetHandle) -> Option<&'a PropertySet> {
        if handle.is_null() {
            return None;
        }
        // SAFETY: the caller's promise; the magic number rejects foreign pointers.
        let set = unsafe { &*(handle as *const PropertySet) };
        (set.magic == MAGIC).then_some(set)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Value>> {
        self.values.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set(&self, name: &str, value: Value) {
        self.lock().insert(name.to_string(), value);
    }

    pub fn set_int(&self, name: &str, v: &[c_int]) {
        self.set(name, Value::Int(v.to_vec()));
    }

    pub fn set_double(&self, name: &str, v: &[c_double]) {
        self.set(name, Value::Double(v.to_vec()));
    }

    pub fn set_str(&self, name: &str, v: &[&str]) {
        self.set(name, Value::Str(v.iter().map(|s| cstring(s)).collect()));
    }

    pub fn set_pointer(&self, name: &str, v: &[*mut c_void]) {
        self.set(name, Value::Pointer(v.iter().map(|p| *p as usize).collect()));
    }

    pub fn get(&self, name: &str) -> Option<Value> {
        self.lock().get(name).cloned()
    }

    pub fn has(&self, name: &str) -> bool {
        self.lock().contains_key(name)
    }

    pub fn int(&self, name: &str, index: usize) -> Option<c_int> {
        match self.lock().get(name)? {
            Value::Int(v) => v.get(index).copied(),
            Value::Double(v) => v.get(index).map(|d| *d as c_int),
            _ => None,
        }
    }

    pub fn double(&self, name: &str, index: usize) -> Option<c_double> {
        match self.lock().get(name)? {
            Value::Double(v) => v.get(index).copied(),
            Value::Int(v) => v.get(index).map(|i| *i as c_double),
            _ => None,
        }
    }

    pub fn doubles(&self, name: &str) -> Vec<c_double> {
        match self.lock().get(name) {
            Some(Value::Double(v)) => v.clone(),
            Some(Value::Int(v)) => v.iter().map(|i| *i as c_double).collect(),
            _ => Vec::new(),
        }
    }

    pub fn ints(&self, name: &str) -> Vec<c_int> {
        match self.lock().get(name) {
            Some(Value::Int(v)) => v.clone(),
            Some(Value::Double(v)) => v.iter().map(|d| *d as c_int).collect(),
            _ => Vec::new(),
        }
    }

    pub fn string(&self, name: &str, index: usize) -> Option<String> {
        match self.lock().get(name)? {
            Value::Str(v) => v.get(index).map(|s| s.to_string_lossy().into_owned()),
            _ => None,
        }
    }

    pub fn strings(&self, name: &str) -> Vec<String> {
        match self.lock().get(name) {
            Some(Value::Str(v)) => v.iter().map(|s| s.to_string_lossy().into_owned()).collect(),
            _ => Vec::new(),
        }
    }

    pub fn pointer(&self, name: &str, index: usize) -> Option<*mut c_void> {
        match self.lock().get(name)? {
            Value::Pointer(v) => v.get(index).map(|p| *p as *mut c_void),
            _ => None,
        }
    }

    /// Copies every property of `other` into this set (replacing ones of the same name).
    pub fn copy_from(&self, other: &PropertySet) {
        let theirs = other.lock().clone();
        self.lock().extend(theirs);
    }

    pub fn names(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }

    fn set_at(&self, name: &str, index: c_int, value: Value) -> OfxStatus {
        if index < 0 {
            return STAT_ERR_BAD_INDEX;
        }
        let index = index as usize;
        let mut values = self.lock();
        let entry = values.entry(name.to_string()).or_insert_with(|| match &value {
            Value::Int(_) => Value::Int(Vec::new()),
            Value::Double(_) => Value::Double(Vec::new()),
            Value::Str(_) => Value::Str(Vec::new()),
            Value::Pointer(_) => Value::Pointer(Vec::new()),
        });
        // Ints and doubles convert into each other (plugins mix them up).
        match (entry, value) {
            (Value::Int(v), Value::Int(new)) => put(v, index, new[0]),
            (Value::Int(v), Value::Double(new)) => put(v, index, new[0] as c_int),
            (Value::Double(v), Value::Double(new)) => put(v, index, new[0]),
            (Value::Double(v), Value::Int(new)) => put(v, index, new[0] as c_double),
            (Value::Str(v), Value::Str(mut new)) => put(v, index, new.remove(0)),
            (Value::Pointer(v), Value::Pointer(new)) => put(v, index, new[0]),
            _ => return STAT_ERR_VALUE,
        }
        STAT_OK
    }

    fn set_all(&self, name: &str, value: Value) -> OfxStatus {
        let mut values = self.lock();
        match (values.get(name), value) {
            (Some(Value::Double(_)), Value::Int(v)) => {
                values.insert(name.to_string(), Value::Double(v.iter().map(|i| *i as c_double).collect()));
            }
            (Some(Value::Int(_)), Value::Double(v)) => {
                values.insert(name.to_string(), Value::Int(v.iter().map(|d| *d as c_int).collect()));
            }
            (Some(old), value) if std::mem::discriminant(old) != std::mem::discriminant(&value) => return STAT_ERR_VALUE,
            (_, value) => {
                values.insert(name.to_string(), value);
            }
        }
        STAT_OK
    }
}

fn put<T: Clone + Default>(v: &mut Vec<T>, index: usize, value: T) {
    if index >= v.len() {
        v.resize(index + 1, T::default());
    }
    v[index] = value;
}

/// Runs a suite function, turning a panic (a bug in kimchi) into `kOfxStatErrFatal` rather than
/// unwinding into the plugin.
pub fn guarded(f: impl FnOnce() -> OfxStatus) -> OfxStatus {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| {
        tracing::error!("an OpenFX host function panicked");
        STAT_ERR_FATAL
    })
}

/// A C string argument as `&str` (`None` when null or not UTF-8).
///
/// # Safety
/// `ptr` must be null or a 0-terminated string.
pub unsafe fn arg<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the caller's promise.
    unsafe { CStr::from_ptr(ptr) }.to_str().ok()
}

macro_rules! with_set {
    ($handle:expr, $name:expr, |$set:ident, $n:ident| $body:expr) => {
        guarded(|| {
            // SAFETY: handles come from kimchi; names are C strings (OFX's contract).
            let Some($set) = (unsafe { PropertySet::from_handle($handle) }) else { return STAT_ERR_BAD_HANDLE };
            let Some($n) = (unsafe { arg($name) }) else { return STAT_ERR_UNKNOWN };
            $body
        })
    };
}

unsafe extern "C" fn prop_set_pointer(h: OfxPropertySetHandle, name: *const c_char, index: c_int, value: *mut c_void) -> OfxStatus {
    with_set!(h, name, |set, n| set.set_at(n, index, Value::Pointer(vec![value as usize])))
}

unsafe extern "C" fn prop_set_string(h: OfxPropertySetHandle, name: *const c_char, index: c_int, value: *const c_char) -> OfxStatus {
    with_set!(h, name, |set, n| {
        // SAFETY: a C string from the plugin (or null).
        let s = if value.is_null() { CString::default() } else { unsafe { CStr::from_ptr(value) }.to_owned() };
        set.set_at(n, index, Value::Str(vec![s]))
    })
}

unsafe extern "C" fn prop_set_double(h: OfxPropertySetHandle, name: *const c_char, index: c_int, value: c_double) -> OfxStatus {
    with_set!(h, name, |set, n| set.set_at(n, index, Value::Double(vec![value])))
}

unsafe extern "C" fn prop_set_int(h: OfxPropertySetHandle, name: *const c_char, index: c_int, value: c_int) -> OfxStatus {
    with_set!(h, name, |set, n| set.set_at(n, index, Value::Int(vec![value])))
}

/// # Safety
/// `values` must point at `count` items (or be null with `count` 0).
unsafe fn slice<'a, T>(values: *const T, count: c_int) -> &'a [T] {
    if values.is_null() || count <= 0 {
        &[]
    } else {
        // SAFETY: the caller's promise.
        unsafe { std::slice::from_raw_parts(values, count as usize) }
    }
}

unsafe extern "C" fn prop_set_pointer_n(h: OfxPropertySetHandle, name: *const c_char, count: c_int, values: *const *mut c_void) -> OfxStatus {
    with_set!(h, name, |set, n| set.set_all(n, Value::Pointer(unsafe { slice(values, count) }.iter().map(|p| *p as usize).collect())))
}

unsafe extern "C" fn prop_set_string_n(h: OfxPropertySetHandle, name: *const c_char, count: c_int, values: *const *const c_char) -> OfxStatus {
    with_set!(h, name, |set, n| {
        let strings = unsafe { slice(values, count) }
            .iter()
            // SAFETY: C strings from the plugin.
            .map(|p| if p.is_null() { CString::default() } else { unsafe { CStr::from_ptr(*p) }.to_owned() })
            .collect();
        set.set_all(n, Value::Str(strings))
    })
}

unsafe extern "C" fn prop_set_double_n(h: OfxPropertySetHandle, name: *const c_char, count: c_int, values: *const c_double) -> OfxStatus {
    with_set!(h, name, |set, n| set.set_all(n, Value::Double(unsafe { slice(values, count) }.to_vec())))
}

unsafe extern "C" fn prop_set_int_n(h: OfxPropertySetHandle, name: *const c_char, count: c_int, values: *const c_int) -> OfxStatus {
    with_set!(h, name, |set, n| set.set_all(n, Value::Int(unsafe { slice(values, count) }.to_vec())))
}

/// Reads item `index` of a property for a getter, converting ints and doubles.
fn read<T>(set: &PropertySet, name: &str, index: c_int, pick: impl FnOnce(&Value, usize) -> Option<T>) -> Result<T, OfxStatus> {
    let values = set.lock();
    let Some(value) = values.get(name) else { return Err(STAT_ERR_UNKNOWN) };
    if index < 0 || index as usize >= value.len() {
        return Err(STAT_ERR_BAD_INDEX);
    }
    pick(value, index as usize).ok_or(STAT_ERR_VALUE)
}

unsafe extern "C" fn prop_get_pointer(h: OfxPropertySetHandle, name: *const c_char, index: c_int, out: *mut *mut c_void) -> OfxStatus {
    with_set!(h, name, |set, n| match read(set, n, index, |v, i| match v {
        Value::Pointer(p) => Some(p[i] as *mut c_void),
        _ => None,
    }) {
        Ok(p) => {
            // SAFETY: `out` is the plugin's place for the answer.
            unsafe { *out = p };
            STAT_OK
        }
        Err(e) => e,
    })
}

unsafe extern "C" fn prop_get_string(h: OfxPropertySetHandle, name: *const c_char, index: c_int, out: *mut *mut c_char) -> OfxStatus {
    with_set!(h, name, |set, n| match read(set, n, index, |v, i| match v {
        // The pointer stays valid until the property changes or the set goes (the spec's rule).
        Value::Str(s) => Some(s[i].as_ptr() as *mut c_char),
        _ => None,
    }) {
        Ok(p) => {
            // SAFETY: as above.
            unsafe { *out = p };
            STAT_OK
        }
        Err(e) => e,
    })
}

unsafe extern "C" fn prop_get_double(h: OfxPropertySetHandle, name: *const c_char, index: c_int, out: *mut c_double) -> OfxStatus {
    with_set!(h, name, |set, n| match read(set, n, index, |v, i| match v {
        Value::Double(d) => Some(d[i]),
        Value::Int(d) => Some(d[i] as c_double),
        _ => None,
    }) {
        Ok(d) => {
            // SAFETY: as above.
            unsafe { *out = d };
            STAT_OK
        }
        Err(e) => e,
    })
}

unsafe extern "C" fn prop_get_int(h: OfxPropertySetHandle, name: *const c_char, index: c_int, out: *mut c_int) -> OfxStatus {
    with_set!(h, name, |set, n| match read(set, n, index, |v, i| match v {
        Value::Int(d) => Some(d[i]),
        Value::Double(d) => Some(d[i] as c_int),
        _ => None,
    }) {
        Ok(d) => {
            // SAFETY: as above.
            unsafe { *out = d };
            STAT_OK
        }
        Err(e) => e,
    })
}

/// Fills `count` items of `out` from a property.
fn read_n<T>(set: &PropertySet, name: &str, count: c_int, out: *mut T, pick: impl Fn(&Value, usize) -> Option<T>) -> OfxStatus {
    let values = set.lock();
    let Some(value) = values.get(name) else { return STAT_ERR_UNKNOWN };
    if count < 0 || count as usize > value.len() {
        return STAT_ERR_BAD_INDEX;
    }
    for i in 0..count as usize {
        let Some(item) = pick(value, i) else { return STAT_ERR_VALUE };
        // SAFETY: the plugin gave room for `count` items.
        unsafe { out.add(i).write(item) };
    }
    STAT_OK
}

unsafe extern "C" fn prop_get_pointer_n(h: OfxPropertySetHandle, name: *const c_char, count: c_int, out: *mut *mut c_void) -> OfxStatus {
    with_set!(h, name, |set, n| read_n(set, n, count, out, |v, i| match v {
        Value::Pointer(p) => Some(p[i] as *mut c_void),
        _ => None,
    }))
}

unsafe extern "C" fn prop_get_string_n(h: OfxPropertySetHandle, name: *const c_char, count: c_int, out: *mut *mut c_char) -> OfxStatus {
    with_set!(h, name, |set, n| read_n(set, n, count, out, |v, i| match v {
        Value::Str(s) => Some(s[i].as_ptr() as *mut c_char),
        _ => None,
    }))
}

unsafe extern "C" fn prop_get_double_n(h: OfxPropertySetHandle, name: *const c_char, count: c_int, out: *mut c_double) -> OfxStatus {
    with_set!(h, name, |set, n| read_n(set, n, count, out, |v, i| match v {
        Value::Double(d) => Some(d[i]),
        Value::Int(d) => Some(d[i] as c_double),
        _ => None,
    }))
}

unsafe extern "C" fn prop_get_int_n(h: OfxPropertySetHandle, name: *const c_char, count: c_int, out: *mut c_int) -> OfxStatus {
    with_set!(h, name, |set, n| read_n(set, n, count, out, |v, i| match v {
        Value::Int(d) => Some(d[i]),
        Value::Double(d) => Some(d[i] as c_int),
        _ => None,
    }))
}

unsafe extern "C" fn prop_reset(h: OfxPropertySetHandle, name: *const c_char) -> OfxStatus {
    with_set!(h, name, |set, n| if set.has(n) { STAT_OK } else { STAT_ERR_UNKNOWN })
}

unsafe extern "C" fn prop_get_dimension(h: OfxPropertySetHandle, name: *const c_char, count: *mut c_int) -> OfxStatus {
    with_set!(h, name, |set, n| match set.lock().get(n) {
        Some(v) => {
            // SAFETY: the plugin's place for the answer.
            unsafe { *count = v.len() as c_int };
            STAT_OK
        }
        None => STAT_ERR_UNKNOWN,
    })
}

pub static PROPERTY_SUITE_V1: OfxPropertySuiteV1 = OfxPropertySuiteV1 {
    prop_set_pointer,
    prop_set_string,
    prop_set_double,
    prop_set_int,
    prop_set_pointer_n,
    prop_set_string_n,
    prop_set_double_n,
    prop_set_int_n,
    prop_get_pointer,
    prop_get_string,
    prop_get_double,
    prop_get_int,
    prop_get_pointer_n,
    prop_get_string_n,
    prop_get_double_n,
    prop_get_int_n,
    prop_reset,
    prop_get_dimension,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_suite_reads_and_writes_typed_values() {
        let set = PropertySet::new();
        let h = set.handle();
        let s = &PROPERTY_SUITE_V1;
        unsafe {
            assert_eq!((s.prop_set_int)(h, c"a".as_ptr(), 0, 7), STAT_OK);
            assert_eq!((s.prop_set_int)(h, c"a".as_ptr(), 2, 9), STAT_OK);
            let mut dim = 0;
            assert_eq!((s.prop_get_dimension)(h, c"a".as_ptr(), &mut dim), STAT_OK);
            assert_eq!(dim, 3);
            let mut d = 0.0;
            // Ints read as doubles.
            assert_eq!((s.prop_get_double)(h, c"a".as_ptr(), 2, &mut d), STAT_OK);
            assert_eq!(d, 9.0);
            let mut i = 0;
            assert_eq!((s.prop_get_int)(h, c"a".as_ptr(), 5, &mut i), STAT_ERR_BAD_INDEX);
            assert_eq!((s.prop_get_int)(h, c"missing".as_ptr(), 0, &mut i), STAT_ERR_UNKNOWN);

            let strings = [c"x".as_ptr(), c"yz".as_ptr()];
            assert_eq!((s.prop_set_string_n)(h, c"s".as_ptr(), 2, strings.as_ptr()), STAT_OK);
            let mut out = std::ptr::null_mut();
            assert_eq!((s.prop_get_string)(h, c"s".as_ptr(), 1, &mut out), STAT_OK);
            assert_eq!(CStr::from_ptr(out).to_str().unwrap(), "yz");
            // A string set where an int is: refused.
            assert_eq!((s.prop_set_string)(h, c"a".as_ptr(), 0, c"no".as_ptr()), STAT_ERR_VALUE);

            let mut many = [0.0f64; 2];
            (s.prop_set_double_n)(h, c"d".as_ptr(), 2, [1.5, 2.5].as_ptr());
            assert_eq!((s.prop_get_double_n)(h, c"d".as_ptr(), 2, many.as_mut_ptr()), STAT_OK);
            assert_eq!(many, [1.5, 2.5]);
            assert_eq!((s.prop_get_double_n)(h, c"d".as_ptr(), 3, many.as_mut_ptr()), STAT_ERR_BAD_INDEX);
        }
        assert_eq!(set.strings("s"), vec!["x", "yz"]);
        let mut fake = [0u64; 8];
        assert!(unsafe { PropertySet::from_handle(fake.as_mut_ptr() as OfxPropertySetHandle) }.is_none());
    }
}
