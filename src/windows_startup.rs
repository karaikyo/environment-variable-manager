//! Current-user Run registration. No StartupApproved or policy values are written.
use crate::startup::{RegistrationState, StartupRegistration};
use std::ptr::{null, null_mut};
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
    System::Registry::*,
};
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
pub struct WindowsStartup {
    path: String,
    name: String,
    production: bool,
}
impl WindowsStartup {
    pub fn current_user() -> Self {
        Self {
            path: "Software\\Microsoft\\Windows\\CurrentVersion\\Run".into(),
            name: "EnvironmentVariableManager".into(),
            production: true,
        }
    }
    /// Fresh fixture keys only; does not write the real sign-in registration.
    /// Policy/approval reads are redirected beneath path/Policies and
    /// path/StartupApproved, allowing owned fixtures to exercise the real parser.
    pub fn under_current_user(path: &str, name: &str) -> Self {
        Self {
            path: path.into(),
            name: name.into(),
            production: false,
        }
    }
    fn blocked(&self) -> Result<bool, String> {
        // This app registers HKCU Run. The documented user legacy-Run policy
        // governs that list; NoRun only removes Run-dialog/menu entry points.
        let policy = if self.production {
            "Software\\Microsoft\\Windows\\CurrentVersion\\Policies\\Explorer".into()
        } else {
            format!("{}\\Policies", self.path)
        };
        if let Some((kind, bytes)) = read(HKEY_CURRENT_USER, &policy, "DisableCurrentUserRun")? {
            if kind != REG_DWORD || bytes.as_slice() != [0, 0, 0, 0] {
                return Ok(true);
            }
        }
        let approvals =
            if self.production {
                vec![
            "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run".into(),
            "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run32".into(),
        ]
            } else {
                vec![format!("{}\\StartupApproved", self.path)]
            };
        for path in approvals {
            if let Some((kind, bytes)) = read(HKEY_CURRENT_USER, &path, &self.name)? {
                if kind != REG_BINARY || !matches!(bytes.first(), Some(2 | 6)) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}
fn read(hive: HKEY, path: &str, name: &str) -> Result<Option<(u32, Vec<u8>)>, String> {
    let mut key = null_mut();
    let code = unsafe { RegOpenKeyExW(hive, wide(path).as_ptr(), 0, KEY_QUERY_VALUE, &mut key) };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if code != ERROR_SUCCESS {
        return Err(format!(
            "Cannot inspect startup registration (Windows error {code})"
        ));
    }
    let key = Key(key);
    let mut kind = 0;
    let mut size = 0;
    let name = wide(name);
    let code = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            null(),
            &mut kind,
            null_mut(),
            &mut size,
        )
    };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if code != ERROR_SUCCESS || size > 32768 {
        return Err("Cannot inspect startup value safely".into());
    }
    let mut bytes = vec![0; size as usize];
    let code = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            null(),
            &mut kind,
            bytes.as_mut_ptr(),
            &mut size,
        )
    };
    if code != ERROR_SUCCESS {
        return Err(format!(
            "Cannot inspect startup value (Windows error {code})"
        ));
    }
    bytes.truncate(size as usize);
    Ok(Some((kind, bytes)))
}
impl StartupRegistration for WindowsStartup {
    fn state(&self) -> Result<RegistrationState, String> {
        let command = match read(HKEY_CURRENT_USER, &self.path, &self.name)? {
            None => None,
            Some((kind, bytes)) => {
                if kind != REG_SZ || bytes.len() % 2 != 0 {
                    return Err("Existing startup value has an unsupported type; preserved".into());
                }
                let mut units = bytes
                    .chunks_exact(2)
                    .map(|b| u16::from_le_bytes([b[0], b[1]]))
                    .collect::<Vec<_>>();
                if units.pop() != Some(0) || units.contains(&0) {
                    return Err("Existing startup value is malformed; preserved".into());
                }
                Some(
                    String::from_utf16(&units)
                        .map_err(|_| "Existing startup value is malformed; preserved")?,
                )
            }
        };
        Ok(RegistrationState {
            command,
            blocked: self.blocked()?,
        })
    }
    fn replace(&self, expected: Option<&str>, desired: Option<&str>) -> Result<(), String> {
        let state = self.state()?;
        if state.command.as_deref() != expected {
            return Err("Startup entry changed; existing entry preserved".into());
        }
        if desired.is_some() && state.blocked {
            return Err("Startup blocked by Windows settings or policy".into());
        }
        if desired.is_none() && state.command.is_none() {
            return Ok(());
        }
        let mut key = null_mut();
        let code = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(&self.path).as_ptr(),
                0,
                null(),
                0,
                KEY_SET_VALUE,
                null(),
                &mut key,
                null_mut(),
            )
        };
        if code != ERROR_SUCCESS {
            return Err(format!(
                "Cannot change own startup entry (Windows error {code})"
            ));
        }
        let key = Key(key);
        let code = if let Some(command) = desired {
            let bytes = wide(command)
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>();
            unsafe {
                RegSetValueExW(
                    key.0,
                    wide(&self.name).as_ptr(),
                    0,
                    REG_SZ,
                    bytes.as_ptr(),
                    bytes.len() as u32,
                )
            }
        } else {
            unsafe { RegDeleteValueW(key.0, wide(&self.name).as_ptr()) }
        };
        if code != ERROR_SUCCESS {
            return Err(format!(
                "Cannot change own startup entry (Windows error {code})"
            ));
        }
        Ok(())
    }
}
