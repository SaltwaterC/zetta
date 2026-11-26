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
    let Some((tag_class, bundle)) = launch_services_strings() else {
        anyhow::bail!("could not create Launch Services strings");
    };
    let mut failure = None;
    for extension in MACOS_SCRIPT_EXTENSIONS {
        let Some(content_type) = content_type_for(&tag_class, extension) else {
            failure.get_or_insert_with(|| {
                format!("could not create a Launch Services type for .{extension}")
            });
            continue;
        };
        // SAFETY: `content_type` and `bundle` are live CFStrings; Launch
        // Services only reads them.
        let status = unsafe {
            cf::LSSetDefaultRoleHandlerForContentType(content_type.0, cf::ALL_ROLES, bundle.0)
        };
        if status != 0 {
            failure.get_or_insert_with(|| format!("Launch Services returned OSStatus {status}"));
        }
    }
    if let Some(failure) = failure {
        anyhow::bail!(failure)
    }
    Ok(())
}

pub(super) fn macos_script_handlers_are_zetta() -> bool {
    let Some((tag_class, bundle)) = launch_services_strings() else {
        return false;
    };
    MACOS_SCRIPT_EXTENSIONS.iter().all(|extension| {
        let Some(content_type) = content_type_for(&tag_class, extension) else {
            return false;
        };
        // SAFETY: `content_type` is a live CFString, and the handler returned
        // follows the Copy rule, so the `CfOwned` is its only owner.
        let handler = unsafe {
            cf::CfOwned::new(cf::LSCopyDefaultRoleHandlerForContentType(
                content_type.0,
                cf::ALL_ROLES,
            ))
        };
        // SAFETY: `handler` and `bundle` are live CFStrings.
        handler.is_some_and(|handler| unsafe { cf::CFEqual(handler.0, bundle.0) } != 0)
    })
}

/// The tag class every extension is looked up under, and Zetta's bundle
/// identifier.
fn launch_services_strings() -> Option<(cf::CfOwned, cf::CfOwned)> {
    cf::CfOwned::string("public.filename-extension").zip(cf::CfOwned::string("com.zetta.Zetta"))
}

/// The uniform type identifier Launch Services prefers for `extension`.
fn content_type_for(tag_class: &cf::CfOwned, extension: &str) -> Option<cf::CfOwned> {
    let tag = cf::CfOwned::string(extension)?;
    // SAFETY: `tag_class` and `tag` are live CFStrings, the conforming-to type
    // is optional, and the identifier returned follows the Create rule, so the
    // `CfOwned` is its only owner.
    unsafe {
        cf::CfOwned::new(cf::UTTypeCreatePreferredIdentifierForTag(
            tag_class.0,
            tag.0,
            std::ptr::null(),
        ))
    }
}

/// The Core Foundation and Launch Services calls this module makes, and the
/// owner that releases what they return.
mod cf {
    use std::ffi::c_void;

    pub(super) type CfString = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    pub(super) const ALL_ROLES: u32 = 0xFFFF_FFFF;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithBytes(
            allocator: *const c_void,
            bytes: *const u8,
            length: isize,
            encoding: u32,
            external_representation: u8,
        ) -> CfString;
        pub(super) fn CFEqual(left: CfString, right: CfString) -> u8;
        fn CFRelease(value: CfString);
    }
    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        pub(super) fn UTTypeCreatePreferredIdentifierForTag(
            tag_class: CfString,
            tag: CfString,
            conforming_to: CfString,
        ) -> CfString;
        pub(super) fn LSSetDefaultRoleHandlerForContentType(
            content_type: CfString,
            role_handler: u32,
            bundle_identifier: CfString,
        ) -> i32;
        pub(super) fn LSCopyDefaultRoleHandlerForContentType(
            content_type: CfString,
            role_handler: u32,
        ) -> CfString;
    }

    /// A Core Foundation object returned under the Create or Copy rule, so
    /// owned by the caller: released exactly once, on every path, when this
    /// is dropped.
    pub(super) struct CfOwned(pub(super) CfString);

    impl CfOwned {
        /// Takes ownership of `value`; `None` when the call that returned it
        /// failed.
        ///
        /// # Safety
        ///
        /// `value` is null or a Core Foundation object returned under the
        /// Create or Copy rule that nothing else releases.
        pub(super) unsafe fn new(value: CfString) -> Option<Self> {
            (!value.is_null()).then_some(Self(value))
        }

        pub(super) fn string(text: &str) -> Option<Self> {
            let length = isize::try_from(text.len()).ok()?;
            // SAFETY: `text` is `length` readable bytes of UTF-8, which the
            // call copies, with no NUL terminator needed; the string it
            // returns follows the Create rule and nothing else holds it.
            unsafe {
                Self::new(CFStringCreateWithBytes(
                    std::ptr::null(),
                    text.as_ptr(),
                    length,
                    UTF8,
                    0,
                ))
            }
        }
    }

    impl Drop for CfOwned {
        fn drop(&mut self) {
            // SAFETY: `self.0` is non-null, was returned under the Create or
            // Copy rule, and this is its only owner.
            unsafe { CFRelease(self.0) };
        }
    }
}

#[cfg(test)]
#[path = "../tests/default_terminal/macos.rs"]
mod tests;
