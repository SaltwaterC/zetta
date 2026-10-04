use super::*;

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::session_state::{PaneState, TabState};

/// Holds a request until the test lets it through — a daemon that is slow to
/// answer, without depending on how slow.
#[derive(Clone, Default)]
struct Gate(Arc<(Mutex<bool>, Condvar)>);

impl Gate {
    fn release(&self) {
        let (released, changed) = &*self.0;
        *released.lock().unwrap() = true;
        changed.notify_all();
    }

    fn wait(&self) {
        let (released, changed) = &*self.0;
        let mut released = released.lock().unwrap();
        while !*released {
            released = changed.wait(released).unwrap();
        }
    }
}

/// Records every request in the order it arrived, and refuses the ones it was
/// told to.
#[derive(Default)]
struct FakeDaemon {
    calls: Mutex<Vec<String>>,
    gate: Option<Gate>,
    refuse_close: Option<u64>,
    refuse_share: bool,
    refuse_detach: bool,
}

impl FakeDaemon {
    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn wait_for_gate(&self) {
        if let Some(gate) = &self.gate {
            gate.wait();
        }
    }
}

impl HandoverDaemon for FakeDaemon {
    fn close_pane(&self, _: u64, mux_pane_id: u64) -> anyhow::Result<()> {
        self.record(format!("close {mux_pane_id}"));
        anyhow::ensure!(self.refuse_close != Some(mux_pane_id), "close refused");
        Ok(())
    }

    fn send_snapshot(&self, _: u64, checkpoint: PaneCheckpointData) -> anyhow::Result<()> {
        anyhow::ensure!(!checkpoint.snapshot.is_empty(), "an empty checkpoint");
        self.record(format!("snapshot {}", checkpoint.mux_pane_id));
        Ok(())
    }

    fn share(
        &self,
        _: u64,
        _: SessionPublication,
        _: Option<&SessionAuthentication>,
        offered: bool,
    ) -> anyhow::Result<()> {
        self.wait_for_gate();
        self.record(format!("share {offered}"));
        anyhow::ensure!(!self.refuse_share, "share refused");
        Ok(())
    }

    fn detach(
        &self,
        _: u64,
        _: SessionPublication,
        _: Option<&SessionAuthentication>,
        snapshots: Vec<(u64, Vec<u8>)>,
    ) -> anyhow::Result<()> {
        self.wait_for_gate();
        let panes = snapshots
            .iter()
            .map(|(id, _)| id.to_string())
            .collect::<Vec<_>>();
        self.record(format!("detach [{}]", panes.join(",")));
        anyhow::ensure!(!self.refuse_detach, "detach refused");
        Ok(())
    }

    fn shared_snapshot(&self, session_id: u64) -> anyhow::Result<SharedSessionState> {
        self.record("shared snapshot".to_owned());
        Ok(SharedSessionState::new(
            session_id,
            summary(session_id),
            serde_json::Value::Null,
        ))
    }
}

fn summary(session_id: u64) -> BackgroundSessionSummary {
    BackgroundSessionSummary {
        id: session_id,
        title: "handover".to_owned(),
        authentication_required: false,
        active_pane: 1,
        layout: BackgroundPaneLayout::Pane { pane_id: 1 },
        panes: Vec::new(),
        held: false,
        scoped_to: None,
        key_envelope: None,
    }
}

fn publication() -> SessionPublication {
    SessionPublication {
        summary: summary(9),
        state: serde_json::Value::Null,
    }
}

fn stacked(entry_id: u64, mux_pane_id: u64) -> StackedRelease {
    StackedRelease {
        entry_id,
        mux_pane_id,
        reader: RetiredReader::default(),
    }
}

fn detach_work(
    daemon: &Arc<FakeDaemon>,
    stacked: Vec<StackedRelease>,
    panes: Vec<PaneRetirement>,
) -> DetachWork {
    DetachWork {
        daemon: daemon.clone(),
        session_id: 9,
        stacked,
        panes,
        publication: publication(),
        authentication: None,
    }
}

fn offer_work(
    daemon: &Arc<FakeDaemon>,
    offered: bool,
    checkpoints: Vec<PaneCheckpoint>,
) -> OfferWork {
    OfferWork {
        daemon: daemon.clone(),
        reports: Arc::default(),
        session_id: 9,
        offered,
        checkpoints,
        publication: publication(),
        authentication: None,
    }
}

fn init(cx: &mut gpui::TestAppContext) {
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
    cx.executor().allow_parking();
}

fn display_terminal(text: &str, cx: &mut gpui::VisualTestContext) -> Entity<Terminal> {
    let terminal = cx.new(|cx| {
        terminal::TerminalBuilder::new_display_only(
            terminal::terminal_settings::CursorShape::Block,
            terminal::terminal_settings::AlternateScroll::On,
            None,
            0,
            cx.background_executor(),
            util::paths::PathStyle::local(),
        )
        .subscribe(cx)
    });
    let text = text.to_owned();
    terminal.update(cx, |terminal, cx| {
        terminal.write_output(text.as_bytes(), cx);
    });
    terminal
}

fn zetta_window(cx: &mut gpui::TestAppContext) -> (Entity<Zetta>, &mut gpui::VisualTestContext) {
    init(cx);
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
}

fn pane_state(id: u64) -> PaneState {
    PaneState {
        id,
        mux_pane_id: Some(id),
        label_number: id as usize,
        generated_label: None,
        custom_label: None,
        profile: "System".to_owned(),
        theme_override: None,
        environment_overrides: HashMap::new(),
        overlay: None,
        exit: None,
        base_exited: false,
        pending_command: None,
        active_command: None,
        detected_worktree_title: None,
        stack: Vec::new(),
        selected_stacked: None,
    }
}

fn tab(tab_id: u64) -> Tab {
    TabState {
        pane_theme_source: None,
        attention_id: 17,
        next_pane_label: 2,
        layout: crate::session_state::LayoutState::Pane { pane_id: 1 },
        active_pane: 1,
        focus_history: vec![1],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: false,
        silent_mode: false,
        keep_running: false,
        shared: false,
        custom_title: None,
        worktree_seed_title: None,
        process_title: None,
        icon: None,
        icon_override: None,
        pinned: false,
        panes: vec![pane_state(1)],
        theme_override: None,
    }
    .into_tab(tab_id, |_| Profile {
        name: "System".into(),
        command: task::Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::default(),
    })
    .unwrap()
}

/// Drives the window until `done`, giving the worker thread real time too.
fn settle_until(
    zetta: &Entity<Zetta>,
    cx: &mut gpui::VisualTestContext,
    done: impl Fn(&Zetta) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        if zetta.read_with(cx, |zetta, _| done(zetta)) {
            return;
        }
        assert!(Instant::now() < deadline, "the handover never committed");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_detach_releases_stacked_panes_before_it_is_sent() {
    let daemon = Arc::new(FakeDaemon::default());
    let outcome = detach_work(
        &daemon,
        vec![stacked(10, 100), stacked(11, 101)],
        Vec::new(),
    )
    .run();
    outcome.result.unwrap();
    assert_eq!(outcome.released_stacked, vec![10, 11]);
    assert_eq!(daemon.calls(), ["close 100", "close 101", "detach []"]);
}

/// A refused stacked close ends the detach before it is sent, and says which
/// stacked panes the daemon no longer holds so the rest can be restored.
#[test]
fn a_refused_stacked_close_stops_before_the_detach() {
    let daemon = Arc::new(FakeDaemon {
        refuse_close: Some(101),
        ..Default::default()
    });
    let outcome = detach_work(
        &daemon,
        vec![stacked(10, 100), stacked(11, 101), stacked(12, 102)],
        Vec::new(),
    )
    .run();
    assert!(outcome.result.is_err());
    assert_eq!(outcome.released_stacked, vec![10]);
    assert_eq!(daemon.calls(), ["close 100", "close 101"]);
}

#[test]
fn a_refused_offer_is_not_bound() {
    let daemon = Arc::new(FakeDaemon {
        refuse_share: true,
        ..Default::default()
    });
    let outcome = offer_work(&daemon, true, Vec::new()).run();
    assert!(outcome.published.is_err());
    assert!(outcome.binding.is_none());
    assert_eq!(daemon.calls(), ["share true"]);
}

#[test]
fn withdrawing_an_offer_binds_nothing() {
    let daemon = Arc::new(FakeDaemon::default());
    let outcome = offer_work(&daemon, false, Vec::new()).run();
    outcome.published.unwrap();
    assert!(outcome.binding.is_none());
    assert_eq!(daemon.calls(), ["share false"]);
}

/// Only the commit started under the current generation may take the entry: a
/// stale one, whose transition was settled by a closing tab, finds nothing.
#[test]
fn a_stale_commit_does_not_take_a_newer_transition() {
    let mut handovers = SessionHandovers::default();
    let pending = || {
        let (_, outcome) = mpsc::channel();
        PendingHandover::Offer {
            offered: true,
            previous_shared: false,
            outcome,
        }
    };
    let first = handovers.begin(5, pending());
    assert!(handovers.take(5, Some(first)).is_some());
    let second = handovers.begin(5, pending());
    assert!(handovers.take(5, Some(first)).is_none());
    assert!(handovers.in_flight(5));
    assert!(handovers.take(5, None).is_some());
    assert!(!handovers.in_flight(5));
    assert_ne!(first, second);
}

/// Every pane's screen is in the detach, serialized from its grid after the
/// reader finished, and in the tab's pane order however the threads finished.
#[gpui::test]
fn a_detach_carries_each_panes_screen(cx: &mut gpui::TestAppContext) {
    let (_, cx) = zetta_window(cx);
    let daemon = Arc::new(FakeDaemon::default());
    let panes = [(3, "third"), (1, "first"), (2, "second")]
        .into_iter()
        .map(|(mux_pane_id, text)| {
            let terminal = display_terminal(text, cx);
            terminal.update(cx, |terminal, _| PaneRetirement {
                mux_pane_id,
                reader: terminal.retire_pty_loop(),
                snapshot: Some(terminal.grid_snapshot_source()),
            })
        })
        .collect();
    let outcome = detach_work(&daemon, Vec::new(), panes).run();
    outcome.result.unwrap();
    assert_eq!(daemon.calls(), ["detach [3,1,2]"]);
}

#[gpui::test]
fn an_offer_checkpoints_every_pane_before_it_is_sent(cx: &mut gpui::TestAppContext) {
    let (_, cx) = zetta_window(cx);
    let daemon = Arc::new(FakeDaemon::default());
    let checkpoints = [4, 5]
        .into_iter()
        .map(|mux_pane_id| {
            let terminal = display_terminal("screen", cx);
            PaneCheckpoint {
                mux_pane_id,
                source: terminal.read_with(cx, |terminal, _| terminal.grid_snapshot_source()),
                columns: 80,
                lines: 24,
            }
        })
        .collect();
    // The runtime holds the registry for as long as the window does.
    let reports = Arc::<SharedSessionReports>::default();
    let outcome = OfferWork {
        reports: reports.clone(),
        ..offer_work(&daemon, true, checkpoints)
    }
    .run();
    outcome.published.unwrap();
    let binding = outcome.binding.unwrap().unwrap();
    assert!(
        !binding.receiver.is_closed(),
        "the subscription must be live"
    );
    let calls = daemon.calls();
    let mut checkpointed = calls[..2].to_vec();
    checkpointed.sort();
    assert_eq!(checkpointed, ["snapshot 4", "snapshot 5"]);
    assert_eq!(calls[2..], ["share true", "shared snapshot"]);
}

/// The finding this module exists for: a daemon that is slow to answer an
/// offer no longer holds the window. It keeps drawing, the tab keeps showing
/// the state it was asked to enter, a second toggle is refused rather than
/// racing the first, and the refusal is committed when the answer comes.
#[gpui::test]
fn a_slow_offer_leaves_the_window_running_until_its_answer_commits(cx: &mut gpui::TestAppContext) {
    let (zetta, cx) = zetta_window(cx);
    let gate = Gate::default();
    let daemon = Arc::new(FakeDaemon {
        gate: Some(gate.clone()),
        refuse_share: true,
        ..Default::default()
    });
    zetta.update_in(cx, |zetta, window, cx| {
        let mut tab = tab(5);
        tab.shared = true;
        zetta.tabs.push(tab);
        zetta.active_tab = 0;
        zetta.await_session_offer(
            5,
            true,
            false,
            offer_work(&daemon, true, Vec::new()),
            window,
            cx,
        );
    });

    // The worker is parked inside the request. The window is not.
    cx.run_until_parked();
    zetta.update_in(cx, |zetta, window, cx| {
        assert!(zetta.session_handover_in_flight(5));
        assert!(
            zetta.tabs[0].shared,
            "the tab shows the state being entered"
        );
        zetta.set_tab_sharing(5, false, None, window, cx);
        assert_eq!(
            zetta.transient_notice.message(),
            Some(HANDOVER_IN_FLIGHT),
            "a second toggle must be refused, not raced against the first"
        );
        assert!(zetta.tabs[0].shared);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(daemon.calls().is_empty(), "the request is still held");

    gate.release();
    settle_until(&zetta, cx, |zetta| !zetta.session_handover_in_flight(5));
    zetta.read_with(cx, |zetta, _| {
        assert!(!zetta.tabs[0].shared, "a refused offer rolls the tab back");
    });
    assert_eq!(daemon.calls(), ["share true"]);
}

/// A refused detach puts the tab back where it was, and restores the mapping
/// of every stacked pane the daemon still holds.
#[gpui::test]
fn a_refused_detach_puts_the_tab_back(cx: &mut gpui::TestAppContext) {
    let (zetta, cx) = zetta_window(cx);
    let daemon = Arc::new(FakeDaemon {
        refuse_close: Some(100),
        ..Default::default()
    });
    zetta.update_in(cx, |zetta, window, cx| {
        zetta.tabs.push(tab(6));
        zetta.active_tab = 0;
        // Preparation forgot it so the publication would not name it.
        zetta.mux_panes.forget_pane(10);
        zetta.await_multiplexer_handover(
            tab(5),
            HandoverOrigin::Tab {
                index: 0,
                shared: false,
            },
            PreparedDetach {
                work: detach_work(&daemon, vec![stacked(10, 100)], Vec::new()),
                stacked: vec![(10, 100)],
            },
            Some(window),
            cx,
        );
    });
    settle_until(&zetta, cx, |zetta| !zetta.session_handover_in_flight(5));
    zetta.read_with(cx, |zetta, _| {
        assert_eq!(
            zetta.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
            [5, 6]
        );
        assert_eq!(zetta.active_tab, 0);
        assert_eq!(zetta.mux_panes.mux_pane_id(10), Some(100));
    });
}

/// The close barrier remains pending while the foreground can process updates.
#[gpui::test]
fn a_closing_window_awaits_its_detach_without_blocking(cx: &mut gpui::TestAppContext) {
    let (zetta, cx) = zetta_window(cx);
    let gate = Gate::default();
    let daemon = Arc::new(FakeDaemon {
        gate: Some(gate.clone()),
        ..Default::default()
    });
    let close = zetta.update(cx, |zetta, cx| {
        zetta.await_multiplexer_handover(
            tab(5),
            HandoverOrigin::WindowClose,
            PreparedDetach {
                work: detach_work(&daemon, Vec::new(), Vec::new()),
                stacked: Vec::new(),
            },
            None,
            cx,
        );
        zetta.prepare_for_background_window_close(cx)
    });
    let finished = std::rc::Rc::new(std::cell::Cell::new(false));
    let observed = finished.clone();
    cx.spawn(async move |_| {
        close.await;
        observed.set(true);
    })
    .detach();
    cx.run_until_parked();
    assert!(!finished.get());
    zetta.update(cx, |zetta, cx| {
        assert!(zetta.session_handover_in_flight(5));
        cx.notify();
    });
    assert!(daemon.calls().is_empty());
    gate.release();
    settle_until(&zetta, cx, |_| finished.get());
    assert_eq!(daemon.calls(), ["detach []"]);
}

#[gpui::test]
fn closing_a_tab_during_an_offer_defers_until_the_reply(cx: &mut gpui::TestAppContext) {
    let (zetta, cx) = zetta_window(cx);
    let gate = Gate::default();
    let daemon = Arc::new(FakeDaemon {
        gate: Some(gate.clone()),
        refuse_share: true,
        ..Default::default()
    });
    zetta.update_in(cx, |zetta, window, cx| {
        zetta.tabs.push(tab(5));
        zetta.tabs.push(tab(6));
        zetta.await_session_offer(
            5,
            true,
            false,
            offer_work(&daemon, true, Vec::new()),
            window,
            cx,
        );
        zetta.close_tab_at_with_policy(0, false, window, cx);
        assert_eq!(zetta.tabs.len(), 2);
    });
    cx.run_until_parked();
    assert!(daemon.calls().is_empty());
    gate.release();
    settle_until(&zetta, cx, |zetta| zetta.tabs.len() == 1);
    zetta.read_with(cx, |zetta, _| assert_eq!(zetta.tabs[0].id, 6));
}

#[gpui::test]
fn a_handover_commits_after_its_window_is_removed(cx: &mut gpui::TestAppContext) {
    let (zetta, cx) = zetta_window(cx);
    let gate = Gate::default();
    let daemon = Arc::new(FakeDaemon {
        gate: Some(gate.clone()),
        ..Default::default()
    });
    zetta.update_in(cx, |zetta, window, cx| {
        zetta.await_multiplexer_handover(
            tab(5),
            HandoverOrigin::WindowClose,
            PreparedDetach {
                work: detach_work(&daemon, Vec::new(), Vec::new()),
                stacked: Vec::new(),
            },
            Some(window),
            cx,
        );
        window.remove_window();
    });
    cx.run_until_parked();
    assert!(zetta.read_with(cx, |zetta, _| zetta.session_handover_in_flight(5)));
    // A second window can run while the first is gone and its worker is held.
    let (other, other_cx) = zetta_window(&mut cx.cx);
    other.update(other_cx, |_, cx| cx.notify());
    other_cx.run_until_parked();
    gate.release();
    settle_until(&zetta, cx, |zetta| !zetta.session_handover_in_flight(5));
    assert_eq!(daemon.calls(), ["detach []"]);
}
