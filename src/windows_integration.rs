//! Windows shell integration: registering Zetta as the default terminal, the
//! taskbar jump list of profiles, and the application identity both rely on.
//! The COM server that receives a console handed over by Windows lives in
//! `handoff`.

use std::{
    collections::HashSet,
    env,
    ffi::c_void,
    os::windows::io::{AsRawHandle as _, FromRawHandle as _, IntoRawHandle as _, OwnedHandle},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::channel,
    },
    thread,
    time::Duration,
};
use zetta_sync::RecoverPoison as _;

use anyhow::{Context as _, Result};
use futures::channel::mpsc::UnboundedSender;
use terminal::{ConsolePalette, PtyControl, PtyHandover};
use windows::{
    Win32::{
        Foundation::{E_FAIL, E_INVALIDARG, HANDLE, PROPERTYKEY, RPC_E_CHANGED_MODE, S_OK},
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE,
            FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
            FILE_SHARE_MODE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
            SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
        },
        System::Com::{
            CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED,
            COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoRegisterClassObject,
            CoUninitialize, IClassFactory, IClassFactory_Impl, IPersistFile, REGCLS_MULTIPLEUSE,
            STGM_READWRITE, StructuredStorage::PROPVARIANT,
        },
        System::Pipes::{
            CreateNamedPipeW, NAMED_PIPE_MODE, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_TYPE_BYTE, PIPE_WAIT,
        },
        System::Threading::{GetExitCodeProcess, GetProcessId, INFINITE, WaitForSingleObject},
        UI::Shell::{
            Common::{IObjectArray, IObjectCollection},
            DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW,
            PropertiesSystem::IPropertyStore,
            SetCurrentProcessExplicitAppUserModelID, ShellLink,
        },
    },
    core::{GUID, HSTRING, Interface, implement},
};
use windows_core::{HRESULT, Ref, interface};

use crate::{Profile, ZETTA_APP_ID, profile_is_hidden};

const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};
const PKEY_TITLE: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xf29f85e0_4ff9_1068_ab91_08002b27b3d9),
    pid: 2,
};
/// Zetta's stable COM class identity. It must not change between releases:
/// Windows stores this value in the per-user default-terminal setting.
pub(crate) const ZETTA_TERMINAL_HANDOFF_CLSID: GUID =
    GUID::from_u128(0x7f6f0d2e_0b8c_4a36_9c71_42b3c6d89e10);
const ZETTA_TERMINAL_HANDOFF_CLSID_TEXT: &str = "{7F6F0D2E-0B8C-4A36-9C71-42B3C6D89E10}";
// Microsoft ships this class in OpenConsole.exe. It is the stable release
// CLSID used by the Windows default-terminal delegation protocol.
const OPEN_CONSOLE_HANDOFF_CLSID_TEXT: &str = "{2EACA947-7F5F-4CFA-BA87-8F7FBEEFBE69}";
const ZETTA_CLASS_PATH: &str = "Software\\Classes\\CLSID\\{7F6F0D2E-0B8C-4A36-9C71-42B3C6D89E10}";
const OPEN_CONSOLE_CLASS_PATH: &str =
    "Software\\Classes\\CLSID\\{2EACA947-7F5F-4CFA-BA87-8F7FBEEFBE69}";
const DELEGATION_PATH: &str = "Console\\%%Startup";
const DEFAULT_TERMINAL_BACKUP_PATH: &str = "Software\\Zetta\\DefaultTerminal";
static JUMP_LIST_GENERATION: AtomicU64 = AtomicU64::new(0);
static JUMP_LIST_UPDATE: Mutex<()> = Mutex::new(());

mod handoff;
pub(crate) use handoff::{WindowsHandoffRequest, monitor_handoff_child, start_handoff_server};

/// `!Send`: the apartment belongs to the thread that joined it, and only that
/// thread may leave it.
struct ComApartment {
    uninitialize: bool,
    _thread_bound: std::marker::PhantomData<*const ()>,
}

impl ComApartment {
    fn initialize() -> windows::core::Result<Self> {
        // SAFETY: CoInitializeEx takes no pointers (the reserved argument is
        // None); it only sets this thread's apartment, and `uninitialize`
        // records whether Drop owes it the matching CoUninitialize.
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if result.is_ok() {
            Ok(Self {
                uninitialize: true,
                _thread_bound: std::marker::PhantomData,
            })
        } else if result == RPC_E_CHANGED_MODE {
            Ok(Self {
                uninitialize: false,
                _thread_bound: std::marker::PhantomData,
            })
        } else {
            result.ok()?;
            unreachable!()
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.uninitialize {
            // SAFETY: balances the CoInitializeEx that succeeded in
            // `initialize`; every ComApartment is a local dropped on the
            // thread that created it, after the COM objects used under it.
            unsafe { CoUninitialize() };
        }
    }
}

pub(crate) fn register_shell_integration(
    shortcut_path: &Path,
    profiles: &[Profile],
    hidden_profiles: &HashSet<String>,
) -> Result<()> {
    let _ = register_terminal_server()?;
    let _apartment = ComApartment::initialize().context("initializing COM")?;
    set_process_app_id()?;
    set_shortcut_app_id(shortcut_path)?;
    write_profile_jump_list(profiles, hidden_profiles)
}

fn gui_executable() -> Result<PathBuf> {
    let executable = env::current_exe().context("finding the Zetta executable")?;
    let executable = executable.with_file_name("zetta-gui.exe");
    anyhow::ensure!(
        executable.is_file(),
        "GUI launcher not found at {}",
        executable.display()
    );
    Ok(executable)
}

fn local_server_command(executable: &Path) -> String {
    format!(
        "{} -Embedding",
        quote_windows_argument(&executable.to_string_lossy())
    )
}

fn registry_default(path: &str) -> Option<String> {
    registry_value(path, "")
}

fn registry_value(path: &str, name: &str) -> Option<String> {
    windows_registry::CURRENT_USER
        .open(path)
        .ok()
        .and_then(|key| key.get_string(name).ok())
}

fn server_path_from_command(command: &str) -> Option<PathBuf> {
    let command = command.trim();
    if command.is_empty() {
        return None;
    }
    let executable = if let Some(command) = command.strip_prefix('"') {
        command.split_once('"')?.0
    } else {
        command.split_whitespace().next()?
    };
    Some(PathBuf::from(executable))
}

fn valid_registered_server(path: &str) -> bool {
    let Some(path) = class_root_path(path) else {
        return false;
    };
    windows_registry::CLASSES_ROOT
        .open(format!("{path}\\LocalServer32"))
        .ok()
        .and_then(|key| key.get_string("").ok())
        .and_then(|command| server_path_from_command(&command))
        .is_some_and(|executable| executable.is_file())
}

fn class_root_path(clsid: &str) -> Option<String> {
    let clsid = clsid.trim();
    let bytes = clsid.as_bytes();
    if bytes.len() != 38
        || !bytes.iter().enumerate().all(|(index, byte)| match index {
            0 => *byte == b'{',
            37 => *byte == b'}',
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
    {
        return None;
    }
    Some(format!("CLSID\\{clsid}"))
}

fn selected_console_provider() -> Option<String> {
    registry_value(DELEGATION_PATH, "DelegationConsole")
        .filter(|clsid| valid_registered_server(clsid))
        .or_else(|| {
            valid_registered_server(OPEN_CONSOLE_HANDOFF_CLSID_TEXT)
                .then_some(OPEN_CONSOLE_HANDOFF_CLSID_TEXT.to_owned())
        })
}

fn restore_local_server_default(class_path: &str, previous: Option<&str>) {
    let Ok(key) = windows_registry::CURRENT_USER.create(format!("{class_path}\\LocalServer32"))
    else {
        return;
    };
    match previous {
        Some(previous) => {
            let _ = key.set_string("", previous);
        }
        None => {
            let _ = key.remove_value("");
        }
    }
}

fn register_bundled_console_provider(open_console: &Path) -> Result<()> {
    let provider_path = OPEN_CONSOLE_CLASS_PATH;
    let provider = windows_registry::CURRENT_USER
        .create(format!("{provider_path}\\LocalServer32"))
        .context("opening the console handoff registration")?;
    let previous = provider.get_string("").ok();
    let command = local_server_command(open_console);
    let state = windows_registry::CURRENT_USER
        .create(DEFAULT_TERMINAL_BACKUP_PATH)
        .context("opening Zetta's integration state")?;
    let already_owned = state.get_string("ConsoleProviderOwned").ok().as_deref() == Some("1")
        && state
            .get_string("ConsoleProviderClsid")
            .ok()
            .is_some_and(|clsid| clsid.eq_ignore_ascii_case(OPEN_CONSOLE_HANDOFF_CLSID_TEXT));
    if !already_owned {
        if let Some(previous) = &previous {
            state
                .set_string("ConsoleProviderPrevious", previous)
                .context("saving the previous console handoff registration")?;
            state.set_string("ConsoleProviderPreviousPresent", "1")?;
        } else {
            state.set_string("ConsoleProviderPreviousPresent", "0")?;
            let _ = state.remove_value("ConsoleProviderPrevious");
        }
    }

    if let Err(error) = provider.set_string("", &command) {
        return Err(error).context("registering the bundled console handoff provider");
    }
    if let Err(error) = (|| -> Result<()> {
        state.set_string("ConsoleProviderOwned", "1")?;
        state.set_string("ConsoleProviderClsid", OPEN_CONSOLE_HANDOFF_CLSID_TEXT)?;
        state.set_string("ConsoleProviderPath", open_console.to_string_lossy())?;
        Ok(())
    })() {
        restore_local_server_default(provider_path, previous.as_deref());
        return Err(error).context("recording console handoff ownership");
    }
    Ok(())
}

/// Registers only the COM classes needed for delegation. The OpenConsole
/// class is reused when Windows Terminal already registered a usable provider;
/// a Zetta-owned HKCU entry is written only when no valid provider exists.
fn register_terminal_server() -> Result<String> {
    let gui = gui_executable()?;
    let class = windows_registry::CURRENT_USER
        .create(format!("{ZETTA_CLASS_PATH}\\LocalServer32"))
        .context("opening Zetta's per-user COM registration")?;
    let previous = class.get_string("").ok();
    class
        .set_string("", local_server_command(&gui))
        .context("registering Zetta's terminal handoff server")?;

    if let Some(provider) = selected_console_provider() {
        return Ok(provider);
    }

    let result = (|| -> Result<String> {
        let open_console = env::current_exe()
            .context("finding the bundled console handoff provider")?
            .with_file_name("OpenConsole.exe");
        anyhow::ensure!(
            open_console.is_file(),
            "bundled OpenConsole.exe was not found at {}",
            open_console.display()
        );
        register_bundled_console_provider(&open_console)?;
        Ok(OPEN_CONSOLE_HANDOFF_CLSID_TEXT.to_owned())
    })();
    match result {
        Ok(provider) => Ok(provider),
        Err(error) => {
            restore_local_server_default(ZETTA_CLASS_PATH, previous.as_deref());
            Err(error)
        }
    }
}

fn clear_delegation_backup() {
    let Ok(state) = windows_registry::CURRENT_USER.open(DEFAULT_TERMINAL_BACKUP_PATH) else {
        return;
    };
    for name in [
        "DelegationTerminal",
        "DelegationConsole",
        "DelegationTerminalPresent",
        "DelegationConsolePresent",
        "Saved",
        "Active",
        "ActiveDelegationTerminal",
        "ActiveDelegationConsole",
    ] {
        let _ = state.remove_value(name);
    }
}

fn save_previous_delegation_values() -> Result<()> {
    let state = windows_registry::CURRENT_USER
        .create(DEFAULT_TERMINAL_BACKUP_PATH)
        .context("opening Zetta's default-terminal backup")?;
    if state.get_string("Saved").is_ok()
        && registry_value(DELEGATION_PATH, "DelegationTerminal")
            .is_some_and(|value| value.eq_ignore_ascii_case(ZETTA_TERMINAL_HANDOFF_CLSID_TEXT))
    {
        return Ok(());
    }
    // A previous attempt may have been interrupted after the user selected a
    // different terminal. Discard that stale snapshot before taking the new
    // value so uninstall restores the choice that preceded this activation.
    clear_delegation_backup();
    let state = windows_registry::CURRENT_USER
        .create(DEFAULT_TERMINAL_BACKUP_PATH)
        .context("reopening Zetta's default-terminal backup")?;
    let delegation = windows_registry::CURRENT_USER.open(DELEGATION_PATH).ok();
    for name in ["DelegationTerminal", "DelegationConsole"] {
        if let Some(value) = delegation
            .as_ref()
            .and_then(|key| key.get_string(name).ok())
        {
            state
                .set_string(name, value)
                .with_context(|| format!("saving the previous {name} default-terminal value"))?;
            state.set_string(format!("{name}Present"), "1")?;
        } else {
            state.set_string(format!("{name}Present"), "0")?;
        }
    }
    state
        .set_string("Saved", "1")
        .context("marking the default-terminal backup")?;
    Ok(())
}

fn set_delegation_values(console_provider: &str) -> Result<()> {
    let delegation = windows_registry::CURRENT_USER
        .create(DELEGATION_PATH)
        .context("opening Windows default-terminal settings")?;
    delegation
        .set_string("DelegationTerminal", ZETTA_TERMINAL_HANDOFF_CLSID_TEXT)
        .context("selecting Zetta as the Windows terminal")?;
    delegation
        .set_string("DelegationConsole", console_provider)
        .context("selecting the console handoff provider")?;
    Ok(())
}

fn restore_previous_delegation_values() -> Result<()> {
    let Ok(state) = windows_registry::CURRENT_USER.open(DEFAULT_TERMINAL_BACKUP_PATH) else {
        return Ok(());
    };
    let delegation = windows_registry::CURRENT_USER
        .create(DELEGATION_PATH)
        .context("opening Windows default-terminal settings for restore")?;
    for name in ["DelegationTerminal", "DelegationConsole"] {
        if state.get_string(format!("{name}Present")).ok().as_deref() == Some("1") {
            if let Ok(value) = state.get_string(name) {
                delegation.set_string(name, value)?;
            }
        } else {
            let _ = delegation.remove_value(name);
        }
    }
    Ok(())
}

pub(crate) fn set_default_terminal() -> Result<String> {
    let console_provider = register_terminal_server()?;
    save_previous_delegation_values()?;
    if let Err(error) = set_delegation_values(&console_provider) {
        if let Err(restore_error) = restore_previous_delegation_values() {
            return Err(error).context(format!(
                "Windows blocked the default-terminal setting and restoring the previous setting also failed: {restore_error:#}"
            ));
        }
        clear_delegation_backup();
        return Err(error).context(
            "Windows blocked the default-terminal setting; try again for the current user",
        );
    }
    let state = windows_registry::CURRENT_USER.create(DEFAULT_TERMINAL_BACKUP_PATH);
    if let Err(error) = state.and_then(|state| {
        state.set_string(
            "ActiveDelegationTerminal",
            ZETTA_TERMINAL_HANDOFF_CLSID_TEXT,
        )?;
        state.set_string("ActiveDelegationConsole", console_provider)?;
        state.set_string("Active", "1")
    }) {
        if let Err(restore_error) = restore_previous_delegation_values() {
            return Err(error).context(format!(
                "Windows set the default terminal but could not record its state or restore the previous setting: {restore_error:#}"
            ));
        }
        clear_delegation_backup();
        return Err(error).context(
            "Windows set the default terminal but could not record its state; the previous setting was restored",
        );
    }
    Ok("Zetta is now the default Windows terminal".to_owned())
}

/// Removes only registrations that still point at Zetta's installed files.
/// Delegation is restored only when Zetta remains the active choice, so an
/// uninstall cannot overwrite a later choice made by the user.
pub(crate) fn unregister_shell_integration() -> Result<()> {
    let gui = gui_executable().ok();
    let zetta_owned = gui
        .as_deref()
        .and_then(|gui| {
            registry_default(&format!("{ZETTA_CLASS_PATH}\\LocalServer32"))
                .map(|value| (gui, value))
        })
        .is_some_and(|(gui, value)| value == local_server_command(gui));
    if zetta_owned {
        let _ = windows_registry::CURRENT_USER.remove_tree(ZETTA_CLASS_PATH);
    }
    if zetta_still_owns_delegation() {
        restore_previous_delegation_values()?;
    }
    let provider_owned = windows_registry::CURRENT_USER
        .open(DEFAULT_TERMINAL_BACKUP_PATH)
        .ok()
        .filter(|state| state.get_string("ConsoleProviderOwned").ok().as_deref() == Some("1"))
        .and_then(|state| {
            let clsid = state.get_string("ConsoleProviderClsid").ok()?;
            let path = PathBuf::from(state.get_string("ConsoleProviderPath").ok()?);
            let command = local_server_command(&path);
            let current =
                registry_default(&format!("Software\\Classes\\CLSID\\{clsid}\\LocalServer32"))?;
            (current == command).then_some((clsid, path))
        });
    if let Some((clsid, _path)) = provider_owned {
        let state = windows_registry::CURRENT_USER
            .open(DEFAULT_TERMINAL_BACKUP_PATH)
            .ok();
        let previous = state.as_ref().and_then(|state| {
            (state
                .get_string("ConsoleProviderPreviousPresent")
                .ok()
                .as_deref()
                == Some("1"))
            .then(|| state.get_string("ConsoleProviderPrevious").ok())
            .flatten()
        });
        restore_local_server_default(
            &format!("Software\\Classes\\CLSID\\{clsid}"),
            previous.as_deref(),
        );
    }
    let _ = windows_registry::CURRENT_USER.remove_tree(DEFAULT_TERMINAL_BACKUP_PATH);
    Ok(())
}

fn zetta_still_owns_delegation() -> bool {
    if !registry_value(DELEGATION_PATH, "DelegationTerminal")
        .is_some_and(|value| value.eq_ignore_ascii_case(ZETTA_TERMINAL_HANDOFF_CLSID_TEXT))
    {
        return false;
    }

    // A user can change only the console provider while leaving the terminal
    // CLSID untouched. Compare both values when the activation snapshot has
    // the markers, so uninstall does not restore a stale pair over that later
    // choice. Older state without the markers keeps the original terminal-only
    // ownership check for upgrade compatibility.
    let Some(state) = windows_registry::CURRENT_USER
        .open(DEFAULT_TERMINAL_BACKUP_PATH)
        .ok()
    else {
        return true;
    };
    let Some(active_console) = state.get_string("ActiveDelegationConsole").ok() else {
        return true;
    };
    registry_value(DELEGATION_PATH, "DelegationConsole")
        .is_some_and(|value| value.eq_ignore_ascii_case(&active_console))
}

pub(crate) fn is_default_terminal() -> bool {
    registry_value(DELEGATION_PATH, "DelegationTerminal")
        .is_some_and(|value| value.eq_ignore_ascii_case(ZETTA_TERMINAL_HANDOFF_CLSID_TEXT))
}

pub(crate) fn update_profile_jump_list(profiles: Vec<Profile>, hidden_profiles: HashSet<String>) {
    let generation = JUMP_LIST_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    if let Err(error) = thread::Builder::new()
        .name("zetta-jump-list".to_owned())
        .spawn(move || {
            let _update = JUMP_LIST_UPDATE.lock().recover_poison();
            if generation != JUMP_LIST_GENERATION.load(Ordering::Relaxed) {
                return;
            }
            let result = ComApartment::initialize()
                .context("initializing COM")
                .and_then(|_apartment| write_profile_jump_list(&profiles, &hidden_profiles));
            if let Err(error) = result {
                eprintln!("Could not update the Zetta profile Jump List: {error:#}");
            }
        })
    {
        eprintln!("Could not start the Zetta Jump List update: {error}");
    }
}

fn set_process_app_id() -> Result<()> {
    // SAFETY: the temporary HSTRING is a valid NUL-terminated wide string
    // that outlives the call, which copies it.
    unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(ZETTA_APP_ID)) }
        .context("setting the Zetta AppUserModelID")
}

fn set_shortcut_app_id(shortcut_path: &Path) -> Result<()> {
    // SAFETY: the caller holds a ComApartment on this thread, and the CLSID
    // and IID references are static.
    let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
        .context("creating a Shell link")?;
    let persist: IPersistFile = link.cast().context("opening the shortcut storage")?;
    let shortcut_path = HSTRING::from(shortcut_path.as_os_str());
    // SAFETY: `persist` holds a live reference to the link object, and the
    // path HSTRING outlives the call.
    unsafe { persist.Load(&shortcut_path, STGM_READWRITE) }
        .context("loading the Start Menu shortcut")?;

    let store: IPropertyStore = link.cast().context("opening shortcut properties")?;
    let app_id = PROPVARIANT::from(ZETTA_APP_ID);
    // SAFETY: `store` is a live interface on the loaded link, and the key and
    // PROPVARIANT are borrowed for the call, which copies the value.
    unsafe { store.SetValue(&PKEY_APP_USER_MODEL_ID, &app_id) }
        .context("setting the shortcut AppUserModelID")?;
    // SAFETY: `store` is a live interface on the loaded link.
    unsafe { store.Commit() }.context("committing shortcut properties")?;
    // SAFETY: `persist` is a live interface, and the path HSTRING outlives the
    // call.
    unsafe { persist.Save(&shortcut_path, true) }.context("saving the Start Menu shortcut")?;
    Ok(())
}

fn visible_profile_jump_list_profiles<'a>(
    profiles: &'a [Profile],
    hidden_profiles: &'a HashSet<String>,
) -> impl Iterator<Item = &'a Profile> {
    profiles
        .iter()
        .filter(move |profile| !profile_is_hidden(profile, hidden_profiles))
}

fn write_profile_jump_list(profiles: &[Profile], hidden_profiles: &HashSet<String>) -> Result<()> {
    let target = env::current_exe()
        .context("finding the Zetta executable")?
        .with_file_name("zetta-gui.exe");
    anyhow::ensure!(
        target.is_file(),
        "GUI launcher not found at {}",
        target.display()
    );

    // SAFETY: the caller holds a ComApartment on this thread, and the CLSID
    // and IID references are static.
    let list: ICustomDestinationList =
        unsafe { CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER) }
            .context("creating the destination list")?;
    // SAFETY: `list` is a live interface, and the temporary HSTRING outlives
    // the call, which copies it.
    unsafe { list.SetAppID(&HSTRING::from(ZETTA_APP_ID)) }
        .context("selecting the Zetta destination list")?;

    let mut slots = 0;
    // SAFETY: `list` is a live interface, and `slots` is a writable u32 that
    // outlives the call.
    let _removed: IObjectArray =
        unsafe { list.BeginList(&mut slots) }.context("opening the Zetta destination list")?;
    let update = (|| -> Result<()> {
        // SAFETY: still on the caller's COM apartment thread; the CLSID and
        // IID references are static.
        let tasks: IObjectCollection =
            unsafe { CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER) }
                .context("creating the profile task collection")?;
        for profile in visible_profile_jump_list_profiles(profiles, hidden_profiles) {
            let link = create_profile_link(&target, profile)?;
            // SAFETY: `tasks` and `link` are live interfaces; the collection
            // takes its own reference to the link.
            unsafe { tasks.AddObject(&link) }
                .with_context(|| format!("adding profile {:?}", profile.name))?;
        }
        // SAFETY: `list` is a live interface with a list begun above, and
        // `tasks` is a live collection it takes its own reference to.
        unsafe { list.AddUserTasks(&tasks) }.context("adding profile tasks")?;
        // SAFETY: `list` is a live interface with a list begun above.
        unsafe { list.CommitList() }.context("committing profile tasks")?;
        Ok(())
    })();
    if update.is_err() {
        // SAFETY: `list` is a live interface whose list was begun above and
        // not committed.
        _ = unsafe { list.AbortList() };
    }
    update
}

fn create_profile_link(target: &Path, profile: &Profile) -> Result<IShellLinkW> {
    // SAFETY: the caller holds a ComApartment on this thread, and the CLSID
    // and IID references are static.
    let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
        .context("creating a profile Shell link")?;
    let (icon_path, icon_index) = profile.icon.jump_list_icon_location(target);
    let icon_path = HSTRING::from(icon_path.as_os_str());
    let target = HSTRING::from(target.as_os_str());
    let arguments = HSTRING::from(profile_jump_list_arguments(&profile.name));
    let description = HSTRING::from(format!("Open Zetta with {}", profile.name));
    // SAFETY: `link` is a live interface, and `target` outlives the call,
    // which copies the string.
    unsafe { link.SetPath(&target) }.context("setting the profile target")?;
    // SAFETY: `link` is live; `arguments` outlives the call.
    unsafe { link.SetArguments(&arguments) }.context("setting the profile arguments")?;
    // SAFETY: `link` is live; `description` outlives the call.
    unsafe { link.SetDescription(&description) }.context("setting the profile description")?;
    // SAFETY: `link` is live; `icon_path` outlives the call.
    unsafe { link.SetIconLocation(&icon_path, icon_index) }.context("setting the profile icon")?;
    if let Some(home) = env::var_os("USERPROFILE") {
        // SAFETY: `link` is live; the temporary HSTRING outlives the call.
        unsafe { link.SetWorkingDirectory(&HSTRING::from(home.as_os_str())) }
            .context("setting the profile working directory")?;
    }

    let store: IPropertyStore = link.cast().context("opening profile task properties")?;
    let title = PROPVARIANT::from(profile.name.as_str());
    // SAFETY: `store` is a live interface on the link, and the key and
    // PROPVARIANT are borrowed for the call, which copies the value.
    unsafe { store.SetValue(&PKEY_TITLE, &title) }.context("setting the profile task title")?;
    // SAFETY: `store` is a live interface on the link.
    unsafe { store.Commit() }.context("committing profile task properties")?;
    Ok(link)
}

fn profile_jump_list_arguments(profile_name: &str) -> String {
    format!(
        "--new-window --profile {}",
        quote_windows_argument(profile_name)
    )
}

fn quote_windows_argument(argument: &str) -> String {
    if !argument.is_empty()
        && !argument
            .chars()
            .any(|character| character.is_whitespace() || character == '"')
    {
        return argument.to_owned();
    }

    let mut quoted = String::from('"');
    let mut backslashes = 0;
    for character in argument.chars() {
        if character == '\\' {
            backslashes += 1;
        } else {
            if character == '"' {
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
            } else {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
            }
            quoted.push(character);
            backslashes = 0;
        }
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

#[cfg(test)]
#[path = "tests/windows_integration.rs"]
mod tests;
