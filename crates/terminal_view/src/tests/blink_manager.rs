use super::*;
use gpui::{AppContext as _, Entity, TestAppContext};

fn advance(cx: &mut TestAppContext, millis: u64) {
    cx.background_executor
        .advance_clock(Duration::from_millis(millis));
    cx.run_until_parked();
}

fn assert_state(
    manager: &Entity<BlinkManager>,
    cx: &mut TestAppContext,
    visible: bool,
    paused: bool,
) {
    manager.update(cx, |manager, _| {
        assert_eq!(manager.visible, visible);
        assert_eq!(manager.paused, paused);
    });
}

#[gpui::test]
fn pause_shows_the_cursor_immediately_and_preserves_blink_cadence(cx: &mut TestAppContext) {
    let manager = cx.new(|_| BlinkManager::new());
    manager.update(cx, BlinkManager::enable);
    cx.run_until_parked();
    advance(cx, 500);
    assert_state(&manager, cx, false, false);

    manager.update(cx, BlinkManager::pause);
    assert_state(&manager, cx, true, true);
    cx.run_until_parked();
    advance(cx, 499);
    assert_state(&manager, cx, true, true);
    advance(cx, 1);
    assert_state(&manager, cx, false, false);
    advance(cx, 500);
    assert_state(&manager, cx, true, false);
    advance(cx, 500);
    assert_state(&manager, cx, false, false);
}

#[gpui::test]
fn repeat_reuses_the_task_and_blinks_at_the_latest_deadline(cx: &mut TestAppContext) {
    let manager = cx.new(|_| BlinkManager::new());
    manager.update(cx, BlinkManager::enable);
    manager.update(cx, BlinkManager::pause);
    cx.run_until_parked();

    for _ in 0..20 {
        advance(cx, 30);
        assert_eq!(cx.dispatcher.scheduler().pending_task_counts(), (0, 0));
        manager.update(cx, BlinkManager::pause);
        // Starting another task on each input queues a runnable immediately,
        // even if an epoch would later make its completion harmless.
        assert_eq!(cx.dispatcher.scheduler().pending_task_counts(), (0, 0));
        assert_state(&manager, cx, true, true);
    }
    // Last input was at 600 ms. The task has already woken once at 500 ms
    // and must not hide at its previous 980 ms deadline or drift to 1500 ms.
    advance(cx, 380);
    assert_state(&manager, cx, true, true);
    advance(cx, 119);
    assert_state(&manager, cx, true, true);
    advance(cx, 1);
    assert_state(&manager, cx, false, false);
    manager.update(cx, |manager, _| {
        assert!(manager.pause_task.is_none());
        assert!(manager.pause_deadline.is_none());
    });
}

#[gpui::test]
fn pause_just_before_a_blink_invalidates_the_old_blink(cx: &mut TestAppContext) {
    let manager = cx.new(|_| BlinkManager::new());
    manager.update(cx, BlinkManager::enable);
    cx.run_until_parked();
    advance(cx, 499);
    manager.update(cx, BlinkManager::pause);
    cx.run_until_parked();
    advance(cx, 1);
    assert_state(&manager, cx, true, true);
    advance(cx, 498);
    assert_state(&manager, cx, true, true);
    advance(cx, 1);
    assert_state(&manager, cx, false, false);
}

#[gpui::test]
fn delayed_first_poll_keeps_the_input_deadline(cx: &mut TestAppContext) {
    let manager = cx.new(|_| BlinkManager::new());
    manager.update(cx, BlinkManager::enable);
    manager.update(cx, BlinkManager::pause);
    // advance_clock also polls tasks, so move the underlying clock directly
    // to model a foreground task that could not run immediately.
    cx.dispatcher
        .scheduler()
        .clock()
        .advance(Duration::from_millis(200));
    cx.run_until_parked();
    advance(cx, 299);
    assert_state(&manager, cx, true, true);
    advance(cx, 1);
    assert_state(&manager, cx, false, false);
}

#[gpui::test]
fn disable_invalidates_the_pause_and_later_input_can_rearm_it(cx: &mut TestAppContext) {
    let manager = cx.new(|_| BlinkManager::new());
    manager.update(cx, BlinkManager::enable);
    manager.update(cx, BlinkManager::pause);
    cx.run_until_parked();
    advance(cx, 200);
    manager.update(cx, BlinkManager::disable);
    advance(cx, 300);
    manager.update(cx, |manager, _| {
        assert!(manager.visible);
        assert!(!manager.enabled);
        assert!(manager.pause_task.is_none());
        assert!(manager.pause_deadline.is_none());
    });

    manager.update(cx, BlinkManager::enable);
    manager.update(cx, BlinkManager::pause);
    cx.run_until_parked();
    advance(cx, 499);
    assert_state(&manager, cx, true, true);
    advance(cx, 1);
    assert_state(&manager, cx, false, false);
}

#[gpui::test]
fn new_input_after_disable_reuses_the_sleeping_task_with_a_fresh_epoch(cx: &mut TestAppContext) {
    let manager = cx.new(|_| BlinkManager::new());
    manager.update(cx, BlinkManager::enable);
    manager.update(cx, BlinkManager::pause);
    cx.run_until_parked();
    advance(cx, 200);
    manager.update(cx, BlinkManager::disable);
    manager.update(cx, BlinkManager::enable);
    manager.update(cx, BlinkManager::pause);
    assert_eq!(cx.dispatcher.scheduler().pending_task_counts(), (0, 0));
    advance(cx, 300);
    assert_state(&manager, cx, true, true);
    advance(cx, 199);
    assert_state(&manager, cx, true, true);
    advance(cx, 1);
    assert_state(&manager, cx, false, false);
}

#[gpui::test]
fn a_pause_while_disabled_expires_without_blinking(cx: &mut TestAppContext) {
    let manager = cx.new(|_| BlinkManager::new());
    manager.update(cx, BlinkManager::pause);
    cx.run_until_parked();
    advance(cx, 500);
    assert_state(&manager, cx, true, false);
    manager.update(cx, BlinkManager::enable);
    assert_state(&manager, cx, true, false);
    cx.run_until_parked();
    advance(cx, 500);
    assert_state(&manager, cx, false, false);
}

#[gpui::test]
fn dropping_the_manager_releases_its_pause_task(cx: &mut TestAppContext) {
    let manager = cx.new(|_| BlinkManager::new());
    manager.update(cx, BlinkManager::pause);
    cx.run_until_parked();
    let weak = manager.downgrade();
    drop(manager);
    cx.run_until_parked();
    assert!(weak.upgrade().is_none());
    advance(cx, 500);
    assert!(!cx.dispatcher.scheduler().has_pending_tasks());
}

#[gpui::test]
fn reenable_cannot_revive_a_pause_invalidated_by_disable(cx: &mut TestAppContext) {
    let manager = cx.new(|_| BlinkManager::new());
    manager.update(cx, BlinkManager::enable);
    manager.update(cx, BlinkManager::pause);
    cx.run_until_parked();
    advance(cx, 200);
    manager.update(cx, BlinkManager::disable);
    manager.update(cx, BlinkManager::enable);
    let state = manager.update(cx, |manager, _| {
        (manager.visible, manager.paused, manager.blink_epoch)
    });
    advance(cx, 300);
    manager.update(cx, |manager, _| {
        assert_eq!(
            (manager.visible, manager.paused, manager.blink_epoch),
            state
        );
        assert!(manager.pause_task.is_none());
    });
}

#[gpui::test]
fn an_overdue_first_poll_resumes_without_an_extra_interval(cx: &mut TestAppContext) {
    let manager = cx.new(|_| BlinkManager::new());
    manager.update(cx, BlinkManager::enable);
    manager.update(cx, BlinkManager::pause);
    cx.dispatcher
        .scheduler()
        .clock()
        .advance(Duration::from_millis(700));
    cx.run_until_parked();
    assert_state(&manager, cx, false, false);
    advance(cx, 499);
    assert_state(&manager, cx, false, false);
    advance(cx, 1);
    assert_state(&manager, cx, true, false);
}
