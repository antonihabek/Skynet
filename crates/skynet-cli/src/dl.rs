//! Loading a driver shared object.
//!
//! The one place in this workspace with `unsafe` in it, and it is unavoidable: the thing being
//! loaded is arbitrary native code compiled by somebody else, and no amount of Rust can make
//! calling into it a safe operation. What can be done is to keep the unsafety in one small module
//! whose whole surface is "open a library, look up one symbol, close it", and to say at each block
//! what is being assumed.
//!
//! The platform declarations are written out rather than taken from `libc` or `libloading` for the
//! workspace's stated reason: no third-party dependencies. Four functions on each platform, and
//! their signatures have been stable for decades.
//!
//! # What this deliberately does not do
//!
//! No `RTLD_GLOBAL`, no lazy binding, no unloading while a context is open. A driver is opened with
//! `RTLD_NOW` so a missing symbol is a load failure rather than a crash halfway through a
//! conformance run, which is exactly the difference between a diagnosable failure and a puzzling
//! one.

use std::ffi::{c_char, c_void, CString};
use std::path::Path;

/// An open shared object.
#[derive(Debug)]
pub struct Library {
    handle: *mut c_void,
}

/// Why a driver could not be loaded. Both variants name the fix, per §64.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DlError {
    /// The file could not be opened as a shared object.
    Open {
        /// What was tried.
        path: String,
        /// What the loader said.
        detail: String,
    },
    /// The library loaded and does not export the entry point.
    MissingSymbol {
        /// The symbol looked for.
        symbol: String,
    },
}

impl std::fmt::Display for DlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DlError::Open { path, detail } => write!(
                f,
                "{path} could not be loaded as a shared object: {detail}\n\
                 A driver is built as a shared library — `-shared -fPIC` for C, \
                 `crate-type = [\"cdylib\"]` for Rust — and every symbol it needs must resolve at \
                 load time."
            ),
            DlError::MissingSymbol { symbol } => write!(
                f,
                "the library loaded and exports no `{symbol}`.\n\
                 A driver exports exactly one symbol, and it must have external linkage: check for \
                 a missing `extern \"C\"`, a `static` on the entry function, or a version script \
                 that is hiding it."
            ),
        }
    }
}

impl std::error::Error for DlError {}

impl Library {
    /// Open a shared object by path.
    pub fn open(path: &Path) -> Result<Library, DlError> {
        let display = path.display().to_string();
        let handle = platform::open(path).map_err(|detail| DlError::Open {
            path: display,
            detail,
        })?;
        Ok(Library { handle })
    }

    /// Look up a symbol and transmute it to the RHAL entry signature.
    ///
    /// # Safety
    ///
    /// The caller asserts that a symbol of this name in this library really is
    /// `const skr_driver *(*)(void)`. That assertion cannot be checked — C has no runtime type
    /// information — and it is the reason this function is `unsafe` rather than the lookup itself.
    /// It holds when the library was built against `bindings/c/skynet_rhal.h`, and the ABI version
    /// check that immediately follows is what catches the case where it was built against a
    /// different version of that header.
    pub unsafe fn entry(&self) -> Result<skynet_rhal::EntryFn, DlError> {
        let name = CString::new(skynet_rhal::ENTRY_SYMBOL).expect("symbol name has no NUL");
        let address = platform::symbol(self.handle, name.as_ptr());
        if address.is_null() {
            return Err(DlError::MissingSymbol {
                symbol: skynet_rhal::ENTRY_SYMBOL.to_string(),
            });
        }
        // SAFETY: a non-null symbol address from the platform loader, and the caller's assertion
        // above that it has this signature. Function pointers and data pointers are the same width
        // on every platform this targets.
        Ok(unsafe { std::mem::transmute::<*mut c_void, skynet_rhal::EntryFn>(address) })
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: `handle` came from the platform loader and is closed exactly once, here, because
        // `Library` is not `Clone` and owns it. Any driver context must already be closed — the
        // conformance harness closes before it drops, and unloading under a live context is what
        // turns a clean failure into a crash in someone else's atexit handler.
        unsafe { platform::close(self.handle) };
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::ffi::{c_int, OsStr};
    use std::os::unix::ffi::OsStrExt;

    const RTLD_NOW: c_int = 2;

    extern "C" {
        fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn dlclose(handle: *mut c_void) -> c_int;
        fn dlerror() -> *const c_char;
    }

    pub fn open(path: &Path) -> Result<*mut c_void, String> {
        let bytes = OsStr::new(path).as_bytes();
        let c_path = CString::new(bytes).map_err(|_| "the path contains a NUL byte".to_string())?;
        // SAFETY: `c_path` is NUL-terminated and outlives the call. `dlerror` is read immediately
        // afterwards on the same thread, which is where its value is defined to be valid.
        unsafe {
            dlerror(); // clear any stale error
            let handle = dlopen(c_path.as_ptr(), RTLD_NOW);
            if handle.is_null() {
                let message = dlerror();
                let detail = if message.is_null() {
                    "dlopen returned null and dlerror said nothing".to_string()
                } else {
                    std::ffi::CStr::from_ptr(message)
                        .to_string_lossy()
                        .into_owned()
                };
                return Err(detail);
            }
            Ok(handle)
        }
    }

    pub fn symbol(handle: *mut c_void, name: *const c_char) -> *mut c_void {
        // SAFETY: `handle` is a live handle from `open`, `name` is a NUL-terminated string owned by
        // the caller for the duration of the call.
        unsafe { dlsym(handle, name) }
    }

    /// # Safety
    /// `handle` must be a live handle from [`open`], closed exactly once.
    pub unsafe fn close(handle: *mut c_void) {
        unsafe { dlclose(handle) };
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
        fn FreeLibrary(module: *mut c_void) -> i32;
        fn GetLastError() -> u32;
    }

    pub fn open(path: &Path) -> Result<*mut c_void, String> {
        // An absolute path, because `LoadLibraryW` searches a path list for a relative one and
        // "it loaded a different driver with the same name" is a debugging session nobody enjoys.
        let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let mut wide: Vec<u16> = absolute.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err("the path contains a NUL character".to_string());
        }
        wide.push(0);
        // SAFETY: `wide` is NUL-terminated and outlives the call.
        unsafe {
            let handle = LoadLibraryW(wide.as_ptr());
            if handle.is_null() {
                return Err(format!("LoadLibraryW failed with error {}", GetLastError()));
            }
            Ok(handle)
        }
    }

    pub fn symbol(handle: *mut c_void, name: *const c_char) -> *mut c_void {
        // SAFETY: `handle` is live, `name` is NUL-terminated and owned by the caller.
        unsafe { GetProcAddress(handle, name) }
    }

    /// # Safety
    /// `handle` must be a live handle from [`open`], closed exactly once.
    pub unsafe fn close(handle: *mut c_void) {
        unsafe { FreeLibrary(handle) };
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use super::*;

    pub fn open(_path: &Path) -> Result<*mut c_void, String> {
        Err("this platform has no shared-object loader in skynet-cli. \
             `skynet driver check` runs on Linux, macOS and Windows."
            .to_string())
    }

    pub fn symbol(_handle: *mut c_void, _name: *const c_char) -> *mut c_void {
        std::ptr::null_mut()
    }

    /// # Safety
    /// Never called: [`open`] always fails on this platform.
    pub unsafe fn close(_handle: *mut c_void) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_that_is_not_a_library_fails_with_the_loader_message_and_the_fix() {
        let path = std::env::temp_dir().join("skynet-not-a-library.txt");
        std::fs::write(&path, b"this is not a shared object").unwrap();
        let err = Library::open(&path).unwrap_err();
        let rendered = err.to_string();
        assert!(rendered.contains("could not be loaded"), "{rendered}");
        assert!(rendered.contains("-shared -fPIC"), "{rendered}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_missing_file_says_so_rather_than_panicking() {
        let err = Library::open(Path::new("./definitely-not-here.so")).unwrap_err();
        assert!(matches!(err, DlError::Open { .. }));
    }
}
