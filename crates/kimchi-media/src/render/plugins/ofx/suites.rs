//! The host descriptor (`OfxHost`, its properties, `fetchSuite`) and the small suites: memory,
//! multi-thread (real threads), message (to `tracing`), progress and timeline.

use std::alloc::Layout;
use std::cell::Cell;
use std::ffi::{CStr, VaList, c_char, c_double, c_int, c_long, c_longlong, c_uint, c_ulong, c_ulonglong, c_void};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread::ThreadId;

use super::ffi::*;
use super::objects::{Effect, IMAGE_EFFECT_SUITE_V1, PARAMETER_SUITE_V1};
use super::props::{PROPERTY_SUITE_V1, PropertySet, arg, guarded};

/// The `OfxHost` given to every plugin, and its properties. Made once, never freed.
struct HostBox {
    _props: Box<PropertySet>,
    host: Box<OfxHost>,
}

// SAFETY: the host struct is read-only after it's made; its property set has a lock.
unsafe impl Send for HostBox {}
unsafe impl Sync for HostBox {}

pub fn host() -> *mut OfxHost {
    static HOST: OnceLock<HostBox> = OnceLock::new();
    let h = HOST.get_or_init(|| {
        let props = PropertySet::new();
        host_props(&props);
        let host = Box::new(OfxHost { host: props.handle(), fetch_suite });
        HostBox { _props: props, host }
    });
    &*h.host as *const OfxHost as *mut OfxHost
}

fn host_props(p: &PropertySet) {
    use super::ffi::prop::*;
    p.set_int(API_VERSION, &[1, 4]);
    p.set_str(TYPE, &[TYPE_IMAGE_EFFECT_HOST.to_str().unwrap_or_default()]);
    p.set_str(NAME, &["xyz.lsuite.kimchi"]);
    p.set_str(LABEL, &["kimchi"]);
    let version: Vec<c_int> = env!("CARGO_PKG_VERSION").split('.').map(|n| n.parse().unwrap_or(0)).collect();
    p.set_int(VERSION, &version);
    p.set_str(VERSION_LABEL, &[env!("CARGO_PKG_VERSION")]);
    p.set_int(HOST_IS_BACKGROUND, &[0]);
    p.set_int(SUPPORTS_OVERLAYS, &[0]);
    p.set_int(SUPPORTS_MULTI_RESOLUTION, &[1]);
    p.set_int(SUPPORTS_TILES, &[1]);
    p.set_int(TEMPORAL_CLIP_ACCESS, &[0]);
    p.set_str(SUPPORTED_COMPONENTS, &[COMPONENT_RGBA, COMPONENT_RGB, COMPONENT_ALPHA]);
    p.set_str(SUPPORTED_PIXEL_DEPTHS, &[BIT_DEPTH_FLOAT, BIT_DEPTH_SHORT, BIT_DEPTH_BYTE]);
    p.set_str(SUPPORTED_CONTEXTS, &[CONTEXT_FILTER, CONTEXT_GENERAL, CONTEXT_GENERATOR, CONTEXT_TRANSITION]);
    p.set_int(SUPPORTS_MULTIPLE_CLIP_DEPTHS, &[0]);
    p.set_int(SUPPORTS_MULTIPLE_CLIP_PARS, &[0]);
    p.set_int(SETABLE_FRAME_RATE, &[0]);
    p.set_int(SETABLE_FIELDING, &[0]);
    p.set_int(PARAM_HOST_SUPPORTS_CUSTOM_INTERACT, &[0]);
    p.set_int(PARAM_HOST_SUPPORTS_STRING_ANIMATION, &[0]);
    p.set_int(PARAM_HOST_SUPPORTS_CHOICE_ANIMATION, &[0]);
    p.set_int(PARAM_HOST_SUPPORTS_BOOLEAN_ANIMATION, &[0]);
    p.set_int(PARAM_HOST_SUPPORTS_CUSTOM_ANIMATION, &[0]);
    p.set_int(PARAM_HOST_SUPPORTS_STR_CHOICE, &[1]);
    p.set_int(PARAM_HOST_SUPPORTS_STR_CHOICE_ANIMATION, &[0]);
    p.set_int(PARAM_HOST_SUPPORTS_PARAMETRIC_ANIMATION, &[0]);
    p.set_pointer(HOST_OS_HANDLE, &[std::ptr::null_mut()]);
    p.set_int(PARAM_HOST_MAX_PARAMETERS, &[-1]);
    p.set_int(PARAM_HOST_MAX_PAGES, &[0]);
    p.set_int(PARAM_HOST_PAGE_ROW_COLUMN_COUNT, &[0, 0]);
    p.set_int(SEQUENTIAL_RENDER, &[0]);
    for gpu in [OPENGL_RENDER_SUPPORTED, CUDA_RENDER_SUPPORTED, CUDA_STREAM_SUPPORTED, METAL_RENDER_SUPPORTED, OPENCL_RENDER_SUPPORTED] {
        p.set_str(gpu, &["false"]);
    }
    p.set_int(RENDER_QUALITY_DRAFT, &[1]);
    p.set_str(NATIVE_ORIGIN, &[NATIVE_ORIGIN_BOTTOM_LEFT]);
}

unsafe extern "C" fn fetch_suite(_host: OfxPropertySetHandle, name: *const c_char, version: c_int) -> *const c_void {
    // SAFETY: a C string from the plugin.
    let Some(name) = (unsafe { arg(name) }) else { return std::ptr::null() };
    let suite: *const c_void = match (name.as_bytes(), version) {
        (n, 1) if n == PROPERTY_SUITE.to_bytes() => &PROPERTY_SUITE_V1 as *const _ as *const c_void,
        (n, 1) if n == IMAGE_EFFECT_SUITE.to_bytes() => &IMAGE_EFFECT_SUITE_V1 as *const _ as *const c_void,
        (n, 1) if n == PARAMETER_SUITE.to_bytes() => &PARAMETER_SUITE_V1 as *const _ as *const c_void,
        (n, 1) if n == MEMORY_SUITE.to_bytes() => &MEMORY_SUITE_V1 as *const _ as *const c_void,
        (n, 1) if n == MULTI_THREAD_SUITE.to_bytes() => &MULTI_THREAD_SUITE_V1 as *const _ as *const c_void,
        (n, 1 | 2) if n == MESSAGE_SUITE.to_bytes() => &MESSAGE_SUITE_V2 as *const _ as *const c_void,
        (n, 1) if n == PROGRESS_SUITE.to_bytes() => &PROGRESS_SUITE_V1 as *const _ as *const c_void,
        (n, 2) if n == PROGRESS_SUITE.to_bytes() => &PROGRESS_SUITE_V2 as *const _ as *const c_void,
        (n, 1) if n == TIME_LINE_SUITE.to_bytes() => &TIME_LINE_SUITE_V1 as *const _ as *const c_void,
        _ => std::ptr::null(),
    };
    if suite.is_null() {
        tracing::debug!(suite = name, version, "an OpenFX plugin asked for a suite kimchi doesn't have");
    }
    suite
}

// --- Memory -------------------------------------------------------------------------------

const HEADER: usize = 16;

unsafe extern "C" fn memory_alloc(_h: *mut c_void, bytes: usize, out: *mut *mut c_void) -> OfxStatus {
    guarded(|| {
        let Ok(layout) = Layout::from_size_align(bytes.saturating_add(HEADER), 16) else { return STAT_ERR_MEMORY };
        // SAFETY: a non-zero size (the header).
        let base = unsafe { std::alloc::alloc(layout) };
        if base.is_null() {
            return STAT_ERR_MEMORY;
        }
        // SAFETY: the block starts with room for the size; `out` is the plugin's place.
        unsafe {
            (base as *mut usize).write(bytes);
            *out = base.add(HEADER) as *mut c_void;
        }
        STAT_OK
    })
}

unsafe extern "C" fn memory_free(ptr: *mut c_void) -> OfxStatus {
    guarded(|| {
        if ptr.is_null() {
            return STAT_OK;
        }
        // SAFETY: `ptr` came from memory_alloc, so a header sits before it.
        unsafe {
            let base = (ptr as *mut u8).sub(HEADER);
            let bytes = (base as *const usize).read();
            std::alloc::dealloc(base, Layout::from_size_align_unchecked(bytes + HEADER, 16));
        }
        STAT_OK
    })
}

pub static MEMORY_SUITE_V1: OfxMemorySuiteV1 = OfxMemorySuiteV1 { memory_alloc, memory_free };

// --- Threads ------------------------------------------------------------------------------

thread_local! {
    /// This thread's index while it runs a plugin's thread function.
    static THREAD_INDEX: Cell<Option<c_uint>> = const { Cell::new(None) };
}

pub fn cpus() -> c_uint {
    std::thread::available_parallelism().map(|n| n.get() as c_uint).unwrap_or(1)
}

struct Job {
    func: OfxThreadFunctionV1,
    arg: *mut c_void,
}

// SAFETY: the plugin asks for its function to run on several threads with this argument.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

unsafe extern "C" fn multi_thread(func: OfxThreadFunctionV1, count: c_uint, arg: *mut c_void) -> OfxStatus {
    guarded(|| {
        if count == 0 {
            return STAT_OK;
        }
        let job = Job { func, arg };
        let run = |i: c_uint| {
            let before = THREAD_INDEX.replace(Some(i));
            // SAFETY: the plugin's own function and argument.
            unsafe { (job.func)(i, count, job.arg) };
            THREAD_INDEX.set(before);
        };
        // One thread asked for, or called again from one of its threads: run here.
        if count == 1 || THREAD_INDEX.get().is_some() {
            (0..count).for_each(run);
            return STAT_OK;
        }
        let next = AtomicU32::new(0);
        let workers = count.min(cpus()).max(1);
        let ok = std::thread::scope(|s| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    std::thread::Builder::new()
                        .name("ofx-plugin".into())
                        .stack_size(8 << 20)
                        .spawn_scoped(s, || {
                            loop {
                                let i = next.fetch_add(1, Ordering::Relaxed);
                                if i >= count {
                                    break;
                                }
                                run(i);
                            }
                        })
                })
                .collect();
            handles.into_iter().all(|h| h.map(|h| h.join().is_ok()).unwrap_or(false))
        });
        // Indexes no thread could take (a spawn failed) run here.
        loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            if i >= count {
                break;
            }
            run(i);
        }
        if ok { STAT_OK } else { STAT_FAILED }
    })
}

unsafe extern "C" fn multi_thread_num_cpus(out: *mut c_uint) -> OfxStatus {
    // SAFETY: the plugin's place for the answer.
    unsafe { *out = cpus() };
    STAT_OK
}

unsafe extern "C" fn multi_thread_index(out: *mut c_uint) -> OfxStatus {
    // SAFETY: as above.
    unsafe { *out = THREAD_INDEX.get().unwrap_or(0) };
    STAT_OK
}

unsafe extern "C" fn multi_thread_is_spawned_thread() -> c_int {
    THREAD_INDEX.get().is_some() as c_int
}

/// A recursive mutex with a lock count, as the suite describes.
struct OfxMutex {
    state: Mutex<(Option<ThreadId>, u32)>,
    free: Condvar,
}

unsafe extern "C" fn mutex_create(out: *mut OfxMutexHandle, count: c_int) -> OfxStatus {
    guarded(|| {
        let owner = (count > 0).then(|| std::thread::current().id());
        let m = Box::new(OfxMutex { state: Mutex::new((owner, count.max(0) as u32)), free: Condvar::new() });
        // SAFETY: the plugin's place for the answer.
        unsafe { *out = Box::into_raw(m) as OfxMutexHandle };
        STAT_OK
    })
}

unsafe extern "C" fn mutex_destroy(m: OfxMutexHandle) -> OfxStatus {
    if m.is_null() {
        return STAT_ERR_BAD_HANDLE;
    }
    // SAFETY: made by mutex_create, destroyed once.
    drop(unsafe { Box::from_raw(m as *mut OfxMutex) });
    STAT_OK
}

unsafe fn mutex<'a>(m: OfxMutexHandle) -> Option<&'a OfxMutex> {
    // SAFETY: made by mutex_create.
    (!m.is_null()).then(|| unsafe { &*(m as *const OfxMutex) })
}

unsafe extern "C" fn mutex_lock(m: OfxMutexHandle) -> OfxStatus {
    // SAFETY: as above.
    let Some(m) = (unsafe { mutex(m) }) else { return STAT_ERR_BAD_HANDLE };
    let me = std::thread::current().id();
    let mut s = m.state.lock().unwrap_or_else(|e| e.into_inner());
    while s.0.is_some_and(|o| o != me) {
        s = m.free.wait(s).unwrap_or_else(|e| e.into_inner());
    }
    s.0 = Some(me);
    s.1 += 1;
    STAT_OK
}

unsafe extern "C" fn mutex_unlock(m: OfxMutexHandle) -> OfxStatus {
    // SAFETY: as above.
    let Some(m) = (unsafe { mutex(m) }) else { return STAT_ERR_BAD_HANDLE };
    let mut s = m.state.lock().unwrap_or_else(|e| e.into_inner());
    if s.0 == Some(std::thread::current().id()) {
        s.1 = s.1.saturating_sub(1);
        if s.1 == 0 {
            s.0 = None;
            m.free.notify_one();
        }
    }
    STAT_OK
}

unsafe extern "C" fn mutex_try_lock(m: OfxMutexHandle) -> OfxStatus {
    // SAFETY: as above.
    let Some(m) = (unsafe { mutex(m) }) else { return STAT_ERR_BAD_HANDLE };
    let me = std::thread::current().id();
    let mut s = m.state.lock().unwrap_or_else(|e| e.into_inner());
    if s.0.is_some_and(|o| o != me) {
        return STAT_FAILED;
    }
    s.0 = Some(me);
    s.1 += 1;
    STAT_OK
}

pub static MULTI_THREAD_SUITE_V1: OfxMultiThreadSuiteV1 = OfxMultiThreadSuiteV1 {
    multi_thread,
    multi_thread_num_cpus,
    multi_thread_index,
    multi_thread_is_spawned_thread,
    mutex_create,
    mutex_destroy,
    mutex_lock,
    mutex_unlock,
    mutex_try_lock,
};

// --- Messages -----------------------------------------------------------------------------

/// A small `printf`: `%d %i %u %x %X %o %c %s %f %F %e %E %g %G %p %%` with flags, width,
/// precision and length modifiers (`h hh l ll z j t`). Reads exactly the arguments the format
/// names.
///
/// # Safety
/// `args` must hold arguments matching `format`, as for C's `printf`.
pub unsafe fn printf(format: &[u8], args: &mut VaList) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < format.len() {
        let c = format[i];
        if c != b'%' {
            let start = i;
            while i < format.len() && format[i] != b'%' {
                i += 1;
            }
            out.push_str(&String::from_utf8_lossy(&format[start..i]));
            continue;
        }
        i += 1;
        let mut left = false;
        let mut zero = false;
        while i < format.len() && b"-+ #0".contains(&format[i]) {
            left |= format[i] == b'-';
            zero |= format[i] == b'0';
            i += 1;
        }
        let mut width = 0usize;
        if i < format.len() && format[i] == b'*' {
            // SAFETY: the caller's promise.
            width = unsafe { args.next_arg::<c_int>() }.max(0) as usize;
            i += 1;
        }
        while i < format.len() && format[i].is_ascii_digit() {
            width = width * 10 + (format[i] - b'0') as usize;
            i += 1;
        }
        let mut precision = None;
        if i < format.len() && format[i] == b'.' {
            i += 1;
            let mut p = 0usize;
            if i < format.len() && format[i] == b'*' {
                // SAFETY: as above.
                p = unsafe { args.next_arg::<c_int>() }.max(0) as usize;
                i += 1;
            }
            while i < format.len() && format[i].is_ascii_digit() {
                p = p * 10 + (format[i] - b'0') as usize;
                i += 1;
            }
            precision = Some(p);
        }
        let mut long = 0;
        while i < format.len() && b"hlzjtLq".contains(&format[i]) {
            if matches!(format[i], b'l' | b'z' | b'j' | b't' | b'q') {
                long += 1;
            }
            i += 1;
        }
        let Some(&conv) = format.get(i) else { break };
        i += 1;
        // SAFETY (each next_arg below): the caller's promise that arguments match the format.
        let text = match conv {
            b'%' => "%".to_string(),
            b'd' | b'i' => {
                let v: i64 = match long {
                    0 => unsafe { args.next_arg::<c_int>() } as i64,
                    1 => unsafe { args.next_arg::<c_long>() } as i64,
                    _ => unsafe { args.next_arg::<c_longlong>() },
                };
                v.to_string()
            }
            b'u' | b'x' | b'X' | b'o' => {
                let v: u64 = match long {
                    0 => unsafe { args.next_arg::<c_uint>() } as u64,
                    1 => unsafe { args.next_arg::<c_ulong>() } as u64,
                    _ => unsafe { args.next_arg::<c_ulonglong>() },
                };
                match conv {
                    b'x' => format!("{v:x}"),
                    b'X' => format!("{v:X}"),
                    b'o' => format!("{v:o}"),
                    _ => v.to_string(),
                }
            }
            b'c' => (unsafe { args.next_arg::<c_int>() } as u8 as char).to_string(),
            b's' => {
                let p = unsafe { args.next_arg::<*const c_char>() };
                let s = if p.is_null() { "(null)".to_string() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() };
                match precision {
                    Some(p) => s.chars().take(p).collect(),
                    None => s,
                }
            }
            b'f' | b'F' => format!("{:.*}", precision.unwrap_or(6), unsafe { args.next_arg::<c_double>() }),
            b'e' | b'E' => format!("{:.*e}", precision.unwrap_or(6), unsafe { args.next_arg::<c_double>() }),
            b'g' | b'G' | b'a' | b'A' => {
                let v = unsafe { args.next_arg::<c_double>() };
                match precision {
                    Some(p) => format!("{v:.p$}"),
                    None => format!("{v}"),
                }
            }
            b'p' => format!("{:p}", unsafe { args.next_arg::<*const c_void>() }),
            b'n' => {
                let _ = unsafe { args.next_arg::<*mut c_int>() };
                String::new()
            }
            other => format!("%{}", other as char),
        };
        let pad = width.saturating_sub(text.chars().count());
        if left {
            out.push_str(&text);
            out.extend(std::iter::repeat_n(' ', pad));
        } else {
            out.extend(std::iter::repeat_n(if zero && conv != b's' { '0' } else { ' ' }, pad));
            out.push_str(&text);
        }
    }
    out
}

/// Logs a plugin's message and keeps errors and warnings on its effect.
///
/// # Safety
/// As [`printf`]; `handle` is null or an effect handle.
unsafe fn post(handle: *mut c_void, kind: *const c_char, id: *const c_char, format: *const c_char, args: &mut VaList) -> OfxStatus {
    // SAFETY: C strings from the plugin.
    let kind = unsafe { arg(kind) }.unwrap_or(MESSAGE_MESSAGE);
    let id = unsafe { arg(id) }.unwrap_or("");
    let text = if format.is_null() {
        String::new()
    } else {
        // SAFETY: the caller's promise.
        unsafe { printf(CStr::from_ptr(format).to_bytes(), args) }
    };
    let text = text.trim().to_string();
    match kind {
        MESSAGE_FATAL | MESSAGE_ERROR | MESSAGE_WARNING => tracing::warn!(kind, id, "OpenFX plugin: {text}"),
        MESSAGE_LOG => tracing::debug!(id, "OpenFX plugin: {text}"),
        _ => tracing::info!(kind, id, "OpenFX plugin: {text}"),
    }
    if matches!(kind, MESSAGE_FATAL | MESSAGE_ERROR | MESSAGE_WARNING)
        // SAFETY: checked by its magic number.
        && let Some(effect) = unsafe { Effect::from_handle(handle) }
    {
        *effect.message.lock().unwrap_or_else(|e| e.into_inner()) = Some(text);
    }
    // Nobody is there to answer a question.
    if kind == MESSAGE_QUESTION { STAT_FAILED } else { STAT_OK }
}

unsafe extern "C" fn message(handle: *mut c_void, kind: *const c_char, id: *const c_char, format: *const c_char, mut args: ...) -> OfxStatus {
    // SAFETY: the suite's contract (printf-style arguments).
    guarded(|| unsafe { post(handle, kind, id, format, &mut args) })
}

unsafe extern "C" fn set_persistent_message(handle: *mut c_void, kind: *const c_char, id: *const c_char, format: *const c_char, mut args: ...) -> OfxStatus {
    // SAFETY: as above.
    guarded(|| unsafe { post(handle, kind, id, format, &mut args) })
}

unsafe extern "C" fn clear_persistent_message(handle: *mut c_void) -> OfxStatus {
    // SAFETY: checked by its magic number.
    if let Some(effect) = unsafe { Effect::from_handle(handle) } {
        *effect.message.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    STAT_OK
}

pub static MESSAGE_SUITE_V2: OfxMessageSuiteV2 = OfxMessageSuiteV2 { message, set_persistent_message, clear_persistent_message };

// --- Progress and timeline ----------------------------------------------------------------

unsafe extern "C" fn progress_start_v1(_h: *mut c_void, label: *const c_char) -> OfxStatus {
    // SAFETY: a C string from the plugin.
    tracing::debug!("OpenFX plugin progress: {}", unsafe { arg(label) }.unwrap_or(""));
    STAT_OK
}

unsafe extern "C" fn progress_start_v2(h: *mut c_void, label: *const c_char, _id: *const c_char) -> OfxStatus {
    // SAFETY: as above.
    unsafe { progress_start_v1(h, label) }
}

unsafe extern "C" fn progress_update(_h: *mut c_void, _progress: c_double) -> OfxStatus {
    STAT_OK
}

unsafe extern "C" fn progress_end(_h: *mut c_void) -> OfxStatus {
    STAT_OK
}

pub static PROGRESS_SUITE_V1: OfxProgressSuiteV1 = OfxProgressSuiteV1 { progress_start: progress_start_v1, progress_update, progress_end };
pub static PROGRESS_SUITE_V2: OfxProgressSuiteV2 = OfxProgressSuiteV2 { progress_start: progress_start_v2, progress_update, progress_end };

unsafe extern "C" fn get_time(h: *mut c_void, out: *mut c_double) -> OfxStatus {
    // SAFETY: checked by its magic number; `out` is the plugin's place.
    let Some(e) = (unsafe { Effect::from_handle(h) }) else { return STAT_ERR_BAD_HANDLE };
    unsafe { *out = *e.time.lock().unwrap_or_else(|e| e.into_inner()) };
    STAT_OK
}

unsafe extern "C" fn goto_time(_h: *mut c_void, _time: c_double) -> OfxStatus {
    // kimchi's playhead isn't the plugin's to move.
    STAT_FAILED
}

unsafe extern "C" fn get_time_bounds(h: *mut c_void, first: *mut c_double, last: *mut c_double) -> OfxStatus {
    // SAFETY: as above.
    if unsafe { Effect::from_handle(h) }.is_none() {
        return STAT_ERR_BAD_HANDLE;
    }
    unsafe {
        *first = 0.0;
        *last = super::FRAME_RANGE_END;
    }
    STAT_OK
}

pub static TIME_LINE_SUITE_V1: OfxTimeLineSuiteV1 = OfxTimeLineSuiteV1 { get_time, goto_time, get_time_bounds };

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn format(out: *mut String, fmt: *const c_char, mut args: ...) {
        unsafe { *out = printf(CStr::from_ptr(fmt).to_bytes(), &mut args) };
    }

    #[test]
    fn printf_formats_like_c() {
        let mut s = String::new();
        unsafe { format(&mut s, c"%s has %d params, %.2f%% done, %5x|%-3c|".as_ptr(), c"Blur".as_ptr(), 12 as c_int, 99.5f64, 255 as c_uint, b'z' as c_int) };
        assert_eq!(s, "Blur has 12 params, 99.50% done,    ff|z  |");
        unsafe { format(&mut s, c"%ld %lu %03d".as_ptr(), -5 as c_long, 7 as c_ulong, 4 as c_int) };
        assert_eq!(s, "-5 7 004");
    }

    #[test]
    fn multi_thread_runs_every_index_once() {
        static HITS: [AtomicU32; 16] = [const { AtomicU32::new(0) }; 16];
        unsafe extern "C" fn work(i: c_uint, max: c_uint, arg: *mut c_void) {
            assert_eq!(max, 16);
            assert_eq!(arg as usize, 42);
            HITS[i as usize].fetch_add(1, Ordering::Relaxed);
            let mut idx = 99;
            unsafe { multi_thread_index(&mut idx) };
            assert_eq!(idx, i);
            assert_eq!(unsafe { multi_thread_is_spawned_thread() }, 1);
        }
        assert_eq!(unsafe { multi_thread(work, 16, 42 as *mut c_void) }, STAT_OK);
        assert!(HITS.iter().all(|h| h.load(Ordering::Relaxed) == 1));
        assert_eq!(unsafe { multi_thread_is_spawned_thread() }, 0);
    }

    #[test]
    fn mutexes_nest_and_exclude() {
        let mut m = std::ptr::null_mut();
        unsafe {
            mutex_create(&mut m, 0);
            assert_eq!(mutex_lock(m), STAT_OK);
            assert_eq!(mutex_lock(m), STAT_OK);
            let addr = m as usize;
            let other = std::thread::spawn(move || mutex_try_lock(addr as OfxMutexHandle)).join().unwrap();
            assert_eq!(other, STAT_FAILED);
            mutex_unlock(m);
            mutex_unlock(m);
            let other = std::thread::spawn(move || {
                let r = mutex_try_lock(addr as OfxMutexHandle);
                mutex_unlock(addr as OfxMutexHandle);
                r
            })
            .join()
            .unwrap();
            assert_eq!(other, STAT_OK);
            mutex_destroy(m);
        }
    }

    #[test]
    fn memory_is_aligned_and_frees() {
        let mut p = std::ptr::null_mut();
        unsafe {
            assert_eq!(memory_alloc(std::ptr::null_mut(), 100, &mut p), STAT_OK);
            assert_eq!(p as usize % 16, 0);
            std::ptr::write_bytes(p as *mut u8, 7, 100);
            assert_eq!(memory_free(p), STAT_OK);
        }
    }
}
