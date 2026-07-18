use super::*;

impl Zetta {
    pub(crate) fn render_serial_console_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let prompt = self.serial_console.as_ref()?;
        let colors = cx.theme().colors().clone();
        let handle = cx.entity().downgrade();

        let field_row =
            |label: &'static str, value: String, field: SerialField| -> gpui::AnyElement {
                let selected = prompt.field == field;
                let click_handle = handle.clone();
                div()
                    .id(("serial-field", field as usize))
                    .w_full()
                    .h_9()
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(if selected {
                        colors.border_focused
                    } else {
                        colors.border
                    })
                    .bg(colors.editor_background)
                    .cursor_pointer()
                    .child(Label::new(label).size(LabelSize::Small).color(Color::Muted))
                    .child(Label::new(value).size(LabelSize::Small))
                    .on_click(move |_, _, cx| {
                        click_handle
                            .update(cx, |this, cx| {
                                if let Some(prompt) = this.serial_console.as_mut() {
                                    let cycle =
                                        prompt.field == field && field != SerialField::BaudRate;
                                    prompt.field = field;
                                    if cycle {
                                        prompt.cycle_current_value(false);
                                    }
                                    cx.notify();
                                }
                            })
                            .ok();
                    })
                    .into_any_element()
            };

        let device_value = if prompt.loading {
            "Scanning…".to_owned()
        } else {
            prompt
                .devices
                .get(prompt.selected_device)
                .map(|device| match &device.description {
                    Some(description) => format!("{} — {description}", device.port_name),
                    None => device.port_name.clone(),
                })
                .unwrap_or_else(|| "No devices found".to_owned())
        };
        let baud_value = if prompt.field == SerialField::BaudRate {
            let cursor = prompt.baud_cursor.min(prompt.baud_rate.len());
            format!(
                "{}|{}",
                &prompt.baud_rate[..cursor],
                &prompt.baud_rate[cursor..]
            )
        } else {
            prompt.baud_rate.clone()
        };
        let status = if prompt.connecting {
            "Connecting…".to_owned()
        } else {
            "Tab: next field · arrows: change · Enter: connect · Ctrl/Cmd-R: refresh · Esc: cancel"
                .to_owned()
        };

        Some(
            div()
                .id("serial-console-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(transparent_black().opacity(0.24))
                .track_focus(&self.serial_console_focus)
                .child(
                    div()
                        .w(px(560.))
                        .max_w(gpui::relative(0.9))
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(Label::new("Open serial console").size(LabelSize::Large))
                                .child(
                                    Label::new(format!(
                                        "{} · {} baud",
                                        prompt.framing_label(),
                                        prompt.baud_rate
                                    ))
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                                ),
                        )
                        .child(field_row("Device", device_value, SerialField::Device))
                        .child(field_row("Baud rate", baud_value, SerialField::BaudRate))
                        .child(field_row(
                            "Data bits",
                            data_bits_label(prompt.data_bits).to_owned(),
                            SerialField::DataBits,
                        ))
                        .child(field_row(
                            "Parity",
                            parity_label(prompt.parity).to_owned(),
                            SerialField::Parity,
                        ))
                        .child(field_row(
                            "Stop bits",
                            stop_bits_label(prompt.stop_bits).to_owned(),
                            SerialField::StopBits,
                        ))
                        .child(field_row(
                            "Flow control",
                            flow_control_label(prompt.flow_control).to_owned(),
                            SerialField::FlowControl,
                        ))
                        .when_some(prompt.error.as_ref(), |panel, error| {
                            panel.child(
                                div()
                                    .px_2()
                                    .text_sm()
                                    .text_color(cx.theme().status().error)
                                    .child(error.clone()),
                            )
                        })
                        .child(
                            div()
                                .pt_1()
                                .text_xs()
                                .text_color(colors.text_muted)
                                .child(status),
                        ),
                )
                .into_any_element(),
        )
    }

    pub(crate) fn toggle_serial_console(
        &mut self,
        _: &ToggleSerialConsole,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.serial_console.is_some() {
            self.dismiss_serial_console(window, cx);
            return;
        }
        self.command_palette = None;
        self.multi_command = None;
        self.tab_search = None;
        self.settings_editor = None;
        self.serial_console_generation = self.serial_console_generation.wrapping_add(1);
        self.serial_console = Some(SerialConsolePrompt::default());
        self.serial_console_focus.focus(window, cx);
        self.refresh_serial_devices(cx);
        cx.notify();
    }

    pub(crate) fn dismiss_serial_console(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.serial_console = None;
        self.serial_console_generation = self.serial_console_generation.wrapping_add(1);
        self.focus_active(window, cx);
        cx.notify();
    }

    fn refresh_serial_devices(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.serial_console.as_mut() else {
            return;
        };
        prompt.loading = true;
        prompt.error = None;
        let generation = self.serial_console_generation;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    serialport::available_ports()
                        .map(|ports| {
                            ports
                                .into_iter()
                                .map(SerialDevice::from)
                                .collect::<Vec<_>>()
                        })
                        .context("enumerating serial devices")
                })
                .await;
            this.update(cx, |this, cx| {
                if this.serial_console_generation != generation {
                    return;
                }
                let Some(prompt) = this.serial_console.as_mut() else {
                    return;
                };
                prompt.loading = false;
                match result {
                    Ok(mut devices) => {
                        devices.sort_by(|left, right| left.port_name.cmp(&right.port_name));
                        prompt.devices = devices;
                        prompt.selected_device = prompt
                            .selected_device
                            .min(prompt.devices.len().saturating_sub(1));
                    }
                    Err(error) => prompt.error = Some(format!("{error:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn submit_serial_console(&mut self, cx: &mut Context<Self>) {
        if self
            .tabs
            .get(self.active_tab)
            .is_some_and(|tab| !can_add_panes(tab.panes.len(), 1))
        {
            if let Some(prompt) = self.serial_console.as_mut() {
                prompt.error = Some(format!(
                    "This tab has reached the {MAX_PANES_PER_TAB}-pane limit"
                ));
            }
            cx.notify();
            return;
        }
        let Some(prompt) = self.serial_console.as_mut() else {
            return;
        };
        if prompt.connecting {
            return;
        }
        let Some(device) = prompt.devices.get(prompt.selected_device) else {
            prompt.error = Some("No serial device is selected".to_owned());
            cx.notify();
            return;
        };
        let baud_rate = match prompt.baud_rate.parse::<u32>() {
            Ok(baud_rate) if baud_rate > 0 => baud_rate,
            _ => {
                prompt.error = Some("Baud rate must be a positive whole number".to_owned());
                cx.notify();
                return;
            }
        };
        let settings = SerialConnectionSettings {
            port_name: device.port_name.clone(),
            baud_rate,
            data_bits: prompt.data_bits,
            parity: prompt.parity,
            stop_bits: prompt.stop_bits,
            flow_control: prompt.flow_control,
        };
        prompt.connecting = true;
        prompt.error = None;
        let generation = self.serial_console_generation;
        let task_settings = settings.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { open_serial_connection(&task_settings) })
                .await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(connection) => {
                    if this.serial_console_generation != generation
                        || !this
                            .serial_console
                            .as_ref()
                            .is_some_and(|prompt| prompt.connecting)
                    {
                        return;
                    }
                    this.serial_console = None;
                    this.serial_console_generation = this.serial_console_generation.wrapping_add(1);
                    this.open_serial_pane(connection, settings, window, cx);
                }
                Err(error) => {
                    if this.serial_console_generation != generation {
                        return;
                    }
                    if let Some(prompt) = this.serial_console.as_mut() {
                        prompt.connecting = false;
                        prompt.error = Some(format!("{error:#}"));
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn open_serial_pane(
        &mut self,
        connection: OpenSerialConnection,
        serial: SerialConnectionSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        if !can_add_panes(tab.panes.len(), 1) {
            return;
        }
        let tab_id = tab.id;
        let active_pane_id = tab.active_pane;
        let Some(profile) = tab.active_profile().cloned() else {
            return;
        };
        let terminal_theme = match resolve_profile_theme(&profile, cx) {
            Ok(theme) => theme,
            Err(error) => {
                self.configuration_error =
                    Some(format!("Could not apply profile theme: {error:#}"));
                cx.notify();
                return;
            }
        };
        let pane_id = self.next_pane_id;
        self.next_pane_id += 1;
        let label = format!("Serial: {}", serial.port_name);

        let tab = &mut self.tabs[self.active_tab];
        tab.maximized_pane = None;
        if !tab
            .layout
            .split(active_pane_id, SplitAxis::Vertical, pane_id)
        {
            return;
        }
        tab.push_pane(TerminalPane {
            id: pane_id,
            label_number: 0,
            generated_label: Some(label.clone()),
            custom_label: None,
            profile,
            view: None,
            error: None,
            wsl_cwd_file: None,
            pending_command: None,
        });
        tab.activate_pane(pane_id);

        let settings = TerminalSpawnSettings::current(cx);
        let builder = TerminalBuilder::new_byte_stream(
            connection.reader,
            connection.writer,
            format!("{} @ {} baud ({})", serial.port_name, serial.baud_rate, {
                let prompt = SerialConsolePrompt {
                    data_bits: serial.data_bits,
                    parity: serial.parity,
                    stop_bits: serial.stop_bits,
                    ..Default::default()
                };
                prompt.framing_label()
            }),
            settings.cursor_shape,
            settings.alternate_scroll,
            settings.max_scroll_history_lines,
            cx.entity_id().as_u64(),
            cx.background_executor(),
            PathStyle::local(),
        );
        let terminal = cx.new(|cx| builder.subscribe(cx));
        let view = cx.new(|cx| TerminalView::new_with_theme(terminal, terminal_theme, window, cx));
        let emit_input_events = self.tabs[self.active_tab].broadcast_input;
        view.update(cx, |view, _| view.set_emit_input_events(emit_input_events));
        cx.subscribe_in(
            &view,
            window,
            move |this, _, event, window, cx| match event {
                TerminalViewEvent::Close => this.close_pane(tab_id, pane_id, window, cx),
                TerminalViewEvent::TitleChanged => cx.notify(),
                TerminalViewEvent::Input(input) => this.broadcast_input(tab_id, pane_id, input, cx),
            },
        )
        .detach();
        let focus_handle = view.focus_handle(cx);
        cx.on_focus(&focus_handle, window, move |this, _, cx| {
            if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.activate_pane(pane_id);
                cx.notify();
            }
        })
        .detach();
        if let Some(pane) = self.tabs[self.active_tab].pane_mut(pane_id) {
            pane.view = Some(view.clone());
        }
        view.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    pub(crate) fn serial_console_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(prompt) = self.serial_console.as_mut() else {
            return false;
        };
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss_serial_console(window, cx),
            "enter" => self.submit_serial_console(cx),
            "tab" => {
                prompt.field = prompt.field.adjacent(event.keystroke.modifiers.shift);
                cx.notify();
            }
            "up" | "left" => {
                prompt.cycle_current_value(true);
                cx.notify();
            }
            "down" | "right" => {
                prompt.cycle_current_value(false);
                cx.notify();
            }
            "r" if event.keystroke.modifiers.control || event.keystroke.modifiers.platform => {
                self.refresh_serial_devices(cx)
            }
            "backspace" if prompt.field == SerialField::BaudRate => {
                if prompt.baud_cursor > 0 {
                    prompt.baud_cursor -= 1;
                    prompt.baud_rate.remove(prompt.baud_cursor);
                }
                cx.notify();
            }
            key if prompt.field == SerialField::BaudRate
                && key.len() == 1
                && key.as_bytes()[0].is_ascii_digit() =>
            {
                prompt.baud_rate.insert_str(prompt.baud_cursor, key);
                prompt.baud_cursor += 1;
                cx.notify();
            }
            _ => {}
        }
        true
    }
}
