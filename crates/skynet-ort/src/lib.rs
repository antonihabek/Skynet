//! §23.3's production inference runner: an ONNX Runtime session behind `skynet-infer`'s seam.
//!
//! §23.3's table sends an `onnx` load plan to an in-process runner and names **ONNX Runtime**. This
//! is that runner. It implements one trait — [`skynet_infer::Session`] — and it implements nothing
//! else: what reaches a model, what is refused and what an output becomes are all one crate below, in
//! [`skynet_infer::Policy`], and were finished and tested before this existed.
//!
//! # Why `dlopen` and not a Cargo dependency
//!
//! `DRY-COVERAGE-PLAN.md` *Decisions* 12, and it is worth having the short version here because this
//! is where somebody will come to change it.
//!
//! `CONTRIBUTING.md` forbids third-party Rust dependencies, and `ci.yml` enforces it by asserting
//! `Cargo.lock` holds exactly the workspace members. The stated reason is that a vendor building a
//! driver years from now should not need a package registry to have survived. A maintained ONNX
//! Runtime binding crate brings a transitive tree, so taking one means either waiving that rule or
//! narrowing the check until it admits tens of packages.
//!
//! Loading the library at runtime needs no package at all. What it costs is this crate: a
//! transcription of part of ONNX Runtime's C API, where a field in the wrong place calls the wrong
//! function through a raw pointer. F15 judged that cost higher than the dependency's and was working
//! from a wrong premise — the struct's layout is the one a caller **requests**, and a library too old
//! to provide it returns a null pointer rather than a shifted struct. So the failure mode is a
//! refusal at load, and the route that keeps the rule intact is also the one whose worst case is
//! diagnosable.
//!
//! # Why the `unsafe` is here rather than anywhere else
//!
//! Same argument `skynet-dl` makes for itself, one library along. `ci.yml` requires every crate
//! outside a named allow-list to `#![forbid(unsafe_code)]`, and this crate is on that list for one
//! stated reason: **calling into a shared library compiled by somebody else cannot be made safe, so
//! it is confined to one small crate whose whole surface is "load ONNX Runtime, run one tensor
//! through it".** Nothing else in the workspace gains an `unsafe` block because this exists.
//! [`skynet_infer`] stays `#![forbid(unsafe_code)]`, and so does the kernel, and so does everything
//! that decides what a joint is told to do.
//!
//! # What this crate refuses to do
//!
//! * **No provider but CPU.** §23.3 puts CUDA above the kernel and outside everything that has to
//!   stay small, and a policy process that silently picked a GPU would be a policy whose timing
//!   depends on what else is on the machine. A CUDA session is a later, deliberate decision.
//!   `intra_op` threads are pinned to 1 for the same reason: §23.3's *"a 40 ms inference spike delays
//!   a torque command"* is about predictability, and a thread pool sized to the machine is the
//!   opposite of predictable.
//! * **No clock, no ring, no opinion.** [`skynet_infer::Session`]'s contract. Numbers in, numbers
//!   out.
//! * **No clamping.** §7's guards are the only thing that narrows a command, and they are in another
//!   process on the far side of the ring.
//!
//! # The file
//!
//! ```json
//! {
//!   "kind": "onnx",
//!   "version": 1,
//!   "inputs": { "channels": 2, "reads": ["position"] },
//!   "model": "policy.onnx"
//! }
//! ```
//!
//! `model` is resolved **relative to the artefact**, so a policy directory can be moved or mounted
//! somewhere else without editing it. The `inputs` block is read by
//! [`skynet_infer::read_artefact`] — the same parser the `mlp` runner uses, because a second one
//! would eventually disagree about what a layout means and the disagreement would produce plausible
//! numbers rather than an error.
//!
//! The library itself is found in this order: `SKYNET_ORT_LIBRARY` in the environment, then a
//! `library` field in the artefact, then the platform's default name (`onnxruntime.dll`,
//! `libonnxruntime.so`, `libonnxruntime.dylib`) resolved by the system loader. Explicit beats
//! implicit, and an operator who has two ONNX Runtimes on a machine can say which one moves the
//! robot.

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};

use skynet_dl::Library;
use skynet_infer::session::{Session, SessionError};
use skynet_infer::Inputs;
use skynet_json::Json;

pub mod api;

use api::{Handle, OrtApi, OrtApiBase, Status};

/// The API version this crate was transcribed against.
///
/// `ORT_API_VERSION` in `onnxruntime_c_api.h` at tag `v1.30.0`. Asked for by number rather than
/// taking whatever the library offers: `GetApi` returns the struct **for the version requested**, so
/// this constant is what makes [`api::OrtApi`]'s field order a fact about this file rather than a
/// property of whichever library happens to be installed. A library too old to serve it returns
/// null, which is [`OrtError::ApiVersion`].
pub const API_VERSION: u32 = 30;

/// The exported symbol every ONNX Runtime has, and the only one this crate looks up.
const ENTRY_SYMBOL: &str = "OrtGetApiBase";

/// Why an ONNX Runtime session could not be created or run. Every variant names the fix (§64).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrtError {
    /// The library could not be loaded, or has no `OrtGetApiBase`.
    Library {
        /// What was looked for.
        path: String,
        /// What the loader said.
        detail: String,
    },
    /// The library loaded and will not serve the API version this crate reads.
    ApiVersion {
        /// What was asked for.
        wanted: u32,
        /// What the library says it is, when it would say.
        library: String,
    },
    /// The library served the API and then failed its own round-trip.
    ///
    /// See [`Ort::open`]. This is the transcription being wrong, and it is reported rather than run
    /// past.
    SelfCheck {
        /// What went wrong.
        detail: String,
    },
    /// ONNX Runtime refused, in its own words.
    Runtime {
        /// The call that failed.
        call: &'static str,
        /// ONNX Runtime's message.
        detail: String,
    },
    /// The model's shape is not one this seam can drive.
    Shape {
        /// What is wrong.
        detail: String,
    },
    /// The artefact is malformed.
    Artefact {
        /// What is wrong.
        detail: String,
    },
}

impl std::fmt::Display for OrtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrtError::Library { path, detail } => write!(
                f,
                "ONNX Runtime could not be loaded from `{path}`: {detail}\n\
                 §23.3's runner loads the library at run time rather than linking it, so the \
                 library has to be on the machine. Point `SKYNET_ORT_LIBRARY` at it, or put it \
                 where the system loader looks."
            ),
            OrtError::ApiVersion { wanted, library } => write!(
                f,
                "this ONNX Runtime will not serve C API version {wanted} (the library reports \
                 `{library}`).\n\
                 skynet-ort is transcribed against that version and asks for it by number, which \
                 is what makes the call table's layout knowable. A newer library serves it; an \
                 older one cannot, and running against a layout nobody checked is the one thing \
                 this refusal exists to prevent."
            ),
            OrtError::SelfCheck { detail } => write!(
                f,
                "ONNX Runtime loaded and failed skynet-ort's own round-trip: {detail}\n\
                 That is this crate being wrong, not the library. `crates/skynet-ort/src/api.rs` \
                 is a transcription of a struct whose field order is its ABI — re-derive it from \
                 the header for the version above and diff it against that file. Refusing here is \
                 deliberate: the alternative is calling the wrong function through a raw pointer \
                 on a machine that can move."
            ),
            OrtError::Runtime { call, detail } => {
                write!(f, "ONNX Runtime refused `{call}`: {detail}")
            }
            OrtError::Shape { detail } => write!(
                f,
                "{detail}\n\
                 A policy's input is one flat vector and its output is one setpoint per commanded \
                 channel (§23.3). A model with a different interface needs an adapter, and this \
                 seam is deliberately not one."
            ),
            OrtError::Artefact { detail } => write!(f, "{detail}"),
        }
    }
}

impl std::error::Error for OrtError {}

/// A loaded ONNX Runtime, and the call table it served.
///
/// Holds the [`Library`] for as long as `api` points into it. Those two are never separated: the
/// pointer dangles the instant the library is unloaded, and nothing in the type system says so, so
/// the containment is the guarantee.
#[derive(Debug)]
pub struct Ort {
    /// Kept alive because `api` and `version` point into it. Never read directly.
    #[allow(dead_code)]
    library: Library,
    api: *const OrtApi,
    version: String,
}

impl Ort {
    /// Load ONNX Runtime and take the call table for [`API_VERSION`].
    ///
    /// # The round-trip, and why it runs on every load
    ///
    /// After `GetApi` succeeds, this makes an `OrtStatus` carrying a known code and a known message
    /// and reads both back — `CreateStatus`, `GetErrorCode`, `GetErrorMessage`, `ReleaseStatus`.
    ///
    /// It is not there to check that ONNX Runtime works. It is there because those four are the
    /// **error path**, and an error path is exercised only when something has already gone wrong. A
    /// successful load and a successful run touch about twenty entries in the table and would prove
    /// them all; the ones that would report a problem would still be unproven at the moment a problem
    /// needed reporting. So they are called once, deliberately, while nothing is wrong and the answer
    /// is known in advance.
    ///
    /// `GetVersionString` is read first, and it can be trusted before anything else here because it
    /// lives on [`OrtApiBase`] — the two-pointer struct reached from the exported symbol — rather
    /// than on the large one this crate transcribes.
    pub fn open(library_path: &str) -> Result<Ort, OrtError> {
        let library =
            Library::open(Path::new(library_path)).map_err(|error| OrtError::Library {
                path: library_path.to_string(),
                detail: error.to_string(),
            })?;
        let address = library
            .address(ENTRY_SYMBOL)
            .map_err(|error| OrtError::Library {
                path: library_path.to_string(),
                detail: error.to_string(),
            })?;

        // SAFETY: `address` is a non-null symbol named `OrtGetApiBase` in a library that exported it.
        // ONNX Runtime's public header declares it `const OrtApiBase* OrtGetApiBase(void)`, and
        // `OrtApiBase` is two function pointers whose order has been fixed since the C API was
        // introduced. The transmute is a function pointer to a function pointer of the same width.
        let get_api_base: unsafe extern "system" fn() -> *const OrtApiBase =
            unsafe { std::mem::transmute::<*mut c_void, _>(address) };

        // SAFETY: calling the symbol above with its declared signature. It takes no arguments,
        // allocates nothing and returns a pointer to a static structure inside the library, which
        // stays valid for as long as the library is loaded — and `library` above is what keeps it so.
        let base = unsafe { get_api_base() };
        if base.is_null() {
            return Err(OrtError::Library {
                path: library_path.to_string(),
                detail: "OrtGetApiBase returned null, which no released ONNX Runtime does"
                    .to_string(),
            });
        }
        // SAFETY: `base` is non-null and points to the library's own static `OrtApiBase`.
        let base = unsafe { &*base };

        // Read before any offset in the big struct is trusted — see the method documentation.
        let version = match base.get_version_string {
            // SAFETY: the second of `OrtApiBase`'s two pointers, non-null, taking no arguments and
            // returning a NUL-terminated string the library owns.
            Some(get) => unsafe {
                let raw = get();
                if raw.is_null() {
                    "unreported".to_string()
                } else {
                    CStr::from_ptr(raw).to_string_lossy().into_owned()
                }
            },
            None => "unreported".to_string(),
        };

        let get_api = base.get_api.ok_or_else(|| OrtError::Library {
            path: library_path.to_string(),
            detail: "OrtApiBase::GetApi is null".to_string(),
        })?;
        // SAFETY: the first of `OrtApiBase`'s two pointers, called with the version this crate was
        // transcribed against. Documented to return null for a version the library cannot serve,
        // which is checked immediately below — that null is the whole reason this route is safer
        // than F15 judged it.
        let api = unsafe { get_api(API_VERSION) };
        if api.is_null() {
            return Err(OrtError::ApiVersion {
                wanted: API_VERSION,
                library: version,
            });
        }

        let ort = Ort {
            library,
            api,
            version,
        };
        ort.round_trip()?;
        Ok(ort)
    }

    /// The library's reported version, for [`Session::describe`] and for a log line.
    pub fn version(&self) -> &str {
        &self.version
    }

    fn api(&self) -> &OrtApi {
        // SAFETY: `self.api` was non-null when `open` took it and points into `self.library`, which
        // this struct owns and which therefore outlives every borrow of `self`.
        unsafe { &*self.api }
    }

    /// Make a status with a known code and message, read both back, and release it.
    fn round_trip(&self) -> Result<(), OrtError> {
        let api = self.api();
        let refuse = |detail: String| Err(OrtError::SelfCheck { detail });

        let (Some(create), Some(code_of), Some(message_of), Some(release)) = (
            api.create_status,
            api.get_error_code,
            api.get_error_message,
            api.release_status,
        ) else {
            return refuse(
                "one of CreateStatus, GetErrorCode, GetErrorMessage or ReleaseStatus is null in the \
                 call table, which a served API version never leaves unset"
                    .to_string(),
            );
        };

        // 2 is ORT_FAIL. Any non-zero code would do; what matters is that it comes back.
        const CODE: std::ffi::c_int = 2;
        let message = CString::new("skynet-ort round-trip").expect("no NUL in a literal");

        // SAFETY: `create` is member 0 of the served call table, called with an error code and a
        // NUL-terminated string that outlives the call. It returns an owned `OrtStatus*`, released
        // below on every path out.
        let status = unsafe { create(CODE, message.as_ptr()) };
        if status.is_null() {
            return refuse("CreateStatus returned null".to_string());
        }

        // The **integer** first, and the pointer only if the integer was right. That order is not
        // stylistic and it was found by running the control: a reordered call table hands
        // `get_error_message` a function that returns an `OrtErrorCode`, so reading the message
        // first means calling `CStr::from_ptr` on the integer `2` — which faults instead of
        // refusing. An `i32` coming back wrong is inspectable; a `*const c_char` coming back wrong
        // is not, so the check that can be made safely is made before the one that cannot.
        //
        // SAFETY: member 1, on the status just created. Returns a plain integer, so nothing here can
        // fault however wrong the table is.
        let code = unsafe { code_of(status) };
        if code != CODE {
            // SAFETY: member 93, releasing the status created above exactly once.
            unsafe { release(status) };
            return refuse(format!(
                "a status made with code {CODE} read back as {code}, so the call table's members \
                 are not where this crate thinks they are. Nothing was dereferenced"
            ));
        }

        // SAFETY: member 2, on the same status. Returns a pointer into it, which is copied before
        // the release below. Reached only because the integer above came back exactly right, which
        // is the evidence that this member is the one it claims to be.
        let text = unsafe {
            let raw = message_of(status);
            if raw.is_null() {
                None
            } else {
                Some(CStr::from_ptr(raw).to_string_lossy().into_owned())
            }
        };
        // SAFETY: member 93, releasing the status created above exactly once. Nothing borrows it
        // after this: `text` is an owned `String`.
        unsafe { release(status) };

        match text.as_deref() {
            Some("skynet-ort round-trip") => Ok(()),
            Some(other) => refuse(format!(
                "a status made with a known message read back as {other:?}"
            )),
            None => refuse("GetErrorMessage returned null for a status that has one".to_string()),
        }
    }

    /// Turn a returned `OrtStatus*` into a `Result`, taking ownership of it.
    ///
    /// Null is success, which is ONNX Runtime's convention throughout.
    fn check(&self, call: &'static str, status: Status) -> Result<(), OrtError> {
        if status.is_null() {
            return Ok(());
        }
        let api = self.api();
        let detail = match api.get_error_message {
            // SAFETY: member 2, on a non-null status ONNX Runtime just returned. The string lives in
            // the status and is copied before the release below.
            Some(message_of) => unsafe {
                let raw = message_of(status);
                if raw.is_null() {
                    "no message".to_string()
                } else {
                    CStr::from_ptr(raw).to_string_lossy().into_owned()
                }
            },
            None => "no message".to_string(),
        };
        if let Some(release) = api.release_status {
            // SAFETY: member 93, releasing a status this function owns, exactly once.
            unsafe { release(status) };
        }
        Err(OrtError::Runtime { call, detail })
    }
}

/// An ONNX model, loaded and ready to run.
///
/// One input tensor and one output tensor, both `float32`, both flat. A model with several inputs is
/// refused at load rather than fed the first one, because §23.3's seam is one vector in and one
/// vector out and guessing which input is "the" input is exactly the kind of decision this layer must
/// not make.
pub struct Onnx {
    // Declaration order is not the drop order here — `Drop` below is written by hand precisely so the
    // handles are released before `ort` unloads the library out from under them.
    ort: Ort,
    env: Handle,
    session: Handle,
    memory_info: Handle,
    // No allocator field. ONNX Runtime's default allocator is needed exactly twice, at load, to
    // produce the two tensor names — which are copied into the `CString`s below and handed straight
    // back. Keeping the handle afterwards would be a field nothing reads, which is the shape
    // `tools/check-production-callers.mjs` exists to refuse one level up.
    input_name: CString,
    output_name: CString,
    input_width: usize,
    output_width: usize,
    /// The input tensor is created over this buffer every pass rather than once, because
    /// [`Session::run`] is handed the caller's slice and copies into here — a tensor built once over
    /// a `Vec` that later reallocated would point at freed memory.
    input: Vec<f32>,
    describe: String,
}

impl std::fmt::Debug for Onnx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Onnx")
            .field("describe", &self.describe)
            .field("input_width", &self.input_width)
            .field("output_width", &self.output_width)
            .finish_non_exhaustive()
    }
}

impl Onnx {
    /// Read an `onnx` artefact and open the session it names.
    ///
    /// `label` is what refusals and [`Session::describe`] will name — a path, usually. `text` is the
    /// artefact's JSON; `beside` is the directory the `model` field is resolved against, which is the
    /// artefact's own directory for every real caller.
    pub fn load(
        label: &str,
        text: &str,
        beside: &Path,
    ) -> Result<(Onnx, Inputs), Box<dyn std::error::Error>> {
        let doc = Json::parse(text).map_err(|error| OrtError::Artefact {
            detail: format!(
                "{label} is not JSON: {error:?}. A policy file is a document, not a tensor dump"
            ),
        })?;
        let inputs = skynet_infer::read_artefact(label, &doc, "onnx")
            .map_err(|detail| OrtError::Artefact { detail })?;

        let named = doc
            .get("model")
            .and_then(Json::as_str)
            .ok_or_else(|| OrtError::Artefact {
                detail: format!(
                    "{label} declares no `model`, which is the path to the .onnx file — resolved \
                     relative to this artefact so the pair can be moved together"
                ),
            })?;
        let model_path: PathBuf = if Path::new(named).is_absolute() {
            PathBuf::from(named)
        } else {
            beside.join(named)
        };
        let bytes = std::fs::read(&model_path).map_err(|error| OrtError::Artefact {
            detail: format!(
                "{label} names model `{}` and it could not be read: {error}",
                model_path.display()
            ),
        })?;

        let library = library_name(&doc);
        let ort = Ort::open(&library)?;
        let session = Onnx::over(ort, label, &bytes, &model_path)?;
        Ok((session, inputs))
    }

    /// Open a session on model bytes, with a library already loaded.
    ///
    /// Split from [`Onnx::load`] so a caller that has the bytes from somewhere other than a file —
    /// §8's package store is the one that will — does not have to write them to disk first.
    pub fn over(
        ort: Ort,
        label: &str,
        model: &[u8],
        model_path: &Path,
    ) -> Result<Onnx, Box<dyn std::error::Error>> {
        let api = ort.api();

        let logid = CString::new("skynet-ort").expect("no NUL in a literal");
        let mut env: Handle = std::ptr::null_mut();
        let create_env = api.create_env.ok_or_else(null_member("CreateEnv"))?;
        // SAFETY: member 3, with a NUL-terminated log id that outlives the call and an out-pointer
        // to a handle this struct then owns. Every ONNX Runtime call below is the same shape: a
        // member of the served table, arguments that outlive it, and a status checked immediately.
        ort.check("CreateEnv", unsafe {
            create_env(api::LOGGING_LEVEL_WARNING, logid.as_ptr(), &mut env)
        })?;

        // From here on, anything that fails has to release `env`, so the fallible part is a closure
        // and the release is written once. A `?` between the two would leak the environment on every
        // refusal, and a policy that failed to load twice would leak twice.
        let built = Onnx::build(&ort, api, env, label, model, model_path);
        match built {
            Ok((session, memory_info, input_name, output_name, widths)) => {
                let describe = format!(
                    "onnx[{}->{}] from {} via ONNX Runtime {} (C API {})",
                    widths.0,
                    widths.1,
                    model_path.display(),
                    ort.version(),
                    API_VERSION
                );
                Ok(Onnx {
                    ort,
                    env,
                    session,
                    memory_info,
                    input_name,
                    output_name,
                    input_width: widths.0,
                    output_width: widths.1,
                    input: vec![0.0; widths.0],
                    describe,
                })
            }
            Err(error) => {
                if let Some(release) = api.release_env {
                    // SAFETY: member 92, releasing the environment created above exactly once, on
                    // the path where nothing else took ownership of it.
                    unsafe { release(env) };
                }
                Err(error)
            }
        }
    }

    #[allow(clippy::type_complexity)]
    fn build(
        ort: &Ort,
        api: &OrtApi,
        env: Handle,
        label: &str,
        model: &[u8],
        model_path: &Path,
    ) -> Result<(Handle, Handle, CString, CString, (usize, usize)), Box<dyn std::error::Error>>
    {
        // Session options, released as soon as the session exists: ONNX Runtime copies what it needs
        // out of them, and holding one for the life of a robot is a handle nobody reads.
        let mut options: Handle = std::ptr::null_mut();
        let create_options = api
            .create_session_options
            .ok_or_else(null_member("CreateSessionOptions"))?;
        // SAFETY: member 10, out-pointer to a handle released on every path below.
        ort.check("CreateSessionOptions", unsafe {
            create_options(&mut options)
        })?;

        let configured = (|| -> Result<Handle, Box<dyn std::error::Error>> {
            if let Some(set_threads) = api.set_intra_op_num_threads {
                // One thread, deliberately — see the crate documentation. SAFETY: member 24 on the
                // options handle above.
                ort.check("SetIntraOpNumThreads", unsafe { set_threads(options, 1) })?;
            }
            if let Some(set_level) = api.set_session_graph_optimization_level {
                // SAFETY: member 23 on the options handle above. Optimisation is a property of the
                // loaded graph and changes no result this seam can observe.
                ort.check("SetSessionGraphOptimizationLevel", unsafe {
                    set_level(options, api::GRAPH_OPT_ALL)
                })?;
            }
            let mut session: Handle = std::ptr::null_mut();
            let create_session = api
                .create_session_from_array
                .ok_or_else(null_member("CreateSessionFromArray"))?;
            // SAFETY: member 8. `model` outlives the call and ONNX Runtime copies what it keeps;
            // `env` and `options` are live handles from above.
            ort.check("CreateSessionFromArray", unsafe {
                create_session(
                    env,
                    model.as_ptr() as *const c_void,
                    model.len(),
                    options,
                    &mut session,
                )
            })?;
            Ok(session)
        })();

        if let Some(release) = api.release_session_options {
            // SAFETY: member 100, releasing the options exactly once whether the session was
            // created or not.
            unsafe { release(options) };
        }
        let session = configured?;

        // Everything past here can fail, and the session has to go with it.
        let inspected = Onnx::inspect(ort, api, session, label, model_path);
        match inspected {
            Ok((memory_info, input_name, output_name, widths)) => {
                Ok((session, memory_info, input_name, output_name, widths))
            }
            Err(error) => {
                if let Some(release) = api.release_session {
                    // SAFETY: member 95, releasing the session exactly once on the failure path.
                    unsafe { release(session) };
                }
                Err(error)
            }
        }
    }

    #[allow(clippy::type_complexity)]
    fn inspect(
        ort: &Ort,
        api: &OrtApi,
        session: Handle,
        label: &str,
        model_path: &Path,
    ) -> Result<(Handle, CString, CString, (usize, usize)), Box<dyn std::error::Error>> {
        let mut allocator: Handle = std::ptr::null_mut();
        let get_allocator = api
            .get_allocator_with_default_options
            .ok_or_else(null_member("GetAllocatorWithDefaultOptions"))?;
        // SAFETY: member 78. The allocator returned is ONNX Runtime's own static one; it is not
        // owned here and has no release.
        ort.check("GetAllocatorWithDefaultOptions", unsafe {
            get_allocator(&mut allocator)
        })?;

        let count = |what: &'static str,
                     f: Option<unsafe extern "system" fn(Handle, *mut usize) -> Status>|
         -> Result<usize, Box<dyn std::error::Error>> {
            let f = f.ok_or_else(null_member(what))?;
            let mut out = 0usize;
            // SAFETY: members 30 and 31, on a live session, with an out-pointer to a local.
            ort.check(what, unsafe { f(session, &mut out) })?;
            Ok(out)
        };
        let inputs = count("SessionGetInputCount", api.session_get_input_count)?;
        let outputs = count("SessionGetOutputCount", api.session_get_output_count)?;
        if inputs != 1 || outputs != 1 {
            // Refused rather than "use the first". Which of three inputs is the observation is a
            // question about a model somebody trained, and answering it by position is how a policy
            // ends up fed its own previous action.
            return Err(Box::new(OrtError::Shape {
                detail: format!(
                    "{label} names a model with {inputs} input(s) and {outputs} output(s), and this \
                     seam drives exactly one of each"
                ),
            }));
        }

        let input_width = Onnx::width(ort, api, session, true, label)?;
        let output_width = Onnx::width(ort, api, session, false, label)?;

        let input_name = Onnx::name(ort, api, session, allocator, true)?;
        let output_name = Onnx::name(ort, api, session, allocator, false)?;

        let mut memory_info: Handle = std::ptr::null_mut();
        let create_info = api
            .create_cpu_memory_info
            .ok_or_else(null_member("CreateCpuMemoryInfo"))?;
        // SAFETY: member 69, out-pointer to a handle this struct then owns and releases.
        ort.check("CreateCpuMemoryInfo", unsafe {
            create_info(
                api::DEVICE_ALLOCATOR,
                api::MEM_TYPE_DEFAULT,
                &mut memory_info,
            )
        })?;

        let _ = model_path;
        Ok((
            memory_info,
            input_name,
            output_name,
            (input_width, output_width),
        ))
    }

    /// The flat width of the model's single input or output tensor.
    ///
    /// Rank 1 is `[width]`; rank 2 is `[batch, width]` with a batch of 1 or a dynamic one — ONNX
    /// writes a dynamic dimension as a non-positive value, and a model exported with a symbolic batch
    /// is the common case rather than an odd one. Anything else is refused: a rank-4 image tensor is a
    /// model that wants a convolution over a real frame, which is a different seam.
    fn width(
        ort: &Ort,
        api: &OrtApi,
        session: Handle,
        is_input: bool,
        label: &str,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        let which = if is_input { "input" } else { "output" };
        let get_type_info = if is_input {
            api.session_get_input_type_info
                .ok_or_else(null_member("SessionGetInputTypeInfo"))?
        } else {
            api.session_get_output_type_info
                .ok_or_else(null_member("SessionGetOutputTypeInfo"))?
        };
        let mut type_info: Handle = std::ptr::null_mut();
        // SAFETY: members 33 and 34, index 0 of a session with exactly one of each (checked above).
        ort.check("SessionGet*TypeInfo", unsafe {
            get_type_info(session, 0, &mut type_info)
        })?;

        // The type info is owned here and released on every path out, so the reading is a closure.
        let read = (|| -> Result<usize, Box<dyn std::error::Error>> {
            let cast = api
                .cast_type_info_to_tensor_info
                .ok_or_else(null_member("CastTypeInfoToTensorInfo"))?;
            let mut tensor_info: Handle = std::ptr::null_mut();
            // SAFETY: member 55, on the type info above.
            ort.check("CastTypeInfoToTensorInfo", unsafe {
                cast(type_info, &mut tensor_info)
            })?;
            if tensor_info.is_null() {
                // Documented `_Outptr_result_maybenull_`: a non-tensor casts to null with a null
                // status, so this is a success that has to be checked rather than an error.
                return Err(Box::new(OrtError::Shape {
                    detail: format!("{label}'s {which} is not a tensor"),
                }));
            }

            let element_type = api
                .get_tensor_element_type
                .ok_or_else(null_member("GetTensorElementType"))?;
            let mut kind = 0;
            // SAFETY: member 60, on the tensor info above.
            ort.check("GetTensorElementType", unsafe {
                element_type(tensor_info, &mut kind)
            })?;
            if kind != api::TENSOR_ELEMENT_FLOAT {
                return Err(Box::new(OrtError::Shape {
                    detail: format!(
                        "{label}'s {which} tensor is ONNX element type {kind} and this seam carries \
                         float32 (type {}). Reading four bytes of a narrower element is not an \
                         error, it is a different number — so export the model as float32 rather \
                         than having this reinterpret it",
                        api::TENSOR_ELEMENT_FLOAT
                    ),
                }));
            }

            let dims_count = api
                .get_dimensions_count
                .ok_or_else(null_member("GetDimensionsCount"))?;
            let mut rank = 0usize;
            // SAFETY: member 61, on the tensor info above.
            ort.check("GetDimensionsCount", unsafe {
                dims_count(tensor_info, &mut rank)
            })?;
            if rank == 0 || rank > 2 {
                return Err(Box::new(OrtError::Shape {
                    detail: format!(
                        "{label}'s {which} tensor has rank {rank}, and this seam drives `[width]` or \
                         `[batch, width]`"
                    ),
                }));
            }
            let mut dims = vec![0i64; rank];
            let get_dims = api
                .get_dimensions
                .ok_or_else(null_member("GetDimensions"))?;
            // SAFETY: member 62, writing exactly `rank` values into a buffer of that length.
            ort.check("GetDimensions", unsafe {
                get_dims(tensor_info, dims.as_mut_ptr(), rank)
            })?;

            if rank == 2 && dims[0] > 1 {
                return Err(Box::new(OrtError::Shape {
                    detail: format!(
                        "{label}'s {which} tensor has batch dimension {}, and a control cycle runs \
                         one observation",
                        dims[0]
                    ),
                }));
            }
            let width = dims[rank - 1];
            if width <= 0 {
                // A dynamic *width* is refused where a dynamic batch is accepted, and the asymmetry
                // is the point: the batch is this crate's to fix at 1, and the width is the thing
                // `skynet_infer::Policy` checks the body against. A model that will accept any width
                // cannot be checked against anything.
                return Err(Box::new(OrtError::Shape {
                    detail: format!(
                        "{label}'s {which} tensor has a dynamic last dimension ({width}). The \
                         width is what a policy's channel count is checked against, so a model \
                         that accepts any width can be checked against no body"
                    ),
                }));
            }
            Ok(width as usize)
        })();

        if let Some(release) = api.release_type_info {
            // SAFETY: member 98, releasing the type info exactly once. The tensor info reached
            // through it is owned by it and must not be released separately.
            unsafe { release(type_info) };
        }
        read
    }

    /// The model's name for its single input or output, copied out of ONNX Runtime's allocator.
    fn name(
        ort: &Ort,
        api: &OrtApi,
        session: Handle,
        allocator: Handle,
        is_input: bool,
    ) -> Result<CString, Box<dyn std::error::Error>> {
        let get_name = if is_input {
            api.session_get_input_name
                .ok_or_else(null_member("SessionGetInputName"))?
        } else {
            api.session_get_output_name
                .ok_or_else(null_member("SessionGetOutputName"))?
        };
        let mut raw: *mut c_char = std::ptr::null_mut();
        // SAFETY: members 36 and 37, index 0, with ONNX Runtime's own allocator. On success `raw`
        // is an allocation this function owns and frees below.
        ort.check("SessionGet*Name", unsafe {
            get_name(session, 0, allocator, &mut raw)
        })?;
        if raw.is_null() {
            return Err(Box::new(OrtError::Runtime {
                call: "SessionGet*Name",
                detail: "returned a null name with a null status".to_string(),
            }));
        }
        // SAFETY: `raw` is a non-null NUL-terminated string from the allocator. Copied into an owned
        // `CString` so the allocation can be handed straight back rather than held for the life of
        // the robot.
        let owned = unsafe { CStr::from_ptr(raw) }.to_owned();
        if let Some(free) = api.allocator_free {
            // SAFETY: member 76, freeing the allocation just copied out of, exactly once, with the
            // allocator that produced it.
            unsafe { free(allocator, raw as *mut c_void) };
        }
        Ok(owned)
    }
}

impl Session for Onnx {
    fn input_width(&self) -> usize {
        self.input_width
    }

    fn output_width(&self) -> usize {
        self.output_width
    }

    fn run(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), SessionError> {
        let expected = (self.input_width, self.output_width);
        if input.len() != expected.0 || output.len() != expected.1 {
            return Err(SessionError::Shape {
                expected,
                given: (input.len(), output.len()),
            });
        }
        self.input.copy_from_slice(input);
        self.forward(output)
            .map_err(|error| SessionError::Refused(error.to_string()))
    }

    fn describe(&self) -> &str {
        &self.describe
    }
}

impl Onnx {
    /// One forward pass. Split from [`Session::run`] so the error type stays this crate's until the
    /// boundary, where it becomes [`SessionError::Refused`] with the message intact.
    fn forward(&mut self, output: &mut [f32]) -> Result<(), OrtError> {
        let api = self.ort.api();
        let shape = [1i64, self.input_width as i64];

        let create_tensor = api
            .create_tensor_with_data_as_ort_value
            .ok_or(OrtError::Runtime {
                call: "CreateTensorWithDataAsOrtValue",
                detail: "null in the call table".to_string(),
            })?;
        let mut input_value: Handle = std::ptr::null_mut();
        // SAFETY: member 49. The tensor is created **over** `self.input`, borrowing it rather than
        // copying: the buffer is a field of this struct, is not resized anywhere, and the tensor is
        // released at the end of this function, so it cannot outlive what it points at. `shape` is a
        // local that outlives the call.
        self.ort.check("CreateTensorWithDataAsOrtValue", unsafe {
            create_tensor(
                self.memory_info,
                self.input.as_mut_ptr() as *mut c_void,
                std::mem::size_of_val(self.input.as_slice()),
                shape.as_ptr(),
                shape.len(),
                api::TENSOR_ELEMENT_FLOAT,
                &mut input_value,
            )
        })?;

        let run = api.run.ok_or(OrtError::Runtime {
            call: "Run",
            detail: "null in the call table".to_string(),
        })?;
        let input_names = [self.input_name.as_ptr()];
        let output_names = [self.output_name.as_ptr()];
        let inputs = [input_value];
        let mut outputs: [Handle; 1] = [std::ptr::null_mut()];

        // SAFETY: member 9. Every array outlives the call; `run_options` is documented `_In_opt_`
        // and null means "the defaults". On success `outputs[0]` is an owned value released below.
        let status = unsafe {
            run(
                self.session,
                std::ptr::null_mut(),
                input_names.as_ptr(),
                inputs.as_ptr(),
                1,
                output_names.as_ptr(),
                1,
                outputs.as_mut_ptr(),
            )
        };
        let ran = self.ort.check("Run", status);

        // The input tensor has done its job either way, and is released before the result is
        // inspected so no early return can leak it.
        if let Some(release) = api.release_value {
            // SAFETY: member 96, releasing the input tensor exactly once. `self.input` is untouched
            // by this — the tensor borrowed the buffer, it did not own it.
            unsafe { release(input_value) };
        }
        ran?;

        let copied = (|| -> Result<(), OrtError> {
            if outputs[0].is_null() {
                return Err(OrtError::Runtime {
                    call: "Run",
                    detail: "produced a null output with a null status".to_string(),
                });
            }
            let get_data = api.get_tensor_mutable_data.ok_or(OrtError::Runtime {
                call: "GetTensorMutableData",
                detail: "null in the call table".to_string(),
            })?;
            let mut data: *mut c_void = std::ptr::null_mut();
            // SAFETY: member 51, on the value `Run` produced.
            self.ort
                .check("GetTensorMutableData", unsafe {
                    get_data(outputs[0], &mut data)
                })
                .and_then(|()| {
                    if data.is_null() {
                        return Err(OrtError::Runtime {
                            call: "GetTensorMutableData",
                            detail: "returned null for a tensor Run produced".to_string(),
                        });
                    }
                    // SAFETY: `data` points at `output_width` contiguous `f32` — the width was read
                    // off this model's own output tensor at load and the graph is fixed, so the
                    // tensor `Run` just produced has that many elements. Copied out immediately;
                    // nothing retains the pointer past the release below.
                    let produced = unsafe {
                        std::slice::from_raw_parts(data as *const f32, self.output_width)
                    };
                    output.copy_from_slice(produced);
                    Ok(())
                })
        })();

        if let Some(release) = api.release_value {
            // SAFETY: member 96, releasing the output value exactly once, after the copy above.
            unsafe { release(outputs[0]) };
        }
        copied
    }
}

impl Drop for Onnx {
    /// Released in the reverse of the order they were made, and **before** `self.ort` is dropped.
    ///
    /// Written by hand for that second reason. Rust drops fields in declaration order, and `ort` is
    /// declared first — so leaving this to the derive would unload the library and then ask it to
    /// release three handles, which is a call through a freed page in a robot's policy process on the
    /// way out. The allocator is absent on purpose: it is ONNX Runtime's own static one and has no
    /// release.
    fn drop(&mut self) {
        let api = self.ort.api();
        if let Some(release) = api.release_memory_info {
            // SAFETY: member 94, once, on the handle this struct owns.
            unsafe { release(self.memory_info) };
        }
        if let Some(release) = api.release_session {
            // SAFETY: member 95, once. No value or tensor outlives a `run` call, so nothing is
            // borrowing the session here.
            unsafe { release(self.session) };
        }
        if let Some(release) = api.release_env {
            // SAFETY: member 92, once, and last of the three because the session was created in it.
            unsafe { release(self.env) };
        }
    }
}

fn null_member(what: &'static str) -> impl Fn() -> Box<dyn std::error::Error> {
    move || {
        Box::new(OrtError::Runtime {
            call: what,
            detail: "null in the call table, which a served API version never leaves unset"
                .to_string(),
        }) as Box<dyn std::error::Error>
    }
}

/// The name of the environment variable that overrides where ONNX Runtime is loaded from.
pub const LIBRARY_ENV: &str = "SKYNET_ORT_LIBRARY";

/// Where to load ONNX Runtime from: the environment, then the artefact, then the platform default.
fn library_name(doc: &Json) -> String {
    resolve_library(std::env::var(LIBRARY_ENV).ok().as_deref(), doc)
}

/// The precedence, as a function of its inputs rather than of the process.
///
/// Split from [`library_name`] so the three cases are testable without `set_var`. That is not
/// fastidiousness: the suite runs in threads, `set_var` is process-wide, and a test that mutated the
/// environment while another read it would fail on one machine in ten and pass on review.
///
/// The environment wins because it is the one an operator can change without editing the artefact,
/// and §8's packages are signed. An empty value is *not* a choice — an exported-but-blank variable is
/// a shell accident, and treating it as "no library" would shadow a perfectly good artefact field.
fn resolve_library(from_env: Option<&str>, doc: &Json) -> String {
    if let Some(named) = from_env.filter(|value| !value.is_empty()) {
        return named.to_string();
    }
    if let Some(named) = doc
        .get("library")
        .and_then(Json::as_str)
        .filter(|value| !value.is_empty())
    {
        return named.to_string();
    }
    default_library().to_string()
}

/// The platform's own name for the library, resolved by the system loader rather than by a search
/// this crate invents.
pub fn default_library() -> &'static str {
    if cfg!(windows) {
        "onnxruntime.dll"
    } else if cfg!(target_vendor = "apple") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    }
}

#[cfg(test)]
mod tests;
