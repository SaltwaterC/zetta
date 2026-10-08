use super::*;

#[test]
fn the_first_number_and_every_newer_one_are_in_order() {
    let mut window = ReplayWindow::new();
    assert_eq!(window.commit(7), Freshness::InOrder);
    assert_eq!(window.commit(8), Freshness::InOrder);
    assert_eq!(window.commit(500), Freshness::InOrder);
}

#[test]
fn an_unseen_number_inside_the_window_is_out_of_order_once() {
    let mut window = ReplayWindow::new();
    window.commit(100);
    window.commit(103);
    assert_eq!(window.check(101), Freshness::OutOfOrder);
    assert_eq!(window.commit(101), Freshness::OutOfOrder);
    assert_eq!(window.check(101), Freshness::Replay);
    assert_eq!(window.check(102), Freshness::OutOfOrder);
    assert_eq!(window.check(103), Freshness::Replay);
    assert_eq!(window.check(100), Freshness::Replay);
}

#[test]
fn a_number_at_the_window_floor_is_refused_even_if_never_seen() {
    let mut window = ReplayWindow::new();
    window.commit(REPLAY_WINDOW + 10);
    assert_eq!(window.check(11), Freshness::OutOfOrder);
    assert_eq!(window.check(10), Freshness::Replay);
    assert_eq!(window.check(0), Freshness::Replay);
}

#[test]
fn advancing_forgets_the_ring_slots_it_reuses() {
    let mut window = ReplayWindow::new();
    window.commit(0);
    window.commit(1);
    // 1 + REPLAY_WINDOW shares slot 1; 0 + REPLAY_WINDOW shares slot 0.
    window.commit(REPLAY_WINDOW + 2);
    for seq in 3..REPLAY_WINDOW + 2 {
        assert_eq!(window.check(seq), Freshness::OutOfOrder, "seq {seq}");
    }
    assert_eq!(window.check(2), Freshness::Replay);
}

#[test]
fn a_jump_wider_than_the_window_clears_it() {
    let mut window = ReplayWindow::new();
    for seq in 0..64 {
        window.commit(seq);
    }
    window.commit(10 * REPLAY_WINDOW);
    for seq in 9 * REPLAY_WINDOW + 1..10 * REPLAY_WINDOW {
        assert_eq!(window.check(seq), Freshness::OutOfOrder, "seq {seq}");
    }
}

#[test]
fn a_replay_is_not_recorded_again() {
    let mut window = ReplayWindow::new();
    window.commit(5);
    assert_eq!(window.commit(5), Freshness::Replay);
    assert_eq!(window.check(6), Freshness::InOrder);
}
