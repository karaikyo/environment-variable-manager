use environment_variable_manager::{
    windows_registry::WindowsRegistry, Application, RegistryReader, Scope, Variable,
};
use std::ptr::{null, null_mut};
use windows_sys::Win32::{Foundation::ERROR_SUCCESS, System::Registry::*};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

#[test]
fn native_registry_reads_exact_raw_types_empty_names_and_mixed_case_identity() {
    let path = format!(
        "Software\\EnvironmentManagerTests\\{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let mut key = null_mut();
    let mut disposition = 0;
    unsafe {
        assert_eq!(
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(&path).as_ptr(),
                0,
                null(),
                0,
                KEY_READ | KEY_WRITE,
                null(),
                &mut key,
                &mut disposition
            ),
            ERROR_SUCCESS
        );
        assert_eq!(
            disposition, REG_CREATED_NEW_KEY,
            "Never overwrite a pre-existing fixture"
        );
        let values = [
            Variable::text(Scope::User, "", "", false),
            Variable::text(Scope::User, "mIxEd", "one\nline;;dup;dup;", true),
            Variable {
                scope: Scope::User,
                name: "Binary".into(),
                registry_type: REG_BINARY,
                bytes: vec![0, 255, 1],
            },
        ];
        for value in &values {
            assert_eq!(
                RegSetValueExW(
                    key,
                    wide(&value.name).as_ptr(),
                    0,
                    value.registry_type,
                    value.bytes.as_ptr(),
                    value.bytes.len() as u32
                ),
                ERROR_SUCCESS
            );
        }
        RegCloseKey(key);
        let reader = WindowsRegistry::under_current_user(&path, &path);
        let actual = reader.read_scope(Scope::User).unwrap();
        for expected in &values {
            assert!(actual.contains(expected), "Raw fixture differs");
        }
        assert_eq!(actual.len(), 3);
        assert_eq!(
            actual
                .iter()
                .find(|v| v.name == "mIxEd")
                .unwrap()
                .entries()
                .unwrap(),
            vec!["one\nline", "", "dup", "dup", ""]
        );
        assert!(actual
            .iter()
            .find(|v| v.name == "Binary")
            .unwrap()
            .read_only());
        let directory = std::env::temp_dir().join(format!(
            "evm-native-save-{}",
            path.rsplit('\\').next().unwrap()
        ));
        assert!(!directory.exists());
        let mut app = Application::open(&reader, &directory);
        app.edit_entries(
            Scope::User,
            "mixed",
            vec!["".into(), "%TEMP%".into(), "line\nline".into(), "".into()],
        )
        .unwrap();
        app.save(&reader, Scope::User, "mixed").unwrap();
        let changed = Variable::text(Scope::User, "mIxEd", ";%TEMP%;line\nline;", true);
        let current = reader.read_scope(Scope::User).unwrap();
        assert!(current.contains(&changed));
        assert!(current.contains(&values[0]) && current.contains(&values[2]));
        assert_eq!(
            current.len(),
            3,
            "Verify entire owned subtree before cleanup"
        );
        assert_eq!(
            app.history(Some((Scope::User, "MIXED")), 0, 10).unwrap()[0].after,
            Some(changed)
        );
        app.create_user("CreatedEmpty").unwrap();
        app.save(&reader, Scope::User, "createdempty").unwrap();
        assert_eq!(
            reader
                .read_scope(Scope::User)
                .unwrap()
                .iter()
                .find(|v| v.name == "CreatedEmpty")
                .unwrap(),
            &Variable::text(Scope::User, "CreatedEmpty", "", false)
        );
        app.request_delete(Scope::User, "CREATEDempty").unwrap();
        app.cancel_delete();
        assert_eq!(reader.read_scope(Scope::User).unwrap().len(), 4);
        app.request_delete(Scope::User, "createdempty").unwrap();
        app.confirm_delete(&reader).unwrap();
        let reopened = Application::open(&reader, &directory);
        let history = reopened
            .history(Some((Scope::User, "createdempty")), 0, 10)
            .unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].after, None);
        assert_eq!(history[1].before, None);
        let final_values = reader.read_scope(Scope::User).unwrap();
        assert_eq!(
            final_values, current,
            "Only the owned new value was created/deleted"
        );
        std::fs::remove_dir_all(directory).unwrap();
        // The subtree was newly created by this run, and its entire content was verified above.
        assert_eq!(
            RegDeleteTreeW(HKEY_CURRENT_USER, wide(&path).as_ptr()),
            ERROR_SUCCESS
        );
    }
}

#[test]
fn native_notifications_observe_both_scopes_and_rearm_without_broadcasts() {
    use std::time::Duration;
    let suffix = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let paths = [
        format!("Software\\EnvironmentManagerTests\\{suffix}\\User"),
        format!("Software\\EnvironmentManagerTests\\{suffix}\\System"),
    ];
    let mut keys = [null_mut(); 2];
    unsafe {
        for i in 0..2 {
            let mut disposition = 0;
            assert_eq!(
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    wide(&paths[i]).as_ptr(),
                    0,
                    null(),
                    0,
                    KEY_READ | KEY_WRITE,
                    null(),
                    &mut keys[i],
                    &mut disposition
                ),
                ERROR_SUCCESS
            );
            assert_eq!(disposition, REG_CREATED_NEW_KEY);
        }
        let registry = WindowsRegistry::under_current_user(&paths[0], &paths[1]);
        let mut watch = registry.watch();
        assert!(watch.poll(Duration::ZERO).errors.is_empty());
        for (i, scope) in [Scope::User, Scope::System].into_iter().enumerate() {
            for raw in ["one", "", "last"] {
                let value = Variable::text(scope, "Owned", raw, true);
                assert_eq!(
                    RegSetValueExW(
                        keys[i],
                        wide("Owned").as_ptr(),
                        0,
                        value.registry_type,
                        value.bytes.as_ptr(),
                        value.bytes.len() as u32
                    ),
                    ERROR_SUCCESS
                );
                let status = watch.poll(Duration::from_secs(2));
                assert!(status.changed && status.errors.is_empty());
                assert_eq!(registry.read_scope(scope).unwrap(), vec![value]);
            }
        }
        assert!(!watch.poll(Duration::ZERO).changed);
        drop(watch);
        for (i, scope) in [Scope::User, Scope::System].into_iter().enumerate() {
            assert_eq!(
                registry.read_scope(scope).unwrap(),
                vec![Variable::text(scope, "Owned", "last", true)]
            );
            RegCloseKey(keys[i]);
            assert_eq!(
                RegDeleteTreeW(HKEY_CURRENT_USER, wide(&paths[i]).as_ptr()),
                ERROR_SUCCESS
            );
        }
        let parent = format!("Software\\EnvironmentManagerTests\\{suffix}");
        assert_eq!(
            RegDeleteKeyW(HKEY_CURRENT_USER, wide(&parent).as_ptr()),
            ERROR_SUCCESS
        );
    }
}

#[test]
fn missing_scope_is_confirmed_absence_and_ancestor_watch_detects_creation() {
    use std::time::Duration;
    let suffix = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let parent = format!("Software\\EnvironmentManagerTests\\missing-{suffix}");
    let child = format!("{parent}\\Environment");
    unsafe {
        let mut key = null_mut();
        let mut disposition = 0;
        assert_eq!(
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(&parent).as_ptr(),
                0,
                null(),
                0,
                KEY_READ | KEY_WRITE,
                null(),
                &mut key,
                &mut disposition
            ),
            ERROR_SUCCESS
        );
        assert_eq!(disposition, REG_CREATED_NEW_KEY);
        RegCloseKey(key);
        let registry = WindowsRegistry::under_current_user(&child, &child);
        assert!(registry.read_scope(Scope::User).unwrap().is_empty());
        let mut watch = registry.watch();
        assert!(watch.poll(Duration::ZERO).errors.is_empty());
        assert_eq!(
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(&child).as_ptr(),
                0,
                null(),
                0,
                KEY_READ | KEY_WRITE,
                null(),
                &mut key,
                &mut disposition
            ),
            ERROR_SUCCESS
        );
        assert_eq!(disposition, REG_CREATED_NEW_KEY);
        let value = Variable::text(Scope::User, "Owned", "created", false);
        assert_eq!(
            RegSetValueExW(
                key,
                wide("Owned").as_ptr(),
                0,
                value.registry_type,
                value.bytes.as_ptr(),
                value.bytes.len() as u32
            ),
            ERROR_SUCCESS
        );
        RegCloseKey(key);
        assert!(watch.poll(Duration::from_secs(2)).changed);
        assert_eq!(registry.read_scope(Scope::User).unwrap(), vec![value]);
        drop(watch);
        assert_eq!(
            RegDeleteTreeW(HKEY_CURRENT_USER, wide(&parent).as_ptr()),
            ERROR_SUCCESS
        );
    }
}

#[test]
fn native_reader_accepts_successive_large_supported_values_and_rejects_invalid_writes() {
    use environment_variable_manager::RegistryWriter;
    let path = format!(
        "Software\\EnvironmentManagerTests\\large-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let mut key = null_mut();
    let mut disposition = 0;
    unsafe {
        assert_eq!(
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(&path).as_ptr(),
                0,
                null(),
                0,
                KEY_READ | KEY_WRITE,
                null(),
                &mut key,
                &mut disposition
            ),
            ERROR_SUCCESS
        );
        assert_eq!(disposition, REG_CREATED_NEW_KEY);
        let values = [
            Variable {
                scope: Scope::User,
                name: "First".into(),
                registry_type: REG_BINARY,
                bytes: vec![17; 9 * 1024 * 1024],
            },
            Variable {
                scope: Scope::User,
                name: "Second".into(),
                registry_type: REG_BINARY,
                bytes: vec![29; 10 * 1024 * 1024],
            },
            Variable::text(Scope::User, "A=B", "preserve invalid name", false),
        ];
        for value in &values {
            assert_eq!(
                RegSetValueExW(
                    key,
                    wide(&value.name).as_ptr(),
                    0,
                    value.registry_type,
                    value.bytes.as_ptr(),
                    value.bytes.len() as u32
                ),
                ERROR_SUCCESS
            );
        }
        let reader = WindowsRegistry::under_current_user(&path, &path);
        let read = reader.read_scope(Scope::User);
        let invalid_write =
            reader.write_user(&Variable::text(Scope::User, "A=B", "changed", false));
        let invalid_delete = reader.delete_user("A=B");
        let oversized_write = reader.write_user(&Variable::text(
            Scope::User,
            "Oversized",
            &"x".repeat(8_388_608),
            false,
        ));
        // Verify exact ownership via raw OS reads even when the adapter under test fails.
        let mut count = 0;
        assert_eq!(
            RegQueryInfoKeyW(
                key,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut count,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut()
            ),
            ERROR_SUCCESS
        );
        assert_eq!(count, 3);
        for value in &values {
            let mut bytes = vec![0; value.bytes.len()];
            let mut length = bytes.len() as u32;
            let mut kind = 0;
            assert_eq!(
                RegQueryValueExW(
                    key,
                    wide(&value.name).as_ptr(),
                    null(),
                    &mut kind,
                    bytes.as_mut_ptr(),
                    &mut length
                ),
                ERROR_SUCCESS
            );
            assert_eq!((kind, bytes), (value.registry_type, value.bytes.clone()));
        }
        RegCloseKey(key);
        assert_eq!(
            RegDeleteTreeW(HKEY_CURRENT_USER, wide(&path).as_ptr()),
            ERROR_SUCCESS
        );
        assert!(invalid_write.is_err() && invalid_delete.is_err() && oversized_write.is_err());
        let read = read.expect("both values are below the 16 MiB raw limit");
        assert_eq!(read.len(), values.len());
        for value in &values {
            assert!(read.contains(value));
        }
        assert!(read
            .iter()
            .find(|value| value.name == "A=B")
            .unwrap()
            .read_only());
    }
}
