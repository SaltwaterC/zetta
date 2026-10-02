use super::*;

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
