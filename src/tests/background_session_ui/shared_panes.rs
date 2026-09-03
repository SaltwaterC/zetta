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
