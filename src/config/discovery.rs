//! Detecting the shells this machine can offer as profiles.
//!
//! Everything here answers one question — what is installed — and answers it
//! per platform: Homebrew prefixes on macOS and Linux, the MSYS2 and Cygwin
//! installation roots on Windows (from the Start Menu shortcut, the registry,
//! and `PATH`), and the WSL distribution list. The result is the profile set
//! `Config::defaults` starts from, before any file is read.
//!
//! Kept apart from `config.rs` because it shares nothing with the parser but
//! the `Profile` type it produces.

use super::*;

use std::{ffi::OsStr, sync::OnceLock};

#[cfg(windows)]
use std::{os::windows::process::CommandExt as _, process::Command};

pub(super) fn discovered_profiles() -> Vec<Profile> {
    static DISCOVERED_PROFILES: OnceLock<Vec<Profile>> = OnceLock::new();
    DISCOVERED_PROFILES.get_or_init(discover_profiles).clone()
}

fn discover_profiles() -> Vec<Profile> {
    let mut profiles = vec![Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::automatic_for_shell(&Shell::System),
    }];
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let homebrew_prefixes = homebrew_prefixes();
    let candidates: &[(&str, &str)] = if cfg!(windows) {
        &[
            ("PowerShell", "powershell.exe"),
            ("PowerShell 7", "pwsh.exe"),
            ("Command Prompt", "cmd.exe"),
        ]
    } else {
        &[
            ("Zsh", "zsh"),
            ("Bash", "bash"),
            ("Fish", "fish"),
            ("Nushell", "nu"),
        ]
    };
    let mut seen = HashSet::new();
    for (name, program) in candidates {
        if let Some(path) = command_path(program)
            && seen.insert(path.clone())
        {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            let profile =
                homebrew_profile_for_path(&path, &homebrew_prefixes).unwrap_or_else(|| Profile {
                    name: (*name).to_owned(),
                    command: Shell::Program((*program).to_owned()),
                    theme: None,
                    dark_theme: None,
                    icon: ProfileIcon::automatic_for_program(program),
                });
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            let profile = Profile {
                name: (*name).to_owned(),
                command: Shell::Program((*program).to_owned()),
                theme: None,
                dark_theme: None,
                icon: ProfileIcon::automatic_for_program(program),
            };
            profiles.push(profile);
        }
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    profiles.extend(
        homebrew_shell_profiles(homebrew_prefixes)
            .into_iter()
            .filter(|profile| {
                let Shell::Program(program) = &profile.command else {
                    return true;
                };
                seen.insert(PathBuf::from(program))
            }),
    );
    #[cfg(windows)]
    if let Some(root) = msys2_installation_root() {
        profiles.extend(msys2_profiles(&root));
    }
    #[cfg(windows)]
    if let Some(root) = cygwin_installation_root() {
        profiles.extend(cygwin_profiles(&root));
    }
    #[cfg(windows)]
    if let Some(program) = wsl_program() {
        profiles.extend(discovered_wsl_profiles(&program));
    }
    profiles
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
const POSIX_PROFILE_CANDIDATES: &[(&str, &str)] = &[
    ("Zsh", "zsh"),
    ("Bash", "bash"),
    ("Fish", "fish"),
    ("Nushell", "nu"),
];

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn homebrew_prefixes() -> Vec<PathBuf> {
    let mut prefixes = env::var_os("HOMEBREW_PREFIX")
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();

    #[cfg(target_os = "macos")]
    prefixes.extend([PathBuf::from("/opt/homebrew"), PathBuf::from("/usr/local")]);
    #[cfg(target_os = "linux")]
    prefixes.push(PathBuf::from("/home/linuxbrew/.linuxbrew"));

    prefixes.dedup();
    prefixes
        .into_iter()
        .filter(|prefix| prefix.join("bin/brew").is_file())
        .collect()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn homebrew_profile_for_path(path: &Path, prefixes: &[PathBuf]) -> Option<Profile> {
    POSIX_PROFILE_CANDIDATES.iter().find_map(|(name, program)| {
        prefixes.iter().find_map(|prefix| {
            let homebrew_path = prefix.join("bin").join(program);
            (homebrew_path == path).then(|| homebrew_profile(name, homebrew_path))
        })
    })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn homebrew_profile(name: &str, path: PathBuf) -> Profile {
    let command = path.to_string_lossy().into_owned();
    Profile {
        name: format!("{name} (Homebrew)"),
        command: Shell::Program(command.clone()),
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::automatic_for_program(&command),
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn homebrew_shell_profiles(prefixes: impl IntoIterator<Item = PathBuf>) -> Vec<Profile> {
    prefixes
        .into_iter()
        .flat_map(|prefix| {
            let bin = prefix.join("bin");
            POSIX_PROFILE_CANDIDATES
                .iter()
                .filter_map(move |(name, program)| {
                    let path = bin.join(program);
                    path.is_file().then(|| homebrew_profile(name, path))
                })
        })
        .collect()
}

#[cfg(windows)]
fn msys2_installation_root() -> Option<PathBuf> {
    msys2_start_menu_installation_roots()
        .into_iter()
        .chain(msys2_registered_installation_roots())
        .chain([PathBuf::from(r"C:\msys64")])
        .find(|root| root.join("msys2_shell.cmd").is_file())
}

#[cfg(windows)]
fn msys2_start_menu_installation_roots() -> Vec<PathBuf> {
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

#[cfg(windows)]
fn shortcut_working_directory(shortcut: &Path) -> Option<PathBuf> {
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

#[cfg(windows)]
fn msys2_registered_installation_roots() -> Vec<PathBuf> {
    use windows::{
        Win32::{
            Foundation::ERROR_SUCCESS,
            System::Registry::{
                HKEY, HKEY_CURRENT_USER, KEY_READ, RegCloseKey, RegEnumKeyW, RegOpenKeyExW,
            },
        },
        core::w,
    };

    fn read_string(key: HKEY, name: windows::core::PCWSTR) -> Option<String> {
        use windows::Win32::{
            Foundation::ERROR_SUCCESS,
            System::Registry::{REG_SZ, RegQueryValueExW},
        };

        let mut value_type = REG_SZ;
        let mut byte_len = 0;
        // SAFETY: The first query requests only the value size and writes to valid local
        // variables. The second query uses a buffer sized from that result.
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
            || value_type != REG_SZ
            || byte_len < 2
        {
            return None;
        }
        let mut value = vec![0_u16; byte_len as usize / 2];
        if unsafe {
            RegQueryValueExW(
                key,
                name,
                None,
                Some(&mut value_type),
                Some(value.as_mut_ptr().cast()),
                Some(&mut byte_len),
            )
        } != ERROR_SUCCESS
        {
            return None;
        }
        let len = value.iter().position(|character| *character == 0)?;
        Some(String::from_utf16_lossy(&value[..len]))
    }

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
        let is_msys2 = read_string(entry, w!("DisplayName")).is_some_and(|display_name| {
            display_name.eq_ignore_ascii_case("MSYS2")
                || display_name.to_ascii_lowercase().starts_with("msys2 ")
        });
        if is_msys2 && let Some(location) = read_string(entry, w!("InstallLocation")) {
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

#[cfg(any(windows, test))]
fn msys2_profiles(root: &Path) -> Vec<Profile> {
    let launcher = root.join("msys2_shell.cmd");
    [("MSYS2", "bash"), ("MSYS2: Zsh", "zsh")]
        .into_iter()
        .filter(|(_, shell)| root.join("usr/bin").join(format!("{shell}.exe")).is_file())
        .map(|(name, shell)| {
            let command = format!(
                "\"\"{}\" -defterm -here -no-start -msys -use-full-path -shell {shell}\"",
                launcher.display()
            );
            Profile {
                name: name.to_owned(),
                command: Shell::WithArguments {
                    program: "cmd.exe".to_owned(),
                    args: vec!["/d".to_owned(), "/s".to_owned(), "/c".to_owned(), command],
                    title_override: Some(name.to_owned()),
                },
                theme: None,
                dark_theme: None,
                icon: match shell {
                    "bash" => ProfileIcon::Bash,
                    "zsh" => ProfileIcon::Zsh,
                    _ => ProfileIcon::Zetta,
                },
            }
        })
        .collect()
}

#[cfg(windows)]
fn cygwin_installation_root() -> Option<PathBuf> {
    select_cygwin_installation_root(cygwin_installation_roots())
}

#[cfg(any(windows, test))]
fn select_cygwin_installation_root(roots: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    let mut first_valid_root = None;
    for root in roots {
        if !root.join("bin").join("cygwin1.dll").is_file() {
            continue;
        }
        if first_valid_root.is_none() {
            first_valid_root = Some(root.clone());
        }
        // A stale registry entry may retain cygwin1.dll after the supported
        // shell executables have been removed. Prefer the next complete
        // installation so that one such entry cannot hide a working root.
        if !cygwin_profiles(&root).is_empty() {
            return Some(root);
        }
    }
    first_valid_root
}

#[cfg(windows)]
fn cygwin_installation_roots() -> Vec<PathBuf> {
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

    add_root(PathBuf::from(r"C:\cygwin64"));
    add_root(PathBuf::from(r"C:\cygwin"));
    roots
}

#[cfg(windows)]
fn cygwin_registered_installation_roots() -> Vec<PathBuf> {
    use windows::{
        Win32::{
            Foundation::ERROR_SUCCESS,
            System::Registry::{
                HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY,
                KEY_WOW64_64KEY, REG_EXPAND_SZ, REG_SZ, RegCloseKey, RegEnumKeyW, RegEnumValueW,
                RegOpenKeyExW, RegQueryValueExW,
            },
        },
        core::{PCWSTR, PWSTR},
    };

    fn registry_string(key: HKEY, name: PCWSTR) -> Option<String> {
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
        // SAFETY: `value` is sized from the preceding registry query.
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
        {
            return None;
        }

        let units = value[..byte_len as usize]
            .chunks_exact(2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
            .take_while(|unit| *unit != 0)
            .collect::<Vec<_>>();
        String::from_utf16(&units).ok()
    }

    fn add_value(roots: &mut Vec<PathBuf>, key: HKEY, name: PCWSTR) {
        if let Some(value) = registry_string(key, name)
            && let Some(root) = normalize_cygwin_registry_path(&value)
        {
            roots.push(root);
        }
    }

    fn add_values(roots: &mut Vec<PathBuf>, key: HKEY) {
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

    fn add_legacy_mount_values(roots: &mut Vec<PathBuf>, key: HKEY) {
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

#[cfg(any(windows, test))]
fn normalize_cygwin_registry_path(value: &str) -> Option<PathBuf> {
    let value = value.trim().trim_end_matches('\0');
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }

    let value = value.replace('/', "\\");
    let value = ["\\\\??\\", "\\??\\", "\\\\?\\"]
        .iter()
        .find_map(|prefix| {
            value
                .get(..prefix.len())
                .filter(|value| value.eq_ignore_ascii_case(prefix))
                .map(|_| &value[prefix.len()..])
        })
        .unwrap_or(&value);
    let value = if value
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("UNC\\"))
    {
        format!("\\\\{}", &value[4..])
    } else {
        value.to_owned()
    };
    (!value.is_empty()).then(|| PathBuf::from(value))
}

#[cfg(any(windows, test))]
fn cygwin_profiles(root: &Path) -> Vec<Profile> {
    [
        ("Cygwin", "bash", ProfileIcon::Bash),
        ("Cygwin: Zsh", "zsh", ProfileIcon::Zsh),
        ("Cygwin: Fish", "fish", ProfileIcon::Fish),
        ("Cygwin: Nushell", "nu", ProfileIcon::Zetta),
    ]
    .into_iter()
    .filter_map(|(name, shell, icon)| {
        let program = root.join("bin").join(format!("{shell}.exe"));
        program.is_file().then(|| Profile {
            name: name.to_owned(),
            command: Shell::WithArguments {
                program: program.display().to_string(),
                args: vec!["-l".to_owned()],
                title_override: Some(name.to_owned()),
            },
            theme: None,
            dark_theme: None,
            icon,
        })
    })
    .collect()
}

#[cfg(windows)]
fn wsl_program() -> Option<String> {
    let system_root = env::var_os("SystemRoot").or_else(|| env::var_os("WINDIR"));
    let system_wsl = system_root.map(PathBuf::from).map(|root| {
        root.join("System32")
            .join("wsl.exe")
            .to_string_lossy()
            .into_owned()
    });

    system_wsl
        .filter(|program| Path::new(program).is_file())
        .or_else(|| command_exists("wsl.exe").then(|| "wsl.exe".to_owned()))
}

#[cfg(windows)]
fn discovered_wsl_profiles(program: &str) -> Vec<Profile> {
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

#[cfg(any(windows, test))]
fn wsl_profiles_from_output(program: &str, output: &[u8]) -> Vec<Profile> {
    parse_wsl_distribution_names(output)
        .into_iter()
        .map(|distribution| {
            let name = format!("WSL: {distribution}");
            Profile {
                name: name.clone(),
                command: Shell::WithArguments {
                    program: program.to_owned(),
                    args: vec!["--distribution".to_owned(), distribution],
                    title_override: Some(name),
                },
                theme: None,
                dark_theme: None,
                icon: ProfileIcon::Tux,
            }
        })
        .collect()
}

#[cfg(any(windows, test))]
fn parse_wsl_distribution_names(output: &[u8]) -> Vec<String> {
    let decoded = if let Some(bytes) = output.strip_prefix(&[0xfe, 0xff]) {
        let code_units = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&code_units)
    } else if output.starts_with(&[0xff, 0xfe]) || output.contains(&0) {
        let bytes = output.strip_prefix(&[0xff, 0xfe]).unwrap_or(output);
        let code_units = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&code_units)
    } else {
        String::from_utf8_lossy(output).into_owned()
    };

    let mut seen = HashSet::new();
    decoded
        .lines()
        .map(|line| line.trim_matches(['\0', '\u{feff}', ' ', '\t', '\r']))
        .filter(|name| !name.is_empty())
        .filter(|name| is_user_wsl_distribution(name))
        .filter(|name| seen.insert(name.to_lowercase()))
        .map(str::to_owned)
        .collect()
}

#[cfg(any(windows, test))]
fn is_user_wsl_distribution(name: &str) -> bool {
    ![
        "docker-desktop",
        "docker-desktop-data",
        "rancher-desktop",
        "rancher-desktop-data",
    ]
    .iter()
    .any(|service| name.eq_ignore_ascii_case(service))
}

fn command_path(program: &str) -> Option<PathBuf> {
    let program_path = Path::new(program);
    if program_path.components().count() > 1 {
        return program_path.is_file().then(|| program_path.to_path_buf());
    }
    env::var_os("PATH").and_then(|path| command_path_in(program, &path))
}

fn command_path_in(program: &str, path: &OsStr) -> Option<PathBuf> {
    env::split_paths(path).find_map(|directory| {
        if cfg!(windows) {
            if directory.join(program).is_file() {
                Some(directory.join(program))
            } else if !program.to_ascii_lowercase().ends_with(".exe")
                && directory.join(format!("{program}.exe")).is_file()
            {
                Some(directory.join(format!("{program}.exe")))
            } else {
                None
            }
        } else {
            directory
                .join(program)
                .is_file()
                .then(|| directory.join(program))
        }
    })
}

#[cfg(windows)]
fn command_exists(program: &str) -> bool {
    command_path(program).is_some()
}

#[cfg(test)]
#[path = "../tests/config/discovery.rs"]
mod tests;
