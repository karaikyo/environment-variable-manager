//! Persistent Windows registry adapter. System mutations use the narrow consent helper.
use crate::{RegistryReader, RegistryWriter, Scope, Variable};
use std::ptr::null_mut;
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS},
    System::Registry::*,
};

pub struct WindowsRegistry {
    user_path: String,
    system_path: String,
    system_in_user_hive: bool,
}
impl WindowsRegistry {
    pub fn environment() -> Self {
        Self {
            user_path: "Environment".into(),
            system_path: "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment".into(),
            system_in_user_hive: false,
        }
    }
    /// Isolated current-user locations for integration fixtures.
    pub fn under_current_user(user_path: &str, system_path: &str) -> Self {
        Self {
            user_path: user_path.into(),
            system_path: system_path.into(),
            system_in_user_hive: true,
        }
    }
}
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
fn stamp(key: &Key) -> Result<(u32, u32), String> {
    let mut time = windows_sys::Win32::Foundation::FILETIME::default();
    let code = unsafe {
        RegQueryInfoKeyW(
            key.0,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut time,
        )
    };
    if code == ERROR_SUCCESS {
        Ok((time.dwHighDateTime, time.dwLowDateTime))
    } else {
        Err(format!(
            "Cannot verify scope enumeration (Windows error {code})"
        ))
    }
}
impl RegistryReader for WindowsRegistry {
    fn watcher(&self) -> Option<RegistryWatch> {
        Some(self.watch())
    }
    fn read_scope(&self, scope: Scope) -> Result<Vec<Variable>, String> {
        let (hive, path) = match scope {
            Scope::User => (HKEY_CURRENT_USER, &self.user_path),
            Scope::System => (
                if self.system_in_user_hive {
                    HKEY_CURRENT_USER
                } else {
                    HKEY_LOCAL_MACHINE
                },
                &self.system_path,
            ),
        };
        let path: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let mut handle = null_mut();
        let code = unsafe { RegOpenKeyExW(hive, path.as_ptr(), 0, KEY_QUERY_VALUE, &mut handle) };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(Vec::new());
        }
        if code != ERROR_SUCCESS {
            return Err(format!("Cannot read scope (Windows error {code})"));
        }
        let key = Key(handle);
        let started = stamp(&key)?;
        let mut values = Vec::new();
        let mut index = 0;
        // Names are limited to 16,383 UTF-16 code units by Windows. The extra
        // space also covers a terminator without truncating unnamed values.
        let mut name = vec![0u16; 32768];
        let mut data = vec![0u8; 1024];
        loop {
            let mut name_len = name.len() as u32;
            let mut data_len = data.len() as u32;
            let mut registry_type = 0;
            let code = unsafe {
                RegEnumValueW(
                    key.0,
                    index,
                    name.as_mut_ptr(),
                    &mut name_len,
                    null_mut(),
                    &mut registry_type,
                    data.as_mut_ptr(),
                    &mut data_len,
                )
            };
            match code {
                ERROR_NO_MORE_ITEMS => {
                    if stamp(&key)? != started {
                        return Err("Scope changed during enumeration; last successful values retained. Refresh to reconcile.".into());
                    }
                    return Ok(values);
                }
                ERROR_MORE_DATA => {
                    let required = data_len as usize;
                    if required > crate::MAX_VALUE_BYTES || data.len() == crate::MAX_VALUE_BYTES {
                        return Err("Scope contains an oversized registry value; nothing was treated as deleted".into());
                    }
                    let capacity = required
                        .max(data.len().saturating_mul(2))
                        .min(crate::MAX_VALUE_BYTES);
                    data.resize(capacity, 0);
                    continue;
                }
                ERROR_SUCCESS => {}
                other => return Err(format!("Cannot enumerate scope (Windows error {other})")),
            }
            let name = String::from_utf16(&name[..name_len as usize])
                .map_err(|_| "Scope contains an unrepresentable registry name".to_owned())?;
            values.push(Variable {
                scope,
                name,
                registry_type,
                bytes: data[..data_len as usize].to_vec(),
            });
            index += 1;
        }
    }
}
impl RegistryWriter for WindowsRegistry {
    fn change_system(
        &self,
        name: &str,
        expected: Option<&Variable>,
        desired: Option<&Variable>,
    ) -> crate::WriteOutcome {
        // Isolated fixtures must never redirect the production privileged writer.
        if self.system_in_user_hive {
            return crate::WriteOutcome::Rejected(
                "Isolated registry adapters do not launch elevation".into(),
            );
        }
        crate::elevation::change_system(name, expected, desired)
    }

    fn delete_user(&self, name: &str) -> Result<(), String> {
        if !crate::valid_name(name) {
            return Err("Invalid user variable name".into());
        }
        let path: Vec<u16> = self.user_path.encode_utf16().chain(Some(0)).collect();
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let mut handle = null_mut();
        let code = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut handle,
            )
        };
        if code != ERROR_SUCCESS {
            return Err(format!(
                "Cannot open user scope for deletion (Windows error {code})"
            ));
        }
        let key = Key(handle);
        let code = unsafe { RegDeleteValueW(key.0, name.as_ptr()) };
        if code != ERROR_SUCCESS {
            return Err(format!("Delete failed (Windows error {code})"));
        }
        self.broadcast_change();
        Ok(())
    }
    fn write_user(&self, value: &Variable) -> Result<(), String> {
        if value.scope != Scope::User || value.read_only() {
            return Err("Only supported user values can be saved".into());
        }
        let path: Vec<u16> = self.user_path.encode_utf16().chain(Some(0)).collect();
        let name: Vec<u16> = value.name.encode_utf16().chain(Some(0)).collect();
        let mut handle = null_mut();
        let code = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut handle,
                null_mut(),
            )
        };
        if code != ERROR_SUCCESS {
            return Err(format!(
                "Cannot open user scope for Save (Windows error {code})"
            ));
        }
        let key = Key(handle);
        let code = unsafe {
            RegSetValueExW(
                key.0,
                name.as_ptr(),
                0,
                value.registry_type,
                value.bytes.as_ptr(),
                value.bytes.len() as u32,
            )
        };
        if code != ERROR_SUCCESS {
            return Err(format!("Save failed (Windows error {code})"));
        }
        self.broadcast_change();
        Ok(())
    }
}

impl WindowsRegistry {
    fn broadcast_change(&self) {
        if !self.system_in_user_hive {
            let environment: Vec<u16> = "Environment".encode_utf16().chain(Some(0)).collect();
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::SendMessageTimeoutW(
                    windows_sys::Win32::UI::WindowsAndMessaging::HWND_BROADCAST,
                    windows_sys::Win32::UI::WindowsAndMessaging::WM_SETTINGCHANGE,
                    0,
                    environment.as_ptr() as isize,
                    windows_sys::Win32::UI::WindowsAndMessaging::SMTO_ABORTIFHUNG,
                    1000,
                    null_mut(),
                );
            }
        }
    }
}

/// Latest event/health state. Events are coalesced hints, never a write count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchStatus {
    pub changed: bool,
    pub gap: bool,
    pub errors: Vec<(Scope, String)>,
}
struct WatchSlot {
    scope: Scope,
    hive: HKEY,
    path: String,
    key: Option<Key>,
    event: windows_sys::Win32::Foundation::HANDLE,
    error: Option<String>,
    retry: std::time::Instant,
}
impl Drop for WatchSlot {
    fn drop(&mut self) {
        self.key.take();
        if !self.event.is_null() {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.event);
            }
        }
    }
}
impl WatchSlot {
    fn arm(&mut self) -> Result<(), String> {
        let code = unsafe {
            RegNotifyChangeKeyValue(
                self.key.as_ref().unwrap().0,
                1,
                REG_NOTIFY_CHANGE_LAST_SET | REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_THREAD_AGNOSTIC,
                self.event,
                1,
            )
        };
        if code == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!("Monitoring interrupted (Windows error {code})"))
        }
    }
    fn connect(&mut self) -> Result<(), String> {
        use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent};
        if self.event.is_null() {
            self.event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        }
        if self.event.is_null() {
            return Err("Cannot create registry notification event".into());
        }
        unsafe {
            ResetEvent(self.event);
        }
        // Watch the nearest existing ancestor if a scope key is currently absent.
        // This detects future key creation without creating it or requiring write access.
        let mut path = self.path.clone();
        loop {
            let name: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
            let mut key = null_mut();
            let code = unsafe { RegOpenKeyExW(self.hive, name.as_ptr(), 0, KEY_NOTIFY, &mut key) };
            if code == ERROR_SUCCESS {
                self.key = Some(Key(key));
                return self.arm();
            }
            if code != ERROR_FILE_NOT_FOUND {
                return Err(format!("Cannot monitor scope (Windows error {code})"));
            }
            match path.rfind('\\') {
                Some(index) => path.truncate(index),
                None if !path.is_empty() => path.clear(),
                None => return Err("Cannot monitor missing scope".into()),
            }
        }
    }
}
/// Owns read-only asynchronous registry registrations for both persistent scopes.
/// Create and poll on one worker thread; closing it releases all keys and events.
pub struct RegistryWatch {
    slots: Vec<WatchSlot>,
}
impl RegistryWatch {
    /// Reopen handles and registrations after a power transition; even unchanged
    /// values must be compared with the persisted baseline across this gap.
    pub fn resume(&mut self) -> WatchStatus {
        for slot in &mut self.slots {
            slot.key.take();
            if !slot.event.is_null() {
                unsafe {
                    windows_sys::Win32::Foundation::CloseHandle(slot.event);
                }
                slot.event = null_mut();
            }
            slot.retry = std::time::Instant::now();
        }
        let mut status = self.poll(std::time::Duration::ZERO);
        status.changed = true;
        status.gap = true;
        status
    }
    pub fn poll(&mut self, timeout: std::time::Duration) -> WatchStatus {
        use windows_sys::Win32::{
            Foundation::{WAIT_FAILED, WAIT_OBJECT_0},
            System::Threading::{ResetEvent, WaitForMultipleObjects, WaitForSingleObject},
        };
        let mut result = WatchStatus {
            changed: false,
            gap: false,
            errors: Vec::new(),
        };
        for slot in &mut self.slots {
            if slot.key.is_none() && std::time::Instant::now() >= slot.retry {
                let recovering = slot.error.is_some();
                match slot.connect() {
                    Ok(()) => {
                        slot.error = None;
                        if recovering {
                            result.changed = true;
                            result.gap = true;
                        }
                    }
                    Err(error) => {
                        slot.key.take();
                        slot.error = Some(error);
                        slot.retry = std::time::Instant::now() + std::time::Duration::from_secs(1);
                    }
                }
            }
        }
        let handles: Vec<_> = self
            .slots
            .iter()
            .filter(|s| s.key.is_some())
            .map(|s| s.event)
            .collect();
        if !handles.is_empty() && !timeout.is_zero() {
            unsafe {
                WaitForMultipleObjects(
                    handles.len() as u32,
                    handles.as_ptr(),
                    0,
                    timeout.as_millis().min(u32::MAX as u128 - 1) as u32,
                );
            }
        }
        for slot in &mut self.slots {
            if slot.key.is_some() {
                let code = unsafe { WaitForSingleObject(slot.event, 0) };
                if code == WAIT_OBJECT_0 {
                    // Rearm before the caller enumerates to retain a concurrent signal.
                    unsafe {
                        ResetEvent(slot.event);
                    }
                    result.changed = true;
                    if let Err(error) = slot.arm() {
                        slot.key.take();
                        slot.error = Some(error);
                        result.gap = true;
                    }
                } else if code == WAIT_FAILED {
                    slot.key.take();
                    slot.error = Some("Registry notification wait failed".into());
                    result.gap = true;
                }
            }
            if let Some(error) = &slot.error {
                result.errors.push((slot.scope, error.clone()));
            }
        }
        result
    }
}
impl WindowsRegistry {
    pub fn watch(&self) -> RegistryWatch {
        RegistryWatch {
            slots: [Scope::User, Scope::System]
                .into_iter()
                .map(|scope| WatchSlot {
                    scope,
                    hive: if scope == Scope::User || self.system_in_user_hive {
                        HKEY_CURRENT_USER
                    } else {
                        HKEY_LOCAL_MACHINE
                    },
                    path: if scope == Scope::User {
                        self.user_path.clone()
                    } else {
                        self.system_path.clone()
                    },
                    key: None,
                    event: null_mut(),
                    error: None,
                    retry: std::time::Instant::now(),
                })
                .collect(),
        }
    }
}
