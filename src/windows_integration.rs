use std::{
    env,
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use anyhow::{Context as _, Result};
use windows::{
    Win32::{
        Foundation::{PROPERTYKEY, RPC_E_CHANGED_MODE},
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
            CoUninitialize, IPersistFile, STGM_READWRITE, StructuredStorage::PROPVARIANT,
        },
        UI::Shell::{
            Common::{IObjectArray, IObjectCollection},
            DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW,
            PropertiesSystem::IPropertyStore,
            SetCurrentProcessExplicitAppUserModelID, ShellLink,
        },
    },
    core::{GUID, HSTRING, Interface},
};

use crate::{Profile, ZETTA_APP_ID};

const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};
const PKEY_TITLE: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xf29f85e0_4ff9_1068_ab91_08002b27b3d9),
    pid: 2,
};
static JUMP_LIST_GENERATION: AtomicU64 = AtomicU64::new(0);
static JUMP_LIST_UPDATE: Mutex<()> = Mutex::new(());

struct ComApartment {
    uninitialize: bool,
}

impl ComApartment {
    fn initialize() -> windows::core::Result<Self> {
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if result.is_ok() {
            Ok(Self { uninitialize: true })
        } else if result == RPC_E_CHANGED_MODE {
            Ok(Self {
                uninitialize: false,
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
            unsafe { CoUninitialize() };
        }
    }
}

pub(crate) fn register_shell_integration(shortcut_path: &Path, profiles: &[Profile]) -> Result<()> {
    let _apartment = ComApartment::initialize().context("initializing COM")?;
    set_process_app_id()?;
    set_shortcut_app_id(shortcut_path)?;
    write_profile_jump_list(profiles)
}

pub(crate) fn update_profile_jump_list(profiles: Vec<Profile>) {
    let generation = JUMP_LIST_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    if let Err(error) = thread::Builder::new()
        .name("zetta-jump-list".to_owned())
        .spawn(move || {
            let _update = JUMP_LIST_UPDATE
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if generation != JUMP_LIST_GENERATION.load(Ordering::Relaxed) {
                return;
            }
            let result = ComApartment::initialize()
                .context("initializing COM")
                .and_then(|_apartment| write_profile_jump_list(&profiles));
            if let Err(error) = result {
                eprintln!("Could not update the Zetta profile Jump List: {error:#}");
            }
        })
    {
        eprintln!("Could not start the Zetta Jump List update: {error}");
    }
}

fn set_process_app_id() -> Result<()> {
    unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(ZETTA_APP_ID)) }
        .context("setting the Zetta AppUserModelID")
}

fn set_shortcut_app_id(shortcut_path: &Path) -> Result<()> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .context("creating a Shell link")?;
        let persist: IPersistFile = link.cast().context("opening the shortcut storage")?;
        let shortcut_path = HSTRING::from(shortcut_path.as_os_str());
        persist
            .Load(&shortcut_path, STGM_READWRITE)
            .context("loading the Start Menu shortcut")?;

        let store: IPropertyStore = link.cast().context("opening shortcut properties")?;
        let app_id = PROPVARIANT::from(ZETTA_APP_ID);
        store
            .SetValue(&PKEY_APP_USER_MODEL_ID, &app_id)
            .context("setting the shortcut AppUserModelID")?;
        store.Commit().context("committing shortcut properties")?;
        persist
            .Save(&shortcut_path, true)
            .context("saving the Start Menu shortcut")?;
    }
    Ok(())
}

fn write_profile_jump_list(profiles: &[Profile]) -> Result<()> {
    let target = env::current_exe()
        .context("finding the Zetta executable")?
        .with_file_name("zetta-gui.exe");
    anyhow::ensure!(
        target.is_file(),
        "GUI launcher not found at {}",
        target.display()
    );

    unsafe {
        let list: ICustomDestinationList =
            CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER)
                .context("creating the destination list")?;
        list.SetAppID(&HSTRING::from(ZETTA_APP_ID))
            .context("selecting the Zetta destination list")?;

        let mut slots = 0;
        let _removed: IObjectArray = list
            .BeginList(&mut slots)
            .context("opening the Zetta destination list")?;
        let update = (|| -> Result<()> {
            let tasks: IObjectCollection =
                CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)
                    .context("creating the profile task collection")?;
            for profile in profiles {
                let link = create_profile_link(&target, profile)?;
                tasks
                    .AddObject(&link)
                    .with_context(|| format!("adding profile {:?}", profile.name))?;
            }
            list.AddUserTasks(&tasks).context("adding profile tasks")?;
            list.CommitList().context("committing profile tasks")?;
            Ok(())
        })();
        if update.is_err() {
            _ = list.AbortList();
        }
        update
    }
}

fn create_profile_link(target: &Path, profile: &Profile) -> Result<IShellLinkW> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .context("creating a profile Shell link")?;
        let target = HSTRING::from(target.as_os_str());
        let arguments = HSTRING::from(format!(
            "--profile {}",
            quote_windows_argument(&profile.name)
        ));
        let description = HSTRING::from(format!("Open Zetta with {}", profile.name));
        link.SetPath(&target)
            .context("setting the profile target")?;
        link.SetArguments(&arguments)
            .context("setting the profile arguments")?;
        link.SetDescription(&description)
            .context("setting the profile description")?;
        link.SetIconLocation(&target, 0)
            .context("setting the profile icon")?;
        if let Some(home) = env::var_os("USERPROFILE") {
            link.SetWorkingDirectory(&HSTRING::from(home.as_os_str()))
                .context("setting the profile working directory")?;
        }

        let store: IPropertyStore = link.cast().context("opening profile task properties")?;
        let title = PROPVARIANT::from(profile.name.as_str());
        store
            .SetValue(&PKEY_TITLE, &title)
            .context("setting the profile task title")?;
        store
            .Commit()
            .context("committing profile task properties")?;
        Ok(link)
    }
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
