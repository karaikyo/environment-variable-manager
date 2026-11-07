//! Win32 authorization and transport. All failures are payload-free diagnostics.
use super::{SystemRequest, MAX_FRAME_BYTES};
use crate::{same_name, Scope, Variable, WriteOutcome};
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::{
        Com::*, LibraryLoader::*, Pipes::*, Registry::*, SystemInformation::*, Threading::*, IO::*,
    },
    UI::{Shell::*, WindowsAndMessaging::*},
};
const MODE: &str = "--system-helper";
const TIMEOUT: Duration = Duration::from_secs(15);
const SYSTEM_KEY: &str = "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment";
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn error() -> String {
    format!("Helper boundary failed (Windows error {})", unsafe {
        GetLastError()
    })
}
struct Handle(HANDLE);
impl Handle {
    fn checked(raw: HANDLE) -> Result<Self, String> {
        if raw.is_null() || raw == INVALID_HANDLE_VALUE {
            Err(error())
        } else {
            Ok(Self(raw))
        }
    }
    fn alive(&self) -> bool {
        unsafe { WaitForSingleObject(self.0, 0) == WAIT_TIMEOUT }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn creation(process: HANDLE) -> Result<u64, String> {
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    if unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
        return Err(error());
    }
    Ok((created.dwHighDateTime as u64) << 32 | created.dwLowDateTime as u64)
}
fn image(process: HANDLE) -> Result<String, String> {
    let mut buffer = vec![0u16; 32768];
    let mut count = buffer.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut count) } == 0 {
        return Err(error());
    }
    String::from_utf16(&buffer[..count as usize])
        .map_err(|_| "Unrepresentable process image".into())
}
struct Identity {
    sid: Vec<u8>,
    elevated: bool,
}
fn identity(process: HANDLE) -> Result<Identity, String> {
    let mut token = null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(error());
    }
    let token = Handle(token);
    let mut needed = 0;
    unsafe {
        GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut needed);
    }
    if needed == 0 || needed > 65536 {
        return Err("Invalid token identity".into());
    }
    // Aligned storage for TOKEN_USER; SID bytes are copied before releasing it.
    let mut storage = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            storage.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(error());
    }
    let user = unsafe { &*(storage.as_ptr().cast::<TOKEN_USER>()) };
    if unsafe { IsValidSid(user.User.Sid) } == 0 {
        return Err("Invalid account SID".into());
    }
    let len = unsafe { GetLengthSid(user.User.Sid) } as usize;
    let mut sid = vec![0u8; len];
    if unsafe { CopySid(len as u32, sid.as_mut_ptr().cast(), user.User.Sid) } == 0 {
        return Err(error());
    }
    let mut elevation = TOKEN_ELEVATION::default();
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut needed,
        )
    } == 0
    {
        return Err(error());
    }
    Ok(Identity {
        sid,
        elevated: elevation.TokenIsElevated != 0,
    })
}
fn same_account(a: &Identity, b: &Identity) -> bool {
    unsafe {
        EqualSid(
            a.sid.as_ptr().cast_mut().cast(),
            b.sid.as_ptr().cast_mut().cast(),
        ) != 0
    }
}
fn verify_peer(process: &Handle, elevated: bool) -> Result<(), String> {
    if !process.alive() {
        return Err("Peer process exited".into());
    }
    if !same_name(&image(process.0)?, &image(unsafe { GetCurrentProcess() })?) {
        return Err("Unexpected helper image".into());
    }
    let peer = identity(process.0)?;
    let own = identity(unsafe { GetCurrentProcess() })?;
    if peer.elevated != elevated || !same_account(&peer, &own) {
        return Err("Same-account administrator consent required".into());
    }
    Ok(())
}
fn endpoint(id: &str) -> Result<Vec<u16>, String> {
    if id.len() != 32 || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("Invalid helper endpoint".into());
    }
    Ok(wide(&format!(r"\\.\pipe\EnvironmentManager-{id}")))
}
fn random_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    crate::system_libraries::random_bytes(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
struct Descriptor(*mut core::ffi::c_void);
impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
fn server(id: &str) -> Result<Handle, String> {
    let own = identity(unsafe { GetCurrentProcess() })?;
    let mut sid_text = null_mut();
    if unsafe { ConvertSidToStringSidW(own.sid.as_ptr().cast_mut().cast(), &mut sid_text) } == 0 {
        return Err(error());
    }
    let text_owner = Descriptor(sid_text.cast());
    let mut len = 0;
    unsafe {
        while *sid_text.add(len) != 0 {
            len += 1;
        }
    }
    let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(sid_text, len) })
        .map_err(|_| "Invalid SID".to_owned())?;
    // Individual data/attribute rights, SYNCHRONIZE and READ_CONTROL only. No
    // FILE_CREATE_PIPE_INSTANCE (bit 4); never generic-write or Everyone.
    let sddl = wide(&format!("D:P(A;;0x00120083;;;{sid})"));
    let mut descriptor = null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(error());
    }
    let descriptor = Descriptor(descriptor);
    drop(text_owner);
    let security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    Handle::checked(unsafe {
        CreateNamedPipeW(
            endpoint(id)?.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            65536,
            65536,
            15000,
            &security,
        )
    })
}
/// Completes or cancels and drains every outstanding operation before stack/buffer release.
fn completion(
    pipe: &Handle,
    overlapped: &mut OVERLAPPED,
    peer: &Handle,
    deadline: Instant,
    started: i32,
    immediate_error: u32,
) -> Result<u32, String> {
    if started == 0 && immediate_error != ERROR_IO_PENDING {
        return Err(format!("Pipe I/O failed ({immediate_error})"));
    }
    if started == 0 {
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(u32::MAX as u128 - 1) as u32;
        let waits = [overlapped.hEvent, peer.0];
        let result = unsafe { WaitForMultipleObjects(2, waits.as_ptr(), 0, remaining) };
        if result != WAIT_OBJECT_0 {
            unsafe {
                CancelIoEx(pipe.0, overlapped);
                let mut ignored = 0;
                GetOverlappedResult(pipe.0, overlapped, &mut ignored, 1);
            }
            return Err("Helper I/O deadline or peer lifetime ended".into());
        }
    }
    let mut count = 0;
    if unsafe { GetOverlappedResult(pipe.0, overlapped, &mut count, 0) } == 0 {
        return Err(error());
    }
    Ok(count)
}
fn connect(pipe: &Handle, peer: &Handle, deadline: Instant) -> Result<(), String> {
    let event = Handle::checked(unsafe { CreateEventW(null(), 1, 0, null()) })?;
    let mut overlapped = OVERLAPPED {
        hEvent: event.0,
        ..unsafe { zeroed() }
    };
    let started = unsafe { ConnectNamedPipe(pipe.0, &mut overlapped) };
    let code = unsafe { GetLastError() };
    if started == 0 && code == ERROR_PIPE_CONNECTED {
        return Ok(());
    }
    completion(pipe, &mut overlapped, peer, deadline, started, code).map(|_| ())
}
fn transfer(
    pipe: &Handle,
    peer: &Handle,
    bytes: &mut [u8],
    write: bool,
    deadline: Instant,
) -> Result<(), String> {
    let event = Handle::checked(unsafe { CreateEventW(null(), 1, 0, null()) })?;
    let mut offset = 0;
    while offset < bytes.len() {
        if Instant::now() >= deadline || !peer.alive() {
            return Err("Helper I/O deadline or peer lifetime ended".into());
        }
        unsafe {
            ResetEvent(event.0);
        }
        let mut overlapped = OVERLAPPED {
            hEvent: event.0,
            ..unsafe { zeroed() }
        };
        let slice = &mut bytes[offset..];
        let len = slice.len().min(65536) as u32;
        let started = unsafe {
            if write {
                WriteFile(pipe.0, slice.as_ptr(), len, null_mut(), &mut overlapped)
            } else {
                ReadFile(pipe.0, slice.as_mut_ptr(), len, null_mut(), &mut overlapped)
            }
        };
        let count = completion(pipe, &mut overlapped, peer, deadline, started, unsafe {
            GetLastError()
        })? as usize;
        if count == 0 || count > len as usize {
            return Err("Helper pipe closed or invalid count".into());
        }
        offset += count;
    }
    Ok(())
}
fn send(pipe: &Handle, peer: &Handle, bytes: &[u8], deadline: Instant) -> Result<(), String> {
    if bytes.len() > MAX_FRAME_BYTES {
        return Err("Oversized helper message".into());
    }
    let mut header = (bytes.len() as u32).to_le_bytes();
    transfer(pipe, peer, &mut header, true, deadline)?;
    transfer(pipe, peer, &mut bytes.to_vec(), true, deadline)
}
fn receive(
    pipe: &Handle,
    peer: &Handle,
    maximum: usize,
    deadline: Instant,
) -> Result<Vec<u8>, String> {
    let mut header = [0; 4];
    transfer(pipe, peer, &mut header, false, deadline)?;
    let len = u32::from_le_bytes(header) as usize;
    if len == 0 || len > maximum {
        return Err("Invalid helper frame length".into());
    }
    let mut bytes = vec![0; len];
    transfer(pipe, peer, &mut bytes, false, deadline)?;
    Ok(bytes)
}
struct Com;
impl Drop for Com {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
/// Normal UI and the user's history are never opened under an elevated token.
pub fn require_unelevated() -> Result<(), String> {
    if identity(unsafe { GetCurrentProcess() })?.elevated {
        return Err(
            "Run the application normally; system writes request their own administrator approval"
                .into(),
        );
    }
    if unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) } == 0 {
        return Err(error());
    }
    Ok(())
}
fn launch(id: &str) -> Result<Handle, WriteOutcome> {
    require_unelevated().map_err(WriteOutcome::Rejected)?;
    let hr = unsafe { CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32) };
    if hr < 0 {
        return Err(WriteOutcome::Rejected(
            "Cannot initialize consent worker".into(),
        ));
    }
    let _com = Com;
    let exe = std::env::current_exe()
        .map_err(|_| WriteOutcome::Rejected("Application image unavailable".into()))?;
    let path = wide(
        exe.to_str()
            .ok_or_else(|| WriteOutcome::Rejected("Invalid application path".into()))?,
    );
    let args = wide(&format!(
        "{MODE} {id} {} {}",
        std::process::id(),
        creation(unsafe { GetCurrentProcess() }).map_err(WriteOutcome::Rejected)?
    ));
    let verb = wide("runas");
    let mut directory = vec![0u16; 32768];
    let len = unsafe { GetSystemDirectoryW(directory.as_mut_ptr(), directory.len() as u32) };
    if len == 0 || len as usize >= directory.len() {
        return Err(WriteOutcome::Rejected(
            "System directory unavailable".into(),
        ));
    }
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: verb.as_ptr(),
        lpFile: path.as_ptr(),
        lpParameters: args.as_ptr(),
        lpDirectory: directory.as_ptr(),
        nShow: SW_HIDE,
        ..unsafe { zeroed() }
    };
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(if unsafe { GetLastError() } == ERROR_CANCELLED {
            WriteOutcome::Cancelled
        } else {
            WriteOutcome::Rejected(error())
        });
    }
    Handle::checked(info.hProcess).map_err(WriteOutcome::Rejected)
}
/// An account-restricted single-use local endpoint, created before consent.
/// Request bytes are released only to the retained, same-image elevated child.
pub struct LocalSystemChannel {
    pipe: Handle,
    id: String,
}
impl LocalSystemChannel {
    pub fn bind() -> Result<Self, String> {
        let id = random_id()?;
        let pipe = server(&id)?;
        Ok(Self { pipe, id })
    }
    pub fn identifier(&self) -> &str {
        &self.id
    }
    /// The caller retains the Child; the endpoint also duplicates its process handle.
    pub fn exchange_with_child(
        self,
        child: &std::process::Child,
        request: &SystemRequest,
    ) -> WriteOutcome {
        use std::os::windows::io::AsRawHandle;
        let mut duplicate = null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                child.as_raw_handle(),
                GetCurrentProcess(),
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return WriteOutcome::Rejected(error());
        }
        self.exchange(&Handle(duplicate), request)
    }
    fn exchange(self, child: &Handle, request: &SystemRequest) -> WriteOutcome {
        let bytes = match request.encode() {
            Ok(bytes) => bytes,
            Err(error) => return WriteOutcome::Rejected(error),
        };
        let pipe = &self.pipe;
        let deadline = Instant::now() + TIMEOUT;
        let authorized = (|| {
            connect(pipe, child, deadline)?;
            let mut pid = 0;
            if unsafe { GetNamedPipeClientProcessId(pipe.0, &mut pid) } == 0
                || pid != unsafe { GetProcessId(child.0) }
            {
                return Err("Unexpected pipe client".into());
            }
            verify_peer(child, true)
        })();
        if let Err(error) = authorized {
            return WriteOutcome::Rejected(error);
        }
        // Once any request bytes may have been delivered, broken transport is uncertain.
        if send(pipe, child, &bytes, deadline).is_err() {
            return WriteOutcome::Uncertain;
        }
        let reply = match receive(pipe, child, 8, deadline) {
            Ok(reply) => reply,
            Err(_) => return WriteOutcome::Uncertain,
        };
        let _ = send(pipe, child, &[0], deadline);
        match reply.as_slice() {
            [1] => WriteOutcome::Attempted(Ok(())),
            [2] => WriteOutcome::Conflict,
            [3] => WriteOutcome::Rejected(
                "Helper validation, account or parent lifetime rejected".into(),
            ),
            [4] => WriteOutcome::Attempted(Err("System registry write/readback failed".into())),
            _ => WriteOutcome::Uncertain,
        }
    }
}
pub fn change_system(
    name: &str,
    expected: Option<&Variable>,
    desired: Option<&Variable>,
) -> WriteOutcome {
    let request = SystemRequest {
        name: name.into(),
        expected: expected.cloned(),
        desired: desired.cloned(),
    };
    if let Err(error) = request.validate() {
        return WriteOutcome::Rejected(error);
    }
    let channel = match LocalSystemChannel::bind() {
        Ok(channel) => channel,
        Err(error) => return WriteOutcome::Rejected(error),
    };
    let child = match launch(&channel.id) {
        Ok(child) => child,
        Err(outcome) => return outcome,
    };
    channel.exchange(&child, &request)
}
fn read_value(key: HKEY, name: &str) -> Result<Option<Variable>, String> {
    let encoded = wide(name);
    let mut length = 0;
    let mut kind = 0;
    let code = unsafe {
        RegQueryValueExW(
            key,
            encoded.as_ptr(),
            null(),
            &mut kind,
            null_mut(),
            &mut length,
        )
    };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if code != ERROR_SUCCESS || length as usize > super::MAX_VALUE_BYTES {
        return Err("Cannot read bounded system value".into());
    }
    let mut bytes = vec![0; length as usize];
    let code = unsafe {
        RegQueryValueExW(
            key,
            encoded.as_ptr(),
            null(),
            &mut kind,
            bytes.as_mut_ptr(),
            &mut length,
        )
    };
    if code != ERROR_SUCCESS || length as usize > bytes.len() {
        return Err("System value changed while reading".into());
    }
    bytes.truncate(length as usize);
    Ok(Some(Variable {
        scope: Scope::System,
        name: name.into(),
        registry_type: kind,
        bytes,
    }))
}
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
/// No runtime hive/subkey/path parameter exists for the privileged adapter.
struct FixedSystemEnvironment(Key);
impl FixedSystemEnvironment {
    fn open() -> Result<Self, String> {
        let mut key = null_mut();
        if unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                wide(SYSTEM_KEY).as_ptr(),
                0,
                KEY_QUERY_VALUE | KEY_SET_VALUE | KEY_WOW64_64KEY,
                &mut key,
            )
        } != ERROR_SUCCESS
        {
            return Err("Cannot open fixed system environment key".into());
        }
        Ok(Self(Key(key)))
    }
}
impl super::SystemEnvironment for FixedSystemEnvironment {
    fn read(&self, name: &str) -> Result<Option<Variable>, String> {
        read_value(self.0 .0, name)
    }
    fn replace(&self, name: &str, desired: Option<&Variable>) -> Result<(), String> {
        let name = wide(name);
        let code = unsafe {
            match desired {
                Some(value) => RegSetValueExW(
                    self.0 .0,
                    name.as_ptr(),
                    0,
                    value.registry_type,
                    value.bytes.as_ptr(),
                    value.bytes.len() as u32,
                ),
                None => RegDeleteValueW(self.0 .0, name.as_ptr()),
            }
        };
        if code != ERROR_SUCCESS {
            return Err("System registry mutation failed".into());
        }
        let mut ignored = 0;
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                wide("Environment").as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                1000,
                &mut ignored,
            );
        }
        Ok(())
    }
}
fn mutate(request: &SystemRequest, parent: &Handle) -> u8 {
    if request.validate().is_err() || !parent.alive() {
        return 3;
    }
    let environment = match FixedSystemEnvironment::open() {
        Ok(environment) => environment,
        Err(_) => return 3,
    };
    match request.apply(&environment, || parent.alive()) {
        WriteOutcome::Attempted(Ok(())) => 1,
        WriteOutcome::Conflict => 2,
        WriteOutcome::Attempted(Err(_)) | WriteOutcome::Uncertain => 4,
        _ => 3,
    }
}
fn helper(id: &str, pid: u32, created: u64) -> Result<(), String> {
    let own = identity(unsafe { GetCurrentProcess() })?;
    if !own.elevated {
        return Err("Helper is not elevated".into());
    }
    let parent = Handle::checked(unsafe {
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid)
    })?;
    if creation(parent.0)? != created {
        return Err("Parent process identity changed".into());
    }
    verify_peer(&parent, false)?;
    let pipe = Handle::checked(unsafe {
        CreateFileW(
            endpoint(id)?.as_ptr(),
            FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            0,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            null_mut(),
        )
    })?;
    let mut server_pid = 0;
    if unsafe { GetNamedPipeServerProcessId(pipe.0, &mut server_pid) } == 0 || server_pid != pid {
        return Err("Unexpected pipe server".into());
    }
    let deadline = Instant::now() + TIMEOUT;
    let payload = receive(&pipe, &parent, MAX_FRAME_BYTES, deadline)?;
    let response = match SystemRequest::decode(&payload) {
        Ok(request) => mutate(&request, &parent),
        Err(_) => 3,
    };
    send(&pipe, &parent, &[response], deadline)?;
    if receive(&pipe, &parent, 1, deadline)? != [0] {
        return Err("Invalid helper acknowledgement".into());
    }
    Ok(())
}
/// Must precede all desktop, storage, startup and single-instance initialization.
pub fn dispatch_helper() -> Option<i32> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if !args.iter().any(|argument| argument == MODE) {
        return None;
    }
    // Restrict dynamic loads. Static imports must additionally pass PE inspection.
    if unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) } == 0 {
        return Some(73);
    }
    let parsed = (|| {
        if args.len() != 4 || args[0] != MODE {
            return None;
        }
        let id = args[1].to_str()?;
        endpoint(id).ok()?;
        let pid = args[2].to_str()?.parse::<u32>().ok()?;
        let created = args[3].to_str()?.parse::<u64>().ok()?;
        if pid == 0 || created == 0 {
            return None;
        }
        Some((id, pid, created))
    })();
    Some(match parsed {
        Some((id, pid, created)) if helper(id, pid, created).is_ok() => 0,
        _ => 73,
    })
}
