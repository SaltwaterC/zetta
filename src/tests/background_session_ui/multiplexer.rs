use super::*;

use crate::session_state::{AxisState, LayoutState, PaneState, TabState};

fn pane_summary(id: u64) -> BackgroundPaneSummary {
    BackgroundPaneSummary {
        id,
        label: format!("Pane {id}"),
        profile: "System".to_owned(),
        configured_command: "sh".to_owned(),
        application: "sh".to_owned(),
        foreground_command: None,
        terminal_title: None,
        working_directory: None,
        state: BackgroundPaneState::Running,
        exit: None,
    }
}

fn summary(
    panes: Vec<u64>,
    layout: BackgroundPaneLayout,
    active_pane: u64,
) -> BackgroundSessionSummary {
    BackgroundSessionSummary {
        id: 9,
        title: "shared".to_owned(),
        authentication_required: false,
        active_pane,
        layout,
        panes: panes.into_iter().map(pane_summary).collect(),
        held: false,
        scoped_to: None,
        key_envelope: None,
    }
}

fn pane_state(id: u64, mux_pane_id: Option<u64>) -> PaneState {
    PaneState {
        id,
        mux_pane_id,
        label_number: id as usize,
        generated_label: Some(format!("generated-{id}")),
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

fn tab_state(panes: Vec<PaneState>) -> TabState {
    let first = panes.first().map_or(0, |pane| pane.id);
    TabState {
        pane_theme_source: None,
        attention_id: 17,
        next_pane_label: 20,
        layout: LayoutState::Pane { pane_id: first },
        active_pane: first,
        focus_history: vec![first],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: true,
        silent_mode: true,
        keep_running: true,
        shared: true,
        custom_title: Some("durable title".to_owned()),
        worktree_seed_title: Some("durable worktree".to_owned()),
        process_title: Some("durable process".to_owned()),
        icon: Some("terminal".to_owned()),
        icon_override: None,
        pinned: true,
        panes,
        theme_override: Some("Ayu Dark".to_owned()),
    }
}

fn canonical(
    panes: Vec<u64>,
    layout: BackgroundPaneLayout,
    active_pane: u64,
    state: TabState,
) -> zmux::messages::SharedSessionState {
    let mut canonical = zmux::messages::SharedSessionState::new(
        9,
        summary(panes, layout.clone(), active_pane),
        serde_json::to_value(state).unwrap(),
    );
    canonical.presentation.layout = layout;
    canonical.presentation.active_pane = active_pane;
    canonical
}

#[test]
fn stale_four_pane_blob_is_reduced_to_the_one_live_pane() {
    let stale = tab_state(vec![
        pane_state(101, Some(11)),
        pane_state(102, Some(12)),
        pane_state(103, Some(13)),
        pane_state(104, Some(14)),
    ]);
    let live = canonical(
        vec![14],
        BackgroundPaneLayout::Pane { pane_id: 14 },
        14,
        stale,
    );

    let (repaired, changed) =
        reconcile_shared_attach_state(serde_json::from_value(live.state.clone()).unwrap(), &live)
            .unwrap();

    assert!(changed);
    assert_eq!(repaired.panes.len(), 1);
    assert_eq!(repaired.panes[0].mux_pane_id, Some(14));
    assert_eq!(repaired.panes[0].id, 14);
    assert_eq!(repaired.layout, LayoutState::Pane { pane_id: 14 });
    assert_eq!(repaired.active_pane, 14);
}

#[test]
fn surviving_metadata_and_durable_tab_fields_are_preserved_by_mux_id() {
    let mut stale_survivor = pane_state(204, Some(24));
    stale_survivor.custom_label = Some("kept label".to_owned());
    stale_survivor.theme_override = Some("Dracula".to_owned());
    stale_survivor
        .environment_overrides
        .insert("KEEP".to_owned(), "yes".to_owned());
    let stale = tab_state(vec![pane_state(201, Some(21)), stale_survivor]);
    let canonical = canonical(
        vec![24],
        BackgroundPaneLayout::Pane { pane_id: 24 },
        24,
        stale,
    );

    let (repaired, _) = reconcile_shared_attach_state(
        serde_json::from_value(canonical.state.clone()).unwrap(),
        &canonical,
    )
    .unwrap();

    let survivor = &repaired.panes[0];
    assert_eq!(survivor.custom_label.as_deref(), Some("kept label"));
    assert_eq!(survivor.theme_override.as_deref(), Some("Dracula"));
    assert_eq!(
        survivor
            .environment_overrides
            .get("KEEP")
            .map(String::as_str),
        Some("yes")
    );
    assert_eq!(repaired.custom_title.as_deref(), Some("durable title"));
    assert_eq!(repaired.theme_override.as_deref(), Some("Ayu Dark"));
    assert!(repaired.keep_running);
    assert!(repaired.pinned);
}

#[test]
fn canonical_presentation_repairs_layout_focus_and_visibility() {
    let mut stale = tab_state(vec![pane_state(301, Some(31)), pane_state(302, Some(32))]);
    stale.active_pane = 301;
    stale.focus_history = vec![301, 302];
    stale.maximized_pane = Some(301);
    stale.minimized_panes = vec![302];
    stale.selected_minimized_pane = Some(302);
    let layout = BackgroundPaneLayout::Split {
        axis: "vertical".to_owned(),
        first_ratio: 300,
        first: Box::new(BackgroundPaneLayout::Pane { pane_id: 32 }),
        second: Box::new(BackgroundPaneLayout::Pane { pane_id: 31 }),
    };
    let canonical = canonical(vec![31, 32], layout, 32, stale);

    let (repaired, _) = reconcile_shared_attach_state(
        serde_json::from_value(canonical.state.clone()).unwrap(),
        &canonical,
    )
    .unwrap();

    assert_eq!(
        repaired.layout,
        LayoutState::Split {
            axis: AxisState::Vertical,
            first_ratio: 300,
            first: Box::new(LayoutState::Pane { pane_id: 32 }),
            second: Box::new(LayoutState::Pane { pane_id: 31 }),
        }
    );
    assert_eq!(repaired.active_pane, 32);
    assert_eq!(repaired.focus_history, vec![31, 32]);
    assert_eq!(repaired.maximized_pane, None);
    assert_eq!(repaired.minimized_panes, Vec::<u64>::new());
    assert_eq!(repaired.selected_minimized_pane, None);
}

#[test]
fn a_live_pane_missing_from_the_blob_is_synthesized_from_the_summary() {
    let stale = tab_state(vec![pane_state(401, Some(41))]);
    let layout = BackgroundPaneLayout::Split {
        axis: "horizontal".to_owned(),
        first_ratio: 700,
        first: Box::new(BackgroundPaneLayout::Pane { pane_id: 41 }),
        second: Box::new(BackgroundPaneLayout::Pane { pane_id: 42 }),
    };
    let canonical = canonical(vec![41, 42], layout, 42, stale);

    let (repaired, changed) = reconcile_shared_attach_state(
        serde_json::from_value(canonical.state.clone()).unwrap(),
        &canonical,
    )
    .unwrap();

    assert!(changed);
    let synthesized = repaired.panes.iter().find(|pane| pane.id == 42).unwrap();
    assert_eq!(synthesized.mux_pane_id, Some(42));
    assert_eq!(synthesized.profile, "System");
    assert_eq!(synthesized.generated_label.as_deref(), Some("Pane 42"));
    assert_eq!(repaired.active_pane, 42);
}
