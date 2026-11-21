//! Finding the Windows shells' installations: MSYS2 and Cygwin from the Start
//! Menu shortcut and the registry, the system directory, and the WSL
//! distribution list. The parsers these feed stay in the parent, where the
//! tests on every platform reach them.

use super::*;

use std::{os::windows::process::CommandExt as _, process::Command};

/// `GetSystemDirectoryW`: `System32` wherever Windows is installed.
pub(super) fn system_directory() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt as _;
    use windows::Win32::System::SystemInformation::GetSystemDirectoryW;

    let length = unsafe { GetSystemDirectoryW(None) } as usize;
    if length == 0 {
        return None;
    }
    let mut buffer = vec![0u16; length];
    let written = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    (written > 0 && written < buffer.len()).then(|| {
        buffer.truncate(written);
        PathBuf::from(std::ffi::OsString::from_wide(&buffer))
    })
}

/// What an MSYS2 profile starts, relative to the installation root.
pub(super) const MSYS2_LAUNCHED: &[&str] = &[
    "msys2_shell.cmd",
    "usr/bin/msys-2.0.dll",
    "usr/bin/bash.exe",
    "usr/bin/zsh.exe",
];

/// The installation a shortcut or registration records, or else the
/// conventional root — which only counts when nobody else can have put it
/// there (see `installation_trust`).
pub(super) fn msys2_installation_root() -> Option<PathBuf> {
    let guessed = [PathBuf::from(r"C:\msys64")]
        .into_iter()
        .filter(|root| installation_trust::guessed_root_is_trusted(root, MSYS2_LAUNCHED));
    msys2_start_menu_installation_roots()
        .into_iter()
        .chain(msys2_registered_installation_roots())
        .chain(guessed)
        .find(|root| root.join("msys2_shell.cmd").is_file())
}

pub(super) fn msys2_start_menu_installation_roots() -> Vec<PathBuf> {
    use windows::Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
    };

    let shortcuts = [
        env::var_os("APPDATA").map(PathBuf::from),
        env::var_os("ProgramData").map(PathBuf::from),
    ]
    .into_iter()
    .flatten()
    .map(|start_menu| {
        start_menu
            .join("Microsoft/Windows/Start Menu/Programs/MSYS2")
            .join("MSYS2 MSYS.lnk")
    })
    .filter(|shortcut| shortcut.is_file())
    .collect::<Vec<_>>();
    if shortcuts.is_empty() {
        return Vec::new();
    }

    // SAFETY: COM is initialized for this thread before creating Shell Link objects,
    // and is uninitialized only when this call initialized it successfully.
    unsafe {
        let result = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let initialized = if result.is_ok() {
            true
        } else if result == RPC_E_CHANGED_MODE {
            false
        } else {
            return Vec::new();
        };
        let roots = shortcuts
            .into_iter()
            .filter_map(|shortcut| shortcut_working_directory(&shortcut))
            .collect();
        if initialized {
            CoUninitialize();
        }
        roots
    }
}

pub(super) fn shortcut_working_directory(shortcut: &Path) -> Option<PathBuf> {
    use windows::{
        Win32::{
            System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ},
            UI::Shell::{IShellLinkW, ShellLink},
        },
        core::{HSTRING, Interface},
    };

    // SAFETY: The caller initializes COM before this helper is used. All output
    // buffers remain alive for the duration of the corresponding COM calls.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let persist: IPersistFile = link.cast().ok()?;
        persist
            .Load(&HSTRING::from(shortcut.as_os_str()), STGM_READ)
            .ok()?;
        let mut directory = vec![0_u16; 32_768];
        link.GetWorkingDirectory(&mut directory).ok()?;
        let len = directory.iter().position(|character| *character == 0)?;
        (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&directory[..len])))
    }
}

/// Reads a `REG_SZ` or `REG_EXPAND_SZ` value, unexpanded, as far as its first NUL.
///
/// The one reader of registry strings in this module. Its buffer is a `u8`
/// buffer of exactly the size the first query reports, because that size need
/// not be even: an odd-length value in a `u16` buffer of half the size let the
/// second query write one byte past it. Neither a terminating NUL nor an even
/// length is assumed; a trailing odd byte is dropped.
pub(super) fn registry_string(
    key: windows::Win32::System::Registry::HKEY,
    name: windows::core::PCWSTR,
) -> Option<String> {
    use windows::Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{REG_EXPAND_SZ, REG_SZ, RegQueryValueExW},
    };

    let mut value_type = REG_SZ;
    let mut byte_len = 0u32;
    // SAFETY: The first query writes only the value type and size to local variables.
    if unsafe {
        RegQueryValueExW(
            key,
            name,
            None,
            Some(&mut value_type),
            None,
            Some(&mut byte_len),
        )
    } != ERROR_SUCCESS
        || (value_type != REG_SZ && value_type != REG_EXPAND_SZ)
        || byte_len < 2
    {
        return None;
    }

    let mut value = vec![0u8; byte_len as usize];
    // SAFETY: `value` is exactly `byte_len` bytes, the size passed in `byte_len`, so
    // the query cannot write past it; a value that grew since fails with
    // `ERROR_MORE_DATA` instead.
    if unsafe {
        RegQueryValueExW(
            key,
            name,
            None,
            Some(&mut value_type),
            Some(value.as_mut_ptr()),
            Some(&mut byte_len),
        )
    } != ERROR_SUCCESS
        || (value_type != REG_SZ && value_type != REG_EXPAND_SZ)
    {
        return None;
    }

    let units = value
        .get(..byte_len as usize)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|bytes| u16::from_le_bytes(*bytes))
        .take_while(|unit| *unit != 0)
        .collect::<Vec<_>>();
    String::from_utf16(&units).ok()
}

pub(super) fn msys2_registered_installation_roots() -> Vec<PathBuf> {
    use windows::{
        Win32::{
            Foundation::ERROR_SUCCESS,
            System::Registry::{
                HKEY, HKEY_CURRENT_USER, KEY_READ, RegCloseKey, RegEnumKeyW, RegOpenKeyExW,
            },
        },
        core::w,
    };

    let mut uninstall = HKEY::default();
    // The official MSYS2 installer registers its per-user uninstall entry here.
    // SAFETY: `uninstall` is a valid output pointer and is closed below.
    if unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall"),
            None,
            KEY_READ,
            &mut uninstall,
        )
    } != ERROR_SUCCESS
    {
        return Vec::new();
    }

    let mut roots = Vec::new();
    for index in 0.. {
        let mut name = [0_u16; 256];
        // RegEnumKeyW writes at most the supplied buffer length.
        let result = unsafe { RegEnumKeyW(uninstall, index, Some(&mut name)) };
        if result != ERROR_SUCCESS {
            break;
        }
        if !name.contains(&0) {
            continue;
        }
        let mut entry = HKEY::default();
        // SAFETY: The enumerated name is terminated in the local buffer, and `entry`
        // is a valid output pointer.
        if unsafe {
            RegOpenKeyExW(
                uninstall,
                windows::core::PCWSTR(name.as_ptr()),
                None,
                KEY_READ,
                &mut entry,
            )
        } != ERROR_SUCCESS
        {
            continue;
        }
        let is_msys2 = registry_string(entry, w!("DisplayName")).is_some_and(|display_name| {
            display_name.eq_ignore_ascii_case("MSYS2")
                || display_name.to_ascii_lowercase().starts_with("msys2 ")
        });
        if is_msys2 && let Some(location) = registry_string(entry, w!("InstallLocation")) {
            roots.push(PathBuf::from(location));
        }
        // SAFETY: `entry` was opened successfully in this iteration.
        unsafe {
            let _ = RegCloseKey(entry);
        }
    }
    // SAFETY: `uninstall` was opened successfully above.
    unsafe {
        let _ = RegCloseKey(uninstall);
    }
    roots
}

pub(super) fn cygwin_installation_root() -> Option<PathBuf> {
    select_cygwin_installation_root(cygwin_installation_roots())
}

pub(super) fn cygwin_installation_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut seen = HashSet::new();

    let mut add_root = |root: PathBuf| {
        let key = root
            .to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase();
        if !key.is_empty() && seen.insert(key) {
            roots.push(root);
        }
    };

    for root in cygwin_registered_installation_roots() {
        add_root(root);
    }

    if let Some(path) = env::var_os("PATH") {
        for entry in env::split_paths(&path) {
            if entry
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case(OsStr::new("bin")))
            {
                if let Some(root) = entry.parent() {
                    add_root(root.to_path_buf());
                }
            } else if entry.join("bin").join("cygwin1.dll").is_file() {
                add_root(entry);
            }
        }
    }

    // Nobody recorded these: see `installation_trust`.
    for root in [r"C:\cygwin64", r"C:\cygwin"].map(PathBuf::from) {
        if installation_trust::guessed_root_is_trusted(&root, CYGWIN_LAUNCHED) {
            add_root(root);
        }
    }
    roots
}

/// What a Cygwin profile starts, relative to the installation root.
pub(super) const CYGWIN_LAUNCHED: &[&str] = &[
    "bin/cygwin1.dll",
    "bin/bash.exe",
    "bin/zsh.exe",
    "bin/fish.exe",
    "bin/nu.exe",
];

pub(super) fn cygwin_registered_installation_roots() -> Vec<PathBuf> {
    use windows::{
        Win32::{
            Foundation::ERROR_SUCCESS,
            System::Registry::{
                HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY,
                KEY_WOW64_64KEY, RegCloseKey, RegEnumKeyW, RegEnumValueW, RegOpenKeyExW,
            },
        },
        core::{PCWSTR, PWSTR},
    };

    pub(super) fn add_value(roots: &mut Vec<PathBuf>, key: HKEY, name: PCWSTR) {
        if let Some(value) = registry_string(key, name)
            && let Some(root) = normalize_cygwin_registry_path(&value)
        {
            roots.push(root);
        }
    }

    pub(super) fn add_values(roots: &mut Vec<PathBuf>, key: HKEY) {
        let mut index = 0u32;
        loop {
            let mut name = [0u16; 512];
            let mut name_len = name.len() as u32;
            // SAFETY: The buffers are valid for the duration of this call.
            let status = unsafe {
                RegEnumValueW(
                    key,
                    index,
                    Some(PWSTR(name.as_mut_ptr())),
                    &mut name_len,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if status != ERROR_SUCCESS {
                break;
            }
            let name = name[..name_len as usize]
                .iter()
                .copied()
                .chain([0])
                .collect::<Vec<_>>();
            add_value(roots, key, PCWSTR(name.as_ptr()));
            index += 1;
        }
    }

    pub(super) fn add_legacy_mount_values(roots: &mut Vec<PathBuf>, key: HKEY) {
        add_value(roots, key, PCWSTR::null());
        let mut index = 0u32;
        loop {
            let mut name = [0u16; 512];
            // SAFETY: `name` is a valid output buffer for the enumerated subkey.
            let status = unsafe { RegEnumKeyW(key, index, Some(&mut name)) };
            if status != ERROR_SUCCESS {
                break;
            }
            let name = name
                .iter()
                .copied()
                .take_while(|unit| *unit != 0)
                .chain([0])
                .collect::<Vec<_>>();
            let mut child = HKEY::default();
            // SAFETY: The name is NUL-terminated and `child` is a valid output pointer.
            if unsafe { RegOpenKeyExW(key, PCWSTR(name.as_ptr()), None, KEY_READ, &mut child) }
                == ERROR_SUCCESS
            {
                add_value(roots, child, windows::core::w!("native"));
                // SAFETY: `child` was opened successfully above.
                unsafe {
                    let _ = RegCloseKey(child);
                }
            }
            index += 1;
        }
    }

    let key_paths = [
        (r"SOFTWARE\Cygwin\Installations", 0u8),
        (r"SOFTWARE\Cygwin\setup", 1u8),
        (r"SOFTWARE\Cygnus Solutions\Cygwin\mounts v2", 2u8),
        (r"SOFTWARE\WOW6432Node\Cygwin\Installations", 0u8),
        (r"SOFTWARE\WOW6432Node\Cygwin\setup", 1u8),
        (
            r"SOFTWARE\WOW6432Node\Cygnus Solutions\Cygwin\mounts v2",
            2u8,
        ),
    ];
    let hives = [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE];
    let views = [KEY_WOW64_64KEY, KEY_WOW64_32KEY];
    let mut roots = Vec::new();

    for hive in hives {
        for view in views {
            for (path, kind) in key_paths {
                let path = path.encode_utf16().chain([0]).collect::<Vec<_>>();
                let mut key = HKEY::default();
                // SAFETY: `path` is NUL-terminated and `key` is a valid output pointer.
                if unsafe {
                    RegOpenKeyExW(hive, PCWSTR(path.as_ptr()), None, KEY_READ | view, &mut key)
                } != ERROR_SUCCESS
                {
                    continue;
                }

                match kind {
                    0 => add_values(&mut roots, key),
                    1 => {
                        add_value(&mut roots, key, windows::core::w!("rootdir"));
                        add_value(&mut roots, key, PCWSTR::null());
                    }
                    _ => add_legacy_mount_values(&mut roots, key),
                }
                // SAFETY: `key` was opened successfully above.
                unsafe {
                    let _ = RegCloseKey(key);
                }
            }
        }
    }

    roots
}

pub(super) fn wsl_program() -> Option<String> {
    let system_root = env::var_os("SystemRoot").or_else(|| env::var_os("WINDIR"));
    let system_wsl = system_root.map(PathBuf::from).map(|root| {
        root.join("System32")
            .join("wsl.exe")
            .to_string_lossy()
            .into_owned()
    });

    system_wsl
        .filter(|program| Path::new(program).is_file())
        .or_else(|| command_path("wsl.exe").map(|path| path.display().to_string()))
}

pub(super) fn discovered_wsl_profiles(program: &str) -> Vec<ProfileDefinition> {
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let output = Command::new(program)
        .args(["--list", "--quiet"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    wsl_profiles_from_output(program, &output.stdout)
}
