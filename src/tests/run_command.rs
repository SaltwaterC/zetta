use super::*;

fn pane(id: u64) -> RunPaneIdentity {
    RunPaneIdentity::new(1, id)
}

fn register(
    registry: &RunCommandRegistry,
    owner: u64,
    dependencies: &[u64],
    allow_failure: bool,
) -> RunRegistration {
    registry
        .register(
            pane(owner),
            dependencies.iter().copied().map(pane).collect(),
            allow_failure,
            vec!["echo".to_owned(), owner.to_string()],
        )
        .unwrap()
}

fn assert_ready(registration: RunRegistration) {
    let result = registration.wait();
    assert_eq!(result.resolution, RunResolution::Ready, "{result:?}");
}

fn assert_failed(registration: RunRegistration) {
    assert_eq!(registration.wait().resolution, RunResolution::Failed);
}

fn assert_pending(registration: &RunRegistration) {
    assert_eq!(
        registration.recv_timeout(std::time::Duration::from_millis(20)),
        Err(RecvTimeoutError::Timeout)
    );
}

#[test]
fn all_dependencies_must_finish_successfully() {
    let registry = RunCommandRegistry::default();
    registry.record_result(pane(1), Some(0));
    registry.command_started(pane(2), "test".to_owned());
    let registration = register(&registry, 3, &[1, 2], false);
    let id = registration.id;
    let waiting = std::thread::spawn(move || registration.wait());

    registry.command_started(pane(1), "test".to_owned());
    registry.command_finished(pane(1), Some(0));
    registry.record_result(pane(2), Some(0));
    assert_eq!(waiting.join().unwrap().resolution, RunResolution::Ready);

    registry.complete(id, Some(0));
}

#[test]
fn idle_dependencies_wait_for_the_next_command() {
    let registry = RunCommandRegistry::default();
    registry.record_result(pane(1), Some(0));
    let successful = register(&registry, 2, &[1], false);
    assert_pending(&successful);
    registry.command_started(pane(1), "next".to_owned());
    assert_pending(&successful);
    registry.command_finished(pane(1), Some(0));
    assert_ready(successful);

    registry.record_result(pane(4), None);
    let unavailable = register(&registry, 3, &[4], false);
    assert_pending(&unavailable);
    registry.command_started(pane(4), "unknown".to_owned());
    registry.command_finished(pane(4), None);
    assert_failed(unavailable);
}

#[test]
fn failed_dependencies_require_allow_failure() {
    let registry = RunCommandRegistry::default();
    registry.record_result(pane(1), Some(7));
    let rejected = register(&registry, 2, &[1], false);
    assert_pending(&rejected);
    registry.command_started(pane(1), "failed".to_owned());
    registry.command_finished(pane(1), Some(7));
    assert_failed(rejected);

    let allowed = register(&registry, 3, &[1], true);
    assert_pending(&allowed);
    registry.command_started(pane(1), "failed-again".to_owned());
    registry.command_finished(pane(1), Some(7));
    assert_ready(allowed);

    registry.record_result(pane(4), None);
    let unavailable = register(&registry, 5, &[4], true);
    assert_pending(&unavailable);
    registry.command_started(pane(4), "unknown".to_owned());
    registry.command_finished(pane(4), None);
    assert_failed(unavailable);
}

#[test]
fn managed_dependency_failures_can_be_allowed_but_unavailable_results_cannot() {
    let registry = RunCommandRegistry::default();
    registry.command_started(pane(1), "dependency".to_owned());

    let failed = register(&registry, 2, &[1], false);
    let failed_id = failed.id;
    let blocked = register(&registry, 3, &[2], false);
    let allowed = register(&registry, 4, &[2], true);
    assert_pending(&blocked);
    assert_pending(&allowed);

    registry.command_finished(pane(1), Some(7));
    assert_failed(failed);
    assert_failed(blocked);
    assert_ready(allowed);
    registry.complete(failed_id, Some(7));

    registry.pane_closed(pane(5));
    assert_failed(register(&registry, 6, &[5], true));
}

#[test]
fn managed_failure_survives_the_wrapper_shell_marker() {
    let registry = RunCommandRegistry::default();
    registry.command_started(pane(1), "zetta pane wait".to_owned());

    assert_failed(register(&registry, 1, &[99], false));
    registry.command_finished(pane(1), Some(1));

    let next = register(&registry, 3, &[1], false);
    assert_pending(&next);
    registry.command_started(pane(1), "next".to_owned());
    registry.command_finished(pane(1), Some(1));
    assert_failed(next);
}

#[test]
fn shell_marker_cannot_complete_a_managed_run_before_run_complete() {
    let registry = RunCommandRegistry::default();
    registry.command_started(pane(2), "dependency".to_owned());
    registry.command_started(pane(1), "zetta pane wait".to_owned());

    let registration = register(&registry, 1, &[2], false);
    let id = registration.id;
    assert_pending(&registration);
    registry.command_finished(pane(2), Some(0));
    assert_ready(registration);

    registry.command_finished(pane(1), Some(9));
    registry.complete(id, Some(0));
    registry.command_finished(pane(1), Some(9));

    registry.command_started(pane(1), "next".to_owned());
    let next = register(&registry, 3, &[1], false);
    assert_pending(&next);
    registry.command_finished(pane(1), Some(7));
    assert_failed(next);
}

#[test]
fn repeated_dependency_commands_wait_for_the_new_command() {
    let registry = RunCommandRegistry::default();
    registry.record_result(pane(1), Some(0));

    let first = register(&registry, 2, &[1], false);
    let first_id = first.id;
    assert_pending(&first);
    registry.command_started(pane(1), "first".to_owned());
    registry.command_finished(pane(1), Some(0));
    assert_ready(first);
    registry.complete(first_id, Some(0));

    registry.command_started(pane(1), "make fmt".to_owned());
    let second = register(&registry, 2, &[1], false);
    assert_eq!(
        second.recv_timeout(std::time::Duration::from_millis(20)),
        Err(RecvTimeoutError::Timeout)
    );

    registry.command_finished(pane(1), Some(0));
    assert_ready(second);
}

#[test]
fn an_idle_wait_attaches_to_a_managed_command_started_after_registration() {
    let registry = RunCommandRegistry::default();
    registry.record_result(pane(1), Some(0));
    registry.record_result(pane(2), Some(0));

    let waiting = register(&registry, 3, &[2], false);
    let waiting_id = waiting.id;
    assert_pending(&waiting);

    let managed = register(&registry, 2, &[1], false);
    let managed_id = managed.id;
    assert_pending(&managed);
    assert_pending(&waiting);

    registry.command_started(pane(1), "dependency".to_owned());
    registry.command_finished(pane(1), Some(0));
    assert_ready(managed);
    assert_pending(&waiting);

    registry.complete(managed_id, Some(0));
    assert_ready(waiting);
    registry.complete(waiting_id, Some(0));
}

#[test]
fn stale_managed_identity_uses_the_pane_result_after_node_cleanup() {
    let registry = RunCommandRegistry::default();
    registry.record_result(pane(1), Some(0));

    let mut state = registry.lock_state();
    state
        .panes
        .get_mut(&pane(1))
        .expect("recorded pane state")
        .active = Some(ActiveOperation::Managed(999));
    drop(state);

    assert_ready(register(&registry, 2, &[1], false));
}

#[test]
fn stale_managed_observation_uses_the_pane_result_during_resolution() {
    let registry = RunCommandRegistry::default();
    registry.command_started(pane(1), "current".to_owned());
    let registration = register(&registry, 2, &[1], false);
    let id = registration.id;

    let mut state = registry.lock_state();
    let pane_state = state.panes.get_mut(&pane(1)).expect("active pane state");
    pane_state.active = None;
    pane_state.tracking_ready = true;
    pane_state.last_result = Some(LastResult::Success(0));
    state
        .runs
        .get_mut(&id)
        .expect("registered run")
        .dependencies[0]
        .observation = DependencyObservation::ActiveManaged(999);
    resolve_locked(&mut state, [id]);
    drop(state);

    assert_ready(registration);
}

#[test]
fn dependencies_are_snapshotted_at_registration() {
    let registry = RunCommandRegistry::default();
    registry.record_result(pane(1), Some(0));
    registry.command_started(pane(2), "build".to_owned());
    let registration = register(&registry, 3, &[1, 2], false);

    // The previous result is not reused. A command issued after registration
    // is the command that satisfies the dependency.
    registry.command_started(pane(1), "later".to_owned());
    registry.command_finished(pane(2), Some(0));
    assert_pending(&registration);
    registry.command_finished(pane(1), Some(0));
    assert_ready(registration);
}

#[test]
fn managed_results_gate_transitive_dependents_and_preserve_exit_status() {
    let registry = RunCommandRegistry::default();
    registry.command_started(pane(4), "first dependency".to_owned());

    let first = register(&registry, 1, &[4], false);
    let first_id = first.id;
    let second = register(&registry, 2, &[1], false);
    let waiting = std::thread::spawn(move || second.wait());

    registry.command_finished(pane(4), Some(0));
    assert_ready(first);
    registry.complete(first_id, Some(0));
    assert_eq!(waiting.join().unwrap().resolution, RunResolution::Ready);

    let failed_run = register(&registry, 3, &[1], false);
    let failed_id = failed_run.id;
    assert_pending(&failed_run);
    registry.command_started(pane(1), "second dependency".to_owned());
    registry.command_finished(pane(1), Some(0));
    assert_ready(failed_run);
    let failed = register(&registry, 5, &[3], false);
    let allowed = register(&registry, 6, &[3], true);
    assert_pending(&failed);
    assert_pending(&allowed);
    registry.complete(failed_id, Some(7));
    assert_eq!(failed.wait().resolution, RunResolution::Failed);
    assert_ready(allowed);
}

#[test]
fn pending_managed_dependencies_remain_pending_until_their_dependency_finishes() {
    let registry = RunCommandRegistry::default();
    registry.command_started(pane(1), "build".to_owned());

    let first = register(&registry, 2, &[1], false);
    let first_id = first.id;
    let second = register(&registry, 3, &[1, 2], false);
    let second_id = second.id;

    assert_eq!(
        first.recv_timeout(std::time::Duration::from_millis(20)),
        Err(RecvTimeoutError::Timeout)
    );
    assert_eq!(
        second.recv_timeout(std::time::Duration::from_millis(20)),
        Err(RecvTimeoutError::Timeout)
    );

    registry.command_finished(pane(1), Some(0));
    let timeout = std::time::Duration::from_secs(1);
    assert_eq!(
        first
            .recv_timeout(timeout)
            .map(|message| message.resolution),
        Ok(RunResolution::Ready)
    );
    assert_eq!(
        second.recv_timeout(std::time::Duration::from_millis(20)),
        Err(RecvTimeoutError::Timeout)
    );

    registry.complete(first_id, Some(0));
    assert_eq!(
        second
            .recv_timeout(timeout)
            .map(|message| message.resolution),
        Ok(RunResolution::Ready)
    );

    registry.complete(second_id, Some(0));
}

#[test]
fn concurrent_dependents_are_released_by_one_result() {
    let registry = RunCommandRegistry::default();
    registry.command_started(pane(1), "build".to_owned());
    let first = register(&registry, 2, &[1], false);
    let second = register(&registry, 3, &[1], false);
    let first_waiting = std::thread::spawn(move || first.wait());
    let second_waiting = std::thread::spawn(move || second.wait());

    registry.command_finished(pane(1), Some(0));
    assert_eq!(
        first_waiting.join().unwrap().resolution,
        RunResolution::Ready
    );
    assert_eq!(
        second_waiting.join().unwrap().resolution,
        RunResolution::Ready
    );
}

#[test]
fn cycles_and_self_dependencies_are_rejected() {
    let registry = RunCommandRegistry::default();
    registry.command_started(pane(1), "one".to_owned());
    registry.command_started(pane(2), "two".to_owned());

    let first = register(&registry, 1, &[2], false);
    let second = register(&registry, 2, &[1], false);
    assert_failed(first);
    assert_failed(second);
    assert!(
        registry
            .register(pane(3), vec![pane(3)], false, vec!["echo".to_owned()])
            .is_err()
    );
}

#[test]
fn pane_loss_cancels_waiting_runs() {
    let registry = RunCommandRegistry::default();
    registry.command_started(pane(1), "build".to_owned());
    let registration = register(&registry, 2, &[1], false);
    let waiting = std::thread::spawn(move || registration.wait());

    registry.terminal_lost(pane(1));
    assert_eq!(waiting.join().unwrap().resolution, RunResolution::Failed);
}

#[test]
fn closed_dependency_is_not_released_by_allow_failure() {
    let registry = RunCommandRegistry::default();
    registry.command_started(pane(1), "build".to_owned());
    registry.pane_closed(pane(1));
    assert_failed(register(&registry, 2, &[1], true));

    registry.pane_reopened(pane(1));
    registry.tracking_ready(pane(1));
    registry.record_result(pane(1), Some(0));
    let reopened = register(&registry, 3, &[1], false);
    assert_pending(&reopened);
    registry.command_started(pane(1), "reopened".to_owned());
    registry.command_finished(pane(1), Some(0));
    assert_ready(reopened);
}

#[test]
fn disconnect_and_shutdown_fail_waiters() {
    let registry = RunCommandRegistry::default();
    registry.command_started(pane(1), "build".to_owned());
    let disconnected = register(&registry, 2, &[1], false);
    let disconnected_id = disconnected.id;
    let disconnected_waiting = std::thread::spawn(move || disconnected.wait());
    registry.connection_lost(disconnected_id);
    assert_eq!(
        disconnected_waiting.join().unwrap().resolution,
        RunResolution::Failed
    );

    registry.command_started(pane(3), "test".to_owned());
    let shutdown = register(&registry, 4, &[3], false);
    let shutdown_waiting = std::thread::spawn(move || shutdown.wait());
    registry.shutdown();
    assert_eq!(
        shutdown_waiting.join().unwrap().resolution,
        RunResolution::Failed
    );
}

#[test]
fn disconnect_after_release_fails_downstream_without_hanging() {
    let registry = RunCommandRegistry::default();
    registry.command_started(pane(1), "dependency".to_owned());

    let released = register(&registry, 2, &[1], false);
    let released_id = released.id;
    registry.command_finished(pane(1), Some(0));
    assert_ready(released);

    let downstream = register(&registry, 3, &[2], false);
    assert_pending(&downstream);
    registry.connection_lost(released_id);
    assert_failed(downstream);
}

#[test]
fn shell_lifecycle_markers_record_the_last_observed_result() {
    let registry = RunCommandRegistry::default();
    registry.tracking_ready(pane(1));
    registry.command_started(pane(1), "test".to_owned());
    let waiting = register(&registry, 2, &[1], false);
    let waiting = std::thread::spawn(move || waiting.wait());
    registry.command_finished(pane(1), Some(0));
    assert_eq!(waiting.join().unwrap().resolution, RunResolution::Ready);
}

#[test]
fn tracking_ready_discards_shell_initialization_activity() {
    let registry = RunCommandRegistry::default();
    registry.command_started(pane(1), "shell integration".to_owned());
    registry.tracking_ready(pane(1));

    let waiting = register(&registry, 2, &[1], false);
    assert_pending(&waiting);
    registry.command_finished(pane(1), Some(0));
    assert_pending(&waiting);

    registry.command_started(pane(1), "next command".to_owned());
    registry.command_finished(pane(1), Some(0));
    assert_ready(waiting);
}

#[test]
fn internal_startup_commands_do_not_satisfy_a_wait() {
    let registry = RunCommandRegistry::default();
    registry.tracking_ready(pane(1));
    let waiting = register(&registry, 2, &[1], false);

    registry.command_started(
        pane(1),
        "printf '%s%s%s\\n' __zed_init_command_ready_ 2 __".to_owned(),
    );
    registry.command_finished(pane(1), Some(0));
    assert_pending(&waiting);

    registry.command_started(
        pane(1),
        "[[ ${__ZETTA_LIFECYCLE_TRACKING_INSTALLED:-0} != 1 ]]".to_owned(),
    );
    registry.command_finished(pane(1), Some(0));
    assert_pending(&waiting);

    registry.command_started(pane(1), "next command".to_owned());
    registry.command_finished(pane(1), Some(0));
    assert_ready(waiting);
}
