use environment_variable_manager::{elevation::SystemRequest, Scope, Variable};
#[test]
fn helper_protocol_preserves_exact_states_and_rejects_malformed_requests() {
    let request = SystemRequest {
        name: "Owned".into(),
        expected: None,
        desired: Some(Variable::text(
            Scope::System,
            "Owned",
            "%Root%;;a\nline;",
            true,
        )),
    };
    let bytes = request.encode().unwrap();
    assert_eq!(SystemRequest::decode(&bytes).unwrap(), request);
    for end in 0..bytes.len() {
        assert!(SystemRequest::decode(&bytes[..end]).is_err());
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(SystemRequest::decode(&trailing).is_err());
    let mut version = bytes.clone();
    version[3] = 99;
    assert!(SystemRequest::decode(&version).is_err());
    let invalid = SystemRequest {
        name: "bad=name".into(),
        ..request.clone()
    };
    assert!(invalid.encode().is_err());
    let invalid = SystemRequest {
        desired: Some(Variable::text(Scope::User, "Owned", "value", false)),
        ..request.clone()
    };
    assert!(invalid.encode().is_err());
    let invalid = SystemRequest {
        desired: Some(Variable {
            scope: Scope::System,
            name: "Owned".into(),
            registry_type: 1,
            bytes: vec![0, 216],
        }),
        ..request
    };
    assert!(invalid.encode().is_err());
}
#[test]
fn local_system_channel_rejects_unelevated_same_image_peer_without_disclosing_payload() {
    use environment_variable_manager::{elevation::LocalSystemChannel, WriteOutcome};
    let channel = LocalSystemChannel::bind().unwrap();
    let exe = std::env::current_exe().unwrap();
    let mut child = std::process::Command::new(exe)
        .args(["--ignored", "--exact", "native_pipe_client"])
        .env("EVM_TEST_PIPE", channel.identifier())
        .spawn()
        .unwrap();
    let request = SystemRequest {
        name: "Owned".into(),
        expected: None,
        desired: Some(Variable::text(
            Scope::System,
            "Owned",
            "never disclosed",
            false,
        )),
    };
    assert!(matches!(
        channel.exchange_with_child(&child, &request),
        WriteOutcome::Rejected(_)
    ));
    assert!(child.wait().unwrap().success());
}
#[test]
#[ignore]
fn native_pipe_client() {
    use windows_sys::Win32::{Foundation::*, Storage::FileSystem::*};
    let Some(id) = std::env::var_os("EVM_TEST_PIPE") else {
        return;
    };
    let path: Vec<u16> = format!(r"\\.\pipe\EnvironmentManager-{}", id.to_str().unwrap())
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(handle, INVALID_HANDLE_VALUE);
    let mut buffer = [0u8; 4];
    let mut count = 0;
    let result = unsafe {
        ReadFile(
            handle,
            buffer.as_mut_ptr(),
            4,
            &mut count,
            std::ptr::null_mut(),
        )
    };
    assert_eq!(
        count, 0,
        "Unauthenticated peer must not receive request header or payload"
    );
    assert_eq!(result, 0);
    unsafe {
        CloseHandle(handle);
    }
}
#[test]
fn helper_starts_safely_with_decoy_optional_dlls_and_never_initializes_user_data() {
    let directory = std::env::temp_dir().join(format!(
        "evm-helper-loading-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let executable = directory.join("manager.exe");
    std::fs::write(
        &executable,
        std::fs::read(env!("CARGO_BIN_EXE_environment-variable-manager")).unwrap(),
    )
    .unwrap();
    for name in ["comctl32.dll", "bcrypt.dll"] {
        std::fs::write(directory.join(name), b"invalid decoy, never executable").unwrap();
    }
    let previous = unsafe { windows_sys::Win32::System::Diagnostics::Debug::SetErrorMode(3) };
    let result = std::process::Command::new(&executable)
        .arg("--system-helper")
        .current_dir(&directory)
        .status()
        .unwrap();
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::SetErrorMode(previous);
    }
    assert_eq!(result.code(),Some(73),"Helper must reach the fail-closed dispatcher without resolving optional DLLs from its folder");
    assert!(!directory.join("data").exists());
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn local_system_channel_has_one_protected_account_ace_and_forbids_another_instance() {
    use environment_variable_manager::elevation::LocalSystemChannel;
    use windows_sys::Win32::{
        Foundation::*,
        Security::{Authorization::*, *},
        Storage::FileSystem::*,
        System::{Pipes::*, Threading::*},
    };
    let channel = LocalSystemChannel::bind().unwrap();
    let path: Vec<u16> = format!(r"\\.\pipe\EnvironmentManager-{}", channel.identifier())
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            READ_CONTROL | FILE_READ_DATA | FILE_WRITE_DATA,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(handle, INVALID_HANDLE_VALUE);
    let mut descriptor = std::ptr::null_mut();
    let mut acl = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            GetSecurityInfo(
                handle,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut acl,
                std::ptr::null_mut(),
                &mut descriptor,
            )
        },
        ERROR_SUCCESS
    );
    let mut control = 0;
    let mut revision = 0;
    assert_ne!(
        unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) },
        0
    );
    assert_ne!(control & SE_DACL_PROTECTED, 0);
    assert!(!acl.is_null());
    assert_eq!(unsafe { (*acl).AceCount }, 1);
    let mut ace = std::ptr::null_mut();
    assert_ne!(unsafe { GetAce(acl, 0, &mut ace) }, 0);
    let ace = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
    assert_eq!(ace.Header.AceType, 0);
    assert_eq!(ace.Header.AceFlags, 0);
    assert_eq!(ace.Mask, 0x00120083);
    let mut token = std::ptr::null_mut();
    assert_ne!(
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) },
        0
    );
    let mut length = 0;
    unsafe {
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut length);
    }
    let mut user = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
    assert_ne!(
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                user.as_mut_ptr().cast(),
                length,
                &mut length,
            )
        },
        0
    );
    let user = unsafe { &*user.as_ptr().cast::<TOKEN_USER>() };
    assert_ne!(
        unsafe {
            EqualSid(
                user.User.Sid,
                (&ace.SidStart as *const u32).cast_mut().cast(),
            )
        },
        0
    );
    let second = unsafe {
        CreateNamedPipeW(
            path.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            16,
            16,
            1000,
            std::ptr::null(),
        )
    };
    assert_eq!(second, INVALID_HANDLE_VALUE);
    unsafe {
        CloseHandle(token);
        LocalFree(descriptor);
        CloseHandle(handle);
    }
}
#[test]
fn local_system_channel_peer_exit_cancels_pending_connect_promptly() {
    use environment_variable_manager::{elevation::LocalSystemChannel, WriteOutcome};
    let channel = LocalSystemChannel::bind().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "native_no_connect_client"])
        .spawn()
        .unwrap();
    let request = SystemRequest {
        name: "Owned".into(),
        expected: None,
        desired: Some(Variable::text(Scope::System, "Owned", "secret", false)),
    };
    let started = std::time::Instant::now();
    assert!(matches!(
        channel.exchange_with_child(&child, &request),
        WriteOutcome::Rejected(_)
    ));
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    assert!(child.wait().unwrap().success());
}
#[test]
#[ignore]
fn native_no_connect_client() {
    std::thread::sleep(std::time::Duration::from_millis(100));
}
#[test]
fn local_system_channel_stalled_peer_hits_absolute_deadline_and_drains_connect() {
    use environment_variable_manager::{elevation::LocalSystemChannel, WriteOutcome};
    let channel = LocalSystemChannel::bind().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "native_stalled_client"])
        .spawn()
        .unwrap();
    let request = SystemRequest {
        name: "Owned".into(),
        expected: None,
        desired: Some(Variable::text(Scope::System, "Owned", "secret", false)),
    };
    let started = std::time::Instant::now();
    assert!(matches!(
        channel.exchange_with_child(&child, &request),
        WriteOutcome::Rejected(_)
    ));
    let elapsed = started.elapsed();
    assert!(elapsed >= std::time::Duration::from_secs(14));
    assert!(elapsed < std::time::Duration::from_secs(20));
    child.kill().unwrap();
    child.wait().unwrap();
}
#[test]
#[ignore]
fn native_stalled_client() {
    std::thread::sleep(std::time::Duration::from_secs(60));
}
#[test]
fn normal_application_requires_unelevated_execution_before_opening_history() {
    environment_variable_manager::elevation::require_unelevated().unwrap();
}
#[test]
fn helper_operation_rechecks_owned_registry_after_consent_and_obeys_parent_lifetime() {
    use environment_variable_manager::{
        elevation::SystemEnvironment, windows_registry::WindowsRegistry, RegistryReader,
        RegistryWriter, WriteOutcome,
    };
    use windows_sys::Win32::{Foundation::ERROR_SUCCESS, System::Registry::*};
    struct Owned(WindowsRegistry);
    impl SystemEnvironment for Owned {
        fn read(&self, name: &str) -> Result<Option<Variable>, String> {
            Ok(self
                .0
                .read_scope(Scope::System)?
                .into_iter()
                .find(|v| environment_variable_manager::same_name(&v.name, name)))
        }
        fn replace(&self, name: &str, value: Option<&Variable>) -> Result<(), String> {
            match value {
                Some(value) => {
                    let mut value = value.clone();
                    value.scope = Scope::User;
                    self.0.write_user(&value)
                }
                None => self.0.delete_user(name),
            }
        }
    }
    let path = format!(
        "Software\\EnvironmentManagerTests\\helper-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let encoded: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let mut key = std::ptr::null_mut();
    let mut disposition = 0;
    assert_eq!(
        unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                encoded.as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_READ | KEY_WRITE,
                std::ptr::null(),
                &mut key,
                &mut disposition,
            )
        },
        ERROR_SUCCESS
    );
    assert_eq!(
        disposition, REG_CREATED_NEW_KEY,
        "Abort on any fixture collision"
    );
    unsafe {
        RegCloseKey(key);
    }
    let owned = Owned(WindowsRegistry::under_current_user(&path, &path));
    let created = Variable::text(Scope::System, "Owned", "", true);
    let create = SystemRequest {
        name: "Owned".into(),
        expected: None,
        desired: Some(created.clone()),
    };
    assert_eq!(
        create.apply(&owned, || true),
        WriteOutcome::Attempted(Ok(()))
    );
    let desired = Variable::text(Scope::System, "Owned", " a;;%Root%;x\nline;", true);
    let edit = SystemRequest {
        name: "Owned".into(),
        expected: Some(created.clone()),
        desired: Some(desired.clone()),
    };
    let external = Variable::text(Scope::System, "Owned", "external while consenting", true);
    owned.replace("Owned", Some(&external)).unwrap();
    assert_eq!(edit.apply(&owned, || true), WriteOutcome::Conflict);
    assert_eq!(owned.read("Owned").unwrap(), Some(external.clone()));
    let edit = SystemRequest {
        expected: Some(external.clone()),
        ..edit
    };
    let checks = std::cell::Cell::new(0);
    assert!(matches!(
        edit.apply(&owned, || {
            checks.set(checks.get() + 1);
            checks.get() < 2
        }),
        WriteOutcome::Rejected(_)
    ));
    assert_eq!(owned.read("Owned").unwrap(), Some(external));
    assert_eq!(edit.apply(&owned, || true), WriteOutcome::Attempted(Ok(())));
    assert_eq!(owned.read("Owned").unwrap(), Some(desired.clone()));
    let delete = SystemRequest {
        name: "Owned".into(),
        expected: Some(desired),
        desired: None,
    };
    assert_eq!(
        delete.apply(&owned, || true),
        WriteOutcome::Attempted(Ok(()))
    );
    assert!(
        owned.0.read_scope(Scope::System).unwrap().is_empty(),
        "Verify whole owned subtree before cleanup"
    );
    assert_eq!(
        unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, encoded.as_ptr()) },
        ERROR_SUCCESS
    );
}
#[test]
fn local_system_channel_rejects_wrong_retained_pid_and_wrong_image_without_payload() {
    use environment_variable_manager::{elevation::LocalSystemChannel, WriteOutcome};
    let request = SystemRequest {
        name: "Owned".into(),
        expected: None,
        desired: Some(Variable::text(
            Scope::System,
            "Owned",
            "never disclosed",
            false,
        )),
    };
    let channel = LocalSystemChannel::bind().unwrap();
    let mut unrelated = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "native_stalled_client"])
        .spawn()
        .unwrap();
    let mut client = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "native_pipe_client"])
        .env("EVM_TEST_PIPE", channel.identifier())
        .spawn()
        .unwrap();
    assert!(
        matches!(channel.exchange_with_child(&unrelated,&request),WriteOutcome::Rejected(reason) if reason.contains("pipe client"))
    );
    assert!(client.wait().unwrap().success());
    unrelated.kill().unwrap();
    unrelated.wait().unwrap();
    let directory = std::env::temp_dir().join(format!(
        "evm-peer-image-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let copy = directory.join("different.exe");
    std::fs::write(
        &copy,
        std::fs::read(std::env::current_exe().unwrap()).unwrap(),
    )
    .unwrap();
    let channel = LocalSystemChannel::bind().unwrap();
    let mut client = std::process::Command::new(&copy)
        .args(["--ignored", "--exact", "native_pipe_client"])
        .env("EVM_TEST_PIPE", channel.identifier())
        .spawn()
        .unwrap();
    assert!(
        matches!(channel.exchange_with_child(&client,&request),WriteOutcome::Rejected(reason) if reason.contains("image"))
    );
    assert!(client.wait().unwrap().success());
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn helper_protocol_rejects_oversized_lengths_invalid_tags_types_names_and_empty_operation() {
    let request = SystemRequest {
        name: "Owned".into(),
        expected: None,
        desired: Some(Variable::text(Scope::System, "Owned", "value", false)),
    };
    let bytes = request.encode().unwrap();
    let mut tag = bytes.clone();
    tag[14] = 2;
    assert!(SystemRequest::decode(&tag).is_err());
    let mut kind = bytes.clone();
    kind[15..19].copy_from_slice(&4u32.to_le_bytes());
    assert!(SystemRequest::decode(&kind).is_err());
    let mut size = bytes.clone();
    size[19..23].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(SystemRequest::decode(&size).is_err());
    let mut name = bytes.clone();
    name[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(SystemRequest::decode(&name).is_err());
    let bad = SystemRequest {
        desired: None,
        ..request.clone()
    };
    assert!(bad.encode().is_err());
    let bad = SystemRequest {
        desired: Some(Variable::text(Scope::System, "Different", "value", false)),
        ..request.clone()
    };
    assert!(bad.encode().is_err());
    let bad = SystemRequest {
        desired: Some(Variable::text(Scope::System, "Owned", "a\0b", false)),
        ..request
    };
    assert!(bad.encode().is_err());
}
