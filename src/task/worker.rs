use super::{c_string, completion, loaded_bytes, receive};
use emscripten_rs_sys as ffi;
use std::{
    ffi::{c_char, c_void},
    io,
};

/// Calls a function on an existing legacy Emscripten worker and awaits its reply.
/// Request bytes are copied by Emscripten; response bytes are copied by the callback.
///
/// # Safety
/// `worker` must remain valid until its response arrives, even if the future is
/// dropped. The worker function must reply exactly once with
/// `emscripten_worker_respond`, never `emscripten_worker_respond_provisionally`.
pub async unsafe fn call_worker(
    worker: ffi::worker_handle,
    function: &str,
    data: &[u8],
) -> io::Result<Vec<u8>> {
    unsafe extern "C" fn reply(data: *mut c_char, len: i32, arg: *mut c_void) {
        unsafe { loaded_bytes(arg, data.cast(), len) };
    }
    let function = c_string(function)?;
    let len = i32::try_from(data.len())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    let (arg, receiver) = completion::<Vec<u8>>();
    unsafe {
        ffi::emscripten_call_worker(
            worker,
            function.as_ptr(),
            data.as_ptr().cast_mut().cast(),
            len,
            Some(reply),
            arg,
        )
    };
    receive(receiver).await
}
