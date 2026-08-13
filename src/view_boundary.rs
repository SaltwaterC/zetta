//! Render boundaries that keep unchanged subtrees out of the per-frame render.
//!
//! GPUI re-renders the root view on every frame it draws, so anything built
//! directly inside `Zetta::render` is rebuilt, re-laid-out and re-inserted into
//! the bounds tree even when nothing it displays changed. Scrolling an overlay
//! is the pathological case: one notify per wheel step redraws the title bar,
//! the tab bar and the pane chrome for a frame in which none of them moved.
//!
//! Wrapping a subtree in its own entity fixes both halves of that:
//!
//! - GPUI can cache it. [`gpui::Entity::cached`] reuses the previous prepaint
//!   and paint while the entity is not dirty and its bounds, content mask and
//!   text style are unchanged.
//! - Scroll and hover inside the subtree notify *that* entity rather than the
//!   root, because GPUI's interaction handlers notify `window.current_view()`,
//!   which a [`Subview`] rebinds for everything it renders.
//!
//! The invalidation contract is the observer registered in [`Subview::new`]:
//! a subview renders from its parent's state, so every `cx.notify()` on the
//! parent marks it dirty. That is deliberately conservative — it costs a
//! rebuild the subview may not have needed, and it cannot go stale as long as
//! state changes notify the parent, which is already how every mutation in the
//! application schedules its repaint. Descendants are handled by GPUI itself:
//! notifying a view marks its whole ancestor chain dirty, so terminal output
//! still repaints through a cached window column.
//!
//! [`Subview`] is generic over the parent view so the contract above can be
//! exercised against a stand-in parent; see `tests/view_boundary.rs`.

use super::*;
use gpui::{AnyElement, Context, Entity, Render, WeakEntity, Window};

/// Builds a subview's contents from its parent's state.
pub(crate) type SubviewRender<V> = fn(&mut V, &mut Window, &mut Context<V>) -> AnyElement;

/// The boundary wrapped around one part of `Zetta`'s tree.
pub(crate) type ZettaSubview = Subview<Zetta>;

/// Renders one part of a parent view's tree inside its own entity.
///
/// `render` is a function pointer rather than a boxed closure so a subview
/// carries no captured state: everything it draws is read from the parent at
/// render time, which is what makes the observer in [`Self::new`] a complete
/// invalidation contract.
pub(crate) struct Subview<V: 'static> {
    parent: WeakEntity<V>,
    render: SubviewRender<V>,
}

impl<V: 'static> Subview<V> {
    pub(crate) fn new(parent: &Entity<V>, render: SubviewRender<V>, cx: &mut App) -> Entity<Self> {
        let weak_parent = parent.downgrade();
        cx.new(|cx| {
            cx.observe(parent, |_, _, cx| cx.notify()).detach();
            Self {
                parent: weak_parent,
                render,
            }
        })
    }

    /// Creates the boundary on first use and reuses it afterwards. A boundary
    /// has to outlive the frame that built it — a fresh entity every frame
    /// would be dirty every frame, and so never cached.
    pub(crate) fn get_or_insert(
        slot: &mut Option<Entity<Self>>,
        render: SubviewRender<V>,
        parent: &Entity<V>,
        cx: &mut App,
    ) -> Entity<Self> {
        slot.get_or_insert_with(|| Self::new(parent, render, cx))
            .clone()
    }
}

impl<V: 'static> Render for Subview<V> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let render = self.render;
        self.parent
            .update(cx, |parent, cx| render(parent, window, cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}

/// Places an overlay view over the window.
///
/// The overlay has to sit outside the composer's flex column, and the box that
/// takes it out of flow has to be this one rather than the view's own root: the
/// view is laid out inside it, so an `absolute` root would have no containing
/// block of its own to resolve against.
///
/// Overlay boundaries are deliberately not cached. Whichever overlay the
/// pointer is in is the one being scrolled, so its cache could never hit, and a
/// cached view that misses costs an extra layout pass — measurably more than it
/// saves. What the boundary buys is that the scroll notifies the overlay
/// instead of `Zetta`, which is what keeps the cached window column and
/// settings page out of the frame.
pub(crate) fn overlay_boundary(overlay: impl IntoElement) -> gpui::Div {
    div().absolute().top_0().left_0().size_full().child(overlay)
}

/// The root an overlay boundary's contents are laid out in.
///
/// Overlays position themselves with `absolute inset_0`. A cached view is laid
/// out with `layout_as_root`, so an overlay reaching the cache unwrapped would
/// be an absolutely positioned *root*, with no ancestor to resolve its insets
/// against, and would collapse to its content size in the top-left corner.
/// What this wrapper has to provide is a root that is in flow; `relative` then
/// makes it the explicit containing block rather than an incidental one.
pub(crate) fn overlay_boundary_root(overlay: Option<AnyElement>) -> AnyElement {
    div()
        .size_full()
        .relative()
        .children(overlay)
        .into_any_element()
}

#[cfg(test)]
#[path = "tests/view_boundary.rs"]
mod tests;
