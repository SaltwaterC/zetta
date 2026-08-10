use std::ffi::OsString;
#[cfg(any(not(target_os = "macos"), test))]
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result};

use super::{
    CliServiceCommand, NotificationRequest, NotificationTarget, NotificationTimeout,
    parse_notification_timeout,
};

pub(crate) type NotifyCommand = NotificationRequest;

const NOTIFICATION_WORKER_ENV: &str = "ZETTA_NOTIFICATION_WORKER";
const NOTIFICATION_TARGET_PROCESS_ID_ENV: &str = "ZETTA_NOTIFICATION_TARGET_PROCESS_ID";
const NOTIFICATION_TARGET_ATTENTION_ID_ENV: &str = "ZETTA_NOTIFICATION_TARGET_ATTENTION_ID";

pub(crate) fn parse_notification_target(
    process_id: &str,
    attention_id: &str,
) -> Option<NotificationTarget> {
    let process_id = process_id.parse::<u32>().ok().filter(|id| *id != 0)?;
    let attention_id = attention_id.parse::<u64>().ok().filter(|id| *id != 0)?;
    Some(NotificationTarget {
        process_id,
        attention_id,
    })
}

pub(crate) fn notification_target_from_environment() -> Option<NotificationTarget> {
    parse_notification_target(
        &std::env::var("ZETTA_PROCESS_ID").ok()?,
        &std::env::var("ZETTA_ATTENTION_ID").ok()?,
    )
}

fn notification_target_from_worker_environment() -> Result<NotificationTarget> {
    let process_id = std::env::var(NOTIFICATION_TARGET_PROCESS_ID_ENV)
        .context("notification worker is missing its target process ID")?;
    let attention_id = std::env::var(NOTIFICATION_TARGET_ATTENTION_ID_ENV)
        .context("notification worker is missing its target attention ID")?;
    parse_notification_target(&process_id, &attention_id)
        .context("notification worker has an invalid target")
}

pub(crate) fn notify_help() -> &'static str {
    "Show a desktop notification\n\nUsage: zetta notify [OPTIONS] SUMMARY [BODY]\n\nSUMMARY is the notification's title; BODY is optional additional text.\n\nOptions:\n  -a, --app-name NAME                Set the notification's application name\n  -i, --icon PATH                    Show an image from PATH with the notification (default: Zetta's icon)\n  -s, --sound NAME                   zetta-default, zetta-ok, zetta-alarm, or a platform-specific system sound name\n  -t, --timeout WHEN                 default, never, or a number of milliseconds (default: default)\n  -h, --help                         Print help\n\nShows the notification through the desktop's native notification system: D-Bus\non Linux and BSD, Notification Center on macOS, and toast notifications on\nWindows. Without --icon, Zetta's own icon is shown; it is bundled in the\nbinary, so it is always available. --app-name has no effect on macOS and\n--timeout is ignored by some macOS notification centers; every other option\nbehaves the same on all platforms.\n\n--sound zetta-default, zetta-ok, and zetta-alarm are bundled tones that Zetta\nsynthesizes and plays itself, so they always play the same way regardless of\nthe host's sound theme or configuration. Any other value is passed through as\na platform-specific system sound name (for example a freedesktop sound-theme\nname on Linux, a system sound name on macOS, or a toast sound identifier on\nWindows) and is only played if the platform recognizes it."
}

pub(crate) fn parse_notify_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<CliServiceCommand> {
    let mut app_name = None;
    let mut icon = None;
    let mut sound = None;
    let mut timeout = None;
    let mut positional = Vec::new();
    let mut arguments = args.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.to_string_lossy().as_ref() {
            "--app-name" | "-a" => {
                anyhow::ensure!(app_name.is_none(), "--app-name may only be specified once");
                app_name = Some(
                    arguments
                        .next()
                        .context("--app-name requires a name")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            "--icon" | "-i" => {
                anyhow::ensure!(icon.is_none(), "--icon may only be specified once");
                icon = Some(
                    arguments
                        .next()
                        .context("--icon requires a path")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            "--sound" | "-s" => {
                anyhow::ensure!(sound.is_none(), "--sound may only be specified once");
                sound = Some(
                    arguments
                        .next()
                        .context("--sound requires a name")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            "--timeout" | "-t" => {
                anyhow::ensure!(timeout.is_none(), "--timeout may only be specified once");
                let value = arguments
                    .next()
                    .context("--timeout requires default, never, or a number of milliseconds")?
                    .to_string_lossy()
                    .into_owned();
                timeout = Some(parse_notification_timeout(&value)?);
            }
            "--help" | "-h" => anyhow::bail!("{}", notify_help()),
            option if option.starts_with('-') => anyhow::bail!("unknown notify option {option:?}"),
            _ => positional.push(argument),
        }
    }
    anyhow::ensure!(
        (1..=2).contains(&positional.len()),
        "usage: zetta notify [OPTIONS] SUMMARY [BODY]; run `zetta notify --help` for details"
    );
    let summary = positional[0].to_string_lossy().into_owned();
    anyhow::ensure!(!summary.is_empty(), "SUMMARY must not be empty");
    let body = positional
        .get(1)
        .map(|value| value.to_string_lossy().into_owned());
    Ok(CliServiceCommand::Notify(NotifyCommand {
        summary,
        body,
        app_name,
        icon,
        sound,
        timeout,
    }))
}

#[cfg(not(target_os = "macos"))]
fn default_notification_icon_path() -> Result<PathBuf> {
    write_default_notification_icon(&crate::config::platform_config_dir())
}

#[cfg(any(not(target_os = "macos"), test))]
fn notification_app_name(command: &NotifyCommand) -> &str {
    command.app_name.as_deref().unwrap_or(crate::ZETTA_APP_ID)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn notification_icon_path(command: &NotifyCommand) -> Result<String> {
    Ok(match &command.icon {
        Some(icon) => icon.clone(),
        None => default_notification_icon_path()?
            .to_string_lossy()
            .into_owned(),
    })
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn set_unix_notification_identity(
    notification: &mut notify_rust::Notification,
    command: &NotifyCommand,
) -> Result<()> {
    let icon = notification_icon_path(command)?;
    if command.app_name.is_none() && command.icon.is_none() {
        notification.hint(notify_rust::Hint::DesktopEntry(
            crate::ZETTA_APP_ID.to_owned(),
        ));
    }
    notification.icon(&icon);
    Ok(())
}

#[cfg(linux_like)]
fn try_show_portal_notification(command: &NotifyCommand) -> Result<bool> {
    // Unlike org.freedesktop.Notifications, the portal contract explicitly
    // requires notifications to outlive the process that submitted them.
    // The portal does not expose the millisecond timeout supported by
    // notify-rust, and it cannot override the application identity, so keep
    // those cases on the D-Bus fallback below.
    if command.app_name.is_some()
        || command.icon.is_some()
        || matches!(
            command.timeout,
            Some(NotificationTimeout::Milliseconds(_)) | Some(NotificationTimeout::Never)
        )
        || command
            .sound
            .as_deref()
            .is_some_and(|sound| crate::notification_sounds::BuiltinSound::parse(sound).is_none())
    {
        return Ok(false);
    }

    let icon = notification_icon_path(command)?;
    let icon_uri = match url::Url::from_file_path(&icon) {
        Ok(icon_uri) => icon_uri,
        Err(()) => return Ok(false),
    };
    let portal_notification = ashpd::desktop::notification::Notification::new(&command.summary)
        .body(command.body.as_deref())
        .icon(ashpd::desktop::Icon::Uri(ashpd::Uri::parse(
            icon_uri.as_str(),
        )?));
    let notification_id = format!("zetta-{}", std::process::id());
    let sent = futures::executor::block_on(async {
        let proxy = ashpd::desktop::notification::NotificationProxy::new().await?;
        proxy
            .add_notification(&notification_id, portal_notification)
            .await
    });
    Ok(sent.is_ok())
}

#[cfg(linux_like)]
const NOTIFICATION_DAEMON_ENV: &str = "ZETTA_NOTIFICATION_DAEMON";

fn notification_worker_executable() -> Result<Option<PathBuf>> {
    let current_executable = std::env::current_exe().context("locating the Zetta executable")?;
    #[cfg(target_os = "macos")]
    {
        // The modern macOS notification backend requires an application bundle.
        // Returning the canonical bundle executable also makes a CLI symlink
        // re-enter the signed app instead of the unbundled development binary.
        Ok(macos_bundle_executable(&current_executable))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(Some(current_executable))
    }
}

fn spawn_notification_worker(
    notification: &NotificationRequest,
    target: NotificationTarget,
    executable: PathBuf,
) -> Result<()> {
    let mut command = Command::new(executable);
    command
        .args(notification_reexec_args(notification))
        .env(NOTIFICATION_WORKER_ENV, "1")
        .env(
            NOTIFICATION_TARGET_PROCESS_ID_ENV,
            target.process_id.to_string(),
        )
        .env(
            NOTIFICATION_TARGET_ATTENTION_ID_ENV,
            target.attention_id.to_string(),
        )
        // The worker has its own authenticated target. Do not let it derive a
        // second target from the shell environment it inherited from the pane.
        .env_remove("ZETTA_PROCESS_ID")
        .env_remove("ZETTA_ATTENTION_ID")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // SAFETY: setsid(2) is async-signal-safe and is the only call made in
        // the forked child before it execs.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const DETACHED_PROCESS: u32 = 0x00000008;
        command.creation_flags(DETACHED_PROCESS);
    }

    command
        .spawn()
        .context("spawning the targeted desktop notification worker")?;
    Ok(())
}

#[cfg(linux_like)]
fn spawn_notification_daemon(notification: &NotificationRequest) -> Result<()> {
    let executable = std::env::current_exe().context("locating the zetta executable")?;
    let mut command = Command::new(executable);
    command
        .args(notification_reexec_args(notification))
        .env(NOTIFICATION_DAEMON_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Detach the notification worker from the terminal that invoked the CLI.
    // It must keep its D-Bus connection alive after this parent exits so GNOME
    // does not withdraw the notification with the sender process.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // SAFETY: setsid(2) is async-signal-safe and is the only call made in
        // the forked child before it execs.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    command
        .spawn()
        .context("spawning the desktop notification worker")?;
    Ok(())
}

#[cfg(linux_like)]
fn keep_notification_worker_alive(timeout: Option<NotificationTimeout>) {
    let timeout = timeout.unwrap_or_default();
    match timeout {
        // GNOME's default is commonly five seconds. Keep the sender alive a
        // little longer so the server can expire and archive the notification
        // instead of treating the worker's exit as a dismissal.
        NotificationTimeout::Default => std::thread::sleep(std::time::Duration::from_secs(10)),
        NotificationTimeout::Milliseconds(milliseconds) => {
            if milliseconds == 0 {
                // A never-expiring notification needs a live sender. This is
                // intentionally indefinite, matching the notification's
                // lifetime.
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(60));
                }
            } else {
                std::thread::sleep(
                    std::time::Duration::from_millis(u64::from(milliseconds))
                        + std::time::Duration::from_secs(1),
                );
            }
        }
        // A never-expiring notification needs a live sender. This is
        // intentionally indefinite, matching the notification's lifetime.
        NotificationTimeout::Never => loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        },
    }
}

fn notify_rust_timeout(timeout: NotificationTimeout) -> notify_rust::Timeout {
    match timeout {
        NotificationTimeout::Default => notify_rust::Timeout::Default,
        NotificationTimeout::Never => notify_rust::Timeout::Never,
        NotificationTimeout::Milliseconds(milliseconds) => {
            notify_rust::Timeout::Milliseconds(milliseconds)
        }
    }
}

/// Re-enter the notification-only startup mode when a platform requires a
/// child process. In particular, an `attention --notify` request must not be
/// replayed as `attention`, or the child would route the badge a second time.
pub(crate) fn notification_reexec_args(command: &NotificationRequest) -> Vec<OsString> {
    let mut args = vec![OsString::from("notify")];
    for (option, value) in [
        ("--app-name", command.app_name.as_deref()),
        ("--icon", command.icon.as_deref()),
        ("--sound", command.sound.as_deref()),
    ] {
        if let Some(value) = value {
            args.push(OsString::from(option));
            args.push(OsString::from(value));
        }
    }
    if let Some(timeout) = command.timeout {
        args.push(OsString::from("--timeout"));
        args.push(OsString::from(match timeout {
            NotificationTimeout::Default => "default".to_owned(),
            NotificationTimeout::Never => "never".to_owned(),
            NotificationTimeout::Milliseconds(milliseconds) => milliseconds.to_string(),
        }));
    }
    args.push(OsString::from(&command.summary));
    if let Some(body) = &command.body {
        args.push(OsString::from(body));
    }
    args
}

// Without an explicit `App User Model ID`, notify-rust's Windows backend
// (tauri-winrt-notification) falls back to `Toast::POWERSHELL_APP_ID` - a
// built-in Windows AUMID whose own doc comment warns the toast "will
// erroneously report its origin as powershell", with PowerShell's icon.
// Register Zetta's own AUMID (idempotent; cheap enough to redo on every
// `zetta notify` invocation, mirroring `register_app_user_model_id` in
// crates/gpui_windows/src/system_notifications.rs) and point the toast at it.
//
// `IconUri` must be a plain path to an image file - unlike a shortcut's
// `IconLocation`, it does not understand the `<path>,<index>` resource syntax,
// so pointing it at the exe itself silently produces a blank icon. Reuse the
// same on-disk icon already passed to `Notification::image_path`.
#[cfg(target_os = "windows")]
fn register_windows_notification_identity(
    notification: &mut notify_rust::Notification,
    icon_path: &Path,
) {
    let result = windows_registry::CURRENT_USER
        .create(format!(
            r"Software\Classes\AppUserModelId\{}",
            crate::ZETTA_APP_ID
        ))
        .and_then(|key| {
            key.set_string("DisplayName", "Zetta")?;
            key.set_string("IconBackgroundColor", "0")?;
            key.set_hstring("IconUri", &icon_path.into())
        });
    if let Err(error) = result {
        eprintln!(
            "zetta: failed to register AppUserModelID; notifications may not display correctly: {error}"
        );
    }
    notification.app_id(crate::ZETTA_APP_ID);
}

// D-Bus and winrt-notification take an icon as a filesystem path rather than
// raw bytes, so the icon embedded via ZettaEmbeddedAssets is cached on disk
// once and reused rather than rewritten on every `zetta notify` invocation.
#[cfg(any(not(target_os = "macos"), test))]
fn write_default_notification_icon(config_dir: &Path) -> Result<PathBuf> {
    let icon = crate::zetta_assets::embedded_notification_icon()
        .context("embedded notification icon is missing")?;
    let path = config_dir.join("notification-icon.png");
    let up_to_date = fs::read(&path).is_ok_and(|existing| existing == *icon);
    if !up_to_date {
        fs::create_dir_all(config_dir)
            .with_context(|| format!("creating {}", config_dir.display()))?;
        fs::write(&path, &icon).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(path)
}

#[cfg(target_os = "macos")]
const MACOS_NOTIFICATION_REEXEC_ENV: &str = "ZETTA_INTERNAL_NOTIFICATION_BUNDLE_REEXEC";

#[cfg(target_os = "macos")]
fn macos_bundle_executable(path: &Path) -> Option<PathBuf> {
    let executable = path.canonicalize().ok()?;
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle
            .extension()
            .is_some_and(|extension| extension == "app"))
    .then_some(executable)
}

/// A process entered through `/usr/local/bin/zetta` does not inherit the
/// bundle identity of the signed executable behind that symlink. Re-enter the
/// exact same command through its canonical `.app` path so Notification Center
/// sees Zetta's bundle identifier. Standalone development builds return false
/// and use the script-host fallback below instead.
#[cfg(target_os = "macos")]
fn rerun_notification_from_macos_bundle(notification: &NotificationRequest) -> Result<bool> {
    if std::env::var_os(MACOS_NOTIFICATION_REEXEC_ENV).is_some() {
        return Ok(false);
    }
    let current_executable = std::env::current_exe().context("locating the Zetta executable")?;
    let Some(bundle_executable) = macos_bundle_executable(&current_executable) else {
        return Ok(false);
    };
    let output = Command::new(&bundle_executable)
        .args(notification_reexec_args(notification))
        .env(MACOS_NOTIFICATION_REEXEC_ENV, "1")
        .output()
        .with_context(|| {
            format!(
                "restarting notification command through {}",
                bundle_executable.display()
            )
        })?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        let message = message
            .trim()
            .strip_prefix("Zetta failed to start: ")
            .unwrap_or(message.trim());
        anyhow::bail!(
            "{}",
            if message.is_empty() {
                format!(
                    "bundled macOS notification command exited with {}",
                    output.status
                )
            } else {
                message.to_owned()
            }
        );
    }
    Ok(true)
}

/// `UNUserNotificationCenter` rejects binaries that are not inside an app
/// bundle. Keep `target/debug/zetta notify` and other standalone copies useful
/// by asking macOS's bundled script host to submit the notification instead.
#[cfg(target_os = "macos")]
fn show_unbundled_macos_notification(command: &NotifyCommand, sound: Option<&str>) -> Result<()> {
    const SCRIPT: &str = r#"
function run(argv) {
    const app = Application.currentApplication();
    app.includeStandardAdditions = true;
    const options = { withTitle: argv[0] };
    if (argv[2]) options.subtitle = argv[2];
    if (argv[3]) options.soundName = argv[3];
    app.displayNotification(argv[1], options);
}
"#;
    let status = Command::new("/usr/bin/osascript")
        .args(["-l", "JavaScript", "-e", SCRIPT, "--"])
        .arg(&command.summary)
        .arg(command.body.as_deref().unwrap_or_default())
        .arg(command.app_name.as_deref().unwrap_or("Zetta"))
        .arg(sound.unwrap_or_default())
        .status()
        .context("showing an unbundled macOS desktop notification")?;
    anyhow::ensure!(
        status.success(),
        "macOS notification script exited with {status}"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn show_bundled_macos_notification(command: &NotifyCommand, sound: Option<&str>) -> Result<()> {
    let authorized = mac_usernotifications::blocking::request_auth()
        .map_err(|error| anyhow::anyhow!("{error}"))
        .context("requesting macOS desktop notification authorization")?;
    anyhow::ensure!(
        authorized,
        "macOS desktop notification authorization was denied; enable notifications for Zetta in System Settings"
    );

    let mut notification = mac_usernotifications::Notification::new()
        .title(&command.summary)
        .message(command.body.as_deref().unwrap_or_default())
        .maybe_sound(sound);
    // The signed app bundle already supplies the small Zetta identity icon.
    // Only an explicit --icon is an attachment; adding the embedded icon here
    // would render it a second time on the opposite side of the banner.
    if let Some(icon) = macos_notification_attachment(command) {
        notification = notification.image_path(icon);
    }
    mac_usernotifications::blocking::send(notification)
        .map_err(|error| anyhow::anyhow!("{error}"))
        .context("showing the desktop notification")?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn show_targeted_bundled_macos_notification(
    command: &NotifyCommand,
    bundled_sound: Option<crate::notification_sounds::BuiltinSound>,
    sound: Option<&str>,
) -> Result<notify_rust::NotificationResponse> {
    let authorized = mac_usernotifications::blocking::request_auth()
        .map_err(|error| anyhow::anyhow!("{error}"))
        .context("requesting macOS desktop notification authorization")?;
    anyhow::ensure!(
        authorized,
        "macOS desktop notification authorization was denied; enable notifications for Zetta in System Settings"
    );

    let mut notification = notify_rust::Notification::new();
    notification.summary(&command.summary);
    if let Some(body) = &command.body {
        notification.body(body);
    }
    // The signed app bundle supplies Zetta's identity. Only an explicit icon is
    // an attachment, matching the existing bundled macOS notification path.
    if let Some(icon) = macos_notification_attachment(command) {
        notification.image_path(icon);
    }
    if let Some(sound) = sound {
        notification.sound_name(sound);
    }
    if let Some(timeout) = command.timeout {
        notification.timeout(notify_rust_timeout(timeout));
    }
    let handle = notification
        .show()
        .map_err(|error| anyhow::anyhow!("{error}"))
        .context("showing the desktop notification")?;
    if let Some(sound) = bundled_sound {
        sound.play()?;
    }

    let mut response = None;
    handle
        .wait_for_response(|received: &notify_rust::NotificationResponse| {
            response = Some(received.clone())
        })
        .map_err(|error| anyhow::anyhow!("{error}"))
        .context("waiting for the desktop notification response")?;
    response.context("desktop notification returned no response")
}

#[cfg(target_os = "macos")]
fn macos_notification_attachment(command: &NotifyCommand) -> Option<&str> {
    command.icon.as_deref()
}

#[cfg(target_os = "macos")]
fn macos_notification_sound(command: &NotifyCommand) -> Option<&str> {
    command
        .sound
        .as_deref()
        .filter(|sound| crate::notification_sounds::BuiltinSound::parse(sound).is_none())
}

fn notification_response_activates_tab(response: &notify_rust::NotificationResponse) -> bool {
    response.is_default_action()
}

#[cfg(not(target_os = "macos"))]
fn build_notification(
    command: &NotifyCommand,
    _target: Option<NotificationTarget>,
) -> Result<notify_rust::Notification> {
    let mut notification = notify_rust::Notification::new();
    notification.summary(&command.summary);
    if let Some(body) = &command.body {
        notification.body(body);
    }
    notification.appname(notification_app_name(command));
    #[cfg(target_os = "windows")]
    {
        // notify-rust's Windows backend has no small "app logo" placement -
        // any icon passed to `image_path` renders as a large image below
        // the notification text. That's right for a user's deliberately
        // attached `--icon`, but Zetta's default icon shouldn't also be
        // shown that way: `register_windows_notification_identity` already
        // makes it appear correctly-sized next to the app name via the
        // AUMID registration, so only attach an inline image when the user
        // explicitly asked for one.
        register_windows_notification_identity(
            &mut notification,
            &default_notification_icon_path()?,
        );
        if let Some(icon) = &command.icon {
            notification.image_path(icon);
        }
    }
    #[cfg(not(target_os = "windows"))]
    set_unix_notification_identity(&mut notification, command)?;

    let bundled_sound = command
        .sound
        .as_deref()
        .and_then(crate::notification_sounds::BuiltinSound::parse);
    if let Some(sound) = command.sound.as_deref().filter(|_| bundled_sound.is_none()) {
        notification.sound_name(sound);
    }
    if let Some(timeout) = command.timeout {
        notification.timeout(notify_rust_timeout(timeout));
    }
    #[cfg(linux_like)]
    if _target.is_some() {
        // An explicit default action makes body clicks observable on XDG
        // notification daemons while keeping the action button itself blank.
        notification.action("default", "");
    }
    Ok(notification)
}

pub(crate) fn run_notification(
    command: &NotificationRequest,
    target: Option<NotificationTarget>,
) -> Result<()> {
    if std::env::var_os(NOTIFICATION_WORKER_ENV).is_some() {
        return command.run(Some(notification_target_from_worker_environment()?));
    }
    if let Some(target) = target
        && let Some(executable) = notification_worker_executable()?
    {
        return spawn_notification_worker(command, target, executable);
    }
    // Unbundled macOS development binaries deliberately remain fire-and-forget.
    command.run(None)
}

impl NotificationRequest {
    pub(super) fn run(&self, target: Option<NotificationTarget>) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            self.run_macos(target)
        }

        #[cfg(not(target_os = "macos"))]
        {
            self.run_non_macos(target)
        }
    }

    #[cfg(target_os = "macos")]
    fn run_macos(&self, target: Option<NotificationTarget>) -> Result<()> {
        let bundled = mac_usernotifications::check_bundle().is_ok();
        if !bundled && target.is_none() && rerun_notification_from_macos_bundle(self)? {
            return Ok(());
        }

        let bundled_sound = self
            .sound
            .as_deref()
            .and_then(crate::notification_sounds::BuiltinSound::parse);
        let notification_sound = macos_notification_sound(self);

        if let Some(target) = target
            && bundled
        {
            let response =
                show_targeted_bundled_macos_notification(self, bundled_sound, notification_sound)?;
            if notification_response_activates_tab(&response) {
                let _ = crate::process_control::request_process_focus_tab(
                    target.process_id,
                    target.attention_id,
                );
            }
        } else if bundled {
            show_bundled_macos_notification(self, notification_sound)?;
            if let Some(sound) = bundled_sound {
                sound.play()?;
            }
        } else {
            show_unbundled_macos_notification(self, notification_sound)?;
        }
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    fn run_non_macos(&self, target: Option<NotificationTarget>) -> Result<()> {
        let bundled_sound = self
            .sound
            .as_deref()
            .and_then(crate::notification_sounds::BuiltinSound::parse);

        #[cfg(linux_like)]
        if target.is_none()
            && (bundled_sound.is_some() || self.sound.is_none())
            && try_show_portal_notification(self)?
        {
            if let Some(bundled_sound) = bundled_sound {
                bundled_sound.play()?;
            }
            return Ok(());
        }

        #[cfg(linux_like)]
        if target.is_none() && std::env::var_os(NOTIFICATION_DAEMON_ENV).is_none() {
            return spawn_notification_daemon(self);
        }

        let notification = build_notification(self, target)?;
        let notification_handle = notification
            .show()
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("showing the desktop notification")?;
        if let Some(bundled_sound) = bundled_sound {
            bundled_sound.play()?;
        }
        if let Some(target) = target {
            let mut response = None;
            notification_handle
                .wait_for_response(|received: &notify_rust::NotificationResponse| {
                    response = Some(received.clone())
                })
                .map_err(|error| anyhow::anyhow!("{error}"))
                .context("waiting for the desktop notification response")?;
            if response
                .as_ref()
                .is_some_and(notification_response_activates_tab)
            {
                let _ = crate::process_control::request_process_focus_tab(
                    target.process_id,
                    target.attention_id,
                );
            }
        } else {
            #[cfg(linux_like)]
            if std::env::var_os(NOTIFICATION_DAEMON_ENV).is_some() {
                keep_notification_worker_alive(self.timeout);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/cli_services/notify.rs"]
mod tests;
