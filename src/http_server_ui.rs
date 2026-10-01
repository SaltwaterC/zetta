use super::*;

impl Zetta {
    pub(crate) fn start_http_server(
        &mut self,
        _: &StartHttpServer,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .tabs
            .get(self.active_tab)
            .is_some_and(|tab| !can_add_panes(tab.panes.len(), 1))
        {
            self.show_error_notice(format!(
                "Could not start HTTP server: this tab has reached the {MAX_PANES_PER_TAB}-pane limit"
            ), cx);
            return;
        }
        let root = match self.active_server_root(cx) {
            Ok(root) => root,
            Err(error) => {
                self.show_error_notice(format!("Could not start HTTP server: {error:#}"), cx);
                return;
            }
        };
        let port = self.launch_config.http_server_port;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let root = root.resolve()?;
                    start_http_server(&root, port)
                })
                .await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(server) => this.open_http_server_pane(server, window, cx),
                Err(error) => {
                    this.show_error_notice(format!("Could not start HTTP server: {error:#}"), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn open_http_server_pane(
        &mut self,
        server: OpenHttpServer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_byte_stream_pane(
            ByteStreamPaneRequest {
                reader: server.reader,
                writer: server.writer,
                label: format!("HTTP: {}", server.address),
                title: format!("HTTP server {} — {}", server.address, server.root.display()),
                input: ByteStreamInputPolicy::CloseOnInterrupt,
            },
            window,
            cx,
        );
    }
}
