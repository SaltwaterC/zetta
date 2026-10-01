//! The buttons Zetta's own surfaces use.
//!
//! Every button is a `ui::Button`, dressed by [`DialogButton`] into one of
//! three roles. Before this, the settings dialog alone built buttons five ways —
//! hand-rolled `div`s in the header and the modals, `action_button`, the `ui`
//! button, the icon button, and a link — so the same Close was `px_3 py_1` in
//! one place and `px_4 py_2` in the next, a disabled button faded to 0.5 on one
//! page, to 0.65 on another and not at all on a third, and a destructive action
//! looked exactly like a confirming one.
//!
//! Keyboard focus is shown as a ring *around* the button rather than by
//! swapping its style, so a focused primary button is still recognisably the
//! primary one. The ring's border is always drawn, transparent when unfocused,
//! so focusing a button never nudges its neighbours.

use std::rc::Rc;

use gpui::{
    AnyElement, AnyView, App, ClickEvent, ElementId, KeybindingKeystroke, Keystroke, SharedString,
    Window, div, px,
};
use theme::ThemeColors;
use ui::{KeyBinding, TintColor, Tooltip, prelude::*};

use crate::ui_tokens::RADIUS_CONTROL;

/// What a button does, which decides how it looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ButtonRole {
    /// The action the surface exists for: Save, Create, Connect. At most one per
    /// surface, placed last.
    Primary,
    /// Everything else: Cancel, Close, Add, Browse.
    Secondary,
    /// An action that throws something away: Close tab, Remove, Delete.
    Destructive,
}

/// What builds a button's tooltip when it is shown.
type TooltipBuilder = Box<dyn Fn(&mut Window, &mut App) -> AnyView>;

/// A button in one of the three [`ButtonRole`]s, with its state.
#[must_use]
pub(crate) struct DialogButton {
    id: ElementId,
    label: SharedString,
    role: ButtonRole,
    enabled: bool,
    loading: bool,
    focused: bool,
    compact: bool,
    tooltip: Option<TooltipBuilder>,
}

impl DialogButton {
    pub(crate) fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        role: ButtonRole,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            role,
            enabled: true,
            loading: false,
            focused: false,
            compact: false,
            tooltip: None,
        }
    }

    /// A button that cannot be used right now is shown disabled and ignores
    /// clicks. It keeps its place, so the row does not reflow when it enables.
    pub(crate) fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Work the button started is in flight: it shows a spinner and cannot be
    /// clicked again, and keeps its label so the row keeps its width.
    pub(crate) fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Whether the button holds keyboard focus.
    pub(crate) fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// The smaller size, for a button sitting in a form row beside a field.
    pub(crate) fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }

    pub(crate) fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(Box::new(Tooltip::text(tooltip.into())));
        self
    }

    /// A tooltip naming the action the button runs and the shortcut the
    /// effective keymap binds it to, resolved when it is shown rather than
    /// written into the label. `focus` is where the binding is looked up from;
    /// without one, it is the window's focus.
    pub(crate) fn action_tooltip(
        mut self,
        title: impl Into<SharedString>,
        action: &dyn gpui::Action,
        focus: Option<&gpui::FocusHandle>,
    ) -> Self {
        self.tooltip = Some(match focus {
            Some(focus) => Box::new(Tooltip::for_action_title_in(title.into(), action, focus)),
            None => Box::new(Tooltip::for_action_title(title.into(), action)),
        });
        self
    }

    /// A tooltip naming what the button does and the key that does the same.
    /// For the keys a surface handles itself — Esc to close, Enter to confirm
    /// — which no keymap binds, so [`Self::action_tooltip`] cannot find them.
    pub(crate) fn key_tooltip(mut self, title: impl Into<SharedString>, key: SurfaceKey) -> Self {
        self.tooltip = Some(Box::new(key_tooltip(title.into(), key)));
        self
    }

    pub(crate) fn render(
        self,
        colors: &ThemeColors,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> AnyElement {
        let style = match self.role {
            // Tinted rather than `Filled`, which most themes draw barely apart
            // from the panel, so the primary action did not stand out.
            ButtonRole::Primary => ButtonStyle::Tinted(TintColor::Accent),
            ButtonRole::Secondary => ButtonStyle::Outlined,
            ButtonRole::Destructive => ButtonStyle::Tinted(TintColor::Error),
        };
        let usable = self.enabled && !self.loading;
        let mut button = Button::new(self.id, self.label)
            .style(style)
            .size(if self.compact {
                ButtonSize::Compact
            } else {
                ButtonSize::Default
            })
            .loading(self.loading)
            .disabled(!usable);
        if let Some(tooltip) = self.tooltip {
            button = button.tooltip(tooltip);
        }
        if usable {
            button = button.on_click(on_click);
        }
        focus_ring(self.focused, colors, button)
    }
}

/// A key a surface handles itself rather than through the keymap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceKey {
    /// Closes or cancels.
    Escape,
    /// Confirms.
    Enter,
}

impl SurfaceKey {
    /// The key in keymap notation.
    pub(crate) fn keystroke(self) -> &'static str {
        match self {
            Self::Escape => "escape",
            Self::Enter => "enter",
        }
    }
}

/// A tooltip showing `title` and `key` the way an action's tooltip shows its
/// binding.
pub(crate) fn key_tooltip(
    title: SharedString,
    key: SurfaceKey,
) -> impl Fn(&mut Window, &mut App) -> AnyView {
    move |_, cx| {
        let binding = Keystroke::parse(key.keystroke()).ok().map(|keystroke| {
            KeyBinding::from_keystrokes(
                Rc::from([KeybindingKeystroke::from_keystroke(keystroke)]),
                false,
            )
        });
        cx.new(|_| Tooltip::new(title.clone()).key_binding(binding))
            .into()
    }
}

/// The ring that shows keyboard focus around a control, outside its own border.
pub(crate) fn focus_ring(
    focused: bool,
    colors: &ThemeColors,
    control: impl IntoElement,
) -> AnyElement {
    div()
        .flex_none()
        .rounded(RADIUS_CONTROL + px(2.))
        .border_1()
        .border_color(if focused {
            colors.border_focused
        } else {
            gpui::transparent_black()
        })
        .child(control)
        .into_any_element()
}

/// An icon-only button that removes one row of a list, with its purpose in its
/// tooltip and its accessible label.
///
/// Replaces the bare `×` text buttons, which said nothing about what they
/// removed to a pointer hovering them or to a screen reader.
pub(crate) fn remove_button(
    id: impl Into<ElementId>,
    what: &str,
    focused: bool,
    colors: &ThemeColors,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let label: SharedString = format!("Remove {what}").into();
    icon_action(id, IconName::Trash, label, focused, colors, on_click)
}

/// An icon-only button with a tooltip and an accessible label, carrying the
/// same focus ring as every other button.
pub(crate) fn icon_action(
    id: impl Into<ElementId>,
    icon: IconName,
    label: SharedString,
    focused: bool,
    colors: &ThemeColors,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    focus_ring(
        focused,
        colors,
        IconButton::new(id, icon)
            .icon_size(IconSize::Small)
            .icon_color(Color::Custom(colors.icon))
            .aria_label(label.clone())
            .tooltip(Tooltip::text(label))
            .on_click(on_click),
    )
}

#[cfg(test)]
#[path = "tests/ui_buttons.rs"]
mod tests;
