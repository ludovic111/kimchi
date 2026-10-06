//! The objects behind OpenFX handles (effects, clips, images, parameters) and the two suites
//! that work on them: the image effect suite and the parameter suite.
//!
//! Each object is boxed (its address is its handle) and starts with a magic number, so a handle
//! a plugin passes back is checked before use.

use std::collections::HashMap;
use std::ffi::{CStr, CString, VaList, c_char, c_double, c_int, c_uint, c_void};
use std::sync::{Mutex, MutexGuard, OnceLock};

use super::ffi::*;
use super::props::{PropertySet, arg, guarded};

const EFFECT_MAGIC: u64 = u64::from_le_bytes(*b"kimchiFX");
const CLIP_MAGIC: u64 = u64::from_le_bytes(*b"kimchiCL");
const PARAMS_MAGIC: u64 = u64::from_le_bytes(*b"kimchiPA");
const PARAM_MAGIC: u64 = u64::from_le_bytes(*b"kimchiP1");
const MEMORY_MAGIC: u64 = u64::from_le_bytes(*b"kimchiMM");

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a clip shows during an action: one picture in the clip's depth and components, rows
/// bottom-up as OpenFX wants.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub data: *mut c_void,
    pub width: c_int,
    pub height: c_int,
    pub row_bytes: c_int,
}

/// An effect: a plugin's descriptor (from Describe), a context's descriptor (from
/// DescribeInContext) or an instance.
pub struct Effect {
    magic: u64,
    pub props: Box<PropertySet>,
    pub params: Box<ParamSet>,
    pub clips: Mutex<Vec<Box<Clip>>>,
    pub is_instance: bool,
    /// The last error or warning the plugin posted (for error messages).
    pub message: Mutex<Option<String>>,
    /// The time actions are run at (frames), for the timeline suite.
    pub time: Mutex<f64>,
}

impl Effect {
    pub fn new(is_instance: bool) -> Box<Self> {
        let props = PropertySet::new();
        let params = Box::new(ParamSet { magic: PARAMS_MAGIC, props: props.handle(), params: Mutex::new(Vec::new()) });
        Box::new(Self {
            magic: EFFECT_MAGIC,
            props,
            params,
            clips: Mutex::new(Vec::new()),
            is_instance,
            message: Mutex::new(None),
            time: Mutex::new(0.0),
        })
    }

    pub fn handle(&self) -> OfxImageEffectHandle {
        self as *const Self as OfxImageEffectHandle
    }

    /// # Safety
    /// `h` must be null or readable for an `Effect`'s size.
    pub unsafe fn from_handle<'a>(h: *const c_void) -> Option<&'a Effect> {
        if h.is_null() {
            return None;
        }
        // SAFETY: the caller's promise; the magic rejects foreign pointers.
        let e = unsafe { &*(h as *const Effect) };
        (e.magic == EFFECT_MAGIC).then_some(e)
    }

    pub fn clip(&self, name: &str) -> Option<*const Clip> {
        lock(&self.clips).iter().find(|c| c.name == name).map(|c| &**c as *const Clip)
    }

    pub fn clip_names(&self) -> Vec<String> {
        lock(&self.clips).iter().map(|c| c.name.clone()).collect()
    }

    /// Runs `f` on the clip called `name`.
    pub fn with_clip<T>(&self, name: &str, f: impl FnOnce(&Clip) -> T) -> Option<T> {
        lock(&self.clips).iter().find(|c| c.name == name).map(|c| f(c))
    }

    /// A new instance of this (context) descriptor: properties copied, each clip and parameter
    /// made from its descriptor, parameters at their defaults.
    pub fn instantiate(&self) -> Box<Effect> {
        let inst = Effect::new(true);
        inst.props.copy_from(&self.props);
        inst.props.set_str(prop::TYPE, &[TYPE_IMAGE_EFFECT_INSTANCE.to_str().unwrap_or_default()]);
        for clip in lock(&self.clips).iter() {
            let c = Clip::new(&clip.name);
            c.props.copy_from(&clip.props);
            instance_clip_props(&c.props);
            lock(&inst.clips).push(c);
        }
        for p in lock(&self.params.params).iter() {
            let q = Param::new(&p.name, &p.kind);
            q.props.copy_from(&p.props);
            q.props.set_str(prop::TYPE, &[TYPE_PARAMETER_INSTANCE.to_str().unwrap_or_default()]);
            *lock(&q.value) = ParamValue::default_of(&p.kind, &q.props);
            lock(&inst.params.params).push(q);
        }
        inst
    }
}

/// The parameters of an effect. Its property set is the effect's own (as in the spec's model).
pub struct ParamSet {
    magic: u64,
    props: OfxPropertySetHandle,
    pub params: Mutex<Vec<Box<Param>>>,
}

impl ParamSet {
    pub fn handle(&self) -> OfxParamSetHandle {
        self as *const Self as OfxParamSetHandle
    }

    /// # Safety
    /// As [`Effect::from_handle`].
    unsafe fn from_handle<'a>(h: *const c_void) -> Option<&'a ParamSet> {
        if h.is_null() {
            return None;
        }
        // SAFETY: as above.
        let p = unsafe { &*(h as *const ParamSet) };
        (p.magic == PARAMS_MAGIC).then_some(p)
    }

    pub fn with<T>(&self, name: &str, f: impl FnOnce(&Param) -> T) -> Option<T> {
        lock(&self.params).iter().find(|p| p.name == name).map(|p| f(p))
    }

    pub fn names(&self) -> Vec<String> {
        lock(&self.params).iter().map(|p| p.name.clone()).collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    Doubles(Vec<f64>),
    Ints(Vec<c_int>),
    Text(CString),
    None,
}

impl ParamValue {
    /// The value a parameter starts at: its `OfxParamPropDefault`.
    pub fn default_of(kind: &str, props: &PropertySet) -> ParamValue {
        let dims = dimensions(kind);
        match value_kind(kind) {
            ValueKind::Double => {
                let mut v = props.doubles(prop::PARAM_DEFAULT);
                v.resize(dims, 0.0);
                ParamValue::Doubles(v)
            }
            ValueKind::Int => {
                let mut v = props.ints(prop::PARAM_DEFAULT);
                v.resize(dims, 0);
                ParamValue::Ints(v)
            }
            ValueKind::Text => {
                ParamValue::Text(CString::new(props.string(prop::PARAM_DEFAULT, 0).unwrap_or_default()).unwrap_or_default())
            }
            ValueKind::None => ParamValue::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ValueKind {
    Double,
    Int,
    Text,
    None,
}

pub fn value_kind(kind: &str) -> ValueKind {
    match kind {
        PARAM_TYPE_DOUBLE | PARAM_TYPE_DOUBLE_2D | PARAM_TYPE_DOUBLE_3D | PARAM_TYPE_RGB | PARAM_TYPE_RGBA => ValueKind::Double,
        PARAM_TYPE_INTEGER | PARAM_TYPE_INTEGER_2D | PARAM_TYPE_INTEGER_3D | PARAM_TYPE_BOOLEAN | PARAM_TYPE_CHOICE => ValueKind::Int,
        PARAM_TYPE_STRING | PARAM_TYPE_STR_CHOICE | PARAM_TYPE_CUSTOM => ValueKind::Text,
        _ => ValueKind::None,
    }
}

pub fn dimensions(kind: &str) -> usize {
    match kind {
        PARAM_TYPE_DOUBLE_2D | PARAM_TYPE_INTEGER_2D => 2,
        PARAM_TYPE_DOUBLE_3D | PARAM_TYPE_INTEGER_3D | PARAM_TYPE_RGB => 3,
        PARAM_TYPE_RGBA => 4,
        _ => 1,
    }
}

const KNOWN_TYPES: &[&str] = &[
    PARAM_TYPE_INTEGER,
    PARAM_TYPE_DOUBLE,
    PARAM_TYPE_BOOLEAN,
    PARAM_TYPE_CHOICE,
    PARAM_TYPE_STR_CHOICE,
    PARAM_TYPE_RGBA,
    PARAM_TYPE_RGB,
    PARAM_TYPE_DOUBLE_2D,
    PARAM_TYPE_INTEGER_2D,
    PARAM_TYPE_DOUBLE_3D,
    PARAM_TYPE_INTEGER_3D,
    PARAM_TYPE_STRING,
    PARAM_TYPE_CUSTOM,
    PARAM_TYPE_GROUP,
    PARAM_TYPE_PAGE,
    PARAM_TYPE_PUSH_BUTTON,
];

pub struct Param {
    magic: u64,
    pub name: String,
    pub kind: String,
    pub props: Box<PropertySet>,
    pub value: Mutex<ParamValue>,
}

impl Param {
    fn new(name: &str, kind: &str) -> Box<Self> {
        Box::new(Self { magic: PARAM_MAGIC, name: name.into(), kind: kind.into(), props: PropertySet::new(), value: Mutex::new(ParamValue::None) })
    }

    /// # Safety
    /// As [`Effect::from_handle`].
    unsafe fn from_handle<'a>(h: *const c_void) -> Option<&'a Param> {
        if h.is_null() {
            return None;
        }
        // SAFETY: as above.
        let p = unsafe { &*(h as *const Param) };
        (p.magic == PARAM_MAGIC).then_some(p)
    }

    pub fn set(&self, value: ParamValue) {
        *lock(&self.value) = value;
    }

    pub fn get(&self) -> ParamValue {
        lock(&self.value).clone()
    }

    /// Fills the plugin's pointers (one per dimension) with the value.
    ///
    /// # Safety
    /// `args` must hold as many pointers of the parameter's value type as it has dimensions.
    unsafe fn get_into(&self, args: &mut VaList) -> OfxStatus {
        let value = lock(&self.value);
        match &*value {
            ParamValue::Doubles(v) => {
                for d in v {
                    // SAFETY: the caller's promise.
                    let p = unsafe { args.next_arg::<*mut c_double>() };
                    if p.is_null() {
                        return STAT_ERR_VALUE;
                    }
                    // SAFETY: a pointer to the plugin's double.
                    unsafe { *p = *d };
                }
            }
            ParamValue::Ints(v) => {
                for i in v {
                    // SAFETY: as above.
                    let p = unsafe { args.next_arg::<*mut c_int>() };
                    if p.is_null() {
                        return STAT_ERR_VALUE;
                    }
                    // SAFETY: as above.
                    unsafe { *p = *i };
                }
            }
            ParamValue::Text(s) => {
                // SAFETY: as above. The pointer stays valid until the value changes.
                let p = unsafe { args.next_arg::<*mut *const c_char>() };
                if p.is_null() {
                    return STAT_ERR_VALUE;
                }
                // SAFETY: as above.
                unsafe { *p = s.as_ptr() };
            }
            ParamValue::None => return STAT_ERR_BAD_HANDLE,
        }
        STAT_OK
    }

    /// Reads a new value from the plugin's varargs (one per dimension).
    ///
    /// # Safety
    /// `args` must hold values of the parameter's type (doubles, ints or a C string).
    unsafe fn set_from(&self, args: &mut VaList) -> OfxStatus {
        let mut value = lock(&self.value);
        match &mut *value {
            ParamValue::Doubles(v) => {
                for d in v.iter_mut() {
                    // SAFETY: the caller's promise.
                    *d = unsafe { args.next_arg::<c_double>() };
                }
            }
            ParamValue::Ints(v) => {
                for i in v.iter_mut() {
                    // SAFETY: as above.
                    *i = unsafe { args.next_arg::<c_int>() };
                }
            }
            ParamValue::Text(s) => {
                // SAFETY: as above.
                let p = unsafe { args.next_arg::<*const c_char>() };
                // SAFETY: a C string from the plugin.
                *s = if p.is_null() { CString::default() } else { unsafe { CStr::from_ptr(p) }.to_owned() };
            }
            ParamValue::None => return STAT_ERR_BAD_HANDLE,
        }
        STAT_OK
    }
}

pub struct Clip {
    magic: u64,
    pub name: String,
    pub props: Box<PropertySet>,
    /// The picture it gives during the current action (`None`: no picture, e.g. unconnected).
    pub frame: Mutex<Option<Frame>>,
}

// SAFETY: the frame's pointer is only read while the instance that set it is rendering.
unsafe impl Send for Clip {}
unsafe impl Sync for Clip {}

impl Clip {
    fn new(name: &str) -> Box<Self> {
        let c = Box::new(Self { magic: CLIP_MAGIC, name: name.into(), props: PropertySet::new(), frame: Mutex::new(None) });
        let p = &c.props;
        p.set_str(prop::TYPE, &[TYPE_CLIP.to_str().unwrap_or_default()]);
        p.set_str(prop::NAME, &[name]);
        for l in [prop::LABEL, prop::SHORT_LABEL, prop::LONG_LABEL] {
            p.set_str(l, &[name]);
        }
        p.set_str(prop::SUPPORTED_COMPONENTS, &[]);
        p.set_int(prop::TEMPORAL_CLIP_ACCESS, &[0]);
        p.set_int(prop::CLIP_OPTIONAL, &[0]);
        p.set_int(prop::CLIP_IS_MASK, &[0]);
        p.set_str(prop::FIELD_EXTRACTION, &["OfxFieldDoubled"]);
        p.set_int(prop::SUPPORTS_TILES, &[1]);
        c
    }

    /// # Safety
    /// As [`Effect::from_handle`].
    pub unsafe fn from_handle<'a>(h: *const c_void) -> Option<&'a Clip> {
        if h.is_null() {
            return None;
        }
        // SAFETY: as above.
        let c = unsafe { &*(h as *const Clip) };
        (c.magic == CLIP_MAGIC).then_some(c)
    }

    pub fn handle(&self) -> OfxImageClipHandle {
        self as *const Self as OfxImageClipHandle
    }
}

/// The properties a clip instance has beyond its descriptor's (set for real by the instance).
fn instance_clip_props(p: &PropertySet) {
    p.set_str(prop::PIXEL_DEPTH, &[BIT_DEPTH_NONE]);
    p.set_str(prop::COMPONENTS, &[COMPONENT_NONE]);
    p.set_str(prop::UNMAPPED_PIXEL_DEPTH, &[BIT_DEPTH_NONE]);
    p.set_str(prop::UNMAPPED_COMPONENTS, &[COMPONENT_NONE]);
    p.set_str(prop::PRE_MULTIPLICATION, &[IMAGE_PREMULTIPLIED]);
    p.set_double(prop::PIXEL_ASPECT_RATIO, &[1.0]);
    p.set_double(prop::FRAME_RATE, &[30.0]);
    p.set_double(prop::FRAME_RANGE, &[0.0, super::FRAME_RANGE_END]);
    p.set_str(prop::FIELD_ORDER, &[FIELD_NONE]);
    p.set_int(prop::CLIP_CONNECTED, &[0]);
    p.set_double(prop::UNMAPPED_FRAME_RANGE, &[0.0, super::FRAME_RANGE_END]);
    p.set_double(prop::UNMAPPED_FRAME_RATE, &[30.0]);
    p.set_int(prop::CONTINUOUS_SAMPLES, &[0]);
}

/// The defaults the spec gives a parameter descriptor of type `kind`.
fn param_props(p: &PropertySet, name: &str, kind: &str) {
    p.set_str(prop::TYPE, &[TYPE_PARAMETER.to_str().unwrap_or_default()]);
    p.set_str(prop::PARAM_TYPE, &[kind]);
    for l in [prop::NAME, prop::LABEL, prop::SHORT_LABEL, prop::LONG_LABEL, prop::PARAM_SCRIPT_NAME] {
        p.set_str(l, &[name]);
    }
    p.set_int(prop::PARAM_SECRET, &[0]);
    p.set_str(prop::PARAM_HINT, &[""]);
    p.set_str(prop::PARAM_PARENT, &[""]);
    p.set_int(prop::PARAM_ENABLED, &[1]);
    p.set_pointer(prop::PARAM_DATA_PTR, &[std::ptr::null_mut()]);
    p.set_str(prop::ICON, &["", ""]);
    match kind {
        PARAM_TYPE_GROUP => p.set_int(prop::PARAM_GROUP_OPEN, &[1]),
        PARAM_TYPE_PAGE => p.set_str(prop::PARAM_PAGE_CHILD, &[]),
        PARAM_TYPE_PUSH_BUTTON => {}
        _ => {
            let animates = !matches!(kind, PARAM_TYPE_CUSTOM | PARAM_TYPE_STRING | PARAM_TYPE_BOOLEAN | PARAM_TYPE_CHOICE | PARAM_TYPE_STR_CHOICE);
            p.set_int(prop::PARAM_ANIMATES, &[animates as c_int]);
            p.set_int(prop::PARAM_IS_ANIMATING, &[0]);
            p.set_int(prop::PARAM_IS_AUTO_KEYING, &[0]);
            p.set_int(prop::PARAM_PERSISTANT, &[1]);
            p.set_int(prop::PARAM_EVALUATE_ON_CHANGE, &[1]);
            p.set_int(prop::PARAM_PLUGIN_MAY_WRITE, &[0]);
            p.set_int(prop::PARAM_CAN_UNDO, &[1]);
            p.set_str(prop::PARAM_CACHE_INVALIDATION, &["OfxParamInvalidateValueChange"]);
            let dims = dimensions(kind);
            match value_kind(kind) {
                ValueKind::Double => {
                    p.set_double(prop::PARAM_DEFAULT, &vec![0.0; dims]);
                    let colour = matches!(kind, PARAM_TYPE_RGB | PARAM_TYPE_RGBA);
                    p.set_double(prop::PARAM_MIN, &vec![-f64::MAX; dims]);
                    p.set_double(prop::PARAM_MAX, &vec![f64::MAX; dims]);
                    p.set_double(prop::PARAM_DISPLAY_MIN, &vec![if colour { 0.0 } else { -f64::MAX }; dims]);
                    p.set_double(prop::PARAM_DISPLAY_MAX, &vec![if colour { 1.0 } else { f64::MAX }; dims]);
                    p.set_double(prop::PARAM_INCREMENT, &[1.0]);
                    p.set_int(prop::PARAM_DIGITS, &[2]);
                    if !colour {
                        p.set_str(prop::PARAM_DOUBLE_TYPE, &[DOUBLE_TYPE_PLAIN]);
                        p.set_str(prop::PARAM_DEFAULT_COORDINATE_SYSTEM, &[COORDINATES_CANONICAL]);
                    }
                    if dims == 1 {
                        p.set_int(prop::PARAM_SHOW_TIME_MARKER, &[0]);
                    }
                    let labels: &[&str] = if colour { &["r", "g", "b", "a"] } else { &["x", "y", "z"] };
                    if dims > 1 {
                        p.set_str(prop::PARAM_DIMENSION_LABEL, &labels[..dims]);
                    }
                }
                ValueKind::Int => {
                    p.set_int(prop::PARAM_DEFAULT, &vec![0; dims]);
                    if kind == PARAM_TYPE_CHOICE {
                        p.set_str(prop::PARAM_CHOICE_OPTION, &[]);
                        p.set_int(prop::PARAM_CHOICE_ORDER, &[]);
                    } else if kind != PARAM_TYPE_BOOLEAN {
                        p.set_int(prop::PARAM_MIN, &vec![c_int::MIN; dims]);
                        p.set_int(prop::PARAM_MAX, &vec![c_int::MAX; dims]);
                        p.set_int(prop::PARAM_DISPLAY_MIN, &vec![c_int::MIN; dims]);
                        p.set_int(prop::PARAM_DISPLAY_MAX, &vec![c_int::MAX; dims]);
                        if dims > 1 {
                            p.set_str(prop::PARAM_DIMENSION_LABEL, &["x", "y", "z"][..dims]);
                        }
                    }
                }
                ValueKind::Text => {
                    p.set_str(prop::PARAM_DEFAULT, &[""]);
                    if kind == PARAM_TYPE_STRING {
                        p.set_str(prop::PARAM_STRING_MODE, &[STRING_SINGLE_LINE]);
                        p.set_int(prop::PARAM_STRING_FILE_PATH_EXISTS, &[1]);
                    }
                    if kind == PARAM_TYPE_STR_CHOICE {
                        p.set_str(prop::PARAM_CHOICE_OPTION, &[]);
                        p.set_str(prop::PARAM_CHOICE_ENUM, &[]);
                        p.set_int(prop::PARAM_CHOICE_ORDER, &[]);
                    }
                }
                ValueKind::None => {}
            }
        }
    }
}

/// The defaults the spec gives an effect descriptor.
pub fn effect_props(p: &PropertySet, file: &str) {
    p.set_str(prop::TYPE, &[TYPE_IMAGE_EFFECT.to_str().unwrap_or_default()]);
    for l in [prop::LABEL, prop::SHORT_LABEL, prop::LONG_LABEL, prop::VERSION_LABEL, prop::PLUGIN_DESCRIPTION, prop::GROUPING] {
        p.set_str(l, &[""]);
    }
    p.set_int(prop::VERSION, &[]);
    p.set_str(prop::SUPPORTED_CONTEXTS, &[]);
    p.set_int(prop::SINGLE_INSTANCE, &[0]);
    p.set_str(prop::RENDER_THREAD_SAFETY, &[RENDER_INSTANCE_SAFE]);
    p.set_int(prop::HOST_FRAME_THREADING, &[1]);
    p.set_int(prop::SUPPORTS_MULTI_RESOLUTION, &[1]);
    p.set_int(prop::SUPPORTS_TILES, &[1]);
    p.set_int(prop::TEMPORAL_CLIP_ACCESS, &[0]);
    p.set_str(prop::SUPPORTED_PIXEL_DEPTHS, &[]);
    p.set_int(prop::FIELD_RENDER_TWICE_ALWAYS, &[1]);
    p.set_int(prop::SUPPORTS_MULTIPLE_CLIP_DEPTHS, &[0]);
    p.set_int(prop::SUPPORTS_MULTIPLE_CLIP_PARS, &[0]);
    p.set_str(prop::CLIP_PREFERENCES_SLAVE_PARAM, &[]);
    p.set_int(prop::SEQUENTIAL_RENDER, &[0]);
    p.set_str(prop::PLUGIN_FILE_PATH, &[file]);
    for gpu in [prop::OPENGL_RENDER_SUPPORTED, prop::CUDA_RENDER_SUPPORTED, prop::CUDA_STREAM_SUPPORTED, prop::METAL_RENDER_SUPPORTED, prop::OPENCL_RENDER_SUPPORTED] {
        p.set_str(gpu, &["false"]);
    }
    p.set_str(prop::PARAM_PAGE_ORDER, &[]);
}

// Images handed out by clipGetImage, by handle, until released.
fn images() -> &'static Mutex<HashMap<usize, Box<PropertySet>>> {
    static IMAGES: OnceLock<Mutex<HashMap<usize, Box<PropertySet>>>> = OnceLock::new();
    IMAGES.get_or_init(Default::default)
}

static IMAGE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

// --- The image effect suite ---------------------------------------------------------------

unsafe extern "C" fn get_property_set(h: OfxImageEffectHandle, out: *mut OfxPropertySetHandle) -> OfxStatus {
    guarded(|| {
        // SAFETY: handles from kimchi; `out` is the plugin's place for the answer.
        let Some(e) = (unsafe { Effect::from_handle(h) }) else { return STAT_ERR_BAD_HANDLE };
        unsafe { *out = e.props.handle() };
        STAT_OK
    })
}

unsafe extern "C" fn get_param_set(h: OfxImageEffectHandle, out: *mut OfxParamSetHandle) -> OfxStatus {
    guarded(|| {
        // SAFETY: as above.
        let Some(e) = (unsafe { Effect::from_handle(h) }) else { return STAT_ERR_BAD_HANDLE };
        unsafe { *out = e.params.handle() };
        STAT_OK
    })
}

unsafe extern "C" fn clip_define(h: OfxImageEffectHandle, name: *const c_char, out: *mut OfxPropertySetHandle) -> OfxStatus {
    guarded(|| {
        // SAFETY: as above.
        let Some(e) = (unsafe { Effect::from_handle(h) }) else { return STAT_ERR_BAD_HANDLE };
        let Some(name) = (unsafe { arg(name) }) else { return STAT_ERR_VALUE };
        if e.is_instance {
            return STAT_ERR_UNSUPPORTED;
        }
        let mut clips = lock(&e.clips);
        let props = match clips.iter().find(|c| c.name == name) {
            Some(c) => c.props.handle(),
            None => {
                let c = Clip::new(name);
                let h = c.props.handle();
                clips.push(c);
                h
            }
        };
        if !out.is_null() {
            // SAFETY: as above.
            unsafe { *out = props };
        }
        STAT_OK
    })
}

unsafe extern "C" fn clip_get_handle(h: OfxImageEffectHandle, name: *const c_char, clip: *mut OfxImageClipHandle, props: *mut OfxPropertySetHandle) -> OfxStatus {
    guarded(|| {
        // SAFETY: as above.
        let Some(e) = (unsafe { Effect::from_handle(h) }) else { return STAT_ERR_BAD_HANDLE };
        let Some(name) = (unsafe { arg(name) }) else { return STAT_ERR_UNKNOWN };
        let found = lock(&e.clips).iter().find(|c| c.name == name).map(|c| (c.handle(), c.props.handle()));
        let Some((c, p)) = found else { return STAT_ERR_UNKNOWN };
        // SAFETY: as above.
        unsafe {
            if !clip.is_null() {
                *clip = c;
            }
            if !props.is_null() {
                *props = p;
            }
        }
        STAT_OK
    })
}

unsafe extern "C" fn clip_get_property_set(clip: OfxImageClipHandle, out: *mut OfxPropertySetHandle) -> OfxStatus {
    guarded(|| {
        // SAFETY: as above.
        let Some(c) = (unsafe { Clip::from_handle(clip) }) else { return STAT_ERR_BAD_HANDLE };
        unsafe { *out = c.props.handle() };
        STAT_OK
    })
}

unsafe extern "C" fn clip_get_image(clip: OfxImageClipHandle, _time: OfxTime, _region: *const OfxRectD, out: *mut OfxPropertySetHandle) -> OfxStatus {
    guarded(|| {
        // SAFETY: as above.
        let Some(c) = (unsafe { Clip::from_handle(clip) }) else { return STAT_ERR_BAD_HANDLE };
        if out.is_null() {
            return STAT_ERR_VALUE;
        }
        // Only the current frame exists (the host says it has no temporal access): any time asked
        // for gets it.
        let Some(frame) = *lock(&c.frame) else { return STAT_FAILED };
        let image = PropertySet::new();
        image.set_str(prop::TYPE, &[TYPE_IMAGE.to_str().unwrap_or_default()]);
        for name in [prop::PIXEL_DEPTH, prop::COMPONENTS, prop::PRE_MULTIPLICATION] {
            image.set_str(name, &[c.props.string(name, 0).unwrap_or_default().as_str()]);
        }
        image.set_double(prop::PIXEL_ASPECT_RATIO, &[1.0]);
        let scale = c.props.doubles(prop::RENDER_SCALE);
        image.set_double(prop::RENDER_SCALE, if scale.len() == 2 { &scale } else { &[1.0, 1.0] });
        image.set_pointer(prop::IMAGE_DATA, &[frame.data]);
        let bounds = [0, 0, frame.width, frame.height];
        image.set_int(prop::IMAGE_BOUNDS, &bounds);
        image.set_int(prop::IMAGE_REGION_OF_DEFINITION, &bounds);
        image.set_int(prop::IMAGE_ROW_BYTES, &[frame.row_bytes]);
        image.set_str(prop::IMAGE_FIELD, &[FIELD_NONE]);
        let id = IMAGE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        image.set_str(prop::UNIQUE_IDENTIFIER, &[format!("kimchi-{}-{id}", c.name).as_str()]);
        let handle = image.handle();
        lock(images()).insert(handle as usize, image);
        // SAFETY: as above.
        unsafe { *out = handle };
        STAT_OK
    })
}

unsafe extern "C" fn clip_release_image(image: OfxPropertySetHandle) -> OfxStatus {
    guarded(|| if lock(images()).remove(&(image as usize)).is_some() { STAT_OK } else { STAT_ERR_BAD_HANDLE })
}

unsafe extern "C" fn clip_get_region_of_definition(clip: OfxImageClipHandle, _time: OfxTime, out: *mut OfxRectD) -> OfxStatus {
    guarded(|| {
        // SAFETY: as above.
        let Some(c) = (unsafe { Clip::from_handle(clip) }) else { return STAT_ERR_BAD_HANDLE };
        if out.is_null() {
            return STAT_ERR_VALUE;
        }
        // Every clip covers the project (canonical coordinates).
        let rod = c.props.doubles(super::PROP_KIMCHI_ROD);
        let r = if rod.len() == 4 { OfxRectD { x1: rod[0], y1: rod[1], x2: rod[2], y2: rod[3] } } else { OfxRectD::default() };
        // SAFETY: as above.
        unsafe { *out = r };
        STAT_OK
    })
}

unsafe extern "C" fn abort(_h: OfxImageEffectHandle) -> c_int {
    0
}

/// Image memory: a block with a lock count. Its pointer stays put while it lives.
struct ImageMemory {
    magic: u64,
    data: Vec<u128>,
}

unsafe extern "C" fn image_memory_alloc(_h: OfxImageEffectHandle, bytes: usize, out: *mut OfxImageMemoryHandle) -> OfxStatus {
    guarded(|| {
        if out.is_null() {
            return STAT_ERR_VALUE;
        }
        let mut data = Vec::new();
        if data.try_reserve_exact(bytes.div_ceil(16)).is_err() {
            // SAFETY: the plugin's place for the answer.
            unsafe { *out = std::ptr::null_mut() };
            return STAT_ERR_MEMORY;
        }
        data.resize(bytes.div_ceil(16), 0);
        let m = Box::new(ImageMemory { magic: MEMORY_MAGIC, data });
        // SAFETY: as above.
        unsafe { *out = Box::into_raw(m) as OfxImageMemoryHandle };
        STAT_OK
    })
}

/// # Safety
/// As [`Effect::from_handle`].
unsafe fn memory<'a>(h: OfxImageMemoryHandle) -> Option<&'a mut ImageMemory> {
    if h.is_null() {
        return None;
    }
    // SAFETY: as above.
    let m = unsafe { &mut *(h as *mut ImageMemory) };
    (m.magic == MEMORY_MAGIC).then_some(m)
}

unsafe extern "C" fn image_memory_free(h: OfxImageMemoryHandle) -> OfxStatus {
    guarded(|| {
        // SAFETY: as above.
        if unsafe { memory(h) }.is_none() {
            return STAT_ERR_BAD_HANDLE;
        }
        // SAFETY: made by image_memory_alloc, freed once.
        let mut m = unsafe { Box::from_raw(h as *mut ImageMemory) };
        m.magic = 0;
        STAT_OK
    })
}

unsafe extern "C" fn image_memory_lock(h: OfxImageMemoryHandle, out: *mut *mut c_void) -> OfxStatus {
    guarded(|| {
        // SAFETY: as above.
        let Some(m) = (unsafe { memory(h) }) else { return STAT_ERR_BAD_HANDLE };
        unsafe { *out = m.data.as_mut_ptr() as *mut c_void };
        STAT_OK
    })
}

unsafe extern "C" fn image_memory_unlock(h: OfxImageMemoryHandle) -> OfxStatus {
    // SAFETY: as above.
    guarded(|| if unsafe { memory(h) }.is_some() { STAT_OK } else { STAT_ERR_BAD_HANDLE })
}

pub static IMAGE_EFFECT_SUITE_V1: OfxImageEffectSuiteV1 = OfxImageEffectSuiteV1 {
    get_property_set,
    get_param_set,
    clip_define,
    clip_get_handle,
    clip_get_property_set,
    clip_get_image,
    clip_release_image,
    clip_get_region_of_definition,
    abort,
    image_memory_alloc,
    image_memory_free,
    image_memory_lock,
    image_memory_unlock,
};

// --- The parameter suite ------------------------------------------------------------------

unsafe extern "C" fn param_define(set: OfxParamSetHandle, kind: *const c_char, name: *const c_char, out: *mut OfxPropertySetHandle) -> OfxStatus {
    guarded(|| {
        // SAFETY: as above.
        let Some(s) = (unsafe { ParamSet::from_handle(set) }) else { return STAT_ERR_BAD_HANDLE };
        let (Some(kind), Some(name)) = (unsafe { arg(kind) }, unsafe { arg(name) }) else { return STAT_ERR_VALUE };
        if !KNOWN_TYPES.contains(&kind) {
            return STAT_ERR_UNKNOWN;
        }
        let mut params = lock(&s.params);
        if params.iter().any(|p| p.name == name) {
            return STAT_ERR_EXISTS;
        }
        let p = Param::new(name, kind);
        param_props(&p.props, name, kind);
        if !out.is_null() {
            // SAFETY: as above.
            unsafe { *out = p.props.handle() };
        }
        params.push(p);
        STAT_OK
    })
}

unsafe extern "C" fn param_get_handle(set: OfxParamSetHandle, name: *const c_char, param: *mut OfxParamHandle, props: *mut OfxPropertySetHandle) -> OfxStatus {
    guarded(|| {
        // SAFETY: as above.
        let Some(s) = (unsafe { ParamSet::from_handle(set) }) else { return STAT_ERR_BAD_HANDLE };
        let Some(name) = (unsafe { arg(name) }) else { return STAT_ERR_UNKNOWN };
        let found = s.with(name, |p| (p as *const Param as OfxParamHandle, p.props.handle()));
        let Some((h, ph)) = found else { return STAT_ERR_UNKNOWN };
        // SAFETY: as above.
        unsafe {
            if !param.is_null() {
                *param = h;
            }
            if !props.is_null() {
                *props = ph;
            }
        }
        STAT_OK
    })
}

unsafe extern "C" fn param_set_get_property_set(set: OfxParamSetHandle, out: *mut OfxPropertySetHandle) -> OfxStatus {
    guarded(|| {
        // SAFETY: as above.
        let Some(s) = (unsafe { ParamSet::from_handle(set) }) else { return STAT_ERR_BAD_HANDLE };
        unsafe { *out = s.props };
        STAT_OK
    })
}

unsafe extern "C" fn param_get_property_set(param: OfxParamHandle, out: *mut OfxPropertySetHandle) -> OfxStatus {
    guarded(|| {
        // SAFETY: as above.
        let Some(p) = (unsafe { Param::from_handle(param) }) else { return STAT_ERR_BAD_HANDLE };
        unsafe { *out = p.props.handle() };
        STAT_OK
    })
}

// Animation is kimchi's: the value a plugin reads is the one at the frame being drawn, whatever
// time it asks for.

unsafe extern "C" fn param_get_value(param: OfxParamHandle, mut args: ...) -> OfxStatus {
    guarded(|| {
        // SAFETY: as above; the plugin passes one pointer per dimension (the suite's contract).
        let Some(p) = (unsafe { Param::from_handle(param) }) else { return STAT_ERR_BAD_HANDLE };
        unsafe { p.get_into(&mut args) }
    })
}

unsafe extern "C" fn param_get_value_at_time(param: OfxParamHandle, _time: OfxTime, mut args: ...) -> OfxStatus {
    guarded(|| {
        // SAFETY: as above.
        let Some(p) = (unsafe { Param::from_handle(param) }) else { return STAT_ERR_BAD_HANDLE };
        unsafe { p.get_into(&mut args) }
    })
}

/// Writes `f(value)` into each double pointer in `args` (derivatives and integrals).
///
/// # Safety
/// As [`Param::get_into`], for a double parameter.
unsafe fn write_doubles(p: &Param, args: &mut VaList, f: impl Fn(f64) -> f64) -> OfxStatus {
    let ParamValue::Doubles(v) = p.get() else { return STAT_ERR_BAD_HANDLE };
    for d in v {
        // SAFETY: the caller's promise.
        let ptr = unsafe { args.next_arg::<*mut c_double>() };
        if ptr.is_null() {
            return STAT_ERR_VALUE;
        }
        // SAFETY: a pointer to the plugin's double.
        unsafe { *ptr = f(d) };
    }
    STAT_OK
}

unsafe extern "C" fn param_get_derivative(param: OfxParamHandle, _time: OfxTime, mut args: ...) -> OfxStatus {
    guarded(|| {
        // SAFETY: as above.
        let Some(p) = (unsafe { Param::from_handle(param) }) else { return STAT_ERR_BAD_HANDLE };
        unsafe { write_doubles(p, &mut args, |_| 0.0) }
    })
}

unsafe extern "C" fn param_get_integral(param: OfxParamHandle, t1: OfxTime, t2: OfxTime, mut args: ...) -> OfxStatus {
    guarded(|| {
        // SAFETY: as above.
        let Some(p) = (unsafe { Param::from_handle(param) }) else { return STAT_ERR_BAD_HANDLE };
        unsafe { write_doubles(p, &mut args, |v| v * (t2 - t1)) }
    })
}

unsafe extern "C" fn param_set_value(param: OfxParamHandle, mut args: ...) -> OfxStatus {
    guarded(|| {
        // SAFETY: as above; one value per dimension.
        let Some(p) = (unsafe { Param::from_handle(param) }) else { return STAT_ERR_BAD_HANDLE };
        unsafe { p.set_from(&mut args) }
    })
}

unsafe extern "C" fn param_set_value_at_time(param: OfxParamHandle, _time: OfxTime, mut args: ...) -> OfxStatus {
    guarded(|| {
        // SAFETY: as above.
        let Some(p) = (unsafe { Param::from_handle(param) }) else { return STAT_ERR_BAD_HANDLE };
        unsafe { p.set_from(&mut args) }
    })
}

unsafe extern "C" fn param_get_num_keys(_param: OfxParamHandle, out: *mut c_uint) -> OfxStatus {
    if !out.is_null() {
        // SAFETY: the plugin's place for the answer.
        unsafe { *out = 0 };
    }
    STAT_OK
}

unsafe extern "C" fn param_get_key_time(_param: OfxParamHandle, _nth: c_uint, _out: *mut OfxTime) -> OfxStatus {
    STAT_ERR_BAD_INDEX
}

unsafe extern "C" fn param_get_key_index(_param: OfxParamHandle, _time: OfxTime, _direction: c_int, _out: *mut c_int) -> OfxStatus {
    STAT_FAILED
}

unsafe extern "C" fn param_delete_key(_param: OfxParamHandle, _time: OfxTime) -> OfxStatus {
    STAT_OK
}

unsafe extern "C" fn param_delete_all_keys(_param: OfxParamHandle) -> OfxStatus {
    STAT_OK
}

unsafe extern "C" fn param_copy(to: OfxParamHandle, from: OfxParamHandle, _offset: OfxTime, _range: *const OfxRangeD) -> OfxStatus {
    guarded(|| {
        // SAFETY: as above.
        let (Some(to), Some(from)) = (unsafe { Param::from_handle(to) }, unsafe { Param::from_handle(from) }) else { return STAT_ERR_BAD_HANDLE };
        if to.kind != from.kind {
            return STAT_ERR_VALUE;
        }
        to.set(from.get());
        STAT_OK
    })
}

unsafe extern "C" fn param_edit_begin(_set: OfxParamSetHandle, _name: *const c_char) -> OfxStatus {
    STAT_OK
}

unsafe extern "C" fn param_edit_end(_set: OfxParamSetHandle) -> OfxStatus {
    STAT_OK
}

pub static PARAMETER_SUITE_V1: OfxParameterSuiteV1 = OfxParameterSuiteV1 {
    param_define,
    param_get_handle,
    param_set_get_property_set,
    param_get_property_set,
    param_get_value,
    param_get_value_at_time,
    param_get_derivative,
    param_get_integral,
    param_set_value,
    param_set_value_at_time,
    param_get_num_keys,
    param_get_key_time,
    param_get_key_index,
    param_delete_key,
    param_delete_all_keys,
    param_copy,
    param_edit_begin,
    param_edit_end,
};
