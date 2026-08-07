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
        self.configuration_error = Some("Serial console support is disabled in this build".into());
        cx.notify();
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
        self.configuration_error = Some("HTTP server support is disabled in this build".into());
        cx.notify();
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
        self.configuration_error = Some("TFTP server support is disabled in this build".into());
        cx.notify();
    }
}
