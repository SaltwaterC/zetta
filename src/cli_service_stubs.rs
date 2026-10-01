#[allow(
    unused_imports,
    reason = "needed only when at least one of the three CLI services below is disabled"
)]
use super::*;

#[cfg(not(feature = "serial-console"))]
impl Zetta {
    pub(crate) fn toggle_serial_console(
        &mut self,
        _: &ToggleSerialConsole,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_error_notice("Serial console support is disabled in this build", cx);
    }
}

#[cfg(not(feature = "http-server"))]
impl Zetta {
    pub(crate) fn start_http_server(
        &mut self,
        _: &StartHttpServer,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_error_notice("HTTP server support is disabled in this build", cx);
    }
}

#[cfg(not(feature = "tftp-server"))]
impl Zetta {
    pub(crate) fn start_tftp_server(
        &mut self,
        _: &StartTftpServer,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_error_notice("TFTP server support is disabled in this build", cx);
    }
}
