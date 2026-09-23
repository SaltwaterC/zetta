use super::*;

#[test]
fn proxy_arguments_preserve_all_notification_options() {
    let request = NotificationRequest {
        summary: "Build finished".to_owned(),
        body: Some("All tests passed".to_owned()),
        app_name: Some("CI".to_owned()),
        icon: Some("icon.png".to_owned()),
        sound: Some("zetta-ok".to_owned()),
        timeout: Some(NotificationTimeout::Milliseconds(5_000)),
    };
    let arguments = notification_reexec_args(&request);
    assert_eq!(arguments.first().unwrap(), "--app-name");
    assert_eq!(arguments.last().unwrap(), "All tests passed");
    assert_eq!(parse_notify_args(arguments).unwrap(), request);
}

#[cfg(target_os = "macos")]
#[test]
fn macos_response_only_routes_this_processes_body_click() {
    let tag = format!("zetta-target:{}:7:99-123-1", std::process::id());
    assert_eq!(
        macos_notification_target_for_response(&tag, None),
        Some(NotificationTarget {
            process_id: std::process::id(),
            attention_id: 7
        })
    );
    assert_eq!(
        macos_notification_target_for_response(&tag, Some("action")),
        None
    );
    assert_eq!(
        macos_notification_target_for_response("zetta-target:1:7:99-123-1", None),
        None
    );
}
