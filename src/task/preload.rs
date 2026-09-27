use super::legacy::{LegacyOperation, legacy};
use super::{Completion, c_string, local_queue, receive};
use emscripten_functions_sys::emscripten as ffi;
use std::{
    ffi::{CStr, c_char, c_void},
    io,
};

unsafe extern "C" {
    fn free(ptr: *mut c_void);
}

/// Runs preload plugins on a file already in the virtual filesystem.
pub async fn preload(file: &str) -> io::Result<()> {
    // Avoid the native API's missing-file early return, which leaves a runtime
    // keepalive outstanding in the current SDK.
    std::fs::metadata(file)?;
    legacy(LegacyOperation::Preload(c_string(file)?)).await
}

struct PreloadData {
    sender: Completion<String>,
    data: Vec<u8>,
}

/// Prepares in-memory image/audio bytes with Emscripten preload plugins.
/// Returns the generated asset name; `suffix` is an extension such as "png".
/// Enable the relevant Emscripten preload plugins when linking the application.
pub async fn preload_data(data: &[u8], suffix: &str) -> io::Result<String> {
    unsafe extern "C" fn loaded(arg: *mut c_void, name: *const c_char) {
        let state = unsafe { Box::from_raw(arg.cast::<PreloadData>()) };
        let name_string = unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned();
        // This API transfers ownership of its malloc-allocated name to the caller.
        unsafe { free(name.cast_mut().cast()) };
        let _ = state.sender.send(Ok(name_string));
    }
    unsafe extern "C" fn error(arg: *mut c_void) {
        let state = unsafe { Box::from_raw(arg.cast::<PreloadData>()) };
        let _ = state.sender.send(Err(io::Error::other("preload failed")));
    }
    let suffix = c_string(suffix)?;
    let len = i32::try_from(data.len())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    let (sender, receiver) = local_queue::bounded(1);
    let mut state = Box::new(PreloadData {
        sender,
        data: data.to_vec(),
    });
    let data_ptr = state.data.as_mut_ptr().cast();
    let arg = Box::into_raw(state).cast();
    unsafe {
        ffi::emscripten_run_preload_plugins_data(
            data_ptr,
            len,
            suffix.as_ptr(),
            arg,
            Some(loaded),
            Some(error),
        )
    };
    receive(receiver).await
}
