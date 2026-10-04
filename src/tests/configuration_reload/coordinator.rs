use super::*;

fn reload_windows(cx: &mut gpui::TestAppContext) -> Vec<Entity<Zetta>> {
    cx.update(|cx| {
        theme_settings::init(
            theme::LoadThemes::All(Box::new(crate::zetta_assets::ZettaAssets)),
            cx,
        );
        let registry = ThemeRegistry::global(cx);
        theme_settings::load_bundled_themes(&registry);
        theme::GlobalTheme::update_theme(cx, registry.get("One Light").unwrap());
        terminal::terminal_settings::TerminalSettings::init(cx);
    });
    (0..2)
        .map(|_| {
            cx.add_window_view(|window, cx| {
                let mut config = Config::defaults(None, None);
                config.profiles.clear();
                Zetta::new(
                    config,
                    None,
                    crate::ZettaLaunchOptions {
                        no_mux: true,
                        ..Default::default()
                    },
                    window,
                    cx,
                )
            })
            .0
        })
        .collect()
}

fn prepared(font_size: f32, theme: &str) -> PreparedConfiguration {
    let mut config = Config::defaults(None, None);
    config.terminal_font_size = Some(font_size);
    config.theme = Some(theme.to_owned());
    config.dark_theme = Some(theme.to_owned());
    PreparedConfiguration {
        settings_form: ConfigurationForm::parse(None, &config.config_path, &config)
            .map_err(|e| e.to_string()),
        config: Arc::new(config),
        config_stamp: ConfigFileStamp {
            modified: None,
            len: 0,
        },
        themes: Vec::new(),
        keymap: KeymapSource::default(),
        projects: HashMap::new(),
        #[cfg(feature = "zmux")]
        daemons: HashMap::new(),
        #[cfg(feature = "session-persistence")]
        auto_protect: Ok(None),
        #[cfg(target_os = "linux")]
        desktop_entry_updated: false,
    }
}

fn targets(windows: &[Entity<Zetta>], cx: &mut App) -> Vec<ReloadTarget> {
    windows
        .iter()
        .map(|zetta| ReloadTarget {
            snapshot: zetta.update(cx, |zetta, _| zetta.configuration_reload_snapshot()),
            zetta: zetta.clone(),
        })
        .collect()
}

#[gpui::test]
fn serial_commits_update_both_windows_and_process_theme_before_completion(
    cx: &mut gpui::TestAppContext,
) {
    let windows = reload_windows(cx);
    let keymap = tempfile::NamedTempFile::new().unwrap();
    cx.update(|cx| {
        for (font, theme) in [(17., "One Light"), (19., "One Dark")] {
            let targets = targets(&windows, cx);
            let shortcut = if font == 17. { "ctrl-f11" } else { "ctrl-f12" };
            fs::write(
                keymap.path(),
                format!(r#"[{{"bindings":{{"{shortcut}":"zetta::NewTab"}}}}]"#),
            )
            .unwrap();
            let mut configuration = prepared(font, theme);
            configuration.keymap = read_keymap_source(keymap.path());
            let outcome = commit_reload(
                false,
                PathBuf::from("config.json"),
                targets,
                Ok(configuration),
                cx,
            );
            let completed = std::rc::Rc::new(std::cell::Cell::new(false));
            let observed = completed.clone();
            let windows = windows.clone();
            finish_reload(
                outcome,
                vec![Box::new(move |outcome, cx| {
                    outcome.process_result().unwrap();
                    for window in &windows {
                        assert_eq!(window.read(cx).launch_config.terminal_font_size, Some(font));
                        assert!(window.read(cx).configuration_error.is_none());
                    }
                    assert_eq!(TerminalSettings::get_global(cx).font_size, Some(px(font)));
                    assert_eq!(theme::GlobalTheme::theme(cx).name.as_ref(), theme);
                    let expected = gpui::Keystroke::parse(shortcut).unwrap();
                    assert_eq!(
                        cx.key_bindings()
                            .borrow()
                            .bindings_for_action(&NewTab)
                            .next_back()
                            .unwrap()
                            .match_keystrokes(&[expected]),
                        Some(false),
                    );
                    observed.set(true);
                })],
                cx,
            );
            assert!(completed.get());
        }
        for window in &windows {
            assert_eq!(window.read(cx).configuration_generation, 2);
        }
    });
}

#[gpui::test]
fn a_failed_window_keeps_its_config_while_an_independent_window_commits(
    cx: &mut gpui::TestAppContext,
) {
    let windows = reload_windows(cx);
    cx.update(|cx| {
        let original = windows[0].read(cx).launch_config.terminal_font_size;
        let mut targets = targets(&windows, cx);
        targets[0]
            .snapshot
            .project_roots
            .push(PathBuf::from("unavailable-project"));
        let outcome = commit_reload(
            false,
            PathBuf::from("config.json"),
            targets,
            Ok(prepared(23., "One Light")),
            cx,
        );
        assert!(outcome.process_result().is_err());
        assert_eq!(outcome.window_failures.len(), 1);
        assert_eq!(
            windows[0].read(cx).launch_config.terminal_font_size,
            original
        );
        assert!(windows[0].read(cx).configuration_error.is_some());
        assert_eq!(
            windows[1].read(cx).launch_config.terminal_font_size,
            Some(23.)
        );
        assert_eq!(TerminalSettings::get_global(cx).font_size, Some(px(23.)));
    });
}

#[test]
fn the_first_request_starts_at_once() {
    let mut queue = ReloadQueue::<u32, &str>::default();

    let started = queue.request(ReloadScope::Windows(vec![1]), "first");

    assert_eq!(
        started,
        Some((ReloadScope::Windows(vec![1]), vec!["first"]))
    );
}

#[test]
fn requests_made_while_one_runs_become_one_follow_up() {
    let mut queue = ReloadQueue::<u32, &str>::default();
    queue.request(ReloadScope::Windows(vec![1]), "running");

    assert_eq!(queue.request(ReloadScope::Windows(vec![2]), "second"), None);
    assert_eq!(queue.request(ReloadScope::Windows(vec![1]), "third"), None);
    assert_eq!(queue.request(ReloadScope::Windows(vec![2]), "fourth"), None);

    assert_eq!(
        queue.finish(),
        Some((
            ReloadScope::Windows(vec![2, 1]),
            vec!["second", "third", "fourth"]
        ))
    );
}

#[test]
fn a_process_request_absorbs_window_requests_in_either_order() {
    let mut queue = ReloadQueue::<u32, &str>::default();
    queue.request(ReloadScope::Process, "running");
    queue.request(ReloadScope::Windows(vec![1]), "window");
    queue.request(ReloadScope::Process, "process");
    queue.request(ReloadScope::Windows(vec![2]), "later window");

    assert_eq!(
        queue.finish(),
        Some((
            ReloadScope::Process,
            vec!["window", "process", "later window"]
        ))
    );
}

#[test]
fn the_queue_is_idle_again_once_nothing_follows() {
    let mut queue = ReloadQueue::<u32, &str>::default();
    queue.request(ReloadScope::Process, "running");
    queue.request(ReloadScope::Process, "queued");
    assert!(queue.finish().is_some());

    assert_eq!(queue.finish(), None);
    assert_eq!(
        queue.request(ReloadScope::Process, "next"),
        Some((ReloadScope::Process, vec!["next"]))
    );
}

#[test]
fn a_follow_up_keeps_the_queue_running_until_it_finishes() {
    let mut queue = ReloadQueue::<u32, &str>::default();
    queue.request(ReloadScope::Process, "running");
    queue.request(ReloadScope::Process, "queued");
    assert!(queue.finish().is_some());

    // The follow-up is now running, so this waits behind it.
    assert_eq!(queue.request(ReloadScope::Process, "behind"), None);
}

fn outcome(failure: Option<ReloadFailure>, window_failures: &[(u64, &str)]) -> ReloadOutcome {
    ReloadOutcome {
        config_path: PathBuf::from("/config.json"),
        failure,
        window_failures: window_failures
            .iter()
            .map(|(id, error)| (EntityId::from(*id), (*error).to_owned()))
            .collect(),
    }
}

#[test]
fn a_process_reload_succeeds_only_when_every_window_took_it() {
    assert!(outcome(None, &[]).process_result().is_ok());

    let error = outcome(None, &[(1, "the daemon refused")])
        .process_result()
        .unwrap_err();

    assert!(error.to_string().contains("the daemon refused"), "{error}");
    assert!(error.to_string().contains("/config.json"), "{error}");
}

#[test]
fn a_shared_failure_is_every_windows_failure() {
    let outcome = outcome(Some(ReloadFailure::Load("parsing".to_owned())), &[]);

    assert_eq!(
        outcome.window_result(EntityId::from(7)),
        Err(ReloadFailure::Load("parsing".to_owned()))
    );
    assert!(outcome.process_result().is_err());
}

#[test]
fn a_window_failure_is_only_that_windows() {
    let outcome = outcome(None, &[(1, "project")]);

    assert_eq!(
        outcome.window_result(EntityId::from(1)),
        Err(ReloadFailure::Apply("project".to_owned()))
    );
    assert_eq!(outcome.window_result(EntityId::from(2)), Ok(()));
}

#[test]
fn window_messages_say_which_step_failed() {
    let path = Path::new("/config.json");

    assert_eq!(
        ReloadFailure::Load("bad".to_owned()).window_message(path),
        "Could not load /config.json: bad"
    );
    assert_eq!(
        ReloadFailure::Apply("bad".to_owned()).window_message(path),
        "Could not apply /config.json: bad"
    );
}
