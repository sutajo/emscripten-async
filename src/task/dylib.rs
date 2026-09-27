use super::{c_string, complete, completion, failed, receive};
use emscripten_rs_sys as ffi;
use std::{ffi::c_void, io};

unsafe extern "C" {
    fn dlclose(handle: *mut c_void) -> i32;
}

/// An owned dynamic-library handle. Dropping it calls `dlclose`.
pub struct Library(std::ptr::NonNull<c_void>);

impl Library {
    /// Borrows the native handle, for example for use with `dlsym`.
    pub fn as_ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        unsafe { dlclose(self.0.as_ptr()) };
    }
}

/// Asynchronously loads a dynamic library through `emscripten_dlopen`.
/// Requires an application linked with Emscripten dynamic-linking support.
///
/// # Safety
/// The library's initialization and finalization must preserve Rust memory safety.
/// No code or data from the library may be used after the returned handle is dropped.
pub async unsafe fn dlopen(filename: &str, flags: i32) -> io::Result<Library> {
    unsafe extern "C" fn loaded(arg: *mut c_void, handle: *mut c_void) {
        let result = std::ptr::NonNull::new(handle)
            .map(Library)
            .ok_or_else(|| io::Error::other("dynamic library returned a null handle"));
        unsafe { complete(arg, result) };
    }
    let filename = c_string(filename)?;
    let (arg, receiver) = completion::<Library>();
    unsafe {
        ffi::emscripten_dlopen(
            filename.as_ptr(),
            flags,
            arg,
            Some(loaded),
            Some(failed::<Library>),
        )
    };
    receive(receiver).await
}
