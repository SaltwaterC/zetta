use super::*;

/// The dialog's choices, which every action that widens a session's reach shares.
///
/// Detaching has always taken an empty pair to mean "leave it unprotected", and
/// keeping a tab running the same. Sharing follows the same rule rather than
/// refusing the empty pair: a third dialog that behaved differently would be the
/// odd one out, whichever way it differed.
#[test]
fn an_empty_session_authentication_selects_the_unprotected_path() {
    assert_eq!(
        session_authentication_choice("", ""),
        SessionAuthenticationChoice::Unprotected
    );
}

#[test]
fn matching_non_empty_session_authentication_selects_the_protected_path() {
    assert_eq!(
        session_authentication_choice("secret", "secret"),
        SessionAuthenticationChoice::Protected
    );
}

#[test]
fn partial_or_mismatched_session_authentication_is_incomplete() {
    for (secret, confirmation) in [("secret", ""), ("", "secret"), ("one", "two")] {
        assert_eq!(
            session_authentication_choice(secret, confirmation),
            SessionAuthenticationChoice::Incomplete
        );
    }
}

fn empty_test_zetta(window: &mut Window, cx: &mut Context<Zetta>) -> Zetta {
    let mut config = Config::defaults(None, None);
    config.profiles.clear();
    Zetta::new(
        config,
        None,
        ZettaLaunchOptions {
            no_mux: true,
            ..Default::default()
        },
        window,
        cx,
    )
}

fn init_test_theme(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme_settings::init(theme::LoadThemes::All(Box::new(ZettaAssets)), cx);
        let registry = ThemeRegistry::global(cx);
        GlobalTheme::update_theme(cx, registry.get("One Light").unwrap());
    });
}

#[gpui::test]
fn a_session_operation_failure_becomes_a_transient_notice_without_a_prompt(
    cx: &mut gpui::TestAppContext,
) {
    init_test_theme(cx);
    let (zetta, cx) = cx.add_window_view(empty_test_zetta);
    cx.update_entity(&zetta, |zetta, cx| {
        zetta.show_session_operation_error("Could not attach that session", cx);
    });

    cx.read_entity(&zetta, |zetta, _| {
        assert_eq!(
            zetta.transient_notice.message(),
            Some("Could not attach that session")
        );
        assert!(zetta.pane_output_error.is_none());
    });
}

#[gpui::test]
fn a_session_operation_failure_stays_in_the_authentication_prompt(cx: &mut gpui::TestAppContext) {
    init_test_theme(cx);
    let (zetta, cx) = cx.add_window_view(|window, cx| {
        let mut zetta = empty_test_zetta(window, cx);
        zetta.open_session_authentication_prompt(
            SessionAuthenticationPromptMode::Reconnect {
                runner_id: 1,
                session_id: 2,
            },
            window,
            cx,
        );
        zetta
    });
    cx.update_entity(&zetta, |zetta, cx| {
        zetta.show_session_operation_error("Could not open the session", cx);
    });

    cx.read_entity(&zetta, |zetta, _| {
        assert_eq!(
            zetta
                .session_authentication
                .as_ref()
                .and_then(|prompt| prompt.error.as_deref()),
            Some("Could not open the session")
        );
        assert!(zetta.transient_notice.message().is_none());
        assert!(zetta.pane_output_error.is_none());
    });
}
