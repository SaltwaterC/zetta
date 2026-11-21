//! The remote-session picker in a build without `zmux`: the same names, doing
//! nothing, so its callers need no `cfg` of their own.

use super::*;

#[derive(Default)]
pub(crate) struct RemoteSessionPicker;

impl Zetta {
    pub(crate) fn open_remote_session(
        &mut self,
        _: &OpenRemoteSession,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_notice(
            "Remote sessions require a Zetta build with zmux support.",
            cx,
        );
    }

    pub(crate) fn remote_session_key_down(
        &mut self,
        _: &KeyDownEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> bool {
        false
    }

    pub(crate) fn remote_session_key_down_capture(
        &mut self,
        _: &KeyDownEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
    }
}
