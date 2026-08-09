use super::*;

#[test]
fn configuration_reload_success_is_visible_after_success() {
    let mut feedback = ConfigurationReloadFeedback::default();

    feedback.begin_attempt();
    feedback.show_success();

    assert!(feedback.is_visible());
}

#[test]
fn configuration_reload_success_expires_for_the_current_generation() {
    let mut feedback = ConfigurationReloadFeedback::default();

    feedback.begin_attempt();
    let generation = feedback.show_success();

    assert!(feedback.dismiss_if_current(generation));
    assert!(!feedback.is_visible());
}

#[test]
fn stale_configuration_reload_timer_cannot_dismiss_new_feedback() {
    let mut feedback = ConfigurationReloadFeedback::default();

    feedback.begin_attempt();
    let stale_generation = feedback.show_success();
    feedback.begin_attempt();
    assert!(!feedback.is_visible());
    let current_generation = feedback.show_success();

    assert!(!feedback.dismiss_if_current(stale_generation));
    assert!(feedback.is_visible());
    assert!(feedback.dismiss_if_current(current_generation));
    assert!(!feedback.is_visible());
}
