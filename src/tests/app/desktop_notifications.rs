use super::*;

fn notification(body: &str, expiry: NotificationExpiry) -> DesktopNotification {
    DesktopNotification {
        id: Some("job".to_owned()),
        title: "Build".to_owned(),
        body: body.to_owned(),
        app_name: Some("make".to_owned()),
        occasion: NotificationOccasion::Always,
        expiry,
    }
}

#[test]
fn each_occasion_shows_only_while_the_pane_is_out_of_sight_enough() {
    use NotificationOccasion::*;
    use PaneExposure::*;
    let shown =
        |occasion| [Focused, Visible, Hidden].map(|exposure| occasion_allows(occasion, exposure));
    assert_eq!(shown(Always), [true, true, true]);
    assert_eq!(shown(Unfocused), [false, true, true]);
    assert_eq!(shown(Invisible), [false, false, true]);
}

#[test]
fn the_request_carries_text_name_and_expiry_and_omits_an_empty_body() {
    let request = notification_request(&notification("", NotificationExpiry::Default));
    assert_eq!(request.summary, "Build");
    assert_eq!(request.body, None);
    assert_eq!(request.app_name.as_deref(), Some("make"));
    assert_eq!(request.timeout, None);

    let request = notification_request(&notification("done", NotificationExpiry::Never));
    assert_eq!(request.body.as_deref(), Some("done"));
    assert_eq!(request.timeout, Some(NotificationTimeout::Never));
    let request = notification_request(&notification("", NotificationExpiry::Milliseconds(250)));
    assert_eq!(
        request.timeout,
        Some(NotificationTimeout::Milliseconds(250))
    );
}
