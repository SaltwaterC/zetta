use super::*;

fn session_owned_by(owner: u32, protected: bool) -> Session {
    Session {
        id: 1,
        summary: BackgroundSessionSummary {
            id: 1,
            title: "shell".to_owned(),
            authentication_required: protected,
            active_pane: 1,
            layout: BackgroundPaneLayout::Pane { pane_id: 1 },
            panes: Vec::new(),
            held: false,
            scoped_to: None,
            key_envelope: None,
        },
        state: serde_json::Value::Null,
        shared_state: None,
        authentication: protected.then(|| SessionAuthentication::create("correct horse").unwrap()),
        key_envelope: None,
        failed_authentications: 0,
        refuse_until: None,
        panes: Vec::new(),
        keep: true,
        offered: false,
        owner: Some(owner),
    }
}

#[test]
fn a_protected_sessions_owner_has_to_be_vouched_for_not_claimed() {
    // The hole this closes: where the platform reports no peer credentials, the
    // owner used to be whatever the envelope said it was. Anyone able to read
    // the endpoint token could then name the real owner and detach, kill or
    // rescope a protected session without ever presenting its secret.
    let mut session = session_owned_by(4321, true);

    assert!(
        session_control_authorized(&mut session, Some(4321), None),
        "the owner, vouched for, must still be authorized"
    );
    assert!(
        !session_control_authorized(&mut session, Some(9999), None),
        "another process must not be authorized"
    );
    assert!(
        !session_control_authorized(&mut session, None, None),
        "an unvouched-for peer must not be authorized, whatever it claims"
    );
    // Holder-only controls — resize, palette — are stricter still: not even the
    // owner qualifies, and an unattested peer never does.
    assert!(!protected_holder_authorized(&session, Some(4321)));
    assert!(!protected_holder_authorized(&session, None));

    // A session with no secret is a different question: any same-user process
    // can attach one for itself, so confirming which process is asking would
    // protect nothing, and controls stay open as they always were.
    let mut open = session_owned_by(4321, false);
    assert!(session_control_authorized(&mut open, None, None));
}

fn draft(
    profile: &str,
    command: Option<zetta_profiles::ProfileCommand>,
) -> crate::messages::SharedPaneDraft {
    use crate::protocol::{BackgroundPaneState, BackgroundPaneSummary};

    crate::messages::SharedPaneDraft {
        draft_id: 1,
        profile: profile.to_owned(),
        command,
        env: HashMap::from([
            ("ZETTA_PANE_ROUTING_ID".to_owned(), "7".to_owned()),
            ("PATH".to_owned(), "/the/requesters/path".to_owned()),
        ]),
        working_directory: None,
        inherit_working_directory_from: None,
        load_shell_integration: false,
        size: TerminalSize {
            columns: 80,
            lines: 24,
            cell_width: 0,
            cell_height: 0,
        },
        console_palette: ConsolePalette::default(),
        metadata: BackgroundPaneSummary {
            id: 0,
            label: "pane".to_owned(),
            profile: profile.to_owned(),
            configured_command: String::new(),
            application: "sh".to_owned(),
            foreground_command: None,
            terminal_title: None,
            working_directory: None,
            state: BackgroundPaneState::Running,
            exit: None,
        },
    }
}

/// A daemon has no `TERM` of its own — it is a background process — so a pane
/// it starts inherits none. That is what left a shell in a shared session
/// drawing a monochrome prompt beside identical panes that had colour.
#[test]
fn a_shared_draft_is_given_zettas_terminal_environment() {
    let (_, env) = shared_draft_process(&draft("System", None));

    assert_eq!(env["TERM"], "xterm-256color");
    assert_eq!(env["COLORTERM"], "truecolor");
    assert_eq!(env["TERM_PROGRAM"], "zetta");
    assert_eq!(env["ZETTA_TERM"], "true");
}

#[test]
fn a_shared_draft_keeps_the_requesters_routing_identity() {
    let (_, env) = shared_draft_process(&draft("System", None));

    assert_eq!(
        env.get("ZETTA_PANE_ROUTING_ID").map(String::as_str),
        Some("7"),
        "the pane's identity is the requesting window's to assign"
    );
}

/// A profile name this host does not have still has to produce a working
/// shell: the name came from another machine's session, and refusing it would
/// leave the viewer with a pane that never starts.
#[test]
fn an_unknown_profile_falls_back_to_the_hosts_login_shell() {
    let (command, _) = shared_draft_process(&draft("A Profile This Host Has Never Heard Of", None));

    assert_eq!(command, zetta_profiles::ProfileCommand::system());
}

/// A requester on this host resolved the name against the very configuration
/// the daemon would read, and its resolution carries the working-directory
/// tracking wrappers a name alone cannot express.
#[test]
fn a_command_resolved_on_this_host_is_used_as_it_was_sent() {
    let wrapped = zetta_profiles::ProfileCommand::with_args(
        "cmd.exe",
        vec!["/c".to_owned(), "msys2_shell.cmd".to_owned()],
    );

    let (command, _) = shared_draft_process(&draft("MSYS2", Some(wrapped.clone())));

    assert_eq!(command, wrapped);
}
