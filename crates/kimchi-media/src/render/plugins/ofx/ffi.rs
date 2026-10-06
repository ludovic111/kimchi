//! The OpenFX C API as Rust types, written from the OpenFX 1.5 headers (`ofxCore.h`,
//! `ofxProperty.h`, `ofxImageEffect.h`, `ofxParam.h`, `ofxMemory.h`, `ofxMultiThread.h`,
//! `ofxMessage.h`, `ofxProgress.h`, `ofxTimeLine.h` in github.com/AcademySoftwareFoundation/openfx,
//! read 2026-10-06). Only what kimchi's host uses (no interacts, no OpenGL/CUDA/Metal).
//!
//! This file stands alone (no `crate::` paths): the test fixture plugin includes it too.

#![allow(dead_code, non_camel_case_types, clippy::upper_case_acronyms)]

use std::ffi::{CStr, c_char, c_double, c_int, c_uint, c_void};

pub type OfxStatus = c_int;
pub type OfxTime = c_double;

/// Blind handles: what they point at is the host's business.
pub type OfxPropertySetHandle = *mut c_void;
pub type OfxImageEffectHandle = *mut c_void;
pub type OfxImageClipHandle = *mut c_void;
pub type OfxImageMemoryHandle = *mut c_void;
pub type OfxParamSetHandle = *mut c_void;
pub type OfxParamHandle = *mut c_void;
pub type OfxMutexHandle = *mut c_void;

pub type FetchSuiteFn = unsafe extern "C" fn(host: OfxPropertySetHandle, name: *const c_char, version: c_int) -> *const c_void;

#[repr(C)]
pub struct OfxHost {
    pub host: OfxPropertySetHandle,
    pub fetch_suite: FetchSuiteFn,
}

pub type OfxPluginEntryPoint = unsafe extern "C" fn(action: *const c_char, handle: *const c_void, in_args: OfxPropertySetHandle, out_args: OfxPropertySetHandle) -> OfxStatus;

#[repr(C)]
pub struct OfxPlugin {
    pub plugin_api: *const c_char,
    pub api_version: c_int,
    pub plugin_identifier: *const c_char,
    pub plugin_version_major: c_uint,
    pub plugin_version_minor: c_uint,
    pub set_host: Option<unsafe extern "C" fn(host: *mut OfxHost)>,
    pub main_entry: Option<OfxPluginEntryPoint>,
}

pub type GetNumberOfPluginsFn = unsafe extern "C" fn() -> c_int;
pub type GetPluginFn = unsafe extern "C" fn(nth: c_int) -> *mut OfxPlugin;
pub type SetHostFn = unsafe extern "C" fn(host: *const OfxHost) -> OfxStatus;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OfxRectI {
    pub x1: c_int,
    pub y1: c_int,
    pub x2: c_int,
    pub y2: c_int,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OfxRectD {
    pub x1: c_double,
    pub y1: c_double,
    pub x2: c_double,
    pub y2: c_double,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OfxPointD {
    pub x: c_double,
    pub y: c_double,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OfxRangeD {
    pub min: c_double,
    pub max: c_double,
}

// Status codes (ofxCore.h, ofxImageEffect.h).
pub const STAT_OK: OfxStatus = 0;
pub const STAT_FAILED: OfxStatus = 1;
pub const STAT_ERR_FATAL: OfxStatus = 2;
pub const STAT_ERR_UNKNOWN: OfxStatus = 3;
pub const STAT_ERR_MISSING_HOST_FEATURE: OfxStatus = 4;
pub const STAT_ERR_UNSUPPORTED: OfxStatus = 5;
pub const STAT_ERR_EXISTS: OfxStatus = 6;
pub const STAT_ERR_FORMAT: OfxStatus = 7;
pub const STAT_ERR_MEMORY: OfxStatus = 8;
pub const STAT_ERR_BAD_HANDLE: OfxStatus = 9;
pub const STAT_ERR_BAD_INDEX: OfxStatus = 10;
pub const STAT_ERR_VALUE: OfxStatus = 11;
pub const STAT_REPLY_YES: OfxStatus = 12;
pub const STAT_REPLY_NO: OfxStatus = 13;
pub const STAT_REPLY_DEFAULT: OfxStatus = 14;
pub const STAT_UNLICENSED: OfxStatus = 15;
pub const STAT_ERR_IMAGE_FORMAT: OfxStatus = 1000;

/// The name a status is written with in the headers.
pub fn status_name(status: OfxStatus) -> &'static str {
    match status {
        STAT_OK => "kOfxStatOK",
        STAT_FAILED => "kOfxStatFailed",
        STAT_ERR_FATAL => "kOfxStatErrFatal",
        STAT_ERR_UNKNOWN => "kOfxStatErrUnknown",
        STAT_ERR_MISSING_HOST_FEATURE => "kOfxStatErrMissingHostFeature",
        STAT_ERR_UNSUPPORTED => "kOfxStatErrUnsupported",
        STAT_ERR_EXISTS => "kOfxStatErrExists",
        STAT_ERR_FORMAT => "kOfxStatErrFormat",
        STAT_ERR_MEMORY => "kOfxStatErrMemory",
        STAT_ERR_BAD_HANDLE => "kOfxStatErrBadHandle",
        STAT_ERR_BAD_INDEX => "kOfxStatErrBadIndex",
        STAT_ERR_VALUE => "kOfxStatErrValue",
        STAT_REPLY_YES => "kOfxStatReplyYes",
        STAT_REPLY_NO => "kOfxStatReplyNo",
        STAT_REPLY_DEFAULT => "kOfxStatReplyDefault",
        STAT_UNLICENSED => "kOfxStatUnlicensed",
        STAT_ERR_IMAGE_FORMAT => "kOfxStatErrImageFormat",
        _ => "an unknown status",
    }
}

// Suites.

#[repr(C)]
pub struct OfxPropertySuiteV1 {
    pub prop_set_pointer: unsafe extern "C" fn(OfxPropertySetHandle, *const c_char, c_int, *mut c_void) -> OfxStatus,
    pub prop_set_string: unsafe extern "C" fn(OfxPropertySetHandle, *const c_char, c_int, *const c_char) -> OfxStatus,
    pub prop_set_double: unsafe extern "C" fn(OfxPropertySetHandle, *const c_char, c_int, c_double) -> OfxStatus,
    pub prop_set_int: unsafe extern "C" fn(OfxPropertySetHandle, *const c_char, c_int, c_int) -> OfxStatus,
    pub prop_set_pointer_n: unsafe extern "C" fn(OfxPropertySetHandle, *const c_char, c_int, *const *mut c_void) -> OfxStatus,
    pub prop_set_string_n: unsafe extern "C" fn(OfxPropertySetHandle, *const c_char, c_int, *const *const c_char) -> OfxStatus,
    pub prop_set_double_n: unsafe extern "C" fn(OfxPropertySetHandle, *const c_char, c_int, *const c_double) -> OfxStatus,
    pub prop_set_int_n: unsafe extern "C" fn(OfxPropertySetHandle, *const c_char, c_int, *const c_int) -> OfxStatus,
    pub prop_get_pointer: unsafe extern "C" fn(OfxPropertySetHandle, *const c_char, c_int, *mut *mut c_void) -> OfxStatus,
    pub prop_get_string: unsafe extern "C" fn(OfxPropertySetHandle, *const c_char, c_int, *mut *mut c_char) -> OfxStatus,
    pub prop_get_double: unsafe extern "C" fn(OfxPropertySetHandle, *const c_char, c_int, *mut c_double) -> OfxStatus,
    pub prop_get_int: unsafe extern "C" fn(OfxPropertySetHandle, *const c_char, c_int, *mut c_int) -> OfxStatus,
    pub prop_get_pointer_n: unsafe extern "C" fn(OfxPropertySetHandle, *const c_char, c_int, *mut *mut c_void) -> OfxStatus,
    pub prop_get_string_n: unsafe extern "C" fn(OfxPropertySetHandle, *const c_char, c_int, *mut *mut c_char) -> OfxStatus,
    pub prop_get_double_n: unsafe extern "C" fn(OfxPropertySetHandle, *const c_char, c_int, *mut c_double) -> OfxStatus,
    pub prop_get_int_n: unsafe extern "C" fn(OfxPropertySetHandle, *const c_char, c_int, *mut c_int) -> OfxStatus,
    pub prop_reset: unsafe extern "C" fn(OfxPropertySetHandle, *const c_char) -> OfxStatus,
    pub prop_get_dimension: unsafe extern "C" fn(OfxPropertySetHandle, *const c_char, *mut c_int) -> OfxStatus,
}

#[repr(C)]
pub struct OfxImageEffectSuiteV1 {
    pub get_property_set: unsafe extern "C" fn(OfxImageEffectHandle, *mut OfxPropertySetHandle) -> OfxStatus,
    pub get_param_set: unsafe extern "C" fn(OfxImageEffectHandle, *mut OfxParamSetHandle) -> OfxStatus,
    pub clip_define: unsafe extern "C" fn(OfxImageEffectHandle, *const c_char, *mut OfxPropertySetHandle) -> OfxStatus,
    pub clip_get_handle: unsafe extern "C" fn(OfxImageEffectHandle, *const c_char, *mut OfxImageClipHandle, *mut OfxPropertySetHandle) -> OfxStatus,
    pub clip_get_property_set: unsafe extern "C" fn(OfxImageClipHandle, *mut OfxPropertySetHandle) -> OfxStatus,
    pub clip_get_image: unsafe extern "C" fn(OfxImageClipHandle, OfxTime, *const OfxRectD, *mut OfxPropertySetHandle) -> OfxStatus,
    pub clip_release_image: unsafe extern "C" fn(OfxPropertySetHandle) -> OfxStatus,
    pub clip_get_region_of_definition: unsafe extern "C" fn(OfxImageClipHandle, OfxTime, *mut OfxRectD) -> OfxStatus,
    pub abort: unsafe extern "C" fn(OfxImageEffectHandle) -> c_int,
    pub image_memory_alloc: unsafe extern "C" fn(OfxImageEffectHandle, usize, *mut OfxImageMemoryHandle) -> OfxStatus,
    pub image_memory_free: unsafe extern "C" fn(OfxImageMemoryHandle) -> OfxStatus,
    pub image_memory_lock: unsafe extern "C" fn(OfxImageMemoryHandle, *mut *mut c_void) -> OfxStatus,
    pub image_memory_unlock: unsafe extern "C" fn(OfxImageMemoryHandle) -> OfxStatus,
}

/// `paramGetValue(handle, ...)` and friends take C varargs: pointers to the values to fill, or
/// the values to set, one per dimension.
#[repr(C)]
pub struct OfxParameterSuiteV1 {
    pub param_define: unsafe extern "C" fn(OfxParamSetHandle, *const c_char, *const c_char, *mut OfxPropertySetHandle) -> OfxStatus,
    pub param_get_handle: unsafe extern "C" fn(OfxParamSetHandle, *const c_char, *mut OfxParamHandle, *mut OfxPropertySetHandle) -> OfxStatus,
    pub param_set_get_property_set: unsafe extern "C" fn(OfxParamSetHandle, *mut OfxPropertySetHandle) -> OfxStatus,
    pub param_get_property_set: unsafe extern "C" fn(OfxParamHandle, *mut OfxPropertySetHandle) -> OfxStatus,
    pub param_get_value: unsafe extern "C" fn(OfxParamHandle, ...) -> OfxStatus,
    pub param_get_value_at_time: unsafe extern "C" fn(OfxParamHandle, OfxTime, ...) -> OfxStatus,
    pub param_get_derivative: unsafe extern "C" fn(OfxParamHandle, OfxTime, ...) -> OfxStatus,
    pub param_get_integral: unsafe extern "C" fn(OfxParamHandle, OfxTime, OfxTime, ...) -> OfxStatus,
    pub param_set_value: unsafe extern "C" fn(OfxParamHandle, ...) -> OfxStatus,
    pub param_set_value_at_time: unsafe extern "C" fn(OfxParamHandle, OfxTime, ...) -> OfxStatus,
    pub param_get_num_keys: unsafe extern "C" fn(OfxParamHandle, *mut c_uint) -> OfxStatus,
    pub param_get_key_time: unsafe extern "C" fn(OfxParamHandle, c_uint, *mut OfxTime) -> OfxStatus,
    pub param_get_key_index: unsafe extern "C" fn(OfxParamHandle, OfxTime, c_int, *mut c_int) -> OfxStatus,
    pub param_delete_key: unsafe extern "C" fn(OfxParamHandle, OfxTime) -> OfxStatus,
    pub param_delete_all_keys: unsafe extern "C" fn(OfxParamHandle) -> OfxStatus,
    pub param_copy: unsafe extern "C" fn(OfxParamHandle, OfxParamHandle, OfxTime, *const OfxRangeD) -> OfxStatus,
    pub param_edit_begin: unsafe extern "C" fn(OfxParamSetHandle, *const c_char) -> OfxStatus,
    pub param_edit_end: unsafe extern "C" fn(OfxParamSetHandle) -> OfxStatus,
}

#[repr(C)]
pub struct OfxMemorySuiteV1 {
    pub memory_alloc: unsafe extern "C" fn(*mut c_void, usize, *mut *mut c_void) -> OfxStatus,
    pub memory_free: unsafe extern "C" fn(*mut c_void) -> OfxStatus,
}

pub type OfxThreadFunctionV1 = unsafe extern "C" fn(thread_index: c_uint, thread_max: c_uint, custom_arg: *mut c_void);

#[repr(C)]
pub struct OfxMultiThreadSuiteV1 {
    pub multi_thread: unsafe extern "C" fn(OfxThreadFunctionV1, c_uint, *mut c_void) -> OfxStatus,
    pub multi_thread_num_cpus: unsafe extern "C" fn(*mut c_uint) -> OfxStatus,
    pub multi_thread_index: unsafe extern "C" fn(*mut c_uint) -> OfxStatus,
    pub multi_thread_is_spawned_thread: unsafe extern "C" fn() -> c_int,
    pub mutex_create: unsafe extern "C" fn(*mut OfxMutexHandle, c_int) -> OfxStatus,
    pub mutex_destroy: unsafe extern "C" fn(OfxMutexHandle) -> OfxStatus,
    pub mutex_lock: unsafe extern "C" fn(OfxMutexHandle) -> OfxStatus,
    pub mutex_unlock: unsafe extern "C" fn(OfxMutexHandle) -> OfxStatus,
    pub mutex_try_lock: unsafe extern "C" fn(OfxMutexHandle) -> OfxStatus,
}

/// V2 extends V1 (same first member), so one struct serves both.
#[repr(C)]
pub struct OfxMessageSuiteV2 {
    pub message: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, *const c_char, ...) -> OfxStatus,
    pub set_persistent_message: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, *const c_char, ...) -> OfxStatus,
    pub clear_persistent_message: unsafe extern "C" fn(*mut c_void) -> OfxStatus,
}

#[repr(C)]
pub struct OfxProgressSuiteV1 {
    pub progress_start: unsafe extern "C" fn(*mut c_void, *const c_char) -> OfxStatus,
    pub progress_update: unsafe extern "C" fn(*mut c_void, c_double) -> OfxStatus,
    pub progress_end: unsafe extern "C" fn(*mut c_void) -> OfxStatus,
}

#[repr(C)]
pub struct OfxProgressSuiteV2 {
    pub progress_start: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> OfxStatus,
    pub progress_update: unsafe extern "C" fn(*mut c_void, c_double) -> OfxStatus,
    pub progress_end: unsafe extern "C" fn(*mut c_void) -> OfxStatus,
}

#[repr(C)]
pub struct OfxTimeLineSuiteV1 {
    pub get_time: unsafe extern "C" fn(*mut c_void, *mut c_double) -> OfxStatus,
    pub goto_time: unsafe extern "C" fn(*mut c_void, c_double) -> OfxStatus,
    pub get_time_bounds: unsafe extern "C" fn(*mut c_void, *mut c_double, *mut c_double) -> OfxStatus,
}

// Names (all `&CStr`, as they cross the ABI).

pub const IMAGE_EFFECT_PLUGIN_API: &CStr = c"OfxImageEffectPluginAPI";

pub const PROPERTY_SUITE: &CStr = c"OfxPropertySuite";
pub const IMAGE_EFFECT_SUITE: &CStr = c"OfxImageEffectSuite";
pub const PARAMETER_SUITE: &CStr = c"OfxParameterSuite";
pub const MEMORY_SUITE: &CStr = c"OfxMemorySuite";
pub const MULTI_THREAD_SUITE: &CStr = c"OfxMultiThreadSuite";
pub const MESSAGE_SUITE: &CStr = c"OfxMessageSuite";
pub const PROGRESS_SUITE: &CStr = c"OfxProgressSuite";
pub const TIME_LINE_SUITE: &CStr = c"OfxTimeLineSuite";

// Actions.
pub const ACTION_LOAD: &CStr = c"OfxActionLoad";
pub const ACTION_DESCRIBE: &CStr = c"OfxActionDescribe";
pub const ACTION_UNLOAD: &CStr = c"OfxActionUnload";
pub const ACTION_PURGE_CACHES: &CStr = c"OfxActionPurgeCaches";
pub const ACTION_SYNC_PRIVATE_DATA: &CStr = c"OfxActionSyncPrivateData";
pub const ACTION_CREATE_INSTANCE: &CStr = c"OfxActionCreateInstance";
pub const ACTION_DESTROY_INSTANCE: &CStr = c"OfxActionDestroyInstance";
pub const ACTION_INSTANCE_CHANGED: &CStr = c"OfxActionInstanceChanged";
pub const ACTION_BEGIN_INSTANCE_CHANGED: &CStr = c"OfxActionBeginInstanceChanged";
pub const ACTION_END_INSTANCE_CHANGED: &CStr = c"OfxActionEndInstanceChanged";
pub const ACTION_DESCRIBE_IN_CONTEXT: &CStr = c"OfxImageEffectActionDescribeInContext";
pub const ACTION_GET_REGION_OF_DEFINITION: &CStr = c"OfxImageEffectActionGetRegionOfDefinition";
pub const ACTION_GET_REGIONS_OF_INTEREST: &CStr = c"OfxImageEffectActionGetRegionsOfInterest";
pub const ACTION_GET_TIME_DOMAIN: &CStr = c"OfxImageEffectActionGetTimeDomain";
pub const ACTION_GET_FRAMES_NEEDED: &CStr = c"OfxImageEffectActionGetFramesNeeded";
pub const ACTION_GET_CLIP_PREFERENCES: &CStr = c"OfxImageEffectActionGetClipPreferences";
pub const ACTION_IS_IDENTITY: &CStr = c"OfxImageEffectActionIsIdentity";
pub const ACTION_RENDER: &CStr = c"OfxImageEffectActionRender";
pub const ACTION_BEGIN_SEQUENCE_RENDER: &CStr = c"OfxImageEffectActionBeginSequenceRender";
pub const ACTION_END_SEQUENCE_RENDER: &CStr = c"OfxImageEffectActionEndSequenceRender";

// Object types.
pub const TYPE_IMAGE_EFFECT_HOST: &CStr = c"OfxTypeImageEffectHost";
pub const TYPE_IMAGE_EFFECT: &CStr = c"OfxTypeImageEffect";
pub const TYPE_IMAGE_EFFECT_INSTANCE: &CStr = c"OfxTypeImageEffectInstance";
pub const TYPE_CLIP: &CStr = c"OfxTypeClip";
pub const TYPE_IMAGE: &CStr = c"OfxTypeImage";
pub const TYPE_PARAMETER: &CStr = c"OfxTypeParameter";
pub const TYPE_PARAMETER_INSTANCE: &CStr = c"OfxTypeParameterInstance";

// Contexts.
pub const CONTEXT_GENERATOR: &str = "OfxImageEffectContextGenerator";
pub const CONTEXT_FILTER: &str = "OfxImageEffectContextFilter";
pub const CONTEXT_TRANSITION: &str = "OfxImageEffectContextTransition";
pub const CONTEXT_PAINT: &str = "OfxImageEffectContextPaint";
pub const CONTEXT_GENERAL: &str = "OfxImageEffectContextGeneral";
pub const CONTEXT_RETIMER: &str = "OfxImageEffectContextRetimer";

// Clip and param names fixed by the spec.
pub const CLIP_OUTPUT: &str = "Output";
pub const CLIP_SOURCE: &str = "Source";
pub const CLIP_SOURCE_FROM: &str = "SourceFrom";
pub const CLIP_SOURCE_TO: &str = "SourceTo";
pub const PARAM_TRANSITION: &str = "Transition";

// Pixels.
pub const BIT_DEPTH_NONE: &str = "OfxBitDepthNone";
pub const BIT_DEPTH_BYTE: &str = "OfxBitDepthByte";
pub const BIT_DEPTH_SHORT: &str = "OfxBitDepthShort";
pub const BIT_DEPTH_HALF: &str = "OfxBitDepthHalf";
pub const BIT_DEPTH_FLOAT: &str = "OfxBitDepthFloat";
pub const COMPONENT_NONE: &str = "OfxImageComponentNone";
pub const COMPONENT_RGBA: &str = "OfxImageComponentRGBA";
pub const COMPONENT_RGB: &str = "OfxImageComponentRGB";
pub const COMPONENT_ALPHA: &str = "OfxImageComponentAlpha";
pub const IMAGE_OPAQUE: &str = "OfxImageOpaque";
pub const IMAGE_PREMULTIPLIED: &str = "OfxImageAlphaPremultiplied";
pub const IMAGE_UNPREMULTIPLIED: &str = "OfxImageAlphaUnPremultiplied";
pub const FIELD_NONE: &str = "OfxFieldNone";
pub const NATIVE_ORIGIN_BOTTOM_LEFT: &str = "kOfxImageEffectHostPropNativeOriginBottomLeft";
pub const RENDER_UNSAFE: &str = "OfxImageEffectRenderUnsafe";
pub const RENDER_INSTANCE_SAFE: &str = "OfxImageEffectRenderInstanceSafe";
pub const RENDER_FULLY_SAFE: &str = "OfxImageEffectRenderFullySafe";
pub const CHANGE_USER_EDITED: &str = "OfxChangeUserEdited";
pub const CHANGE_PLUGIN_EDITED: &str = "OfxChangePluginEdited";
pub const CHANGE_TIME: &str = "OfxChangeTime";

// Parameter types.
pub const PARAM_TYPE_INTEGER: &str = "OfxParamTypeInteger";
pub const PARAM_TYPE_DOUBLE: &str = "OfxParamTypeDouble";
pub const PARAM_TYPE_BOOLEAN: &str = "OfxParamTypeBoolean";
pub const PARAM_TYPE_CHOICE: &str = "OfxParamTypeChoice";
pub const PARAM_TYPE_STR_CHOICE: &str = "OfxParamTypeStrChoice";
pub const PARAM_TYPE_RGBA: &str = "OfxParamTypeRGBA";
pub const PARAM_TYPE_RGB: &str = "OfxParamTypeRGB";
pub const PARAM_TYPE_DOUBLE_2D: &str = "OfxParamTypeDouble2D";
pub const PARAM_TYPE_INTEGER_2D: &str = "OfxParamTypeInteger2D";
pub const PARAM_TYPE_DOUBLE_3D: &str = "OfxParamTypeDouble3D";
pub const PARAM_TYPE_INTEGER_3D: &str = "OfxParamTypeInteger3D";
pub const PARAM_TYPE_STRING: &str = "OfxParamTypeString";
pub const PARAM_TYPE_CUSTOM: &str = "OfxParamTypeCustom";
pub const PARAM_TYPE_BYTES: &str = "OfxParamTypeBytes";
pub const PARAM_TYPE_GROUP: &str = "OfxParamTypeGroup";
pub const PARAM_TYPE_PAGE: &str = "OfxParamTypePage";
pub const PARAM_TYPE_PUSH_BUTTON: &str = "OfxParamTypePushButton";
pub const PARAM_TYPE_PARAMETRIC: &str = "OfxParamTypeParametric";

// Double parameter interpretations.
pub const DOUBLE_TYPE_PLAIN: &str = "OfxParamDoubleTypePlain";
pub const DOUBLE_TYPE_SCALE: &str = "OfxParamDoubleTypeScale";
pub const DOUBLE_TYPE_ANGLE: &str = "OfxParamDoubleTypeAngle";
pub const DOUBLE_TYPE_TIME: &str = "OfxParamDoubleTypeTime";
pub const DOUBLE_TYPE_ABSOLUTE_TIME: &str = "OfxParamDoubleTypeAbsoluteTime";
pub const DOUBLE_TYPE_X: &str = "OfxParamDoubleTypeX";
pub const DOUBLE_TYPE_Y: &str = "OfxParamDoubleTypeY";
pub const DOUBLE_TYPE_X_ABSOLUTE: &str = "OfxParamDoubleTypeXAbsolute";
pub const DOUBLE_TYPE_Y_ABSOLUTE: &str = "OfxParamDoubleTypeYAbsolute";
pub const DOUBLE_TYPE_XY: &str = "OfxParamDoubleTypeXY";
pub const DOUBLE_TYPE_XY_ABSOLUTE: &str = "OfxParamDoubleTypeXYAbsolute";
/// OpenFX 1.1's normalised types (deprecated, still found in older plugins): values are
/// fractions of the project size.
pub const DOUBLE_TYPE_NORMALISED_X: &str = "OfxParamDoubleTypeNormalisedX";
pub const DOUBLE_TYPE_NORMALISED_Y: &str = "OfxParamDoubleTypeNormalisedY";
pub const DOUBLE_TYPE_NORMALISED_X_ABSOLUTE: &str = "OfxParamDoubleTypeNormalisedXAbsolute";
pub const DOUBLE_TYPE_NORMALISED_Y_ABSOLUTE: &str = "OfxParamDoubleTypeNormalisedYAbsolute";
pub const DOUBLE_TYPE_NORMALISED_XY: &str = "OfxParamDoubleTypeNormalisedXY";
pub const DOUBLE_TYPE_NORMALISED_XY_ABSOLUTE: &str = "OfxParamDoubleTypeNormalisedXYAbsolute";
pub const COORDINATES_CANONICAL: &str = "OfxParamCoordinatesCanonical";
pub const COORDINATES_NORMALISED: &str = "OfxParamCoordinatesNormalised";

pub const STRING_SINGLE_LINE: &str = "OfxParamStringIsSingleLine";
pub const STRING_MULTI_LINE: &str = "OfxParamStringIsMultiLine";
pub const STRING_FILE_PATH: &str = "OfxParamStringIsFilePath";
pub const STRING_DIRECTORY_PATH: &str = "OfxParamStringIsDirectoryPath";
pub const STRING_LABEL: &str = "OfxParamStringIsLabel";
pub const STRING_RICH_TEXT: &str = "OfxParamStringIsRichTextFormat";

pub const MESSAGE_FATAL: &str = "OfxMessageFatal";
pub const MESSAGE_ERROR: &str = "OfxMessageError";
pub const MESSAGE_WARNING: &str = "OfxMessageWarning";
pub const MESSAGE_MESSAGE: &str = "OfxMessageMessage";
pub const MESSAGE_LOG: &str = "OfxMessageLog";
pub const MESSAGE_QUESTION: &str = "OfxMessageQuestion";

/// Property names. Kept as plain `&str`: the host stores properties by name.
pub mod prop {
    pub const API_VERSION: &str = "OfxPropAPIVersion";
    pub const TIME: &str = "OfxPropTime";
    pub const IS_INTERACTIVE: &str = "OfxPropIsInteractive";
    pub const PLUGIN_FILE_PATH: &str = "OfxPluginPropFilePath";
    pub const INSTANCE_DATA: &str = "OfxPropInstanceData";
    pub const TYPE: &str = "OfxPropType";
    pub const NAME: &str = "OfxPropName";
    pub const VERSION: &str = "OfxPropVersion";
    pub const VERSION_LABEL: &str = "OfxPropVersionLabel";
    pub const PLUGIN_DESCRIPTION: &str = "OfxPropPluginDescription";
    pub const LABEL: &str = "OfxPropLabel";
    pub const ICON: &str = "OfxPropIcon";
    pub const SHORT_LABEL: &str = "OfxPropShortLabel";
    pub const LONG_LABEL: &str = "OfxPropLongLabel";
    pub const CHANGE_REASON: &str = "OfxPropChangeReason";
    pub const EFFECT_INSTANCE: &str = "OfxPropEffectInstance";
    pub const HOST_OS_HANDLE: &str = "OfxPropHostOSHandle";

    pub const SUPPORTED_CONTEXTS: &str = "OfxImageEffectPropSupportedContexts";
    pub const PLUGIN_HANDLE: &str = "OfxImageEffectPropPluginHandle";
    pub const HOST_IS_BACKGROUND: &str = "OfxImageEffectHostPropIsBackground";
    pub const SINGLE_INSTANCE: &str = "OfxImageEffectPluginPropSingleInstance";
    pub const RENDER_THREAD_SAFETY: &str = "OfxImageEffectPluginRenderThreadSafety";
    pub const HOST_FRAME_THREADING: &str = "OfxImageEffectPluginPropHostFrameThreading";
    pub const SUPPORTS_MULTIPLE_CLIP_DEPTHS: &str = "OfxImageEffectPropMultipleClipDepths";
    pub const SUPPORTS_MULTIPLE_CLIP_PARS: &str = "OfxImageEffectPropSupportsMultipleClipPARs";
    pub const CLIP_PREFERENCES_SLAVE_PARAM: &str = "OfxImageEffectPropClipPreferencesSlaveParam";
    pub const SETABLE_FRAME_RATE: &str = "OfxImageEffectPropSetableFrameRate";
    pub const SETABLE_FIELDING: &str = "OfxImageEffectPropSetableFielding";
    pub const SEQUENTIAL_RENDER: &str = "OfxImageEffectInstancePropSequentialRender";
    pub const SEQUENTIAL_RENDER_STATUS: &str = "OfxImageEffectPropSequentialRenderStatus";
    pub const NATIVE_ORIGIN: &str = "OfxImageEffectHostPropNativeOrigin";
    pub const INTERACTIVE_RENDER_STATUS: &str = "OfxImageEffectPropInteractiveRenderStatus";
    pub const GROUPING: &str = "OfxImageEffectPluginPropGrouping";
    pub const OBSOLETE: &str = "OfxImageEffectPluginPropObsolete";
    pub const SUPPORTS_OVERLAYS: &str = "OfxImageEffectPropSupportsOverlays";
    pub const OVERLAY_INTERACT_V1: &str = "OfxImageEffectPluginPropOverlayInteractV1";
    pub const SUPPORTS_MULTI_RESOLUTION: &str = "OfxImageEffectPropSupportsMultiResolution";
    pub const SUPPORTS_TILES: &str = "OfxImageEffectPropSupportsTiles";
    pub const TEMPORAL_CLIP_ACCESS: &str = "OfxImageEffectPropTemporalClipAccess";
    pub const CONTEXT: &str = "OfxImageEffectPropContext";
    pub const PIXEL_DEPTH: &str = "OfxImageEffectPropPixelDepth";
    pub const COMPONENTS: &str = "OfxImageEffectPropComponents";
    pub const UNIQUE_IDENTIFIER: &str = "OfxImagePropUniqueIdentifier";
    pub const CONTINUOUS_SAMPLES: &str = "OfxImageClipPropContinuousSamples";
    pub const UNMAPPED_PIXEL_DEPTH: &str = "OfxImageClipPropUnmappedPixelDepth";
    pub const UNMAPPED_COMPONENTS: &str = "OfxImageClipPropUnmappedComponents";
    pub const PRE_MULTIPLICATION: &str = "OfxImageEffectPropPreMultiplication";
    pub const SUPPORTED_PIXEL_DEPTHS: &str = "OfxImageEffectPropSupportedPixelDepths";
    pub const SUPPORTED_COMPONENTS: &str = "OfxImageEffectPropSupportedComponents";
    pub const CLIP_OPTIONAL: &str = "OfxImageClipPropOptional";
    pub const CLIP_IS_MASK: &str = "OfxImageClipPropIsMask";
    pub const PIXEL_ASPECT_RATIO: &str = "OfxImagePropPixelAspectRatio";
    pub const FRAME_RATE: &str = "OfxImageEffectPropFrameRate";
    pub const UNMAPPED_FRAME_RATE: &str = "OfxImageEffectPropUnmappedFrameRate";
    pub const FRAME_STEP: &str = "OfxImageEffectPropFrameStep";
    pub const FRAME_RANGE: &str = "OfxImageEffectPropFrameRange";
    pub const UNMAPPED_FRAME_RANGE: &str = "OfxImageEffectPropUnmappedFrameRange";
    pub const CLIP_CONNECTED: &str = "OfxImageClipPropConnected";
    pub const FRAME_VARYING: &str = "OfxImageEffectFrameVarying";
    pub const RENDER_SCALE: &str = "OfxImageEffectPropRenderScale";
    pub const RENDER_QUALITY_DRAFT: &str = "OfxImageEffectPropRenderQualityDraft";
    pub const NO_SPATIAL_AWARENESS: &str = "OfxImageEffectPropNoSpatialAwareness";
    pub const THUMBNAIL_RENDER: &str = "OfxImageEffectPropThumbnailRender";
    pub const PROJECT_EXTENT: &str = "OfxImageEffectPropProjectExtent";
    pub const PROJECT_SIZE: &str = "OfxImageEffectPropProjectSize";
    pub const PROJECT_OFFSET: &str = "OfxImageEffectPropProjectOffset";
    pub const PROJECT_PIXEL_ASPECT_RATIO: &str = "OfxImageEffectPropPixelAspectRatio";
    pub const EFFECT_DURATION: &str = "OfxImageEffectInstancePropEffectDuration";
    pub const FIELD_ORDER: &str = "OfxImageClipPropFieldOrder";
    pub const IMAGE_DATA: &str = "OfxImagePropData";
    pub const IMAGE_BOUNDS: &str = "OfxImagePropBounds";
    pub const IMAGE_REGION_OF_DEFINITION: &str = "OfxImagePropRegionOfDefinition";
    pub const IMAGE_ROW_BYTES: &str = "OfxImagePropRowBytes";
    pub const IMAGE_FIELD: &str = "OfxImagePropField";
    pub const FIELD_RENDER_TWICE_ALWAYS: &str = "OfxImageEffectPluginPropFieldRenderTwiceAlways";
    pub const FIELD_EXTRACTION: &str = "OfxImageClipPropFieldExtraction";
    pub const FIELD_TO_RENDER: &str = "OfxImageEffectPropFieldToRender";
    pub const REGION_OF_DEFINITION: &str = "OfxImageEffectPropRegionOfDefinition";
    pub const REGION_OF_INTEREST: &str = "OfxImageEffectPropRegionOfInterest";
    pub const RENDER_WINDOW: &str = "OfxImageEffectPropRenderWindow";
    pub const OPENGL_RENDER_SUPPORTED: &str = "OfxImageEffectPropOpenGLRenderSupported";
    pub const CUDA_RENDER_SUPPORTED: &str = "OfxImageEffectPropCudaRenderSupported";
    pub const CUDA_STREAM_SUPPORTED: &str = "OfxImageEffectPropCudaStreamSupported";
    pub const METAL_RENDER_SUPPORTED: &str = "OfxImageEffectPropMetalRenderSupported";
    pub const OPENCL_RENDER_SUPPORTED: &str = "OfxImageEffectPropOpenCLRenderSupported";

    pub const PARAM_HOST_SUPPORTS_CUSTOM_ANIMATION: &str = "OfxParamHostPropSupportsCustomAnimation";
    pub const PARAM_HOST_SUPPORTS_STRING_ANIMATION: &str = "OfxParamHostPropSupportsStringAnimation";
    pub const PARAM_HOST_SUPPORTS_BOOLEAN_ANIMATION: &str = "OfxParamHostPropSupportsBooleanAnimation";
    pub const PARAM_HOST_SUPPORTS_CHOICE_ANIMATION: &str = "OfxParamHostPropSupportsChoiceAnimation";
    pub const PARAM_HOST_SUPPORTS_STR_CHOICE: &str = "OfxParamHostPropSupportsStrChoice";
    pub const PARAM_HOST_SUPPORTS_STR_CHOICE_ANIMATION: &str = "OfxParamHostPropSupportsStrChoiceAnimation";
    pub const PARAM_HOST_SUPPORTS_CUSTOM_INTERACT: &str = "OfxParamHostPropSupportsCustomInteract";
    pub const PARAM_HOST_SUPPORTS_PARAMETRIC_ANIMATION: &str = "OfxParamHostPropSupportsParametricAnimation";
    pub const PARAM_HOST_MAX_PARAMETERS: &str = "OfxParamHostPropMaxParameters";
    pub const PARAM_HOST_MAX_PAGES: &str = "OfxParamHostPropMaxPages";
    pub const PARAM_HOST_PAGE_ROW_COLUMN_COUNT: &str = "OfxParamHostPropPageRowColumnCount";

    pub const PARAM_TYPE: &str = "OfxParamPropType";
    pub const PARAM_ANIMATES: &str = "OfxParamPropAnimates";
    pub const PARAM_CAN_UNDO: &str = "OfxParamPropCanUndo";
    pub const PARAM_IS_ANIMATING: &str = "OfxParamPropIsAnimating";
    pub const PARAM_PLUGIN_MAY_WRITE: &str = "OfxParamPropPluginMayWrite";
    pub const PARAM_PERSISTANT: &str = "OfxParamPropPersistant";
    pub const PARAM_EVALUATE_ON_CHANGE: &str = "OfxParamPropEvaluateOnChange";
    pub const PARAM_SECRET: &str = "OfxParamPropSecret";
    pub const PARAM_SCRIPT_NAME: &str = "OfxParamPropScriptName";
    pub const PARAM_CACHE_INVALIDATION: &str = "OfxParamPropCacheInvalidation";
    pub const PARAM_HINT: &str = "OfxParamPropHint";
    pub const PARAM_DEFAULT: &str = "OfxParamPropDefault";
    pub const PARAM_DOUBLE_TYPE: &str = "OfxParamPropDoubleType";
    pub const PARAM_DEFAULT_COORDINATE_SYSTEM: &str = "OfxParamPropDefaultCoordinateSystem";
    pub const PARAM_HAS_HOST_OVERLAY_HANDLE: &str = "OfxParamPropHasHostOverlayHandle";
    pub const PARAM_USE_HOST_OVERLAY_HANDLE: &str = "kOfxParamPropUseHostOverlayHandle";
    pub const PARAM_SHOW_TIME_MARKER: &str = "OfxParamPropShowTimeMarker";
    pub const PARAM_PAGE_ORDER: &str = "OfxPluginPropParamPageOrder";
    pub const PARAM_PAGE_CHILD: &str = "OfxParamPropPageChild";
    pub const PARAM_PARENT: &str = "OfxParamPropParent";
    pub const PARAM_GROUP_OPEN: &str = "OfxParamPropGroupOpen";
    pub const PARAM_ENABLED: &str = "OfxParamPropEnabled";
    pub const PARAM_DATA_PTR: &str = "OfxParamPropDataPtr";
    pub const PARAM_CHOICE_OPTION: &str = "OfxParamPropChoiceOption";
    pub const PARAM_CHOICE_ORDER: &str = "OfxParamPropChoiceOrder";
    pub const PARAM_CHOICE_ENUM: &str = "OfxParamPropChoiceEnum";
    pub const PARAM_MIN: &str = "OfxParamPropMin";
    pub const PARAM_MAX: &str = "OfxParamPropMax";
    pub const PARAM_DISPLAY_MIN: &str = "OfxParamPropDisplayMin";
    pub const PARAM_DISPLAY_MAX: &str = "OfxParamPropDisplayMax";
    pub const PARAM_INCREMENT: &str = "OfxParamPropIncrement";
    pub const PARAM_DIGITS: &str = "OfxParamPropDigits";
    pub const PARAM_DIMENSION_LABEL: &str = "OfxParamPropDimensionLabel";
    pub const PARAM_IS_AUTO_KEYING: &str = "OfxParamPropIsAutoKeying";
    pub const PARAM_STRING_MODE: &str = "OfxParamPropStringMode";
    pub const PARAM_STRING_FILE_PATH_EXISTS: &str = "OfxParamPropStringFilePathExists";
    pub const PARAM_INTERACT_V1: &str = "OfxParamPropInteractV1";
    pub const PARAM_INTERACT_SIZE: &str = "OfxParamPropInteractSize";
    pub const PARAM_INTERACT_SIZE_ASPECT: &str = "OfxParamPropInteractSizeAspect";
    pub const PARAM_INTERACT_MINIMUM_SIZE: &str = "OfxParamPropInteractMinimumSize";
    pub const PARAM_INTERACT_PREFERED_SIZE: &str = "OfxParamPropInteractPreferedSize";
}
