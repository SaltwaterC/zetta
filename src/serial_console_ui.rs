use super::*;

impl Zetta {
    pub(crate) fn render_serial_console_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let prompt = self.serial_console.as_ref()?;
        let colors = self.window_theme(cx).colors().clone();
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
                    .when(
                        selected && field == SerialField::BaudRate && prompt.baud_rate.select_all,
                        |row| row.bg(colors.element_selection_background),
                    )
                    .cursor_pointer()
                    .child(
                        Label::new(label)
                            .size(LabelSize::Small)
                            .color(Color::Custom(colors.text_muted)),
                    )
                    .child(
                        Label::new(value)
                            .size(LabelSize::Small)
                            .color(Color::Custom(colors.text)),
                    )
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
            prompt.baud_rate.caret_marker_display()
        } else {
            prompt.baud_rate.text.clone()
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
                        .text_color(colors.text)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    Label::new("Open serial console")
                                        .size(LabelSize::Large)
                                        .color(Color::Custom(colors.text)),
                                )
                                .child(
                                    Label::new(format!(
                                        "{} · {} baud",
                                        prompt.framing_label(),
                                        prompt.baud_rate.text
                                    ))
                                    .size(LabelSize::Small)
                                    .color(Color::Custom(colors.text_muted)),
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
                                    .text_color(self.window_theme(cx).status().error)
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
                        .map(detected_serial_devices)
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
        let baud_rate = match prompt.baud_rate.text.parse::<u32>() {
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
        let title = format!("{} @ {} baud ({})", serial.port_name, serial.baud_rate, {
            let prompt = SerialConsolePrompt {
                data_bits: serial.data_bits,
                parity: serial.parity,
                stop_bits: serial.stop_bits,
                ..Default::default()
            };
            prompt.framing_label()
        });
        self.open_byte_stream_pane(
            ByteStreamPaneRequest {
                reader: connection.reader,
                writer: connection.writer,
                label: format!("Serial: {}", serial.port_name),
                title,
                input: ByteStreamInputPolicy::Broadcast,
            },
            window,
            cx,
        );
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
        // Only the baud rate is typed into; the other rows are cycled with the
        // arrow keys and have no text a clipboard could carry.
        if prompt.field == SerialField::BaudRate {
            match apply_clipboard_shortcut(&mut prompt.baud_rate, &event.keystroke, cx) {
                ClipboardOutcome::Ignored => {}
                ClipboardOutcome::Unchanged | ClipboardOutcome::Edited => {
                    cx.notify();
                    return true;
                }
            }
        }
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss_serial_console(window, cx),
            "enter" => self.submit_serial_console(cx),
            "tab" => {
                prompt.baud_rate.select_all = false;
                prompt.field = prompt.field.adjacent(event.keystroke.modifiers.shift);
                cx.notify();
            }
            "a" if prompt.field == SerialField::BaudRate
                && (event.keystroke.modifiers.control || event.keystroke.modifiers.platform) =>
            {
                prompt.baud_rate.move_to_end();
                prompt.baud_rate.select_all = true;
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
                prompt.baud_rate.backspace();
                cx.notify();
            }
            key if prompt.field == SerialField::BaudRate
                && key.len() == 1
                && key.as_bytes()[0].is_ascii_digit() =>
            {
                prompt.baud_rate.insert(key);
                cx.notify();
            }
            _ => {}
        }
        true
    }
}
