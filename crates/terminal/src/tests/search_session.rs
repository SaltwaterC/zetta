use super::*;
use crate::{
    Point, Range, TerminalBuilder,
    terminal_settings::{AlternateScroll, CursorShape},
};
use gpui::{AppContext as _, TestAppContext};
use util::paths::PathStyle;

fn terminal(cx: &mut TestAppContext, output: &str) -> Entity<Terminal> {
    cx.new(|cx| {
        let mut terminal = TerminalBuilder::new_display_only(
            CursorShape::Block,
            AlternateScroll::On,
            None,
            0,
            cx.background_executor(),
            PathStyle::local(),
        )
        .subscribe(cx);
        terminal.write_output(output.as_bytes(), cx);
        terminal
    })
}

fn session(cx: &mut TestAppContext, count: usize) -> Entity<SearchSession> {
    let terminals = (0..count).map(|_| terminal(cx, "")).collect();
    cx.new(|cx| SearchSession::new(terminals, cx))
}

fn add(session: &mut SearchSession, target: usize, count: usize, cx: &mut Context<SearchSession>) {
    let offset = session.targets[target].shown;
    let older_matches = (0..count)
        .map(|index| {
            let point = Point::new(-((offset + index) as i32), 0);
            Range::new(point, point)
        })
        .collect();
    session.apply_update(
        target,
        SearchUpdate {
            older_matches,
            total_count: offset + count,
            complete: true,
            limit_reached: false,
        },
        cx,
    );
}

#[gpui::test]
fn search_session_navigation_and_incremental_results(cx: &mut TestAppContext) {
    let session = session(cx, 3);
    session.update(cx, |session, cx| {
        session.navigate(false, cx);
        assert_eq!(session.status().active_match, None);
        add(session, 0, 2, cx);
        add(session, 2, 3, cx);
        assert_eq!(session.status().shown, 5);
        assert_eq!(
            session.match_at(2),
            Some(SearchMatch {
                target: 2,
                index: 0
            })
        );
        assert_eq!(
            session.match_at(4),
            Some(SearchMatch {
                target: 2,
                index: 2
            })
        );
        assert_eq!(session.match_at(5), None);
        session.activate(
            SearchMatch {
                target: 2,
                index: 2,
            },
            cx,
        );
        add(session, 2, 10, cx);
        assert_eq!(
            session.active,
            Some(SearchMatch {
                target: 2,
                index: 12
            })
        );
        assert_eq!(session.status().active_match, Some(14));
        add(session, 0, 5, cx);
        assert_eq!(session.status().active_match, Some(19));
        session.navigate(false, cx);
        assert_eq!(session.status().active_match, Some(0));
        session.navigate(true, cx);
        assert_eq!(session.status().active_match, Some(19));
        session.activate(
            SearchMatch {
                target: 0,
                index: 3,
            },
            cx,
        );
        add(session, 2, 4, cx);
        assert_eq!(session.status().active_match, Some(3));
    });
}

#[gpui::test]
fn search_session_first_results_activate_and_counts_include_unhighlighted_matches(
    cx: &mut TestAppContext,
) {
    let session = session(cx, 2);
    session.update(cx, |session, cx| {
        add(session, 1, 0, cx);
        assert_eq!(session.active, None);
        add(session, 1, 3, cx);
        assert_eq!(
            session.active,
            Some(SearchMatch {
                target: 1,
                index: 2
            })
        );
        session.apply_update(
            0,
            SearchUpdate {
                total_count: 100,
                limit_reached: true,
                complete: false,
                ..SearchUpdate::default()
            },
            cx,
        );
        let status = session.status();
        assert_eq!(status.shown, 3);
        assert_eq!(status.total_count, 103);
        assert!(status.limit_reached);
        assert!(!status.complete);
    });
}

#[gpui::test]
async fn search_session_runs_the_same_query_for_one_or_multiple_targets(cx: &mut TestAppContext) {
    for count in [1, 3] {
        let terminals: Vec<_> = (0..count)
            .map(|_| terminal(cx, "needle\r\nneedle\r\n"))
            .collect();
        let session = cx.new(|cx| SearchSession::new(terminals.clone(), cx));
        session.update(cx, |session, cx| session.set_query("needle", cx));
        cx.condition(&session, |session, _| session.status().complete)
            .await;
        session.read_with(cx, |session, _| {
            assert_eq!(session.status().total_count, 2 * count);
            assert_eq!(session.status().shown, 2 * count);
        });
        session.update(cx, SearchSession::clear);
        for terminal in terminals {
            assert!(terminal.read_with(cx, |terminal, _| terminal.matches.is_empty()));
        }
    }
}

#[gpui::test]
async fn search_session_invalidates_only_the_changed_terminal(cx: &mut TestAppContext) {
    let terminals = vec![terminal(cx, "needle\r\n"), terminal(cx, "needle\r\n")];
    let session = cx.new(|cx| SearchSession::new(terminals.clone(), cx));
    session.update(cx, |session, cx| session.set_query("needle", cx));
    cx.condition(&session, |session, _| session.status().complete)
        .await;
    let unchanged = session.read_with(cx, |session, _| session.targets[1].generation);
    for event in [Event::GridSizeChanged, Event::Wakeup] {
        let obsolete = session.read_with(cx, |session, _| session.targets[0].generation);
        terminals[0].update(cx, |_, cx| cx.emit(event));
        session.read_with(cx, |session, _| {
            assert!(!session.is_current(0, obsolete));
            assert_eq!(session.targets[1].generation, unchanged);
            assert_eq!(session.targets[1].shown, 1);
            assert!(session.targets[1].complete);
            assert!(!session.targets[0].complete);
        });
        cx.condition(&session, |session, _| session.status().complete)
            .await;
    }
    terminals[0].update(cx, |terminal, cx| terminal.write_output(b"needle\r\n", cx));
    cx.condition(&session, |session, _| session.status().complete)
        .await;
    assert_eq!(
        session.read_with(cx, |session, _| session.status().total_count),
        3
    );
}

#[gpui::test]
async fn search_session_replaces_queries_and_cancels_pending_jobs(cx: &mut TestAppContext) {
    let terminal = terminal(cx, "old old\r\nnew\r\n");
    let session = cx.new(|cx| SearchSession::new(vec![terminal.clone()], cx));
    session.update(cx, |session, cx| {
        session.set_query("old", cx);
        let generation = session.targets[0].generation;
        session.set_query("new", cx);
        assert!(!session.is_current(0, generation));
        assert!(!session.status().complete);
    });
    cx.condition(&session, |session, _| session.status().complete)
        .await;
    assert_eq!(
        session.read_with(cx, |session, _| session.status().total_count),
        1
    );
    session.update(cx, |session, cx| {
        session.set_query("old", cx);
        let generation = session.targets[0].generation;
        session.clear(cx);
        assert!(!session.is_current(0, generation));
        assert!(session.status().complete);
        assert_eq!(session.status().shown, 0);
        assert_eq!(session.status().active_match, None);
    });
    cx.run_until_parked();
    cx.executor().advance_clock(DEBOUNCE * 2);
    cx.run_until_parked();
    assert!(terminal.read_with(cx, |terminal, _| terminal.matches.is_empty()));
    session.update(cx, |session, cx| session.set_query("old", cx));
    drop(session);
    cx.run_until_parked();
    cx.executor().advance_clock(DEBOUNCE * 2);
    cx.run_until_parked();
    assert!(terminal.read_with(cx, |terminal, _| terminal.matches.is_empty()));
}

#[gpui::test]
async fn search_session_replacing_targets_rejects_old_generations(cx: &mut TestAppContext) {
    let first = terminal(cx, "needle\r\n");
    let second = terminal(cx, "needle needle\r\n");
    let session = cx.new(|cx| SearchSession::new(vec![first.clone()], cx));
    session.update(cx, |session, cx| session.set_query("needle", cx));
    cx.condition(&session, |session, _| session.status().complete)
        .await;
    session.update(cx, |session, cx| {
        let generation = session.targets[0].generation;
        session.set_targets(vec![first.clone()], cx);
        assert!(session.is_current(0, generation));
        session.set_targets(vec![second], cx);
        assert!(!session.is_current(0, generation));
    });
    assert!(first.read_with(cx, |terminal, _| terminal.matches.is_empty()));
    cx.condition(&session, |session, _| session.status().complete)
        .await;
    assert_eq!(
        session.read_with(cx, |session, _| session.status().shown),
        2
    );
}
