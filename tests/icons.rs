//! Verify the packaged executable through Windows resource and Explorer icon APIs.
use std::{mem::size_of, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::*,
    UI::{Shell::ExtractIconExW, WindowsAndMessaging::*},
};
#[test]
fn executable_exposes_only_icon_resources_and_explorer_can_extract_them() {
    let path: Vec<u16> = env!("CARGO_BIN_EXE_environment-variable-manager")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe extern "system" fn resource_type(_: HMODULE, kind: *const u16, parameter: isize) -> i32 {
        let kinds = &mut *(parameter as *mut Vec<usize>);
        kinds.push(kind as usize);
        1
    }
    unsafe {
        // Data-only loading must never execute the application or register startup.
        let module = LoadLibraryExW(
            path.as_ptr(),
            null_mut(),
            LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
        );
        assert!(!module.is_null());
        let mut kinds = Vec::<usize>::new();
        assert_ne!(
            EnumResourceTypesW(module, Some(resource_type), &mut kinds as *mut _ as isize),
            0
        );
        kinds.sort();
        assert_eq!(
            kinds,
            vec![3, 14],
            "Embed icons only, without an activation-context manifest"
        );
        let group = FindResourceW(module, 1usize as *const u16, 14usize as *const u16);
        assert!(!group.is_null());
        let loaded = LoadResource(module, group);
        let bytes = std::slice::from_raw_parts(
            LockResource(loaded) as *const u8,
            SizeofResource(module, group) as usize,
        );
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 7);
        let dimensions: Vec<_> = bytes[6..]
            .chunks_exact(14)
            .map(|entry| if entry[0] == 0 { 256 } else { entry[0] as u16 })
            .collect();
        assert_eq!(dimensions, vec![16, 24, 32, 48, 64, 128, 256]);
        FreeLibrary(module);
        let (mut large, mut small) = (null_mut(), null_mut());
        assert!(ExtractIconExW(path.as_ptr(), 0, &mut large, &mut small, 1) > 0);
        for icon in [large, small] {
            assert!(!icon.is_null());
            let mut info = ICONINFO::default();
            assert_ne!(GetIconInfo(icon, &mut info), 0);
            let mut bitmap = BITMAP::default();
            assert_ne!(
                GetObjectW(
                    info.hbmColor,
                    size_of::<BITMAP>() as i32,
                    &mut bitmap as *mut _ as *mut _
                ),
                0
            );
            assert!(bitmap.bmWidth >= 16 && bitmap.bmHeight >= 16);
            DeleteObject(info.hbmColor);
            if !info.hbmMask.is_null() {
                DeleteObject(info.hbmMask);
            }
            DestroyIcon(icon);
        }
    }
}
