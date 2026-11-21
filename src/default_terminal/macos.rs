//! macOS has no default terminal, only default handlers for script files;
//! this registers Zetta for the extensions Terminal.app opens.

use std::path::{Path, PathBuf};

use anyhow::Result;

const MACOS_SCRIPT_EXTENSIONS: &[&str] = &["command", "tool", "zsh", "csh", "sh", "pl"];

pub(crate) fn accepted_script_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            MACOS_SCRIPT_EXTENSIONS
                .iter()
                .any(|accepted| extension.eq_ignore_ascii_case(accepted))
        })
}

pub(crate) fn script_paths_from_urls(urls: &[String]) -> Vec<PathBuf> {
    urls.iter()
        .filter_map(|url| {
            let url = url::Url::parse(url).ok()?;
            (url.scheme() == "file").then_some(())?;
            let path = url.to_file_path().ok()?;
            accepted_script_extension(&path).then_some(path)
        })
        .collect()
}

pub(super) fn register_macos_script_handlers() -> Result<()> {
    use std::{
        ffi::CString,
        os::raw::{c_char, c_void},
    };

    type CfString = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    const ALL_ROLES: u32 = 0xFFFF_FFFF;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            allocator: *const c_void,
            string: *const c_char,
            encoding: u32,
        ) -> CfString;
        fn CFRelease(value: CfString);
    }
    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn UTTypeCreatePreferredIdentifierForTag(
            tag_class: CfString,
            tag: CfString,
            conforming_to: CfString,
        ) -> CfString;
        fn LSSetDefaultRoleHandlerForContentType(
            content_type: CfString,
            role_handler: u32,
            bundle_identifier: CfString,
        ) -> i32;
    }

    let tag_class = CString::new("public.filename-extension")?;
    let bundle = CString::new("com.zetta.Zetta")?;
    // SAFETY: all C strings remain alive until their corresponding Core
    // Foundation objects have been created, and each returned object is
    // released on every path below.
    let tag_class =
        unsafe { CFStringCreateWithCString(std::ptr::null(), tag_class.as_ptr(), UTF8) };
    let bundle = unsafe { CFStringCreateWithCString(std::ptr::null(), bundle.as_ptr(), UTF8) };
    anyhow::ensure!(
        !tag_class.is_null() && !bundle.is_null(),
        "could not create Launch Services strings"
    );
    let mut failure = None;
    for extension in MACOS_SCRIPT_EXTENSIONS {
        let extension = CString::new(*extension)?;
        let tag = unsafe { CFStringCreateWithCString(std::ptr::null(), extension.as_ptr(), UTF8) };
        if tag.is_null() {
            failure = Some(format!(
                "could not create Launch Services tag for .{extension:?}"
            ));
            continue;
        }
        let content_type =
            unsafe { UTTypeCreatePreferredIdentifierForTag(tag_class, tag, std::ptr::null()) };
        let status = if content_type.is_null() {
            -1
        } else {
            unsafe { LSSetDefaultRoleHandlerForContentType(content_type, ALL_ROLES, bundle) }
        };
        if !content_type.is_null() {
            unsafe { CFRelease(content_type) };
        }
        unsafe { CFRelease(tag) };
        if status != 0 && failure.is_none() {
            failure = Some(format!("Launch Services returned OSStatus {status}"));
        }
    }
    unsafe {
        CFRelease(tag_class);
        CFRelease(bundle);
    }
    if let Some(failure) = failure {
        anyhow::bail!(failure)
    }
    Ok(())
}

pub(super) fn macos_script_handlers_are_zetta() -> bool {
    use std::{
        ffi::CString,
        os::raw::{c_char, c_void},
    };

    type CfString = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    const ALL_ROLES: u32 = 0xFFFF_FFFF;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            allocator: *const c_void,
            string: *const c_char,
            encoding: u32,
        ) -> CfString;
        fn CFEqual(left: CfString, right: CfString) -> u8;
        fn CFRelease(value: CfString);
    }
    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn UTTypeCreatePreferredIdentifierForTag(
            tag_class: CfString,
            tag: CfString,
            conforming_to: CfString,
        ) -> CfString;
        fn LSCopyDefaultRoleHandlerForContentType(
            content_type: CfString,
            role_handler: u32,
        ) -> CfString;
    }

    let Ok(tag_class) = CString::new("public.filename-extension") else {
        return false;
    };
    let Ok(bundle) = CString::new("com.zetta.Zetta") else {
        return false;
    };
    // SAFETY: the C strings remain alive until their corresponding Core
    // Foundation objects have been created. Every non-null returned object is
    // released before this function returns.
    let tag_class =
        unsafe { CFStringCreateWithCString(std::ptr::null(), tag_class.as_ptr(), UTF8) };
    let bundle = unsafe { CFStringCreateWithCString(std::ptr::null(), bundle.as_ptr(), UTF8) };
    if tag_class.is_null() || bundle.is_null() {
        if !tag_class.is_null() {
            unsafe { CFRelease(tag_class) };
        }
        if !bundle.is_null() {
            unsafe { CFRelease(bundle) };
        }
        return false;
    }

    let mut all_registered = true;
    for extension in MACOS_SCRIPT_EXTENSIONS {
        let Ok(extension) = CString::new(*extension) else {
            all_registered = false;
            continue;
        };
        let tag = unsafe { CFStringCreateWithCString(std::ptr::null(), extension.as_ptr(), UTF8) };
        if tag.is_null() {
            all_registered = false;
            continue;
        }
        let content_type =
            unsafe { UTTypeCreatePreferredIdentifierForTag(tag_class, tag, std::ptr::null()) };
        let handler = if content_type.is_null() {
            std::ptr::null()
        } else {
            unsafe { LSCopyDefaultRoleHandlerForContentType(content_type, ALL_ROLES) }
        };
        if handler.is_null() || unsafe { CFEqual(handler, bundle) } == 0 {
            all_registered = false;
        }
        if !handler.is_null() {
            unsafe { CFRelease(handler) };
        }
        if !content_type.is_null() {
            unsafe { CFRelease(content_type) };
        }
        unsafe { CFRelease(tag) };
    }
    unsafe {
        CFRelease(tag_class);
        CFRelease(bundle);
    }
    all_registered
}

#[cfg(test)]
#[path = "../tests/default_terminal/macos.rs"]
mod tests;
