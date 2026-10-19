//! Fast environment lookups for Oxi's feature flags.
//!
//! Oxi reads about 1,600 `OXI_*` environment variables (opt-out switches for its layout
//! heuristics) from inside its layout loops, and each `std::env::var` call scans the whole
//! process environment. That was about 70% of layout time. Binaries that use Oxi define
//! their own `getenv` with [`crate::fast_oxi_env!`]: names starting with `OXI_` are answered
//! from a snapshot taken on first use (so setting a flag before launch still works), and
//! every other name goes to the system's `getenv`.

use std::ffi::{c_char, CStr, CString};
use std::sync::OnceLock;

type GetEnv = unsafe extern "C" fn(*const c_char) -> *mut c_char;

fn real() -> GetEnv {
    static REAL: OnceLock<usize> = OnceLock::new();
    let p = *REAL.get_or_init(|| unsafe { libc::dlsym(libc::RTLD_NEXT, c"getenv".as_ptr()) as usize });
    assert!(p != 0, "getenv not found");
    unsafe { std::mem::transmute::<usize, GetEnv>(p) }
}

fn oxi_flags() -> &'static [(CString, CString)] {
    static FLAGS: OnceLock<Vec<(CString, CString)>> = OnceLock::new();
    FLAGS.get_or_init(|| {
        // vars_os walks `environ` directly (it doesn't call getenv), so no recursion here.
        std::env::vars_os()
            .filter_map(|(k, v)| {
                use std::os::unix::ffi::OsStrExt;
                let k = k.as_bytes();
                k.starts_with(b"OXI_").then(|| (CString::new(k).ok(), CString::new(v.as_bytes()).ok())).and_then(|(a, b)| Some((a?, b?)))
            })
            .collect()
    })
}

/// The `getenv` the binaries export (see the module docs).
///
/// # Safety
/// `name` must be a valid NUL-terminated C string, as for libc `getenv`.
pub unsafe fn getenv(name: *const c_char) -> *mut c_char {
    if name.is_null() {
        return std::ptr::null_mut();
    }
    let n = unsafe { CStr::from_ptr(name) }.to_bytes();
    if n.starts_with(b"OXI_") {
        return oxi_flags().iter().find(|(k, _)| k.as_bytes() == n).map(|(_, v)| v.as_ptr() as *mut c_char).unwrap_or(std::ptr::null_mut());
    }
    unsafe { real()(name) }
}

/// Define the fast `getenv` in a binary that uses Oxi (call once, at the crate root).
#[macro_export]
macro_rules! fast_oxi_env {
    () => {
        #[cfg(unix)]
        #[no_mangle]
        pub unsafe extern "C" fn getenv(name: *const ::std::ffi::c_char) -> *mut ::std::ffi::c_char {
            unsafe { $crate::envcache::getenv(name) }
        }
    };
}
