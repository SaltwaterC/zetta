use std::path::PathBuf;
#[cfg(windows)]
use std::{
    path::Path,
    sync::{Mutex, OnceLock},
};

use anyhow::{Context as _, Result};
use gpui::img;
use gpui::{AnyElement, IntoElement, Styled as _};
use serde_json::Value;
use task::Shell;
use ui::IconSize;

/// The icon used by a terminal profile after configuration and automatic
/// discovery have been resolved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum ProfileIcon {
    #[default]
    Zetta,
    /// Generic Linux artwork used for automatically discovered WSL profiles.
    ///
    /// WSL does not expose the distribution's default shell during profile
    /// discovery, so a shell-specific icon would be misleading. This variant
    /// is intentionally automatic-only; users can still choose an explicit
    /// shell icon in their configuration.
    Tux,
    Bash,
    Zsh,
    Fish,
    /// The executable whose native icon should be displayed on Windows.
    ///
    /// This variant is kept platform-neutral so configuration and Jump List
    /// tests can exercise the model on every host. It is only produced by
    /// automatic discovery on Windows.
    #[allow(dead_code)]
    Executable(PathBuf),
}

impl ProfileIcon {
    pub(crate) fn parse(value: &Value) -> Result<Option<Self>> {
        if value.is_null() {
            return Ok(None);
        }
        let name = value
            .as_str()
            .context("profile.icon must be an icon name, \"auto\", or null")?;
        Self::parse_name(name)
    }

    pub(crate) fn parse_name(name: &str) -> Result<Option<Self>> {
        match name.to_ascii_lowercase().as_str() {
            "auto" => Ok(None),
            "zetta" => Ok(Some(Self::Zetta)),
            "bash" => Ok(Some(Self::Bash)),
            "zsh" => Ok(Some(Self::Zsh)),
            "fish" => Ok(Some(Self::Fish)),
            _ => anyhow::bail!(
                "profile.icon must be \"auto\", \"zetta\", \"bash\", \"zsh\", or \"fish\", got {name:?}"
            ),
        }
    }

    pub(crate) fn name(&self) -> Option<&'static str> {
        match self {
            Self::Zetta => Some("zetta"),
            Self::Tux => None,
            Self::Bash => Some("bash"),
            Self::Zsh => Some("zsh"),
            Self::Fish => Some("fish"),
            Self::Executable(_) => None,
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Zetta => "Zetta",
            Self::Tux => "Tux",
            Self::Bash => "Bash",
            Self::Zsh => "Zsh",
            Self::Fish => "Fish",
            Self::Executable(_) => "Executable",
        }
    }

    pub(crate) fn selector_label(icon: Option<&Self>) -> &'static str {
        icon.map(Self::label).unwrap_or("Automatic")
    }

    pub(crate) fn automatic_for_shell(shell: &Shell) -> Self {
        automatic_for_program(&shell.program())
    }

    pub(crate) fn automatic_for_program(program: &str) -> Self {
        automatic_for_program(program)
    }

    pub(crate) fn automatic_for_profile(name: &str, shell: &Shell) -> Self {
        let lowercase = name.to_ascii_lowercase();
        if lowercase.starts_with("wsl: ") {
            return Self::Tux;
        }
        if name.eq_ignore_ascii_case("msys2") || lowercase.starts_with("msys2: ") {
            if lowercase.contains("zsh") {
                return Self::Zsh;
            }
            return Self::Bash;
        }
        Self::automatic_for_shell(shell)
    }

    /// Infers a shell icon from the executable basename.
    pub(crate) fn automatic_for_program_name(program: &str) -> Self {
        match executable_basename(program).as_deref() {
            Some("bash") => Self::Bash,
            Some("zsh") => Self::Zsh,
            Some("fish") => Self::Fish,
            _ => Self::Zetta,
        }
    }

    pub(crate) fn render(&self, size: IconSize) -> AnyElement {
        let size = size.rems();
        match self {
            Self::Zetta => embedded_icon("icons/profile/zetta.svg", size),
            Self::Tux => embedded_icon("icons/profile/tux.png", size),
            Self::Bash => embedded_icon("icons/profile/bash.svg", size),
            Self::Zsh => embedded_icon("icons/profile/zsh.svg", size),
            Self::Fish => embedded_icon("icons/profile/fish.svg", size),
            Self::Executable(_executable) => {
                #[cfg(windows)]
                if let Some(path) = executable_icon_path(_executable) {
                    return img(path).size(size).flex_none().into_any_element();
                }
                embedded_icon("icons/profile/zetta.svg", size)
            }
        }
    }

    #[cfg(windows)]
    pub(crate) fn jump_list_icon_location(&self, target: &Path) -> (PathBuf, i32) {
        match self {
            Self::Executable(executable) if executable.is_file() => (executable.clone(), 0),
            Self::Zetta => (target.to_path_buf(), WINDOWS_ZETTA_ICON_INDEX),
            Self::Tux => (target.to_path_buf(), WINDOWS_TUX_ICON_INDEX),
            Self::Bash => (target.to_path_buf(), WINDOWS_BASH_ICON_INDEX),
            Self::Zsh => (target.to_path_buf(), WINDOWS_ZSH_ICON_INDEX),
            Self::Fish => (target.to_path_buf(), WINDOWS_FISH_ICON_INDEX),
            Self::Executable(_) => (target.to_path_buf(), WINDOWS_ZETTA_ICON_INDEX),
        }
    }
}

fn embedded_icon(path: &'static str, size: gpui::Rems) -> AnyElement {
    // GPUI's SVG element is monochrome; use the image loader so the bundled
    // profile artwork keeps its colors.
    img(path).size(size).flex_none().into_any_element()
}

pub(crate) fn automatic_for_program(program: &str) -> ProfileIcon {
    #[cfg(windows)]
    {
        if let Some(executable) = resolve_executable(program)
            && executable_icon_path(&executable).is_some()
        {
            return ProfileIcon::Executable(executable);
        }
    }
    ProfileIcon::automatic_for_program_name(program)
}

#[allow(dead_code)]
fn executable_basename(program: &str) -> Option<String> {
    let basename = program.rsplit(['/', '\\']).find(|part| !part.is_empty())?;
    let basename = basename
        .strip_suffix(".exe")
        .or_else(|| basename.strip_suffix(".EXE"))
        .unwrap_or(basename);
    Some(basename.to_ascii_lowercase())
}

#[cfg(windows)]
// IShellLink::SetIconLocation takes a zero-based icon-group index. The
// resource IDs in resources/windows/zetta.rc start at 1, so these are one
// less than the corresponding resource IDs.
const WINDOWS_ZETTA_ICON_INDEX: i32 = 1;
#[cfg(windows)]
const WINDOWS_TUX_ICON_INDEX: i32 = 5;
#[cfg(windows)]
const WINDOWS_BASH_ICON_INDEX: i32 = 2;
#[cfg(windows)]
const WINDOWS_ZSH_ICON_INDEX: i32 = 3;
#[cfg(windows)]
const WINDOWS_FISH_ICON_INDEX: i32 = 4;

#[cfg(windows)]
fn resolve_executable(program: &str) -> Option<PathBuf> {
    let program_path = Path::new(program);
    if program_path.components().count() > 1 {
        return program_path.is_file().then(|| program_path.to_path_buf());
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path).find_map(|directory| {
            let direct = directory.join(program);
            if direct.is_file() {
                return Some(direct);
            }
            (!program.to_ascii_lowercase().ends_with(".exe"))
                .then(|| directory.join(format!("{program}.exe")))
                .filter(|path| path.is_file())
        })
    })
}

#[cfg(windows)]
fn executable_icon_path(executable: &Path) -> Option<PathBuf> {
    static CACHE: OnceLock<Mutex<std::collections::HashMap<PathBuf, Option<PathBuf>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Some(path) = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(executable)
        .cloned()
    {
        return path;
    }

    let extracted = extract_executable_icon(executable).ok();
    cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(executable.to_path_buf(), extracted.clone());
    extracted
}

#[cfg(windows)]
fn extract_executable_icon(executable: &Path) -> Result<PathBuf> {
    use std::ffi::c_void;

    use image::RgbaImage;
    use windows::{
        Win32::{
            Graphics::Gdi::{
                BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection,
                DIB_RGB_COLORS, DeleteDC, DeleteObject, HGDIOBJ, SelectObject,
            },
            UI::{
                Shell::ExtractIconExW,
                WindowsAndMessaging::{DI_NORMAL, DestroyIcon, DrawIconEx, HICON},
            },
        },
        core::HSTRING,
    };

    const SIZE: i32 = 32;
    let path = HSTRING::from(executable.as_os_str());
    let mut icon = HICON::default();
    let extracted = unsafe { ExtractIconExW(&path, 0, Some(&mut icon), None, 1) };
    anyhow::ensure!(
        extracted > 0 && !icon.is_invalid(),
        "executable has no icon"
    );

    // SAFETY: The icon and GDI handles are owned by this function. The DIB is
    // top-down and 32-bit so its bytes can be copied into a safe Rust buffer
    // after drawing.
    let result = unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: SIZE,
                biHeight: -SIZE,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)
            .context("creating the executable icon bitmap")?;
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            _ = DeleteObject(HGDIOBJ::from(bitmap));
            DestroyIcon(icon)?;
            anyhow::bail!("creating the executable icon device context");
        }
        let previous = SelectObject(dc, HGDIOBJ::from(bitmap));
        let draw_result = DrawIconEx(dc, 0, 0, icon, SIZE, SIZE, 0, None, DI_NORMAL);
        let pixels = if draw_result.is_ok() && !bits.is_null() {
            std::slice::from_raw_parts(bits.cast::<u8>(), (SIZE * SIZE * 4) as usize).to_vec()
        } else {
            Vec::new()
        };
        SelectObject(dc, previous);
        _ = DeleteObject(HGDIOBJ::from(bitmap));
        _ = DeleteDC(dc);
        DestroyIcon(icon)?;
        anyhow::ensure!(!pixels.is_empty(), "drawing the executable icon");

        let mut rgba = pixels;
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        RgbaImage::from_raw(SIZE as u32, SIZE as u32, rgba)
            .context("decoding the executable icon bitmap")?
    };

    let cache_dir = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Zetta")
        .join("profile-icons");
    std::fs::create_dir_all(&cache_dir)
        .with_context(|| format!("creating {}", cache_dir.display()))?;
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    executable.hash(&mut hasher);
    let output = cache_dir.join(format!("{:016x}.png", hasher.finish()));
    if !output.is_file() {
        result
            .save(&output)
            .with_context(|| format!("writing {}", output.display()))?;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_profile_icon_values() {
        assert_eq!(ProfileIcon::parse(&Value::Null).unwrap(), None);
        assert_eq!(
            ProfileIcon::parse(&Value::String("auto".into())).unwrap(),
            None
        );
        assert_eq!(
            ProfileIcon::parse(&Value::String("BASH".into())).unwrap(),
            Some(ProfileIcon::Bash)
        );
        assert!(ProfileIcon::parse(&Value::String("terminal".into())).is_err());
    }

    #[test]
    fn maps_shell_executable_basenames() {
        assert_eq!(
            ProfileIcon::automatic_for_program_name("/opt/homebrew/bin/bash"),
            ProfileIcon::Bash
        );
        assert_eq!(
            ProfileIcon::automatic_for_program_name(r"C:\\tools\\zsh.exe"),
            ProfileIcon::Zsh
        );
        assert_eq!(
            ProfileIcon::automatic_for_program_name("custom-shell"),
            ProfileIcon::Zetta
        );
    }

    #[test]
    fn automatic_programs_fall_back_to_bundled_shell_icons() {
        assert_eq!(
            ProfileIcon::automatic_for_program(r"C:\missing\fish.exe"),
            ProfileIcon::Fish
        );
        assert_eq!(
            ProfileIcon::automatic_for_program("/missing/zsh"),
            ProfileIcon::Zsh
        );
    }

    #[test]
    fn special_profiles_use_their_platform_icon() {
        assert_eq!(
            ProfileIcon::automatic_for_profile("WSL: Ubuntu", &Shell::Program("wsl.exe".into())),
            ProfileIcon::Tux
        );
        assert_eq!(
            ProfileIcon::automatic_for_profile(
                "WSL: Ubuntu (Zsh)",
                &Shell::Program("wsl.exe".into()),
            ),
            ProfileIcon::Tux
        );
        assert_eq!(
            ProfileIcon::automatic_for_profile(
                "MSYS2: Zsh",
                &Shell::WithArguments {
                    program: "cmd.exe".into(),
                    args: Vec::new(),
                    title_override: None,
                },
            ),
            ProfileIcon::Zsh
        );
    }
}
