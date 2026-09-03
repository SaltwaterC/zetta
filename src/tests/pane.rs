use super::*;

#[cfg(windows)]
#[test]
fn tracked_windows_shells_prefer_their_reported_directory() {
    for program in ["cmd.exe", "powershell.exe", "pwsh.exe"] {
        assert!(shell_reports_current_directory(&Shell::Program(
            program.to_owned()
        )));
    }
    assert!(shell_reports_current_directory(&Shell::System));
    assert!(shell_reports_current_directory(&Shell::WithArguments {
        program: r"C:\cygwin64\bin\bash.exe".to_owned(),
        args: vec!["-l".to_owned()],
        title_override: Some("Cygwin".to_owned()),
    }));
}

#[test]
fn terminal_size_label_uses_columns_before_rows() {
    assert_eq!(terminal_size_label(120, 40), "120 × 40");
}

#[test]
fn pane_limit_applies_to_total_tab_panes() {
    assert!(can_add_panes(1, MAX_PANES_PER_TAB - 1));
    assert!(!can_add_panes(2, MAX_PANES_PER_TAB - 1));
    assert!(!can_add_panes(usize::MAX, 1));
}

#[test]
fn terminal_spawn_notifications_are_coalesced() {
    let mut pending = false;
    assert!(begin_coalesced_notification(&mut pending));
    assert!(!begin_coalesced_notification(&mut pending));
    assert!(!begin_coalesced_notification(&mut pending));
    pending = false;
    assert!(begin_coalesced_notification(&mut pending));
}

#[test]
fn pane_output_save_guard_blocks_until_the_active_save_finishes() {
    let mut in_progress = false;

    assert!(begin_pane_output_save(&mut in_progress));
    assert!(in_progress);
    assert!(!begin_pane_output_save(&mut in_progress));

    finish_pane_output_save(&mut in_progress);
    assert!(!in_progress);
    assert!(begin_pane_output_save(&mut in_progress));
}

#[test]
fn bounded_launch_queue_applies_backpressure_and_preserves_order() {
    let mut queue = BoundedLaunchQueue::new(2);
    queue.extend([1, 2, 3, 4]);

    assert_eq!(queue.pop_ready(), Some(1));
    assert_eq!(queue.pop_ready(), Some(2));
    assert_eq!(queue.pop_ready(), None);

    queue.complete();
    assert_eq!(queue.pop_ready(), Some(3));
    assert_eq!(queue.pop_ready(), None);

    queue.complete();
    assert_eq!(queue.pop_ready(), Some(4));
}

#[test]
fn pane_launch_metadata_is_prepared_once_per_pane() {
    let mut preparations = 0;
    let launches = prepare_pane_launches([2, 3, 4], |pane_id| {
        preparations += 1;
        format!("tracking-{pane_id}")
    });

    assert_eq!(preparations, 3);
    assert_eq!(
        launches,
        [
            (2, "tracking-2".to_owned()),
            (3, "tracking-3".to_owned()),
            (4, "tracking-4".to_owned()),
        ]
    );
}

#[test]
fn terminal_regexes_are_cloned_then_moved_into_the_final_spawn() {
    let mut regexes = vec!["first".to_owned(), "second".to_owned()];
    let original_buffer = regexes[0].as_ptr();

    let earlier_spawn = clone_or_take_for_final_spawn(&mut regexes, false);
    assert_ne!(earlier_spawn[0].as_ptr(), original_buffer);
    assert_eq!(regexes[0].as_ptr(), original_buffer);

    let final_spawn = clone_or_take_for_final_spawn(&mut regexes, true);
    assert_eq!(final_spawn[0].as_ptr(), original_buffer);
    assert!(regexes.is_empty());
}

#[test]
fn terminal_environment_identifies_zetta() {
    let mut env = HashMap::from([("ZED_TERM".to_string(), "true".to_string())]);

    terminal::insert_zetta_terminal_env(&mut env, &"0.1.0");

    assert_eq!(env.get("ZETTA_TERM").map(String::as_str), Some("true"));
    assert_eq!(env.get("TERM_PROGRAM").map(String::as_str), Some("zetta"));
    assert_eq!(
        env.get("TERM_PROGRAM_VERSION").map(String::as_str),
        Some("0.1.0")
    );
    assert!(!env.contains_key("ZED_TERM"));
}
