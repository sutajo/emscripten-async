use super::legacy::{LegacyOperation, legacy};
use super::{c_string, completion, copy_bytes, failed, loaded_bytes, receive};
use crate::channel::mpsc;
use emscripten_functions_sys::emscripten as ffi;
use std::{
    ffi::{CStr, CString, c_char, c_void},
    io,
};

/// Downloads to owned bytes or the Emscripten virtual filesystem.
///
/// [`Self::data`] and [`Self::file`] support GET and POST through browser XHR.
/// Dropping their pending futures aborts the requests.
///
/// ```no_run
/// use emscripten_futures::task::Wget;
///
/// # async fn example() -> std::io::Result<()> {
/// let bytes = Wget::data("/asset.bin", "GET", "").await?;
/// Wget::file("/asset.bin", "/asset.bin", "GET", "").await?;
/// # Ok(())
/// # }
/// ```
pub struct Wget;

/// Download progress reported by [`Wget::data_with_progress`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    /// Number of bytes received.
    pub loaded: usize,
    /// Total bytes, or `None` when Emscripten reports an unknown or zero total.
    pub total: Option<usize>,
}

enum DownloadEvent<T, P> {
    Complete(io::Result<T>),
    Progress(P),
}

// wget2's abort callback does not call onerror. Keep ownership in a guard so
// cancellation can abort the request and free its sender without a leak.
struct DownloadGuard<'a, T, P> {
    handle: i32,
    sender: Box<mpsc::Sender<DownloadEvent<T, P>>>,
    progress_cb: Box<dyn FnMut(P) + 'a>,
}

impl<T, P> Drop for DownloadGuard<'_, T, P> {
    fn drop(&mut self) {
        // Always synchronize with the native request before freeing user data.
        unsafe { ffi::emscripten_async_wget2_abort(self.handle) };
    }
}

unsafe fn download_event<T, P>(arg: *mut c_void, event: DownloadEvent<T, P>) {
    // The request guard owns the sender passed as native user data.
    let sender = unsafe { &*arg.cast::<mpsc::Sender<DownloadEvent<T, P>>>() };
    let _ = sender.send(event);
}

unsafe extern "C" fn download_bytes(_: u32, arg: *mut c_void, data: *mut c_void, len: u32) {
    let result = unsafe { copy_bytes(data, len as usize) };
    unsafe { download_event::<_, Progress>(arg, DownloadEvent::Complete(result)) };
}

unsafe extern "C" fn download_file(_: u32, arg: *mut c_void, _: *const c_char) {
    unsafe { download_event::<_, i32>(arg, DownloadEvent::Complete(Ok(()))) };
}

unsafe extern "C" fn download_error(_: u32, arg: *mut c_void, status: i32) {
    unsafe {
        download_event::<(), i32>(
            arg,
            DownloadEvent::Complete(Err(io::Error::other(format!(
                "HTTP request failed: {status}"
            )))),
        )
    };
}

unsafe extern "C" fn download_data_error(
    _: u32,
    arg: *mut c_void,
    status: i32,
    message: *const c_char,
) {
    let message = if message.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    };
    unsafe {
        download_event::<Vec<u8>, Progress>(
            arg,
            DownloadEvent::Complete(Err(io::Error::other(format!(
                "HTTP request failed: {status} {message}"
            )))),
        )
    };
}

unsafe extern "C" fn download_data_progress(_: u32, arg: *mut c_void, loaded: i32, total: i32) {
    unsafe {
        download_event::<Vec<u8>, _>(
            arg,
            DownloadEvent::Progress(Progress {
                loaded: loaded as u32 as usize,
                total: (total != 0).then_some(total as u32 as usize),
            }),
        )
    };
}

unsafe extern "C" fn download_file_progress(_: u32, arg: *mut c_void, percent: i32) {
    unsafe { download_event::<(), _>(arg, DownloadEvent::Progress(percent)) };
}

fn download_state<'a, T, P>(
    on_progress: impl FnMut(P) + 'a,
) -> (DownloadGuard<'a, T, P>, mpsc::Receiver<DownloadEvent<T, P>>) {
    let (sender, receiver) = mpsc::channel();
    (
        DownloadGuard {
            handle: -1,
            sender: Box::new(sender),
            progress_cb: Box::new(on_progress),
        },
        receiver,
    )
}

async fn receive_download<T, P>(
    mut receiver: mpsc::Receiver<DownloadEvent<T, P>>,
    on_progress: &mut dyn FnMut(P),
) -> io::Result<T> {
    // User callbacks run while polling the future, never through native user
    // data. They can safely borrow local state even if the future is forgotten.
    while let Some(event) = receiver.recv().await {
        match event {
            DownloadEvent::Complete(result) => return result,
            DownloadEvent::Progress(progress) => on_progress(progress),
        }
    }
    Err(io::Error::other("operation canceled"))
}

impl Wget {
    /// Downloads bytes using `emscripten_async_wget2_data` and XMLHttpRequest.
    /// `method` is GET or POST; POST `params` are form-encoded text.
    /// Dropping a pending future aborts the request. Requires browser XHR.
    pub async fn data(url: &str, method: &str, params: &str) -> io::Result<Vec<u8>> {
        Self::data_with_progress(url, method, params, |_| {}).await
    }

    /// Downloads bytes with progress notifications. Supports GET and POST.
    ///
    /// The callback runs when the future is polled and may borrow local state.
    /// Progress frequency depends on the browser. Dropping the future aborts
    /// the request and prevents further callbacks. Requires browser XHR.
    pub async fn data_with_progress(
        url: &str,
        method: &str,
        params: &str,
        on_progress: impl FnMut(Progress),
    ) -> io::Result<Vec<u8>> {
        let url = c_string(url)?;
        let method = request_method(method)?;
        let params = c_string(params)?;
        let (mut request, receiver) = download_state::<Vec<u8>, Progress>(on_progress);
        let arg = (&*request.sender as *const mpsc::Sender<_>)
            .cast_mut()
            .cast();
        request.handle = unsafe {
            ffi::emscripten_async_wget2_data(
                url.as_ptr(),
                method.as_ptr(),
                params.as_ptr(),
                arg,
                1,
                Some(download_bytes),
                Some(download_data_error),
                Some(download_data_progress),
            )
        };
        let result = receive_download(receiver, &mut *request.progress_cb).await;
        drop(request);
        result
    }

    /// Downloads into the virtual filesystem using `emscripten_async_wget2`.
    /// Does not run preload plugins. Supports GET and POST.
    /// Dropping a pending future aborts the request. Requires browser XHR.
    pub async fn file(url: &str, file: &str, method: &str, params: &str) -> io::Result<()> {
        Self::file_with_progress(url, file, method, params, |_| {}).await
    }

    /// Downloads into the virtual filesystem and reports integer percentages.
    /// Emscripten only reports file progress when the total size is known.
    ///
    /// The callback runs when the future is polled and may borrow local state.
    /// Dropping the future aborts the request and prevents further callbacks.
    /// Supports GET and POST; does not run preload plugins. Requires browser XHR.
    pub async fn file_with_progress(
        url: &str,
        file: &str,
        method: &str,
        params: &str,
        on_progress: impl FnMut(i32),
    ) -> io::Result<()> {
        let url = c_string(url)?;
        let file = c_string(file)?;
        let method = request_method(method)?;
        let params = c_string(params)?;
        let (mut request, receiver) = download_state::<(), i32>(on_progress);
        let arg = (&*request.sender as *const mpsc::Sender<_>)
            .cast_mut()
            .cast();
        request.handle = unsafe {
            ffi::emscripten_async_wget2(
                url.as_ptr(),
                file.as_ptr(),
                method.as_ptr(),
                params.as_ptr(),
                arg,
                Some(download_file),
                Some(download_error),
                Some(download_file_progress),
            )
        };
        let result = receive_download(receiver, &mut *request.progress_cb).await;
        drop(request);
        result
    }

    /// Downloads owned bytes using the legacy `emscripten_async_wget_data` API.
    ///
    /// Dropping the future leaves the request running; its callback cleans up.
    /// Returns an error if the URL contains a NUL byte or the download fails.
    #[deprecated(note = "uses legacy wget-data; use Wget::data for cancellable browser requests")]
    pub async fn legacy_data(url: &str) -> io::Result<Vec<u8>> {
        let url = c_string(url)?;
        let (arg, receiver) = completion::<Vec<u8>>();

        // SAFETY: Emscripten copies the URL during this call. The callbacks own the
        // sender independently of the future, including when the future is dropped.
        unsafe {
            ffi::emscripten_async_wget_data(
                url.as_ptr(),
                arg,
                Some(loaded_bytes),
                Some(failed::<Vec<u8>>),
            )
        };
        receive(receiver).await
    }

    /// Downloads a URL into the virtual filesystem and runs preload plugins.
    /// Uses `emscripten_async_wget`. Legacy file/script operations are serialized.
    /// Dropping the future leaves the operation running.
    #[deprecated(
        note = "uses serialized legacy wget; use Wget::file if preload plugins are not needed"
    )]
    pub async fn legacy_file(url: &str, file: &str) -> io::Result<()> {
        legacy(LegacyOperation::Download(c_string(url)?, c_string(file)?)).await
    }
}

fn request_method(method: &str) -> io::Result<CString> {
    match method {
        "GET" | "POST" => c_string(method),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected GET or POST",
        )),
    }
}
