use super::*;

/// A stand-in subscriber, in place of a real `Zetta` — `Zetta::new` opens a tab,
/// which spawns a shell.
struct SizeWatcher;

impl gpui::Render for SizeWatcher {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        gpui::div()
    }
}

/// Watching a terminal's size must not keep the terminal alive.
///
/// GPUI stores a subscription on the *emitter* and drops it only when the
/// emitter is released, so a closure that captures its own emitter is a cycle
/// nothing can break. A shared pane's terminal caught in one never stops its
/// byte stream when its tab closes, so the relay socket stays open and the
/// multiplexer keeps counting this window among the pane's viewers — which is
/// how unsharing came to be refused for a window that had closed the tab.
#[gpui::test]
async fn watching_a_terminals_size_does_not_keep_it_alive(cx: &mut gpui::TestAppContext) {
    let terminal = cx.update(|cx| {
        cx.new(|cx| {
            terminal::TerminalBuilder::new_display_only(
                terminal::terminal_settings::CursorShape::Block,
                terminal::terminal_settings::AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                util::paths::PathStyle::local(),
            )
            .subscribe(cx)
        })
    });
    let weak = terminal.downgrade();
    let (watcher, window) = cx.add_window_view(|_, _| SizeWatcher);
    watcher.update_in(window, |_, window, cx| {
        watch_grid_size(&terminal, window, cx, |_, _, _| {});
    });

    drop(terminal);
    window.run_until_parked();

    assert!(
        weak.upgrade().is_none(),
        "the subscription must not be the terminal's last owner"
    );
}

/// A local resize that remains clamped by the daemon still has to wake the
/// shared-size reporter, so a later resize of another viewer can grow the
/// common grid.
#[gpui::test]
async fn watching_a_terminals_size_includes_local_capacity_changes(cx: &mut gpui::TestAppContext) {
    let (watcher, window) = cx.add_window_view(|_, _| SizeWatcher);
    let terminal = window.new(|cx| {
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
    let changes = std::rc::Rc::new(std::cell::Cell::new(0));
    watcher.update_in(window, {
        let changes = changes.clone();
        let terminal = terminal.clone();
        move |_, window, cx| {
            watch_grid_size(&terminal, window, cx, move |_, _, _| {
                changes.set(changes.get() + 1);
            });
        }
    });

    let make_bounds = |columns: f32, lines: f32| terminal::TerminalBounds {
        cell_width: gpui::px(10.),
        line_height: gpui::px(10.),
        bounds: gpui::Bounds {
            origin: gpui::Point::default(),
            size: gpui::Size {
                width: gpui::px(columns * 10.),
                height: gpui::px(lines * 10.),
            },
        },
    };
    window.update_window_entity(&terminal, |terminal, window, cx| {
        terminal.set_size(make_bounds(100., 24.));
        terminal.sync(window, cx);
        terminal.set_shared_viewport(80, 20);
        terminal.sync(window, cx);
    });
    changes.set(0);

    window.update_window_entity(&terminal, |terminal, window, cx| {
        terminal.set_size(make_bounds(120., 30.));
        terminal.sync(window, cx);
    });
    assert_eq!(changes.get(), 1);
}
