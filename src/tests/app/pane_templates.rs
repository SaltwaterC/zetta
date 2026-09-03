use super::*;

#[test]
fn cli_replacement_profile_resolution_is_case_insensitive_and_preserves_split_defaults() {
    let profiles = [
        Profile {
            name: "System".to_owned(),
            command: Shell::System,
            theme: Some("Configured Theme".to_owned()),
            dark_theme: None,
            icon: ProfileIcon::Zetta,
        },
        Profile {
            name: "Alternate".to_owned(),
            command: Shell::Program("alternate-shell".to_owned()),
            theme: None,
            dark_theme: None,
            icon: ProfileIcon::Zetta,
        },
    ];

    let selected =
        resolve_cli_replacement_profile(&profiles, Some("sYsTeM"), Some("Dracula"), None)
            .unwrap()
            .unwrap();
    assert_eq!(selected.name, "System");
    assert_eq!(selected.theme.as_deref(), Some("Dracula"));
    assert_eq!(selected.dark_theme.as_deref(), Some("Dracula"));
    assert_eq!(profiles[0].theme.as_deref(), Some("Configured Theme"));

    assert_eq!(
        resolve_cli_replacement_profile(&profiles, None, None, None),
        Some(None)
    );
    assert!(resolve_cli_replacement_profile(&profiles, Some("missing"), None, None).is_none());
    assert!(resolve_cli_replacement_profile(&profiles, None, Some("Dracula"), None).is_none());
    assert!(resolve_cli_replacement_profile(&profiles, Some("System"), Some(""), None).is_none());
}

#[test]
fn cli_replacement_profile_resolution_requires_the_exact_homebrew_name() {
    let homebrew_profile = Profile {
        name: "Fish (Homebrew)".to_owned(),
        command: Shell::Program("/opt/homebrew/bin/fish".to_owned()),
        theme: Some("Homebrew Theme".to_owned()),
        dark_theme: None,
        icon: ProfileIcon::Fish,
    };
    let launch_theme_override = ("fish (homebrew)".to_owned(), "Launch Theme".to_owned());

    assert!(
        resolve_cli_replacement_profile(
            std::slice::from_ref(&homebrew_profile),
            Some("fIsH"),
            None,
            None,
        )
        .is_none()
    );

    let selected = resolve_cli_replacement_profile(
        std::slice::from_ref(&homebrew_profile),
        Some("fIsH (hOmEbReW)"),
        None,
        Some(&launch_theme_override),
    )
    .unwrap()
    .unwrap();
    assert_eq!(selected.name, "Fish (Homebrew)");
    assert_eq!(selected.theme.as_deref(), Some("Launch Theme"));
    assert_eq!(selected.dark_theme.as_deref(), Some("Launch Theme"));

    let exact_profile = Profile {
        name: "Fish".to_owned(),
        command: Shell::Program("fish".to_owned()),
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Fish,
    };
    let profiles = [homebrew_profile, exact_profile];
    let selected = resolve_cli_replacement_profile(&profiles, Some("fIsH"), None, None)
        .unwrap()
        .unwrap();
    assert_eq!(selected.name, "Fish");

    let selected = resolve_cli_replacement_profile(&profiles, Some("fIsH (hOmEbReW)"), None, None)
        .unwrap()
        .unwrap();
    assert_eq!(selected.name, "Fish (Homebrew)");
}

#[test]
fn pane_template_leaf_resolution_applies_profile_command_theme_environment_and_overlay() {
    let config = Config::parse(
        r##"{
            "profiles": [
                { "name": "Worker", "program": "worker-shell", "theme": "One Dark" }
            ],
            "pane_split_templates": {
                "custom": {
                    "env": { "SHARED": "yes", "ROLE": "default" },
                    "layout": {
                        "vertical": [
                            {
                                "label": "worker",
                                "profile": "wOrKeR",
                                "theme": "One Light",
                                "env": { "ROLE": "worker" },
                                "overlay": {
                                    "text": "WORKER",
                                    "size": "2xl",
                                    "opacity": 40,
                                    "color": "#ff00ff"
                                }
                            },
                            {
                                "label": "ssh",
                                "command": { "program": "ssh", "args": ["host", "-p", "2200"] }
                            }
                        ]
                    }
                }
            }
        }"##,
        None,
        None,
    )
    .unwrap();
    let active = config
        .profiles
        .iter()
        .find(|profile| profile.name == "System")
        .cloned()
        .unwrap();
    let leaves =
        resolve_pane_split_leaves(&config.pane_split_templates["custom"], &active, None).unwrap();

    assert_eq!(leaves[0].label.as_deref(), Some("worker"));
    assert_eq!(leaves[0].profile.name, "Worker");
    assert_eq!(leaves[0].profile.theme.as_deref(), Some("One Light"));
    assert_eq!(leaves[0].environment["ROLE"], "worker");
    assert_eq!(leaves[0].environment["SHARED"], "yes");
    assert_eq!(leaves[0].overlay_text.as_deref(), Some("WORKER"));
    assert_eq!(
        leaves[0].overlay_font_size,
        Some(OverlayFontSize::ExtraExtraLarge)
    );
    assert_eq!(leaves[0].overlay_opacity, Some(0.4));
    assert_eq!(leaves[0].overlay_color, overlay_color_from_value("#ff00ff"));

    assert_eq!(leaves[1].label.as_deref(), Some("ssh"));
    assert_eq!(leaves[1].environment["ROLE"], "default");
    assert_eq!(leaves[1].environment["SHARED"], "yes");
    assert_eq!(leaves[1].profile.name, "System");
    assert_eq!(
        leaves[1].profile.command,
        Shell::WithArguments {
            program: "ssh".to_owned(),
            args: vec!["host".to_owned(), "-p".to_owned(), "2200".to_owned()],
            title_override: None,
        }
    );
}

#[test]
fn pane_template_leaf_resolution_quotes_stacked_commands_for_the_resolved_shell() {
    let config = Config::parse(
        r##"{
            "pane_split_templates": {
                "custom": {
                    "layout": {
                        "vertical": [
                            {
                                "label": "server",
                                "stack": [
                                    { "program": "cargo", "args": ["watch", "-x", "test"] },
                                    { "program": "tail", "args": ["-f", "logs/my app.log"] }
                                ]
                            },
                            { "label": "client" }
                        ]
                    }
                }
            }
        }"##,
        None,
        None,
    )
    .unwrap();
    let active = config
        .profiles
        .iter()
        .find(|profile| profile.name == "System")
        .cloned()
        .unwrap();

    let leaves =
        resolve_pane_split_leaves(&config.pane_split_templates["custom"], &active, None).unwrap();

    assert_eq!(leaves[0].stack.len(), 2);
    assert_eq!(leaves[0].stack[0], "cargo watch -x test");
    // The argument with a space has to arrive quoted for the host shell, the way
    // `zetta pane --stack` quotes it.
    assert_eq!(
        leaves[0].stack[1],
        quote_pane_command_for_shell(
            &active.command,
            &[
                "tail".to_owned(),
                "-f".to_owned(),
                "logs/my app.log".to_owned(),
            ],
        )
        .unwrap()
    );
    assert_ne!(leaves[0].stack[1], "tail -f logs/my app.log");
    assert!(leaves[1].stack.is_empty());
}

#[test]
fn pane_template_labels_and_overlays_do_not_require_a_terminal_restart() {
    let profile = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };
    let pane = TerminalPane::new(1, profile.clone());
    let leaf = ResolvedPaneSplitLeaf {
        label: Some("new-label".to_owned()),
        profile,
        environment: HashMap::new(),
        overlay_text: Some("overlay".to_owned()),
        overlay_font_size: Some(OverlayFontSize::Large),
        overlay_opacity: Some(0.5),
        overlay_color: overlay_color_from_value("cyan"),
        stack: Vec::new(),
    };

    assert!(!pane_split_leaf_requires_restart(&pane, &leaf));
    let mut changed = leaf.clone();
    changed
        .environment
        .insert("ROLE".to_owned(), "worker".to_owned());
    assert!(pane_split_leaf_requires_restart(&pane, &changed));

    // A declared stack has to be rebuilt from scratch, or re-applying the
    // template would append its commands to the stack the pane already has.
    let mut stacked = leaf.clone();
    stacked.stack = vec!["cargo watch".to_owned()];
    assert!(pane_split_leaf_requires_restart(&pane, &stacked));
}
