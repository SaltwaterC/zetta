use super::*;

use gpui::{FocusHandle, TestAppContext};
use std::{cell::Cell, rc::Rc};

fn available_for(surfaces: &[FocusSurface]) -> FocusSurfaceAvailability {
    let mut available = FocusSurfaceAvailability::default();
    for surface in surfaces {
        match surface {
            FocusSurface::CloseConfirmation => available.close_confirmation = true,
            FocusSurface::SessionAuthentication => available.session_authentication = true,
            FocusSurface::RemoteSession => available.remote_session = true,
            FocusSurface::SerialConsole => available.serial_console = true,
            FocusSurface::OverlayStylePicker => available.overlay_style_picker = true,
            FocusSurface::ThemePicker => available.theme_picker = true,
            FocusSurface::TabIconPicker => available.tab_icon_picker = true,
            FocusSurface::Settings => available.settings = true,
            FocusSurface::TabSearch => available.tab_search = true,
            FocusSurface::MultiCommand => available.multi_command = true,
            FocusSurface::CommandPalette => available.command_palette = true,
            FocusSurface::InlineEditing => available.inline_editing = true,
        }
    }
    available
}

#[test]
fn focus_router_covers_every_surface_in_paint_order() {
    let ordered = [
        FocusSurface::CloseConfirmation,
        FocusSurface::SessionAuthentication,
        FocusSurface::RemoteSession,
        FocusSurface::SerialConsole,
        FocusSurface::OverlayStylePicker,
        FocusSurface::ThemePicker,
        FocusSurface::TabIconPicker,
        FocusSurface::Settings,
        FocusSurface::TabSearch,
        FocusSurface::MultiCommand,
        FocusSurface::CommandPalette,
        FocusSurface::InlineEditing,
    ];

    assert_eq!(focus_surface(FocusSurfaceAvailability::default()), None);
    for surface in ordered {
        assert_eq!(
            focus_surface(available_for(&[surface])),
            Some(surface),
            "the focus router must handle {surface:?}"
        );
    }

    for pair in ordered.windows(2) {
        assert_eq!(
            focus_surface(available_for(pair)),
            Some(pair[0]),
            "the higher painted surface must win over {:?}",
            pair[1]
        );
    }
}

#[test]
fn overlapping_settings_and_picker_surfaces_follow_paint_order() {
    assert_eq!(
        focus_surface(available_for(&[
            FocusSurface::Settings,
            FocusSurface::ThemePicker,
        ])),
        Some(FocusSurface::ThemePicker)
    );
    assert_eq!(
        focus_surface(available_for(&[
            FocusSurface::Settings,
            FocusSurface::TabIconPicker,
        ])),
        Some(FocusSurface::TabIconPicker)
    );
    assert_eq!(
        focus_surface(available_for(&[
            FocusSurface::CommandPalette,
            FocusSurface::Settings,
        ])),
        Some(FocusSurface::Settings)
    );
}

struct ModalKeyboardHarness {
    zetta: Entity<Zetta>,
    picker_focus: FocusHandle,
    terminal_focus: FocusHandle,
    terminal_received: Rc<Cell<usize>>,
}

impl Render for ModalKeyboardHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let zetta = self.zetta.downgrade();
        let terminal_received = self.terminal_received.clone();
        div()
            .size_full()
            .capture_key_down(move |event, window, cx| {
                zetta
                    .update(cx, |zetta, cx| {
                        zetta.modal_key_down_capture(event, window, cx);
                    })
                    .ok();
            })
            .child(
                div().size_full().track_focus(&self.picker_focus).child(
                    div()
                        .size_full()
                        .track_focus(&self.terminal_focus)
                        .on_key_down(move |_, _, cx| {
                            terminal_received.set(terminal_received.get() + 1);
                            cx.stop_propagation();
                        }),
                ),
            )
    }
}

#[gpui::test]
fn modal_capture_routes_stale_terminal_keys_and_preserves_normal_tab_input(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| theme_settings::init(theme::LoadThemes::JustBase, cx));
    let terminal_received = Rc::new(Cell::new(0));
    let terminal_received_for_view = terminal_received.clone();
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        let mut config = Config::defaults(None, None);
        config.profiles.clear();
        let zetta = cx.new(|cx| {
            let mut zetta = Zetta::new(
                config,
                None,
                ZettaLaunchOptions {
                    no_mux: true,
                    ..Default::default()
                },
                window,
                cx,
            );
            zetta.remote_session_picker =
                Some(crate::remote_session_ui::RemoteSessionPicker::default());
            zetta
        });
        let picker_focus = zetta.read(cx).remote_session_focus.clone();
        ModalKeyboardHarness {
            zetta,
            picker_focus,
            terminal_focus: cx.focus_handle(),
            terminal_received: terminal_received_for_view,
        }
    });
    let zetta = harness.update(cx, |harness, _| harness.zetta.clone());
    let terminal_focus = harness.update(cx, |harness, _| harness.terminal_focus.clone());

    harness.update_in(cx, |harness, window, cx| {
        harness.terminal_focus.focus(window, cx);
    });
    cx.run_until_parked();

    let (allowed, focused_remote) = zetta.update_in(cx, |zetta, window, cx| {
        let allowed = zetta.focus_terminal_if_allowed(&terminal_focus, window, cx);
        (
            allowed,
            window.focused(cx) == Some(zetta.remote_session_focus.clone()),
        )
    });
    assert!(
        !allowed,
        "an open picker must reject automatic terminal focus"
    );
    assert!(
        focused_remote,
        "the guard should restore focus to the picker"
    );

    harness.update_in(cx, |harness, window, cx| {
        harness.terminal_focus.focus(window, cx);
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("tab");

    assert_eq!(terminal_received.get(), 0);
    assert_eq!(
        zetta.update(cx, |zetta, _| {
            zetta
                .remote_session_picker
                .as_ref()
                .map(|picker| picker.field)
                .expect("the remote picker remains open")
        }),
        crate::remote_session_ui::RemoteSessionField::Port
    );

    zetta.update(cx, |zetta, _| zetta.remote_session_picker = None);
    harness.update_in(cx, |harness, window, cx| {
        harness.terminal_focus.focus(window, cx);
    });
    cx.run_until_parked();
    terminal_received.set(0);
    cx.simulate_keystrokes("tab");
    assert_eq!(
        terminal_received.get(),
        1,
        "terminal Tab input must remain unchanged without a modal"
    );
}
