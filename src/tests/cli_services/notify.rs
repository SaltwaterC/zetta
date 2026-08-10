use std::ffi::OsString;

use super::*;
use crate::cli_services::CliServiceCommand;

#[test]
fn notify_parser_accepts_summary_body_and_options() {
    assert_eq!(
        parse_notify_args([OsString::from("Build finished")]).unwrap(),
        CliServiceCommand::Notify(NotifyCommand {
            summary: "Build finished".to_owned(),
            body: None,
            app_name: None,
            icon: None,
            sound: None,
            timeout: None,
        })
    );

    assert_eq!(
        parse_notify_args([
            OsString::from("--app-name"),
            OsString::from("zetta"),
            OsString::from("--icon"),
            OsString::from("/usr/share/icons/zetta.png"),
            OsString::from("--sound"),
            OsString::from("message-new-instant"),
            OsString::from("--timeout"),
            OsString::from("never"),
            OsString::from("Build finished"),
            OsString::from("All tests passed"),
        ])
        .unwrap(),
        CliServiceCommand::Notify(NotifyCommand {
            summary: "Build finished".to_owned(),
            body: Some("All tests passed".to_owned()),
            app_name: Some("zetta".to_owned()),
            icon: Some("/usr/share/icons/zetta.png".to_owned()),
            sound: Some("message-new-instant".to_owned()),
            timeout: Some(NotificationTimeout::Never),
        })
    );

    let shorthand = parse_notify_args([
        OsString::from("-a"),
        OsString::from("zetta"),
        OsString::from("-i"),
        OsString::from("icon.png"),
        OsString::from("-s"),
        OsString::from("bell"),
        OsString::from("-t"),
        OsString::from("5000"),
        OsString::from("Build finished"),
    ])
    .unwrap();
    assert_eq!(
        shorthand,
        CliServiceCommand::Notify(NotifyCommand {
            summary: "Build finished".to_owned(),
            body: None,
            app_name: Some("zetta".to_owned()),
            icon: Some("icon.png".to_owned()),
            sound: Some("bell".to_owned()),
            timeout: Some(NotificationTimeout::Milliseconds(5000)),
        })
    );
}

#[test]
fn notify_requires_a_summary_and_rejects_invalid_options() {
    assert!(parse_notify_args([]).is_err());
    assert!(
        parse_notify_args([
            OsString::from("a"),
            OsString::from("b"),
            OsString::from("c"),
        ])
        .is_err()
    );
    assert!(parse_notify_args([OsString::from("--timeout"), OsString::from("soon")]).is_err());
    assert!(parse_notify_args([OsString::from("--unknown")]).is_err());
}

#[test]
fn notification_targets_require_two_positive_numeric_ids() {
    assert_eq!(
        parse_notification_target("42", "7"),
        Some(NotificationTarget {
            process_id: 42,
            attention_id: 7,
        })
    );
    for (process_id, attention_id) in [
        ("", "7"),
        ("not-a-process", "7"),
        ("0", "7"),
        ("42", ""),
        ("42", "not-an-attention"),
        ("42", "0"),
    ] {
        assert_eq!(parse_notification_target(process_id, attention_id), None);
    }
}

#[test]
fn notification_responses_only_focus_on_default_activation() {
    assert!(notification_response_activates_tab(
        &notify_rust::NotificationResponse::Default
    ));
    for response in [
        notify_rust::NotificationResponse::Action("default".to_owned()),
        notify_rust::NotificationResponse::Action("open".to_owned()),
        notify_rust::NotificationResponse::Reply("reply".to_owned()),
        notify_rust::NotificationResponse::Closed(notify_rust::CloseReason::Dismissed),
        notify_rust::NotificationResponse::Closed(notify_rust::CloseReason::Expired),
    ] {
        assert!(!notification_response_activates_tab(&response));
    }
}

#[cfg(linux_like)]
#[test]
fn targeted_linux_notifications_add_only_the_xdg_default_action() {
    let command = NotifyCommand {
        summary: "Build finished".to_owned(),
        body: Some("All tests passed".to_owned()),
        app_name: None,
        icon: Some("icon.png".to_owned()),
        sound: None,
        timeout: None,
    };
    let target = NotificationTarget {
        process_id: 42,
        attention_id: 7,
    };
    let targeted = build_notification(&command, Some(target), false).unwrap();
    assert_eq!(targeted.actions, ["default", ""]);

    let untargeted = build_notification(&command, None, false).unwrap();
    assert!(untargeted.actions.is_empty());
}

#[cfg(linux_like)]
#[test]
fn silent_notifications_keep_actions_but_suppress_all_sound_sources() {
    let command = NotifyCommand {
        summary: "Build finished".to_owned(),
        body: None,
        app_name: None,
        icon: Some("icon.png".to_owned()),
        sound: Some("message-new-instant".to_owned()),
        timeout: None,
    };
    let target = NotificationTarget {
        process_id: 42,
        attention_id: 7,
    };
    let notification = build_notification(&command, Some(target), true).unwrap();
    assert_eq!(notification.actions, ["default", ""]);
    assert!(
        notification
            .hints
            .contains(&notify_rust::Hint::SuppressSound(true))
    );
    assert!(!notification.hints.contains(&notify_rust::Hint::SoundName(
        "message-new-instant".to_owned()
    )));
}

#[test]
fn notification_worker_reexec_replays_explicit_notify_arguments() {
    let command = NotifyCommand {
        summary: "Build finished".to_owned(),
        body: Some("All tests passed".to_owned()),
        app_name: Some("zetta-ci".to_owned()),
        icon: Some("icon.png".to_owned()),
        sound: Some("zetta-ok".to_owned()),
        timeout: Some(NotificationTimeout::Milliseconds(5000)),
    };
    let arguments = notification_reexec_args(&command)
        .into_iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        arguments,
        vec![
            "notify",
            "--app-name",
            "zetta-ci",
            "--icon",
            "icon.png",
            "--sound",
            "zetta-ok",
            "--timeout",
            "5000",
            "Build finished",
            "All tests passed",
        ]
    );
}

#[test]
fn notifications_default_to_zetta_identity() {
    let command = NotifyCommand {
        summary: "Build finished".to_owned(),
        body: None,
        app_name: None,
        icon: None,
        sound: None,
        timeout: None,
    };
    let mut notification = notify_rust::Notification::new();
    notification.appname(notification_app_name(&command));
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    set_unix_notification_identity(&mut notification, &command).unwrap();

    assert_eq!(notification.appname, "Zetta");
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        assert!(
            notification
                .hints
                .contains(&notify_rust::Hint::DesktopEntry("Zetta".to_owned()))
        );
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[test]
fn custom_notification_identity_is_not_replaced_by_zetta_desktop_entry() {
    let command = NotifyCommand {
        summary: "Build finished".to_owned(),
        body: None,
        app_name: Some("wibble".to_owned()),
        icon: Some("custom.png".to_owned()),
        sound: None,
        timeout: None,
    };
    let mut notification = notify_rust::Notification::new();
    notification.appname(notification_app_name(&command));
    set_unix_notification_identity(&mut notification, &command).unwrap();

    assert_eq!(notification.appname, "wibble");
    assert_eq!(notification.icon, "custom.png");
    assert!(
        !notification
            .hints
            .contains(&notify_rust::Hint::DesktopEntry("Zetta".to_owned()))
    );
}

#[test]
fn default_notification_icon_is_cached_and_kept_up_to_date() {
    let directory = tempfile::tempdir().unwrap();
    let config_dir = directory.path().join("zetta");
    let expected = crate::zetta_assets::embedded_notification_icon().unwrap();

    let path = write_default_notification_icon(&config_dir).unwrap();
    assert_eq!(path, config_dir.join("notification-icon.png"));
    assert_eq!(std::fs::read(&path).unwrap(), *expected);

    // A stale or corrupted cached icon is rewritten rather than trusted as-is.
    std::fs::write(&path, b"stale").unwrap();
    write_default_notification_icon(&config_dir).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), *expected);
}

#[cfg(target_os = "macos")]
#[test]
fn macos_bundle_executable_resolves_a_cli_symlink() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("Zetta.app/Contents/MacOS/zetta");
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    std::fs::write(&executable, b"test executable").unwrap();
    let cli_link = directory.path().join("zetta");
    symlink(&executable, &cli_link).unwrap();
    let standalone = directory.path().join("standalone-zetta");
    std::fs::write(&standalone, b"standalone executable").unwrap();

    assert_eq!(
        macos_bundle_executable(&cli_link),
        executable.canonicalize().ok()
    );
    assert_eq!(macos_bundle_executable(&standalone), None);
}

#[cfg(target_os = "macos")]
#[test]
fn macos_only_attaches_an_explicit_notification_icon() {
    let mut command = NotifyCommand {
        summary: "Build finished".to_owned(),
        body: None,
        app_name: None,
        icon: None,
        sound: None,
        timeout: None,
    };
    assert_eq!(macos_notification_attachment(&command), None);

    command.icon = Some("custom.png".to_owned());
    assert_eq!(macos_notification_attachment(&command), Some("custom.png"));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_routes_system_sounds_through_notification_center() {
    let mut command = NotifyCommand {
        summary: "Build finished".to_owned(),
        body: None,
        app_name: None,
        icon: None,
        sound: Some("Ping".to_owned()),
        timeout: None,
    };
    assert_eq!(macos_notification_sound(&command), Some("Ping"));

    command.sound = Some("zetta-alarm".to_owned());
    assert_eq!(macos_notification_sound(&command), None);
}
