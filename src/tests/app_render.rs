use super::*;
use crate::view_boundary::Subview;
use gpui::{MouseButton, MouseDownEvent, MouseUpEvent, PlatformInput, TestAppContext, point, size};
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaintLayer {
    Tab,
    TitleBarControl,
    PopoverMenu,
    Modal,
    ModalPopup,
}

struct LayeringTestView {
    clicked: Option<PaintLayer>,
    show_title_bar_control: bool,
    show_popover_menu: bool,
    show_modal: bool,
    show_modal_popup: bool,
}

impl LayeringTestView {
    fn layer(
        id: &'static str,
        layer: PaintLayer,
        color: u32,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .absolute()
            .inset_0()
            .bg(gpui::rgb(color))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.clicked = Some(layer);
                cx.stop_propagation();
            }))
    }
}

impl Render for LayeringTestView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = Self::layer("layer-tab", PaintLayer::Tab, 0x111111, cx);
        let title_bar_control = Self::layer(
            "layer-title-bar-control",
            PaintLayer::TitleBarControl,
            0x222222,
            cx,
        );
        let popover_menu = Self::layer("layer-popover-menu", PaintLayer::PopoverMenu, 0x333333, cx);
        let modal_popup = Self::layer("layer-modal-popup", PaintLayer::ModalPopup, 0x555555, cx);
        let modal = Self::layer("layer-modal", PaintLayer::Modal, 0x444444, cx)
            .when(self.show_modal_popup, |modal| {
                modal.child(deferred(modal_popup).with_priority(MODAL_POPUP_PAINT_PRIORITY))
            });

        div()
            .relative()
            .size_full()
            .child(tab)
            .when(self.show_title_bar_control, |root| {
                root.child(
                    deferred(title_bar_control).with_priority(TITLE_BAR_CONTROL_PAINT_PRIORITY),
                )
            })
            .when(self.show_popover_menu, |root| {
                root.child(deferred(popover_menu).with_priority(TITLE_BAR_POPOVER_PAINT_PRIORITY))
            })
            .when(self.show_modal, |root| {
                root.child(deferred(modal).with_priority(MODAL_OVERLAY_PAINT_PRIORITY))
            })
    }
}

fn click_center(
    window: &gpui::WindowHandle<LayeringTestView>,
    cx: &mut TestAppContext,
) -> PaintLayer {
    window
        .update(cx, |view, _, _| {
            view.clicked = None;
        })
        .unwrap();

    cx.update(|cx| {
        cx.update_window((*window).into(), |_, window, cx| {
            let position = point(px(50.), px(50.));
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    position,
                    button: MouseButton::Left,
                    ..Default::default()
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    position,
                    button: MouseButton::Left,
                    ..Default::default()
                }),
                cx,
            );
        })
        .unwrap();
    });
    cx.run_until_parked();

    window
        .update(cx, |view, _, _| {
            view.clicked
                .expect("a visible layer should receive the click")
        })
        .unwrap()
}

#[gpui::test]
fn deferred_layers_follow_the_window_chrome_hierarchy(cx: &mut TestAppContext) {
    let window = cx.open_window(size(px(100.), px(100.)), |_, _| LayeringTestView {
        clicked: None,
        show_title_bar_control: true,
        show_popover_menu: true,
        show_modal: true,
        show_modal_popup: true,
    });
    cx.run_until_parked();

    assert_eq!(click_center(&window, cx), PaintLayer::ModalPopup);

    window
        .update(cx, |view, _, cx| {
            view.show_modal_popup = false;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(click_center(&window, cx), PaintLayer::Modal);

    window
        .update(cx, |view, _, cx| {
            view.show_modal = false;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(click_center(&window, cx), PaintLayer::PopoverMenu);

    window
        .update(cx, |view, _, cx| {
            view.show_popover_menu = false;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(click_center(&window, cx), PaintLayer::TitleBarControl);

    window
        .update(cx, |view, _, cx| {
            view.show_title_bar_control = false;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(click_center(&window, cx), PaintLayer::Tab);
}

/// Records the theme `cx.theme()` resolved to when this view last rendered.
///
/// Stands in for a Zed `ui` component: `Button`, `Label`, `switch` and `Banner`
/// resolve their colors this way, and `Tooltip` and `ContextMenu` offer no
/// override at all, so this is the only way they can follow a project theme.
struct ThemeProbe {
    observed: Rc<Cell<Option<SharedString>>>,
}

impl Render for ThemeProbe {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.observed.set(Some(cx.theme().name.clone()));
        div().size_full()
    }
}

/// A stand-in for `Zetta`, doing what `Zetta::render` does: install the window's
/// own theme globally before building the frame. A stand-in rather than a real
/// `Zetta`, because `Zetta::new` opens a tab, which spawns a shell.
struct ThemeSwapRoot {
    /// What `Zetta::window_theme` would return: the project theme when there is
    /// one, otherwise the application theme.
    window_theme: Arc<Theme>,
    /// Probed from an ordinary child, built during the root's own render.
    inline: Entity<ThemeProbe>,
    /// Probed from behind a cached `Subview`, which renders *after* the root's
    /// `render` has returned — the arrangement the title bar chrome uses.
    cached: Option<Entity<Subview<ThemeSwapRoot>>>,
    cached_probe: Entity<ThemeProbe>,
}

fn render_cached_probe(
    root: &mut ThemeSwapRoot,
    _window: &mut Window,
    _cx: &mut Context<ThemeSwapRoot>,
) -> gpui::AnyElement {
    root.cached_probe.clone().into_any_element()
}

impl Render for ThemeSwapRoot {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.window_theme.clone();
        if !Arc::ptr_eq(GlobalTheme::theme(cx), &theme) {
            GlobalTheme::update_theme(cx, theme);
        }
        let entity = cx.entity();
        let cached = Subview::get_or_insert(&mut self.cached, render_cached_probe, &entity, cx)
            .cached(gpui::StyleRefinement::default().w_full().h(px(10.)));
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(cached)
            .child(self.inline.clone())
    }
}

/// Opens a window whose "project theme" is `project_theme`, with a different
/// theme installed globally beforehand, and returns what each probe observed.
fn observed_probe_themes(
    project_theme: &str,
    application_theme: &str,
    cx: &mut TestAppContext,
) -> (Option<SharedString>, Option<SharedString>) {
    let (window_theme, inline, cached_probe, inline_observed, cached_observed) = cx.update(|cx| {
        theme::init(theme::LoadThemes::All(Box::new(ZettaAssets)), cx);
        let registry = ThemeRegistry::global(cx);
        // `theme::init` registers the asset source but does not read it; Zetta's
        // own bundled themes (`assets/themes/`) arrive with this.
        theme_settings::load_bundled_themes(&registry);
        GlobalTheme::update_theme(cx, registry.get(application_theme).unwrap());
        let inline_observed = Rc::new(Cell::new(None));
        let cached_observed = Rc::new(Cell::new(None));
        (
            registry.get(project_theme).unwrap(),
            cx.new(|_| ThemeProbe {
                observed: inline_observed.clone(),
            }),
            cx.new(|_| ThemeProbe {
                observed: cached_observed.clone(),
            }),
            inline_observed,
            cached_observed,
        )
    });

    let (_root, cx) = cx.add_window_view(move |_, _| ThemeSwapRoot {
        window_theme,
        inline,
        cached: None,
        cached_probe,
    });
    cx.run_until_parked();

    (inline_observed.take(), cached_observed.take())
}

#[gpui::test]
fn rendering_installs_the_windows_theme_for_components_that_read_the_global_one(
    cx: &mut TestAppContext,
) {
    let (inline, cached) = observed_probe_themes("One Dark", "Solarized Light", cx);

    assert_eq!(
        inline.as_deref(),
        Some("One Dark"),
        "a `ui` component built during the frame must see the window's theme, \
         not the launch configuration's"
    );
    assert_eq!(
        cached.as_deref(),
        Some("One Dark"),
        "so must one behind a cached boundary, which renders only after the \
         root's `render` has already returned"
    );
}

#[gpui::test]
fn rendering_leaves_the_global_theme_alone_when_it_already_matches(cx: &mut TestAppContext) {
    let (inline, cached) = observed_probe_themes("Solarized Light", "Solarized Light", cx);

    assert_eq!(inline.as_deref(), Some("Solarized Light"));
    assert_eq!(cached.as_deref(), Some("Solarized Light"));
}

/// Where a transient notice is rendered, in the two arrangements that matter.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NoticePlacement {
    Absent,
    /// Floating over the content, as an overlay sibling of the window column.
    Floating,
    /// Sharing the column with the content, as the feedback banners do.
    InColumn,
}

/// Stands in for `Zetta::render`'s composition: a full-size relative root with
/// the window column as a child and overlays as absolutely-positioned siblings.
/// A stand-in rather than a real `Zetta`, whose `new` opens a tab and spawns a
/// shell.
struct NoticeHarness {
    placement: NoticePlacement,
}

const NOTICE_HEIGHT: Pixels = px(40.);

impl Render for NoticeHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let column = div()
            .size_full()
            .flex()
            .flex_col()
            .when(self.placement == NoticePlacement::InColumn, |column| {
                column.child(div().flex_none().w_full().h(NOTICE_HEIGHT))
            })
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .debug_selector(|| "tab-body".to_owned()),
            );
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .child(column)
            .children((self.placement == NoticePlacement::Floating).then(|| {
                div()
                    .absolute()
                    .bottom(px(12.))
                    .right(px(12.))
                    .w(px(200.))
                    .h(NOTICE_HEIGHT)
                    .debug_selector(|| "notice".to_owned())
            }))
    }
}

/// A transient notice must not take layout space away from the terminals.
///
/// A banner in the feedback column does, and for a *shared* pane that reflow is
/// published: the pane reports its smaller grid, the multiplexer arbitrates every
/// viewer down to the smallest of them, and the other window is resized to match.
/// So telling the user their tab could now be joined resized their windows — and
/// the resize was not undone when the message took itself away again.
#[gpui::test]
fn a_transient_notice_does_not_take_layout_space(cx: &mut TestAppContext) {
    let body_height = |placement: NoticePlacement, cx: &mut TestAppContext| {
        let (_root, cx) = cx.add_window_view(move |_, _| NoticeHarness { placement });
        cx.run_until_parked();
        let notice = cx.debug_bounds("notice").is_some();
        let body = cx
            .debug_bounds("tab-body")
            .expect("the tab body is always laid out")
            .size
            .height;
        (body, notice)
    };

    let (without, no_notice) = body_height(NoticePlacement::Absent, cx);
    assert!(!no_notice);
    let (floating, shown) = body_height(NoticePlacement::Floating, cx);
    assert!(shown, "the notice has to actually be on screen");
    assert_eq!(
        floating, without,
        "a floating notice must leave the terminals exactly as they were"
    );

    // The arrangement this replaced, so the property is pinned rather than
    // assumed: a notice in the column shortens the body by its own height.
    let (in_column, _) = body_height(NoticePlacement::InColumn, cx);
    assert_eq!(in_column, without - NOTICE_HEIGHT);
}

struct ProjectOfferBannerHarness;

impl Render for ProjectOfferBannerHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let root = format!(
            r"C:\Users\saltw\{}",
            "nested-project-directory\\".repeat(20)
        );
        div().size_full().child(Zetta::project_offer_banner(
            Path::new(&root),
            h_flex()
                .debug_selector(|| "project-offer-actions".to_owned())
                .flex_none()
                .gap_1()
                .child(
                    div()
                        .debug_selector(|| "dismiss-project-offer".to_owned())
                        .child(Button::new("dismiss", "Dismiss").style(ButtonStyle::Outlined)),
                )
                .child(
                    div()
                        .debug_selector(|| "accept-project-offer".to_owned())
                        .child(Button::new("accept", "Add project").style(ButtonStyle::Filled)),
                ),
        ))
    }
}

#[gpui::test]
fn project_offer_actions_remain_visible_when_the_message_is_long(cx: &mut TestAppContext) {
    cx.update(|cx| {
        theme_settings::init(theme::LoadThemes::All(Box::new(ZettaAssets)), cx);
        let registry = ThemeRegistry::global(cx);
        GlobalTheme::update_theme(cx, registry.get("One Light").unwrap());
    });
    let (_root, cx) = cx.add_window_view(|_, _| ProjectOfferBannerHarness);
    cx.simulate_resize(size(px(520.), px(320.)));
    cx.run_until_parked();

    let actions = cx
        .debug_bounds("project-offer-actions")
        .expect("project offer actions should be laid out");
    let accept = cx
        .debug_bounds("accept-project-offer")
        .expect("Add project should be laid out");
    assert!(actions.size.width > px(0.));
    assert!(accept.size.width > px(0.));
    assert!(accept.origin.x + accept.size.width <= px(520.));
}
