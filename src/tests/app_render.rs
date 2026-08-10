use super::*;
use gpui::{MouseButton, MouseDownEvent, MouseUpEvent, PlatformInput, TestAppContext, point, size};

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
