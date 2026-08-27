#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::path::PathBuf;

use anyhow::Result;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use anyhow::Context as _;
#[cfg(target_os = "macos")]
use std::path::Path;
#[cfg(target_os = "linux")]
use std::{env, fs, path::Path, process::Command};

#[cfg(target_os = "macos")]
use crate::command_panes::PaneCommand;

/// Selects the platform's user-level default-terminal integration. Installing
/// Zetta only registers the application; this function is called by the
/// explicit action exposed in the application menu.
pub(crate) fn set_default_terminal() -> Result<String> {
    #[cfg(windows)]
    {
        return crate::windows_integration::set_default_terminal();
    }
    #[cfg(target_os = "macos")]
    {
        register_macos_script_handlers()?;
        return Ok("Zetta is now the default terminal for shell-script files".to_owned());
    }
    #[cfg(target_os = "linux")]
    {
        set_linux_default_terminal()
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    anyhow::bail!("default-terminal integration is not supported on this platform")
}

/// Returns whether the platform reports Zetta as the currently selected
/// default terminal. This is intentionally a read-only query so menus can
/// show their checked state without changing the user's configuration.
pub(crate) fn is_default_terminal() -> bool {
    #[cfg(windows)]
    {
        crate::windows_integration::is_default_terminal()
    }
    #[cfg(target_os = "macos")]
    {
        macos_script_handlers_are_zetta()
    }
    #[cfg(target_os = "linux")]
    {
        linux_default_terminal_is_set()
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

impl crate::Zetta {
    pub(crate) fn set_default_terminal(
        &mut self,
        _: &crate::SetDefaultTerminal,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        match set_default_terminal() {
            Ok(message) => {
                #[cfg(target_os = "macos")]
                {
                    let effective = self.effective_config();
                    crate::startup::update_native_macos_menus(
                        cx,
                        &self.profiles,
                        &effective.hidden_profiles,
                        effective.default_profile,
                    );
                }
                self.show_notice(message, cx);
            }
            Err(error) => self.show_notice(
                format!("Could not set Zetta as the default terminal: {error:#}"),
                cx,
            ),
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn open_script_urls(
        &mut self,
        urls: &[String],
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let mut rejected = 0;
        for path in script_paths_from_urls(urls) {
            let request = PaneCommand {
                direction: None,
                label: None,
                pane: None,
                overlay: None,
                stack: false,
                list: false,
                command: vec![path.to_string_lossy().into_owned()],
            };
            if self
                .open_command_in_new_tab(request, path.parent().map(Path::to_path_buf), window, cx)
                .is_err()
            {
                rejected += 1;
            }
        }
        if rejected != 0 {
            self.show_notice(
                format!("Could not open {rejected} shell-script file(s) in Zetta"),
                cx,
            );
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DesktopEnvironment {
    Gnome,
    Budgie,
    Cinnamon,
    Mate,
    Kde,
    Xfce,
}

#[cfg(target_os = "linux")]
pub(crate) fn desktop_environment_from_values(
    current_desktop: Option<&str>,
    session_desktop: Option<&str>,
    kde_full_session: bool,
) -> Option<DesktopEnvironment> {
    let names = current_desktop
        .into_iter()
        .chain(session_desktop)
        .flat_map(|value| value.split([':', ';']))
        .map(|value| value.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    for name in &names {
        let desktop = match name.as_str() {
            "budgie" => DesktopEnvironment::Budgie,
            "cinnamon" | "x-cinnamon" => DesktopEnvironment::Cinnamon,
            "mate" => DesktopEnvironment::Mate,
            "kde" | "kde-plasma" | "plasma" => DesktopEnvironment::Kde,
            "xfce" | "xfce4" => DesktopEnvironment::Xfce,
            "gnome" | "gnome-classic" | "ubuntu" => DesktopEnvironment::Gnome,
            _ => continue,
        };
        return Some(desktop);
    }
    kde_full_session.then_some(DesktopEnvironment::Kde)
}

#[cfg(target_os = "linux")]
fn set_linux_default_terminal() -> Result<String> {
    let executable = env::current_exe().context("resolving Zetta's executable path")?;
    let current_desktop = env::var("XDG_CURRENT_DESKTOP").ok();
    let session_desktop = env::var("XDG_SESSION_DESKTOP").ok();
    let desktop = desktop_environment_from_values(
        current_desktop.as_deref(),
        session_desktop.as_deref(),
        env::var_os("KDE_FULL_SESSION").is_some(),
    );
    let mut changed = false;
    let mut applied = Vec::new();
    let mut failures = Vec::new();

    match set_xdg_terminal_preference(current_desktop.as_deref()) {
        Ok(path) => {
            changed = true;
            applied.push(format!("xdg-terminal-exec ({})", path.display()));
        }
        Err(error) => failures.push(format!("xdg-terminal-exec: {error:#}")),
    }

    match desktop {
        Some(
            desktop @ (DesktopEnvironment::Gnome
            | DesktopEnvironment::Budgie
            | DesktopEnvironment::Cinnamon
            | DesktopEnvironment::Mate),
        ) => {
            match set_gsettings_terminal(
                gsettings_terminal_schema(desktop).expect("GSettings desktop schema"),
                &executable,
            ) {
                Ok(()) => {
                    changed = true;
                    applied.push("legacy GSettings preference".to_owned());
                }
                Err(error) => failures.push(format!("legacy GSettings preference: {error:#}")),
            }
        }
        Some(DesktopEnvironment::Kde) => match set_kde_terminal_preference(&executable) {
            Ok(path) => {
                changed = true;
                applied.push(format!("KDE preference ({})", path.display()));
            }
            Err(error) => failures.push(format!("KDE preference: {error:#}")),
        },
        Some(DesktopEnvironment::Xfce) => match set_xfce_terminal_preference(&executable) {
            Ok(path) => {
                changed = true;
                applied.push(format!("Xfce preference ({})", path.display()));
            }
            Err(error) => failures.push(format!("Xfce preference: {error:#}")),
        },
        None => {}
    }

    let alternatives_available = command_available("update-alternatives");
    let alternatives_privileged = unsafe { libc::geteuid() } == 0;
    let alternatives_changed = if alternatives_available && alternatives_privileged {
        match set_update_alternatives(&executable) {
            Ok(()) => {
                changed = true;
                applied.push("update-alternatives".to_owned());
                true
            }
            Err(error) => {
                failures.push(format!("update-alternatives: {error:#}"));
                false
            }
        }
    } else {
        false
    };

    if changed || alternatives_changed {
        let mut message = format!(
            "Zetta is now the default terminal via {}",
            applied.join(", ")
        );
        if alternatives_available && !alternatives_privileged {
            message.push_str(
                "; update-alternatives was left unchanged because administrator privileges were not available",
            );
        }
        if !failures.is_empty() {
            message.push_str(&format!("; partially applied: {}", failures.join("; ")));
        }
        Ok(message)
    } else if !failures.is_empty() {
        anyhow::bail!(
            "could not set Zetta as the default terminal: {}",
            failures.join("; ")
        )
    } else if alternatives_available {
        anyhow::bail!(
            "unsupported desktop environment; update-alternatives requires administrator privileges"
        )
    } else {
        anyhow::bail!(
            "unsupported desktop environment; Zetta could not find a supported terminal preference"
        )
    }
}

#[cfg(target_os = "linux")]
fn linux_default_terminal_is_set() -> bool {
    let current_desktop = env::var("XDG_CURRENT_DESKTOP").ok();
    let Ok(path) = user_config_path(&xdg_terminal_config_filename(current_desktop.as_deref()))
    else {
        return false;
    };
    fs::read_to_string(path).is_ok_and(|contents| xdg_terminal_list_prefers_zetta(&contents))
}

#[cfg(target_os = "linux")]
fn set_xdg_terminal_preference(current_desktop: Option<&str>) -> Result<PathBuf> {
    let filename = xdg_terminal_config_filename(current_desktop);
    let path = user_config_path(&filename)?;
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", path.display()));
        }
    };
    let updated = update_xdg_terminal_list(&contents);
    if updated != contents {
        crate::project::write_text_atomically(&path, &updated).with_context(|| {
            format!("writing xdg-terminal-exec configuration {}", path.display())
        })?;
    }
    Ok(path)
}

#[cfg(target_os = "linux")]
pub(crate) fn xdg_terminal_config_filename(current_desktop: Option<&str>) -> String {
    let desktop = current_desktop
        .into_iter()
        .flat_map(|value| value.split([':', ';']))
        .find_map(valid_xdg_desktop_name);
    desktop.map_or_else(
        || "xdg-terminals.list".to_owned(),
        |desktop| format!("{desktop}-xdg-terminals.list"),
    )
}

#[cfg(target_os = "linux")]
fn valid_xdg_desktop_name(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    (!value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        }))
    .then_some(value)
}

#[cfg(target_os = "linux")]
pub(crate) fn update_xdg_terminal_list(contents: &str) -> String {
    let newline = if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let trailing_newline = contents.ends_with('\n') || contents.ends_with('\r');
    let mut lines = vec!["Zetta.desktop".to_owned()];
    lines.extend(
        contents
            .lines()
            .filter(|line| !is_zetta_xdg_terminal_entry(line))
            .map(str::to_owned),
    );
    let mut output = lines.join(newline);
    if trailing_newline {
        output.push_str(newline);
    }
    output
}

#[cfg(target_os = "linux")]
pub(crate) fn xdg_terminal_list_prefers_zetta(contents: &str) -> bool {
    first_xdg_terminal_entry(contents) == Some("Zetta.desktop")
}

#[cfg(target_os = "linux")]
fn first_xdg_terminal_entry(contents: &str) -> Option<&str> {
    contents.lines().find_map(|line| {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with('#')
            || line.starts_with('/')
            || line.starts_with('-')
        {
            return None;
        }
        let line = line.strip_prefix('+').unwrap_or(line).trim_start();
        let entry = line.split_once(':').map_or(line, |(entry, _)| entry).trim();
        (!entry.is_empty()).then_some(entry)
    })
}

#[cfg(target_os = "linux")]
fn is_zetta_xdg_terminal_entry(line: &str) -> bool {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with('/') {
        return false;
    }
    let line = line
        .strip_prefix('+')
        .or_else(|| line.strip_prefix('-'))
        .unwrap_or(line)
        .trim_start();
    let entry = line.split_once(':').map_or(line, |(entry, _)| entry).trim();
    entry == "Zetta.desktop"
}

#[cfg(target_os = "linux")]
fn set_kde_terminal_preference(executable: &Path) -> Result<PathBuf> {
    let path = user_config_path("kdeglobals")?;
    let contents = fs::read_to_string(&path).unwrap_or_default();
    let executable = executable.to_string_lossy();
    let updated = update_ini_section(
        &contents,
        "General",
        &[
            ("TerminalApplication", executable.as_ref()),
            ("TerminalService", "Zetta.desktop"),
        ],
    );
    crate::project::write_text_atomically(&path, &updated)
        .with_context(|| format!("writing KDE configuration {}", path.display()))?;
    Ok(path)
}

#[cfg(target_os = "linux")]
fn set_xfce_terminal_preference(executable: &Path) -> Result<PathBuf> {
    let path = user_config_path("xfce4/helpers.rc")?;
    let contents = fs::read_to_string(&path).unwrap_or_default();
    let updated = update_xfce_helpers(&contents, &executable.to_string_lossy());
    crate::project::write_text_atomically(&path, &updated)
        .with_context(|| format!("writing Xfce configuration {}", path.display()))?;
    Ok(path)
}

#[cfg(target_os = "linux")]
fn gsettings_terminal_schema(desktop: DesktopEnvironment) -> Option<&'static str> {
    match desktop {
        DesktopEnvironment::Gnome | DesktopEnvironment::Budgie => {
            Some("org.gnome.desktop.default-applications.terminal")
        }
        DesktopEnvironment::Cinnamon => Some("org.cinnamon.desktop.default-applications.terminal"),
        DesktopEnvironment::Mate => Some("org.mate.applications-terminal"),
        DesktopEnvironment::Kde | DesktopEnvironment::Xfce => None,
    }
}

#[cfg(target_os = "linux")]
fn set_update_alternatives(executable: &Path) -> Result<()> {
    let install = Command::new("update-alternatives")
        .args([
            "--install",
            "/usr/bin/x-terminal-emulator",
            "x-terminal-emulator",
        ])
        .arg(executable)
        .arg("50")
        .status()
        .context("running update-alternatives --install")?;
    anyhow::ensure!(
        install.success(),
        "update-alternatives could not register Zetta (exit status {install})"
    );
    let select = Command::new("update-alternatives")
        .args(["--set", "x-terminal-emulator"])
        .arg(executable)
        .status()
        .context("running update-alternatives --set")?;
    anyhow::ensure!(
        select.success(),
        "update-alternatives could not select Zetta (exit status {select})"
    );
    Ok(())
}

#[cfg(target_os = "linux")]
fn set_gsettings_terminal(schema: &str, executable: &Path) -> Result<()> {
    let executable = executable.to_string_lossy();
    let exec = Command::new("gsettings")
        .args(["set", schema, "exec"])
        .arg(executable.as_ref())
        .status()
        .context("running gsettings")?;
    anyhow::ensure!(exec.success(), "gsettings could not set {schema}.exec");
    let exec_arg = Command::new("gsettings")
        .args(["set", schema, "exec-arg", "-e"])
        .status()
        .context("running gsettings")?;
    anyhow::ensure!(
        exec_arg.success(),
        "gsettings could not set {schema}.exec-arg"
    );
    Ok(())
}

#[cfg(target_os = "linux")]
fn command_available(command: &str) -> bool {
    Command::new(command)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[cfg(target_os = "linux")]
fn user_config_path(relative: &str) -> Result<PathBuf> {
    let root = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .context("could not determine the user's configuration directory")?;
    Ok(root.join(relative))
}

#[cfg(target_os = "linux")]
pub(crate) fn update_ini_section(
    contents: &str,
    section: &str,
    updates: &[(&str, &str)],
) -> String {
    let newline = if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let trailing_newline = contents.ends_with('\n') || contents.ends_with('\r');
    let mut output = Vec::new();
    let mut in_section = false;
    let mut seen = vec![false; updates.len()];
    let mut found_section = false;
    for line in contents.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
        {
            if in_section {
                append_ini_updates(&mut output, updates, &seen);
            }
            in_section = name == section;
            if in_section {
                found_section = true;
                seen.fill(false);
            }
            output.push(line.to_owned());
            continue;
        }
        if in_section
            && let Some((index, (_, value))) = updates.iter().enumerate().find(|(_, (key, _))| {
                trimmed
                    .split_once('=')
                    .is_some_and(|(name, _)| name.trim() == *key)
            })
        {
            let key = updates[index].0;
            output.push(format!("{key}={value}"));
            seen[index] = true;
            continue;
        }
        output.push(line.to_owned());
    }
    if in_section {
        append_ini_updates(&mut output, updates, &seen);
    }
    if !found_section {
        if !output.is_empty() && !output.last().is_some_and(String::is_empty) {
            output.push(String::new());
        }
        output.push(format!("[{section}]"));
        for (key, value) in updates {
            output.push(format!("{key}={value}"));
        }
    }
    let mut output = output.join(newline);
    if trailing_newline {
        output.push_str(newline);
    }
    output
}

#[cfg(target_os = "linux")]
fn append_ini_updates(output: &mut Vec<String>, updates: &[(&str, &str)], seen: &[bool]) {
    for (index, (key, value)) in updates.iter().enumerate() {
        if !seen[index] {
            output.push(format!("{key}={value}"));
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn update_xfce_helpers(contents: &str, executable: &str) -> String {
    let newline = if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let trailing_newline = contents.ends_with('\n') || contents.ends_with('\r');
    let mut replaced = false;
    let mut lines = contents
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("TerminalEmulator=") {
                replaced = true;
                format!("TerminalEmulator={executable}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>();
    if !replaced {
        lines.push(format!("TerminalEmulator={executable}"));
    }
    let mut output = lines.join(newline);
    if trailing_newline {
        output.push_str(newline);
    }
    output
}

#[cfg(target_os = "macos")]
const MACOS_SCRIPT_EXTENSIONS: &[&str] = &["command", "tool", "zsh", "csh", "sh", "pl"];

#[cfg(target_os = "macos")]
pub(crate) fn accepted_script_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            MACOS_SCRIPT_EXTENSIONS
                .iter()
                .any(|accepted| extension.eq_ignore_ascii_case(accepted))
        })
}

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
fn register_macos_script_handlers() -> Result<()> {
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

#[cfg(target_os = "macos")]
fn macos_script_handlers_are_zetta() -> bool {
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
#[path = "tests/default_terminal.rs"]
mod tests;
