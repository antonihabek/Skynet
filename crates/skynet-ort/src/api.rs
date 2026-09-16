//! ONNX Runtime's C API, as much of it as this crate calls.
//!
//! # The one thing to understand before editing this file
//!
//! [`OrtApi`] is a struct of function pointers, and **its field order is the ABI**. A field inserted,
//! removed or reordered here does not fail to compile and does not fail a type check: it makes every
//! later call jump to the wrong function through a raw pointer. `DRY-COVERAGE-PLAN.md` F15 refused to
//! write this file for exactly that reason, and then found the reason was half wrong — worth reading
//! before changing anything below.
//!
//! Three properties make it tractable, and all three are load-bearing:
//!
//! 1. **The order is the *requested* version's, not the installed library's.** `OrtGetApiBase()` is
//!    the only exported symbol; the `GetApi(version)` on it returns the struct for the version asked
//!    for, and a null pointer when the library is too old to provide it. So a mismatch is a refusal
//!    ([`crate::OrtError::ApiVersion`]) rather than a misaligned struct.
//! 2. **A prefix is enough.** Every function this crate calls lives in the first 101 members, so the
//!    transcription stops at index 100. Reading fewer fields than a struct has is sound; the pointer
//!    ONNX Runtime returns is to a struct at least this long for any version ≥ the one requested.
//! 3. **Only what is called needs a signature.** Members this crate never touches are
//!    [`Opaque`] — a pointer of the right width holding the right place. A wrong signature on a
//!    function nobody calls cannot hurt anybody, and writing 70 signatures to use 24 of them would
//!    be 46 more chances to be wrong.
//!
//! # Where the order came from
//!
//! Not from memory and not by eye. It was extracted mechanically from
//! `include/onnxruntime/core/session/onnxruntime_c_api.h` at tag `v1.30.0` by walking the
//! `struct OrtApi` body and listing its members in declaration order — the three shapes being
//! `ORT_API2_STATUS(Name, …)`, `ORT_CLASS_RELEASE(X)` (which expands to a `ReleaseX` pointer) and a
//! plain `Ret(ORT_API_CALL* Name)(…)`. The index on each line below is that member's position, so a
//! future reader can re-derive the list and diff it against this file rather than trusting it.
//!
//! # Calling convention
//!
//! `ORT_API_CALL` is `__stdcall` on Windows and empty everywhere else, which is exactly what Rust's
//! `extern "system"` means. Not `extern "C"`: those agree on x86-64 and differ on 32-bit Windows,
//! and the difference is a corrupted stack rather than an error.

use std::ffi::{c_char, c_int, c_void};

/// A member of [`OrtApi`] this crate never calls.
///
/// Present so the members around it are at the right offset, and deliberately not a typed function
/// pointer: there is nothing to get wrong about a placeholder. Never dereferenced.
pub type Opaque = *const c_void;

/// An opaque ONNX Runtime handle. Each of these is a distinct C type; the distinction is kept in the
/// signatures below rather than in the Rust type system, because a newtype per handle would be eight
/// wrappers whose only job is to be passed straight back.
pub type Handle = *mut c_void;

/// `OrtStatus*` — null on success, and an owned error otherwise.
pub type Status = *mut c_void;

/// `ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT`, the second member of `ONNXTensorElementDataType`.
///
/// The only element type this crate creates or accepts. A model whose input is `float16` or `double`
/// is refused by name rather than reinterpreted, because reading four bytes of a two-byte tensor is
/// not an error — it is a different number.
pub const TENSOR_ELEMENT_FLOAT: c_int = 1;

/// `ORT_LOGGING_LEVEL_WARNING`, the third member of `OrtLoggingLevel`.
///
/// Warnings and worse reach stderr; ONNX Runtime's `INFO` chatter does not. A robot's policy process
/// writes to the journal, and an inference library narrating graph optimisation at `INFO` on every
/// start is noise in the one log somebody reads after an incident.
pub const LOGGING_LEVEL_WARNING: c_int = 2;

/// `OrtDeviceAllocator` — `OrtAllocatorType`'s `0`.
pub const DEVICE_ALLOCATOR: c_int = 0;

/// `OrtMemTypeDefault` — `OrtMemType`'s `0`.
pub const MEM_TYPE_DEFAULT: c_int = 0;

/// `ORT_ENABLE_ALL` — `GraphOptimizationLevel`'s `99`.
pub const GRAPH_OPT_ALL: c_int = 99;

/// The two entry points, reached through the library's single exported symbol.
///
/// `GetVersionString` is second, which matters more than it looks: it can be called **before**
/// anything trusts a single offset in [`OrtApi`], because it is on this struct rather than that one.
/// [`crate::Ort::open`] uses it that way.
#[repr(C)]
pub struct OrtApiBase {
    /// The struct for the API version asked for, or null when the library cannot provide it.
    pub get_api: Option<unsafe extern "system" fn(version: u32) -> *const OrtApi>,
    /// The library's own version, as a NUL-terminated string it owns.
    pub get_version_string: Option<unsafe extern "system" fn() -> *const c_char>,
}

/// The first 101 members of `OrtApi`, in header order.
///
/// See the module documentation before touching this. The index in each comment is the member's
/// position in the struct, and it is the only thing keeping the calls in [`crate`] pointed at the
/// functions they name.
#[repr(C)]
#[allow(clippy::type_complexity)]
pub struct OrtApi {
    /// 0. `OrtStatus*(OrtErrorCode code, const char* msg)`
    pub create_status: Option<unsafe extern "system" fn(code: c_int, msg: *const c_char) -> Status>,
    /// 1. `OrtErrorCode(const OrtStatus*)`
    pub get_error_code: Option<unsafe extern "system" fn(status: Status) -> c_int>,
    /// 2. `const char*(const OrtStatus*)`
    pub get_error_message: Option<unsafe extern "system" fn(status: Status) -> *const c_char>,
    /// 3. `CreateEnv(OrtLoggingLevel, const char* logid, OrtEnv** out)`
    pub create_env: Option<
        unsafe extern "system" fn(level: c_int, logid: *const c_char, out: *mut Handle) -> Status,
    >,
    /// 4. `CreateEnvWithCustomLogger`
    pub create_env_with_custom_logger: Opaque,
    /// 5. `EnableTelemetryEvents`
    pub enable_telemetry_events: Opaque,
    /// 6. `DisableTelemetryEvents`
    pub disable_telemetry_events: Opaque,
    /// 7. `CreateSession` — from a path. Unused: [`Self::create_session_from_array`] takes bytes this
    ///    crate has already read and digested, and a second read from disk could differ from it.
    pub create_session: Opaque,
    /// 8. `CreateSessionFromArray(const OrtEnv*, const void* data, size_t len,
    ///    const OrtSessionOptions*, OrtSession** out)`
    pub create_session_from_array: Option<
        unsafe extern "system" fn(
            env: Handle,
            data: *const c_void,
            len: usize,
            options: Handle,
            out: *mut Handle,
        ) -> Status,
    >,
    /// 9. `Run(OrtSession*, const OrtRunOptions*, const char* const* input_names,
    ///    const OrtValue* const* inputs, size_t input_len, const char* const* output_names,
    ///    size_t output_len, OrtValue** outputs)`
    pub run: Option<
        unsafe extern "system" fn(
            session: Handle,
            run_options: Handle,
            input_names: *const *const c_char,
            inputs: *const Handle,
            input_len: usize,
            output_names: *const *const c_char,
            output_len: usize,
            outputs: *mut Handle,
        ) -> Status,
    >,
    /// 10. `CreateSessionOptions(OrtSessionOptions** out)`
    pub create_session_options: Option<unsafe extern "system" fn(out: *mut Handle) -> Status>,
    /// 11. `SetOptimizedModelFilePath`
    pub set_optimized_model_file_path: Opaque,
    /// 12. `CloneSessionOptions`
    pub clone_session_options: Opaque,
    /// 13. `SetSessionExecutionMode`
    pub set_session_execution_mode: Opaque,
    /// 14. `EnableProfiling`
    pub enable_profiling: Opaque,
    /// 15. `DisableProfiling`
    pub disable_profiling: Opaque,
    /// 16. `EnableMemPattern`
    pub enable_mem_pattern: Opaque,
    /// 17. `DisableMemPattern`
    pub disable_mem_pattern: Opaque,
    /// 18. `EnableCpuMemArena`
    pub enable_cpu_mem_arena: Opaque,
    /// 19. `DisableCpuMemArena`
    pub disable_cpu_mem_arena: Opaque,
    /// 20. `SetSessionLogId`
    pub set_session_log_id: Opaque,
    /// 21. `SetSessionLogVerbosityLevel`
    pub set_session_log_verbosity_level: Opaque,
    /// 22. `SetSessionLogSeverityLevel`
    pub set_session_log_severity_level: Opaque,
    /// 23. `SetSessionGraphOptimizationLevel(OrtSessionOptions*, GraphOptimizationLevel)`
    pub set_session_graph_optimization_level:
        Option<unsafe extern "system" fn(options: Handle, level: c_int) -> Status>,
    /// 24. `SetIntraOpNumThreads(OrtSessionOptions*, int)`
    pub set_intra_op_num_threads:
        Option<unsafe extern "system" fn(options: Handle, threads: c_int) -> Status>,
    /// 25. `SetInterOpNumThreads`
    pub set_inter_op_num_threads: Opaque,
    /// 26. `CreateCustomOpDomain`
    pub create_custom_op_domain: Opaque,
    /// 27. `CustomOpDomain_Add`
    pub custom_op_domain_add: Opaque,
    /// 28. `AddCustomOpDomain`
    pub add_custom_op_domain: Opaque,
    /// 29. `RegisterCustomOpsLibrary`
    pub register_custom_ops_library: Opaque,
    /// 30. `SessionGetInputCount(const OrtSession*, size_t* out)`
    pub session_get_input_count:
        Option<unsafe extern "system" fn(session: Handle, out: *mut usize) -> Status>,
    /// 31. `SessionGetOutputCount(const OrtSession*, size_t* out)`
    pub session_get_output_count:
        Option<unsafe extern "system" fn(session: Handle, out: *mut usize) -> Status>,
    /// 32. `SessionGetOverridableInitializerCount`
    pub session_get_overridable_initializer_count: Opaque,
    /// 33. `SessionGetInputTypeInfo(const OrtSession*, size_t index, OrtTypeInfo** out)`
    pub session_get_input_type_info: Option<
        unsafe extern "system" fn(session: Handle, index: usize, out: *mut Handle) -> Status,
    >,
    /// 34. `SessionGetOutputTypeInfo(const OrtSession*, size_t index, OrtTypeInfo** out)`
    pub session_get_output_type_info: Option<
        unsafe extern "system" fn(session: Handle, index: usize, out: *mut Handle) -> Status,
    >,
    /// 35. `SessionGetOverridableInitializerTypeInfo`
    pub session_get_overridable_initializer_type_info: Opaque,
    /// 36. `SessionGetInputName(const OrtSession*, size_t index, OrtAllocator*, char** out)`
    pub session_get_input_name: Option<
        unsafe extern "system" fn(
            session: Handle,
            index: usize,
            allocator: Handle,
            out: *mut *mut c_char,
        ) -> Status,
    >,
    /// 37. `SessionGetOutputName(const OrtSession*, size_t index, OrtAllocator*, char** out)`
    pub session_get_output_name: Option<
        unsafe extern "system" fn(
            session: Handle,
            index: usize,
            allocator: Handle,
            out: *mut *mut c_char,
        ) -> Status,
    >,
    /// 38. `SessionGetOverridableInitializerName`
    pub session_get_overridable_initializer_name: Opaque,
    /// 39. `CreateRunOptions`
    pub create_run_options: Opaque,
    /// 40. `RunOptionsSetRunLogVerbosityLevel`
    pub run_options_set_run_log_verbosity_level: Opaque,
    /// 41. `RunOptionsSetRunLogSeverityLevel`
    pub run_options_set_run_log_severity_level: Opaque,
    /// 42. `RunOptionsSetRunTag`
    pub run_options_set_run_tag: Opaque,
    /// 43. `RunOptionsGetRunLogVerbosityLevel`
    pub run_options_get_run_log_verbosity_level: Opaque,
    /// 44. `RunOptionsGetRunLogSeverityLevel`
    pub run_options_get_run_log_severity_level: Opaque,
    /// 45. `RunOptionsGetRunTag`
    pub run_options_get_run_tag: Opaque,
    /// 46. `RunOptionsSetTerminate`
    pub run_options_set_terminate: Opaque,
    /// 47. `RunOptionsUnsetTerminate`
    pub run_options_unset_terminate: Opaque,
    /// 48. `CreateTensorAsOrtValue` — allocates. Unused: this crate owns its buffers already.
    pub create_tensor_as_ort_value: Opaque,
    /// 49. `CreateTensorWithDataAsOrtValue(const OrtMemoryInfo*, void* data, size_t len,
    ///     const int64_t* shape, size_t shape_len, ONNXTensorElementDataType, OrtValue** out)`
    pub create_tensor_with_data_as_ort_value: Option<
        unsafe extern "system" fn(
            info: Handle,
            data: *mut c_void,
            data_len: usize,
            shape: *const i64,
            shape_len: usize,
            element_type: c_int,
            out: *mut Handle,
        ) -> Status,
    >,
    /// 50. `IsTensor`
    pub is_tensor: Opaque,
    /// 51. `GetTensorMutableData(OrtValue*, void** out)`
    pub get_tensor_mutable_data:
        Option<unsafe extern "system" fn(value: Handle, out: *mut *mut c_void) -> Status>,
    /// 52. `FillStringTensor`
    pub fill_string_tensor: Opaque,
    /// 53. `GetStringTensorDataLength`
    pub get_string_tensor_data_length: Opaque,
    /// 54. `GetStringTensorContent`
    pub get_string_tensor_content: Opaque,
    /// 55. `CastTypeInfoToTensorInfo(const OrtTypeInfo*, const OrtTensorTypeAndShapeInfo** out)`
    ///
    /// Note `_Outptr_result_maybenull_`: a type info that is not a tensor yields a **null out and a
    /// null status**, which is a success the caller has to check. [`crate`] does.
    pub cast_type_info_to_tensor_info:
        Option<unsafe extern "system" fn(type_info: Handle, out: *mut Handle) -> Status>,
    /// 56. `GetOnnxTypeFromTypeInfo`
    pub get_onnx_type_from_type_info: Opaque,
    /// 57. `CreateTensorTypeAndShapeInfo`
    pub create_tensor_type_and_shape_info: Opaque,
    /// 58. `SetTensorElementType`
    pub set_tensor_element_type: Opaque,
    /// 59. `SetDimensions`
    pub set_dimensions: Opaque,
    /// 60. `GetTensorElementType(const OrtTensorTypeAndShapeInfo*, ONNXTensorElementDataType* out)`
    pub get_tensor_element_type:
        Option<unsafe extern "system" fn(info: Handle, out: *mut c_int) -> Status>,
    /// 61. `GetDimensionsCount(const OrtTensorTypeAndShapeInfo*, size_t* out)`
    pub get_dimensions_count:
        Option<unsafe extern "system" fn(info: Handle, out: *mut usize) -> Status>,
    /// 62. `GetDimensions(const OrtTensorTypeAndShapeInfo*, int64_t* values, size_t len)`
    pub get_dimensions:
        Option<unsafe extern "system" fn(info: Handle, values: *mut i64, len: usize) -> Status>,
    /// 63. `GetSymbolicDimensions`
    pub get_symbolic_dimensions: Opaque,
    /// 64. `GetTensorShapeElementCount`
    pub get_tensor_shape_element_count: Opaque,
    /// 65. `GetTensorTypeAndShape`
    pub get_tensor_type_and_shape: Opaque,
    /// 66. `GetTypeInfo`
    pub get_type_info: Opaque,
    /// 67. `GetValueType`
    pub get_value_type: Opaque,
    /// 68. `CreateMemoryInfo`
    pub create_memory_info: Opaque,
    /// 69. `CreateCpuMemoryInfo(OrtAllocatorType, OrtMemType, OrtMemoryInfo** out)`
    pub create_cpu_memory_info: Option<
        unsafe extern "system" fn(
            allocator_type: c_int,
            mem_type: c_int,
            out: *mut Handle,
        ) -> Status,
    >,
    /// 70. `CompareMemoryInfo`
    pub compare_memory_info: Opaque,
    /// 71. `MemoryInfoGetName`
    pub memory_info_get_name: Opaque,
    /// 72. `MemoryInfoGetId`
    pub memory_info_get_id: Opaque,
    /// 73. `MemoryInfoGetMemType`
    pub memory_info_get_mem_type: Opaque,
    /// 74. `MemoryInfoGetType`
    pub memory_info_get_type: Opaque,
    /// 75. `AllocatorAlloc`
    pub allocator_alloc: Opaque,
    /// 76. `AllocatorFree(OrtAllocator*, void* p)` — frees the names 36 and 37 allocate.
    pub allocator_free:
        Option<unsafe extern "system" fn(allocator: Handle, p: *mut c_void) -> Status>,
    /// 77. `AllocatorGetInfo`
    pub allocator_get_info: Opaque,
    /// 78. `GetAllocatorWithDefaultOptions(OrtAllocator** out)`
    ///
    /// The returned allocator is ONNX Runtime's own and is **not** released by the caller — there is
    /// no `ReleaseAllocator` for it, which is why [`crate::Onnx`] holds it without owning it.
    pub get_allocator_with_default_options:
        Option<unsafe extern "system" fn(out: *mut Handle) -> Status>,
    /// 79. `AddFreeDimensionOverride`
    pub add_free_dimension_override: Opaque,
    /// 80. `GetValue`
    pub get_value: Opaque,
    /// 81. `GetValueCount`
    pub get_value_count: Opaque,
    /// 82. `CreateValue`
    pub create_value: Opaque,
    /// 83. `CreateOpaqueValue`
    pub create_opaque_value: Opaque,
    /// 84. `GetOpaqueValue`
    pub get_opaque_value: Opaque,
    /// 85. `KernelInfoGetAttribute_float`
    pub kernel_info_get_attribute_float: Opaque,
    /// 86. `KernelInfoGetAttribute_int64`
    pub kernel_info_get_attribute_int64: Opaque,
    /// 87. `KernelInfoGetAttribute_string`
    pub kernel_info_get_attribute_string: Opaque,
    /// 88. `KernelContext_GetInputCount`
    pub kernel_context_get_input_count: Opaque,
    /// 89. `KernelContext_GetOutputCount`
    pub kernel_context_get_output_count: Opaque,
    /// 90. `KernelContext_GetInput`
    pub kernel_context_get_input: Opaque,
    /// 91. `KernelContext_GetOutput`
    pub kernel_context_get_output: Opaque,
    /// 92. `ReleaseEnv(OrtEnv*)`
    pub release_env: Option<unsafe extern "system" fn(handle: Handle)>,
    /// 93. `ReleaseStatus(OrtStatus*)`
    pub release_status: Option<unsafe extern "system" fn(handle: Handle)>,
    /// 94. `ReleaseMemoryInfo(OrtMemoryInfo*)`
    pub release_memory_info: Option<unsafe extern "system" fn(handle: Handle)>,
    /// 95. `ReleaseSession(OrtSession*)`
    pub release_session: Option<unsafe extern "system" fn(handle: Handle)>,
    /// 96. `ReleaseValue(OrtValue*)`
    pub release_value: Option<unsafe extern "system" fn(handle: Handle)>,
    /// 97. `ReleaseRunOptions(OrtRunOptions*)`
    pub release_run_options: Opaque,
    /// 98. `ReleaseTypeInfo(OrtTypeInfo*)`
    pub release_type_info: Option<unsafe extern "system" fn(handle: Handle)>,
    /// 99. `ReleaseTensorTypeAndShapeInfo` — not called: the tensor info reached through
    ///     [`Self::cast_type_info_to_tensor_info`] is owned by the `OrtTypeInfo` it came from, and
    ///     releasing it as well would be a double free.
    pub release_tensor_type_and_shape_info: Opaque,
    /// 100. `ReleaseSessionOptions(OrtSessionOptions*)`
    pub release_session_options: Option<unsafe extern "system" fn(handle: Handle)>,
    // The struct continues to index 414 in v1.30.0. Everything past here is unreachable from this
    // crate, and adding it would be transcription with no caller — the shape
    // `tools/check-production-callers.mjs` exists to refuse.
}
