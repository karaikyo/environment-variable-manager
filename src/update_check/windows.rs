//! Windows HTTPS transport. A request owns its callback storage until HANDLE_CLOSING.
use super::{valid_api_url, CheckControl, HttpResponse, ReleaseTransport, MAX_RESPONSE_BYTES};
use std::{
    cell::UnsafeCell,
    ffi::{c_void, CStr},
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, Receiver, RecvTimeoutError, Sender},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{Foundation::*, Networking::WinHttp::*, System::LibraryLoader::*};

type Internet = *mut c_void;
type Open = unsafe extern "system" fn(*const u16, u32, *const u16, *const u16, u32) -> Internet;
type Connect = unsafe extern "system" fn(Internet, *const u16, u16, u32) -> Internet;
type OpenRequest = unsafe extern "system" fn(
    Internet,
    *const u16,
    *const u16,
    *const u16,
    *const u16,
    *const *const u16,
    u32,
) -> Internet;
type SetOption = unsafe extern "system" fn(Internet, u32, *const c_void, u32) -> i32;
type SetTimeouts = unsafe extern "system" fn(Internet, i32, i32, i32, i32) -> i32;
type SetCallback = unsafe extern "system" fn(
    Internet,
    WINHTTP_STATUS_CALLBACK,
    u32,
    usize,
) -> WINHTTP_STATUS_CALLBACK;
type SendRequest =
    unsafe extern "system" fn(Internet, *const u16, u32, *const c_void, u32, u32, usize) -> i32;
type Receive = unsafe extern "system" fn(Internet, *mut c_void) -> i32;
type QueryHeaders =
    unsafe extern "system" fn(Internet, u32, *const u16, *mut c_void, *mut u32, *mut u32) -> i32;
type Read = unsafe extern "system" fn(Internet, *mut c_void, u32, *mut u32) -> i32;
type Close = unsafe extern "system" fn(Internet) -> i32;

struct Api {
    open: Open,
    connect: Connect,
    open_request: OpenRequest,
    set_option: SetOption,
    set_timeouts: SetTimeouts,
    set_callback: SetCallback,
    send: SendRequest,
    receive: Receive,
    query_headers: QueryHeaders,
    read: Read,
    close: Close,
}
impl Api {
    fn get() -> Result<&'static Self, String> {
        static API: OnceLock<Result<Api, String>> = OnceLock::new();
        API.get_or_init(|| unsafe {
            let library = LoadLibraryExW(
                wide("winhttp.dll").as_ptr(),
                null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            );
            if library.is_null() {
                return Err("Windows HTTPS support unavailable".into());
            }
            let load = |name: &CStr| {
                GetProcAddress(library, name.as_ptr().cast())
                    .ok_or_else(|| "Windows HTTPS entry point unavailable".to_owned())
            };
            let result = (|| {
                Ok(Self {
                    open: std::mem::transmute::<unsafe extern "system" fn() -> isize, Open>(load(
                        c"WinHttpOpen",
                    )?),
                    connect: std::mem::transmute::<unsafe extern "system" fn() -> isize, Connect>(
                        load(c"WinHttpConnect")?,
                    ),
                    open_request: std::mem::transmute::<
                        unsafe extern "system" fn() -> isize,
                        OpenRequest,
                    >(load(c"WinHttpOpenRequest")?),
                    set_option: std::mem::transmute::<
                        unsafe extern "system" fn() -> isize,
                        SetOption,
                    >(load(c"WinHttpSetOption")?),
                    set_timeouts: std::mem::transmute::<
                        unsafe extern "system" fn() -> isize,
                        SetTimeouts,
                    >(load(c"WinHttpSetTimeouts")?),
                    set_callback: std::mem::transmute::<
                        unsafe extern "system" fn() -> isize,
                        SetCallback,
                    >(load(c"WinHttpSetStatusCallback")?),
                    send: std::mem::transmute::<unsafe extern "system" fn() -> isize, SendRequest>(
                        load(c"WinHttpSendRequest")?,
                    ),
                    receive: std::mem::transmute::<unsafe extern "system" fn() -> isize, Receive>(
                        load(c"WinHttpReceiveResponse")?,
                    ),
                    query_headers: std::mem::transmute::<
                        unsafe extern "system" fn() -> isize,
                        QueryHeaders,
                    >(load(c"WinHttpQueryHeaders")?),
                    read: std::mem::transmute::<unsafe extern "system" fn() -> isize, Read>(load(
                        c"WinHttpReadData",
                    )?),
                    close: std::mem::transmute::<unsafe extern "system" fn() -> isize, Close>(
                        load(c"WinHttpCloseHandle")?,
                    ),
                })
            })();
            if result.is_err() {
                FreeLibrary(library);
            }
            // Successful loading is process-long: even the final callback returns through
            // WinHTTP code. Unloading from/alongside that callback would race its stack.
            result
        })
        .as_ref()
        .map_err(Clone::clone)
    }
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn failure(code: u32) -> String {
    match code {
        12002 => "Update check timed out; try again".into(),
        12007 => "Cannot resolve GitHub; check your connection and try again".into(),
        12029 | 12030 => "Cannot connect to GitHub; check your connection and try again".into(),
        12175 => "GitHub HTTPS certificate validation failed".into(),
        _ => format!("Update connection failed (Windows error {code}); try again"),
    }
}
fn succeeded(value: i32) -> Result<(), String> {
    if value == 0 {
        Err(failure(unsafe { GetLastError() }))
    } else {
        Ok(())
    }
}
struct Handle {
    raw: Internet,
    api: &'static Api,
}
impl Handle {
    fn new(raw: Internet, api: &'static Api) -> Result<Self, String> {
        if raw.is_null() {
            Err(failure(unsafe { GetLastError() }))
        } else {
            Ok(Self { raw, api })
        }
    }
    fn option(&self, option: u32, value: u32) -> Result<(), String> {
        succeeded(unsafe {
            (self.api.set_option)(self.raw, option, (&value as *const u32).cast(), 4)
        })
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            (self.api.close)(self.raw);
        }
    }
}

// Normally released immediately after a request closes. If the OS delays final
// teardown, allow at most one retained context rather than accumulating retries.
static ACTIVE: AtomicBool = AtomicBool::new(false);
struct Active;
impl Active {
    fn acquire(control: &CheckControl) -> Result<Self, String> {
        loop {
            control.check()?;
            if ACTIVE
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(Self);
            }
            std::thread::sleep(control.remaining().min(Duration::from_millis(25)));
        }
    }
}
impl Drop for Active {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}
enum Completion {
    Sent,
    Headers,
    Read(usize),
    Error(u32),
    Closed,
}
struct Context {
    events: Sender<Completion>,
    buffer: UnsafeCell<[u8; 8192]>,
    _active: Active,
}
// Exactly one WinHttpReadData operation owns the buffer at a time. Only the fetch
// thread accesses its bytes, after the callback reports that read's completion.
// The handle-owned Arc keeps storage live if cancellation races an unfinished read.
unsafe impl Sync for Context {}
unsafe extern "system" fn callback(
    _: Internet,
    context: usize,
    status: u32,
    information: *mut c_void,
    length: u32,
) {
    if context == 0 {
        return;
    }
    let data = &*(context as *const Context);
    let event = match status {
        WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE => Completion::Sent,
        WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE => Completion::Headers,
        WINHTTP_CALLBACK_STATUS_READ_COMPLETE => Completion::Read(length as usize),
        WINHTTP_CALLBACK_STATUS_REQUEST_ERROR => {
            let code = if !information.is_null()
                && length as usize >= std::mem::size_of::<WINHTTP_ASYNC_RESULT>()
            {
                (*(information as *const WINHTTP_ASYNC_RESULT)).dwError
            } else {
                ERROR_INVALID_DATA
            };
            Completion::Error(code)
        }
        WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING => {
            // Windows guarantees this callback is last and does not overlap other
            // callbacks for the request. Release its ownership before signalling.
            let events = data.events.clone();
            drop(Arc::from_raw(context as *const Context));
            let _ = events.send(Completion::Closed);
            return;
        }
        _ => return,
    };
    let _ = data.events.send(event);
}
struct Request {
    handle: Option<Handle>,
    context: Arc<Context>,
    events: Receiver<Completion>,
}
impl Request {
    fn new(handle: Handle, active: Active) -> Result<Self, String> {
        let (sender, receiver) = channel();
        let context = Arc::new(Context {
            events: sender,
            buffer: UnsafeCell::new([0; 8192]),
            _active: active,
        });
        let bound = Arc::into_raw(context.clone()) as usize;
        if let Err(error) = succeeded(unsafe {
            (handle.api.set_option)(
                handle.raw,
                WINHTTP_OPTION_CONTEXT_VALUE,
                (&bound as *const usize).cast(),
                std::mem::size_of::<usize>() as u32,
            )
        }) {
            unsafe {
                drop(Arc::from_raw(bound as *const Context));
            }
            return Err(error);
        }
        let flags = WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE
            | WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE
            | WINHTTP_CALLBACK_STATUS_READ_COMPLETE
            | WINHTTP_CALLBACK_STATUS_REQUEST_ERROR
            | WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING;
        let previous = unsafe { (handle.api.set_callback)(handle.raw, Some(callback), flags, 0) };
        if previous.is_some_and(|value| value as usize == usize::MAX) {
            let error = failure(unsafe { GetLastError() });
            // Callback installation failed, so no callback can consume this binding.
            unsafe {
                drop(Arc::from_raw(bound as *const Context));
            }
            return Err(error);
        }
        Ok(Self {
            handle: Some(handle),
            context,
            events: receiver,
        })
    }
    fn raw(&self) -> Internet {
        self.handle.as_ref().expect("live request").raw
    }
    fn wait(&self, control: &CheckControl) -> Result<Completion, String> {
        loop {
            control.check()?;
            match self
                .events
                .recv_timeout(control.remaining().min(Duration::from_millis(25)))
            {
                Ok(Completion::Error(code)) => return Err(failure(code)),
                Ok(Completion::Closed) => return Err("Update connection closed".into()),
                Ok(event) => {
                    control.check()?;
                    return Ok(event);
                }
                Err(RecvTimeoutError::Timeout) => (),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("Update connection stopped".into())
                }
            }
        }
    }
}
impl Drop for Request {
    fn drop(&mut self) {
        drop(self.handle.take()); // Closing an ASYNC request cancels outstanding I/O.
                                  // Briefly drain normal teardown. Never wait indefinitely during Exit. If
                                  // delayed, HANDLE_CLOSING retains/frees the Arc and the retry gate itself.
        let until = Instant::now() + Duration::from_millis(100);
        while let Ok(event) = self
            .events
            .recv_timeout(until.saturating_duration_since(Instant::now()))
        {
            if matches!(event, Completion::Closed) || Instant::now() >= until {
                break;
            }
        }
    }
}

fn headers(api: &Api, request: Internet) -> Result<(u16, Option<String>, Option<usize>), String> {
    let mut status = 0u32;
    let mut size = 4;
    succeeded(unsafe {
        (api.query_headers)(
            request,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&mut status as *mut u32).cast(),
            &mut size,
            null_mut(),
        )
    })?;
    let mut text = vec![0u16; 32_768];
    size = (text.len() * 2) as u32;
    succeeded(unsafe {
        (api.query_headers)(
            request,
            WINHTTP_QUERY_RAW_HEADERS_CRLF,
            null(),
            text.as_mut_ptr().cast(),
            &mut size,
            null_mut(),
        )
    })?;
    if size as usize > text.len() * 2 || size % 2 != 0 {
        return Err("Invalid release response headers".into());
    }
    let text = String::from_utf16(&text[..size as usize / 2])
        .map_err(|_| "Invalid release response headers")?;
    let mut links = Vec::new();
    let mut content_length = None;
    for line in text.trim_end_matches('\0').split("\r\n").skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("link") {
            links.push(value.trim());
        }
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err("Ambiguous release response length".into());
            }
            let value = value
                .trim()
                .parse::<usize>()
                .map_err(|_| "Invalid release response length")?;
            if value > MAX_RESPONSE_BYTES {
                return Err("Release response exceeds size limit".into());
            }
            content_length = Some(value);
        }
        if name.eq_ignore_ascii_case("content-encoding")
            && !value.trim().eq_ignore_ascii_case("identity")
        {
            return Err("Unsupported release response encoding".into());
        }
    }
    let status = u16::try_from(status).map_err(|_| "Invalid release HTTP status")?;
    Ok((
        status,
        (!links.is_empty()).then(|| links.join(", ")),
        content_length,
    ))
}

/// Performs only explicitly requested HTTPS metadata reads from the fixed repository.
pub struct WindowsTransport;
impl ReleaseTransport for WindowsTransport {
    fn fetch(&self, url: &str, control: &CheckControl) -> Result<HttpResponse, String> {
        control.check()?;
        if !valid_api_url(url) {
            return Err("Unsafe release API URL rejected".into());
        }
        let active = Active::acquire(control)?;
        let api = Api::get()?;
        let session = Handle::new(
            unsafe {
                (api.open)(
                    wide(concat!(
                        "EnvironmentVariableManager/",
                        env!("CARGO_PKG_VERSION")
                    ))
                    .as_ptr(),
                    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                    null(),
                    null(),
                    WINHTTP_FLAG_ASYNC,
                )
            },
            api,
        )?;
        succeeded(unsafe { (api.set_timeouts)(session.raw, 3000, 3000, 3000, 3000) })?;
        session.option(WINHTTP_OPTION_MAX_RESPONSE_HEADER_SIZE, 65_536)?;
        let connection = Handle::new(
            unsafe { (api.connect)(session.raw, wide("api.github.com").as_ptr(), 443, 0) },
            api,
        )?;
        let path = url
            .strip_prefix("https://api.github.com")
            .ok_or("Unsafe release API URL")?;
        let handle = Handle::new(
            unsafe {
                (api.open_request)(
                    connection.raw,
                    wide("GET").as_ptr(),
                    wide(path).as_ptr(),
                    null(),
                    null(),
                    null(),
                    WINHTTP_FLAG_SECURE,
                )
            },
            api,
        )?;
        handle.option(
            WINHTTP_OPTION_DISABLE_FEATURE,
            WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_AUTHENTICATION,
        )?;
        handle.option(
            WINHTTP_OPTION_AUTOLOGON_POLICY,
            WINHTTP_AUTOLOGON_SECURITY_LEVEL_HIGH,
        )?;
        let request = Request::new(handle, active)?;
        control.check()?;
        let context = Arc::as_ptr(&request.context) as usize;
        let extra = wide("Accept: application/vnd.github+json\r\nAccept-Encoding: identity\r\nX-GitHub-Api-Version: 2022-11-28\r\n");
        succeeded(unsafe {
            (api.send)(
                request.raw(),
                extra.as_ptr(),
                extra.len() as u32 - 1,
                null(),
                0,
                0,
                context,
            )
        })?;
        if !matches!(request.wait(control)?, Completion::Sent) {
            return Err("Unexpected update connection state".into());
        }
        succeeded(unsafe { (api.receive)(request.raw(), null_mut()) })?;
        if !matches!(request.wait(control)?, Completion::Headers) {
            return Err("Unexpected update response state".into());
        }
        let (status, link, content_length) = headers(api, request.raw())?;
        // Status is sufficient for errors; never collect arbitrary error pages.
        if status != 200 {
            return Ok(HttpResponse {
                status,
                body: Vec::new(),
                link,
            });
        }
        let mut body = Vec::new();
        loop {
            control.check()?;
            succeeded(unsafe {
                (api.read)(
                    request.raw(),
                    request.context.buffer.get().cast(),
                    8192,
                    null_mut(),
                )
            })?;
            let Completion::Read(count) = request.wait(control)? else {
                return Err("Unexpected update read state".into());
            };
            if count > 8192 || count > MAX_RESPONSE_BYTES.saturating_sub(body.len()) {
                return Err("Release response exceeds size limit".into());
            }
            if count == 0 {
                break;
            }
            // READ_COMPLETE finishes the outstanding write before bytes are read here.
            body.extend_from_slice(unsafe {
                std::slice::from_raw_parts(request.context.buffer.get().cast::<u8>(), count)
            });
        }
        if content_length.is_some_and(|expected| expected != body.len()) {
            return Err("Incomplete release response".into());
        }
        control.check()?;
        Ok(HttpResponse { status, body, link })
    }
}
