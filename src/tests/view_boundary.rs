use super::*;
use gpui::{TestAppContext, VisualTestContext};
use std::cell::Cell;
use std::rc::Rc;

/// A stand-in for `Zetta`: the parent view the boundaries under test render
/// from. Using a stand-in rather than a real `Zetta` keeps these tests to the
/// contract itself — `Zetta::new` opens a tab, which spawns a shell.
///
/// The layout mirrors `Zetta::render`: a cached column boundary filling the
/// window, with overlay boundaries stacked over it.
struct HarnessRoot {
    column: Option<Entity<Subview<HarnessRoot>>>,
    overlay: Option<Entity<Subview<HarnessRoot>>>,
    /// Rendered inside the column boundary, standing in for a terminal view.
    descendant: Entity<HarnessDescendant>,
    show_overlay: bool,
    /// Whether the overlay boundary is cached, which changes how its contents
    /// are laid out: a cached view is laid out as a root of its own.
    cache_overlay: bool,
    column_renders: Rc<Cell<usize>>,
    overlay_renders: Rc<Cell<usize>>,
}

/// A view nested inside the cached column boundary.
struct HarnessDescendant {
    renders: Rc<Cell<usize>>,
}

impl Render for HarnessDescendant {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div().size_full()
    }
}

fn render_column(
    root: &mut HarnessRoot,
    _window: &mut Window,
    _cx: &mut Context<HarnessRoot>,
) -> gpui::AnyElement {
    root.column_renders.set(root.column_renders.get() + 1);
    div()
        .size_full()
        .flex()
        .flex_col()
        .debug_selector(|| "column".to_owned())
        .child(root.descendant.clone())
        .into_any_element()
}

/// Stands in for a settings-style overlay: a backdrop that positions itself
/// with `absolute inset_0` and centres a dialog inside itself.
fn render_overlay(
    root: &mut HarnessRoot,
    _window: &mut Window,
    _cx: &mut Context<HarnessRoot>,
) -> gpui::AnyElement {
    root.overlay_renders.set(root.overlay_renders.get() + 1);
    overlay_boundary_root(Some(
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .debug_selector(|| "overlay-backdrop".to_owned())
            .child(
                div()
                    .w_full()
                    .h_full()
                    .debug_selector(|| "dialog".to_owned()),
            )
            .into_any_element(),
    ))
}

impl Render for HarnessRoot {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let column = Subview::get_or_insert(&mut self.column, render_column, &entity, cx)
            .cached(gpui::StyleRefinement::default().size_full());
        let cache_overlay = self.cache_overlay;
        let overlay = self.show_overlay.then(|| {
            let boundary = Subview::get_or_insert(&mut self.overlay, render_overlay, &entity, cx);
            if cache_overlay {
                overlay_boundary(boundary.cached(gpui::StyleRefinement::default().size_full()))
            } else {
                overlay_boundary(boundary)
            }
        });
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .child(column)
            .children(overlay)
    }
}

struct Harness {
    root: Entity<HarnessRoot>,
    column_renders: Rc<Cell<usize>>,
    overlay_renders: Rc<Cell<usize>>,
    descendant_renders: Rc<Cell<usize>>,
    descendant: Entity<HarnessDescendant>,
}

impl Harness {
    fn open(show_overlay: bool, cx: &mut TestAppContext) -> (Self, &mut VisualTestContext) {
        Self::open_with(show_overlay, false, cx)
    }

    fn open_with(
        show_overlay: bool,
        cache_overlay: bool,
        cx: &mut TestAppContext,
    ) -> (Self, &mut VisualTestContext) {
        let column_renders = Rc::new(Cell::new(0));
        let overlay_renders = Rc::new(Cell::new(0));
        let descendant_renders = Rc::new(Cell::new(0));
        let (root, cx) = cx.add_window_view({
            let column_renders = column_renders.clone();
            let overlay_renders = overlay_renders.clone();
            let descendant_renders = descendant_renders.clone();
            move |_, cx| {
                let descendant = cx.new(|_| HarnessDescendant {
                    renders: descendant_renders,
                });
                HarnessRoot {
                    column: None,
                    overlay: None,
                    descendant,
                    show_overlay,
                    cache_overlay,
                    column_renders,
                    overlay_renders,
                }
            }
        });
        cx.run_until_parked();
        let descendant = root.read_with(cx, |root, _| root.descendant.clone());
        (
            Self {
                root,
                column_renders,
                overlay_renders,
                descendant_renders,
                descendant,
            },
            cx,
        )
    }

    fn counts(&self) -> (usize, usize, usize) {
        (
            self.column_renders.get(),
            self.overlay_renders.get(),
            self.descendant_renders.get(),
        )
    }
}

#[gpui::test]
fn a_boundary_is_created_once_and_reused_across_frames(cx: &mut TestAppContext) {
    let (harness, cx) = Harness::open(false, cx);
    assert_eq!(
        harness.counts(),
        (1, 0, 1),
        "the column and its descendant render once on the first frame"
    );
    let boundary = harness
        .root
        .read_with(cx, |root, _| root.column.as_ref().map(Entity::entity_id))
        .expect("the column boundary is created on the first frame");

    harness.root.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();

    assert_eq!(
        harness
            .root
            .read_with(cx, |root, _| root.column.as_ref().map(Entity::entity_id)),
        Some(boundary),
        "the boundary has to outlive the frame that built it; a fresh entity \
         per frame would be dirty every frame and so never cached"
    );

    let counts = harness.counts();
    cx.run_until_parked();
    assert_eq!(
        harness.counts(),
        counts,
        "a boundary must not schedule frames of its own"
    );
}

#[gpui::test]
fn notifying_the_parent_busts_a_cached_boundary(cx: &mut TestAppContext) {
    let (harness, cx) = Harness::open(false, cx);
    let (before, _, _) = harness.counts();

    harness.root.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();

    let (after, _, _) = harness.counts();
    assert_eq!(
        after,
        before + 1,
        "every notify on the parent has to rebuild the boundary, because a \
         boundary renders from the parent's state and nothing narrower tracks \
         which part of it changed"
    );
}

#[gpui::test]
fn notifying_one_boundary_leaves_the_other_cached(cx: &mut TestAppContext) {
    let (harness, cx) = Harness::open(true, cx);
    let (column_before, overlay_before, _) = harness.counts();
    assert_eq!(overlay_before, 1, "the overlay should render when shown");

    // What a scroll inside the overlay does: GPUI's handlers notify
    // `window.current_view()`, which is the overlay's own boundary.
    let overlay = harness
        .root
        .read_with(cx, |root, _| root.overlay.clone())
        .expect("the overlay boundary is created when the overlay is shown");
    overlay.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();

    let (column_after, overlay_after, _) = harness.counts();
    assert_eq!(
        overlay_after,
        overlay_before + 1,
        "the notified boundary re-renders"
    );
    assert_eq!(
        column_after, column_before,
        "the cached column must stay out of a frame it did not cause; this is \
         what makes scrolling an overlay cheap"
    );
}

#[gpui::test]
fn notifying_a_descendant_rebuilds_the_cached_boundary_above_it(cx: &mut TestAppContext) {
    let (harness, cx) = Harness::open(true, cx);
    let (column_before, _, descendant_before) = harness.counts();

    // A terminal view repainting behind an open dialog: GPUI marks the whole
    // ancestor chain dirty, so the cached column cannot serve a stale frame.
    harness.descendant.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();

    let (column_after, _, descendant_after) = harness.counts();
    assert_eq!(
        descendant_after,
        descendant_before + 1,
        "the notified descendant re-renders"
    );
    assert_eq!(
        column_after,
        column_before + 1,
        "a cached boundary must not swallow a repaint its descendant asked for"
    );
}

#[gpui::test]
fn a_cached_boundary_fills_the_box_the_composer_gives_it(cx: &mut TestAppContext) {
    let (_harness, cx) = Harness::open(false, cx);
    let viewport = cx.update(|window, _| window.viewport_size());

    assert_eq!(
        cx.debug_bounds("column").map(|bounds| bounds.size),
        Some(viewport),
        "a cached view is laid out from the style the composer passes to \
         `cached`, not measured from its contents"
    );
}

#[gpui::test]
fn an_overlay_boundary_covers_the_window_without_taking_space_in_the_column(
    cx: &mut TestAppContext,
) {
    let (_harness, cx) = Harness::open(true, cx);
    let viewport = cx.update(|window, _| window.viewport_size());

    assert_eq!(
        cx.debug_bounds("column").map(|bounds| bounds.size),
        Some(viewport),
        "an overlay boundary is positioned out of flow, so it must not take a \
         share of the composer's flex column"
    );
    assert_eq!(
        cx.debug_bounds("overlay-backdrop")
            .map(|bounds| bounds.size),
        Some(viewport),
        "an `absolute inset_0` overlay root needs the containing block \
         `overlay_boundary_root` gives it, or it collapses to its content size"
    );
    assert_eq!(
        cx.debug_bounds("dialog").map(|bounds| bounds.size),
        Some(viewport),
        "content sized against the backdrop should see the full window"
    );
}

#[gpui::test]
fn a_cached_boundary_gives_absolutely_positioned_contents_a_root_to_resolve_against(
    cx: &mut TestAppContext,
) {
    let (_harness, cx) = Harness::open_with(true, true, cx);
    let viewport = cx.update(|window, _| window.viewport_size());

    // A cached view is laid out with `layout_as_root`, so an `absolute` root
    // has no ancestor at all to resolve its insets against and collapses to
    // its content size. `overlay_boundary_root` is what stands between the
    // cache and that collapse.
    assert_eq!(
        cx.debug_bounds("overlay-backdrop")
            .map(|bounds| bounds.size),
        Some(viewport),
        "a cached overlay must still cover the window"
    );
    assert_eq!(
        cx.debug_bounds("dialog").map(|bounds| bounds.size),
        Some(viewport),
        "and content sized against it must still see the full window"
    );
}
