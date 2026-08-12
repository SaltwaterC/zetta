use super::*;

impl Zetta {
    pub(crate) fn start_tftp_server(
        &mut self,
        _: &StartTftpServer,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .tabs
            .get(self.active_tab)
            .is_some_and(|tab| !can_add_panes(tab.panes.len(), 1))
        {
            self.configuration_error = Some(format!(
                "Could not start TFTP server: this tab has reached the {MAX_PANES_PER_TAB}-pane limit"
            ));
            cx.notify();
            return;
        }
        let root = match self.active_server_root(cx) {
            Ok(root) => root,
            Err(error) => {
                self.configuration_error = Some(format!("Could not start TFTP server: {error:#}"));
                cx.notify();
                return;
            }
        };
        let port = self.launch_config.tftp_server_port;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let root = root.resolve()?;
                    // Read only: this action serves the active pane's working
                    // directory to the whole network on a single keystroke, so
                    // uploads stay behind `zetta tftp server --writable`, where
                    // the user has stated the directory and the intent.
                    start_server(&root, port, false)
                })
                .await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(server) => this.open_tftp_server_pane(server, window, cx),
                Err(error) => {
                    this.configuration_error =
                        Some(format!("Could not start TFTP server: {error:#}"));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn open_tftp_server_pane(
        &mut self,
        server: OpenTftpServer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_byte_stream_pane(
            ByteStreamPaneRequest {
                reader: server.reader,
                writer: server.writer,
                label: format!("TFTP: {}", server.address),
                title: format!("TFTP server {} — {}", server.address, server.root.display()),
                input: ByteStreamInputPolicy::CloseOnInterrupt,
            },
            window,
            cx,
        );
    }
}
