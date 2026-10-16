//! Establishes the packaged application's own macOS privacy identity.
//!
//! A direct executable launch otherwise inherits its terminal's responsible
//! process, including a dead GUI identity retained by a persistent daemon.
//! Re-executing with the same responsibility attribute LLDB uses makes Zetta
//! responsible for itself and its children, as a Launch Services launch does.
//! Only GUI launches and the bundled daemon use this; ordinary CLI operations
//! retain their caller's identity. Bare development executables are unchanged.
//! The dynamically resolved spawn attribute follows LLVM's
//! `lldb/source/Host/macosx/objcxx/PosixSpawnResponsible.h`.

use std::{
    env,
    ffi::{CString, OsString},
    mem::MaybeUninit,
    os::unix::ffi::OsStrExt as _,
    path::Path,
    ptr,
};

use anyhow::{Context as _, Result};

const INITIALIZED_PID: &str = "ZETTA_MACOS_RESPONSIBLE_PID";

pub(super) fn establish() -> Result<()> {
    let executable = env::current_exe().context("locating the application")?;
    if !is_bundled_application(&executable)
        || env::var(INITIALIZED_PID).ok().as_deref() == Some(&std::process::id().to_string())
    {
        return Ok(());
    }
    // This SPI has shipped since before macOS introduced Local Network
    // privacy. Resolve it dynamically, like LLDB, for older systems.
    // SAFETY: the symbol name is terminated and RTLD_DEFAULT is valid.
    let symbol = unsafe {
        libc::dlsym(
            libc::RTLD_DEFAULT,
            c"responsibility_spawnattrs_setdisclaim".as_ptr(),
        )
    };
    if symbol.is_null() {
        return Ok(());
    }
    // SAFETY: libSystem exports this exact posix_spawn attribute signature.
    let disclaim: unsafe extern "C" fn(*mut libc::posix_spawnattr_t, bool) -> libc::c_int =
        unsafe { std::mem::transmute(symbol) };
    let program = CString::new(executable.as_os_str().as_bytes())?;
    let arguments = env::args_os()
        .map(|value| CString::new(value.as_bytes()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let environment = launch_environment(env::vars_os(), std::process::id())?;
    let mut arguments = pointers(&arguments);
    let mut environment = pointers(&environment);
    let mut attributes = MaybeUninit::<libc::posix_spawnattr_t>::uninit();
    // SAFETY: initialization writes the uninitialized attribute storage.
    checked(unsafe { libc::posix_spawnattr_init(attributes.as_mut_ptr()) })?;
    // SAFETY: posix_spawnattr_init succeeded.
    let mut attributes = unsafe { attributes.assume_init() };
    let result = (|| {
        // SETEXEC retains the PID, CWD, terminal and inherited handover FDs.
        // The PID marker prevents another exec in this process, without
        // suppressing initialization in a later child application.
        // SAFETY: attributes is initialized, and these are valid flags.
        checked(unsafe {
            libc::posix_spawnattr_setflags(&mut attributes, libc::POSIX_SPAWN_SETEXEC as i16)
        })?;
        // SAFETY: the initialized attributes have the SPI's expected type.
        checked(unsafe { disclaim(&mut attributes, true) })?;
        let mut pid = 0;
        // SAFETY: every string and terminated pointer array outlives the call;
        // no file actions are needed. A successful SETEXEC does not return.
        checked(unsafe {
            libc::posix_spawn(
                &mut pid,
                program.as_ptr(),
                ptr::null(),
                &attributes,
                arguments.as_mut_ptr(),
                environment.as_mut_ptr(),
            )
        })
    })();
    // SAFETY: attributes was initialized and exec did not replace this image.
    unsafe { libc::posix_spawnattr_destroy(&mut attributes) };
    result.context("establishing Zetta's macOS application responsibility")
}

fn checked(status: libc::c_int) -> std::io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(status))
    }
}

fn pointers(strings: &[CString]) -> Vec<*mut libc::c_char> {
    strings
        .iter()
        .map(|value| value.as_ptr().cast_mut())
        .chain(std::iter::once(ptr::null_mut()))
        .collect()
}

fn is_bundled_application(executable: &Path) -> bool {
    let Some(macos) = executable.parent() else {
        return false;
    };
    executable.file_name().is_some_and(|name| name == "zetta")
        && macos.file_name().is_some_and(|name| name == "MacOS")
        && macos.parent().is_some_and(|contents| {
            contents.file_name().is_some_and(|name| name == "Contents")
                && contents.parent().is_some_and(|bundle| {
                    bundle
                        .extension()
                        .is_some_and(|extension| extension == "app")
                })
                && contents.join("Info.plist").is_file()
        })
}

fn launch_environment(
    values: impl IntoIterator<Item = (OsString, OsString)>,
    pid: u32,
) -> Result<Vec<CString>> {
    values
        .into_iter()
        .filter(|(key, _)| key != INITIALIZED_PID)
        .chain([(INITIALIZED_PID.into(), pid.to_string().into())])
        .map(|(mut key, value)| {
            key.push("=");
            key.push(value);
            CString::new(key.as_bytes()).context("encoding the launch environment")
        })
        .collect()
}

#[cfg(test)]
#[path = "../tests/startup/macos_responsibility.rs"]
mod tests;
