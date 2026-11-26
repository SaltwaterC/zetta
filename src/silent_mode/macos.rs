//! macOS Focus, read through the Intents framework's Focus status center. Only
//! a packaged app bundle can be granted access to it.

use super::{FocusStatusAccess, SystemSilentState, macos_focus_status};

fn is_packaged_gui_launch() -> bool {
    let Some(executable) = std::env::current_exe()
        .ok()
        .and_then(|executable| executable.canonicalize().ok())
    else {
        return false;
    };
    let Some(macos) = executable.parent() else {
        return false;
    };
    let Some(contents) = macos.parent() else {
        return false;
    };
    let Some(bundle) = contents.parent() else {
        return false;
    };
    macos.file_name().and_then(|name| name.to_str()) == Some("MacOS")
        && contents.file_name().and_then(|name| name.to_str()) == Some("Contents")
        && bundle.extension().and_then(|extension| extension.to_str()) == Some("app")
}

pub(super) fn request_focus_status_authorization() -> Option<FocusStatusAccess> {
    use block2::RcBlock;
    use objc2_intents::{INFocusStatusAuthorizationStatus, INFocusStatusCenter};

    if !is_packaged_gui_launch() {
        return None;
    }
    // SAFETY: a class getter with no arguments; objc2 leaves it unsafe only
    // because the binding is unaudited, and it returns a retained object.
    let center = unsafe { INFocusStatusCenter::defaultCenter() };
    // SAFETY: a no-argument getter on the live `center` returning a plain
    // enum value.
    let status = unsafe { center.authorizationStatus() };
    let access = focus_status_access(status);
    if status == INFocusStatusAuthorizationStatus::NotDetermined {
        let completion = RcBlock::new(|status: INFocusStatusAuthorizationStatus| {
            eprintln!("zetta: macOS Focus status authorization result: {status:?}");
        });
        // SAFETY: `center` is live, and the block has the declared
        // signature. Intents copies the block before returning and may call
        // it on any thread, which is sound because it captures nothing.
        unsafe {
            center.requestAuthorizationWithCompletionHandler(Some(&completion));
        }
    }
    Some(if access == FocusStatusAccess::Authorized {
        detect_macos_focus_observation().0
    } else {
        access
    })
}

fn focus_status_access(
    status: objc2_intents::INFocusStatusAuthorizationStatus,
) -> FocusStatusAccess {
    use objc2_intents::INFocusStatusAuthorizationStatus;

    if status == INFocusStatusAuthorizationStatus::NotDetermined {
        FocusStatusAccess::NotDetermined
    } else if status == INFocusStatusAuthorizationStatus::Denied {
        FocusStatusAccess::Denied
    } else if status == INFocusStatusAuthorizationStatus::Restricted {
        FocusStatusAccess::Restricted
    } else if status == INFocusStatusAuthorizationStatus::Authorized {
        FocusStatusAccess::Authorized
    } else {
        FocusStatusAccess::Unknown
    }
}

pub(super) fn detect_macos_focus_observation() -> (FocusStatusAccess, SystemSilentState) {
    use objc2_intents::INFocusStatusCenter;

    if !is_packaged_gui_launch() {
        return (FocusStatusAccess::Unknown, SystemSilentState::Unknown);
    }
    // SAFETY: a class getter with no arguments; objc2 leaves it unsafe only
    // because the binding is unaudited, and it returns a retained object.
    let center = unsafe { INFocusStatusCenter::defaultCenter() };
    // SAFETY: a no-argument getter on the live `center` returning a plain
    // enum value.
    let access = focus_status_access(unsafe { center.authorizationStatus() });
    if access != FocusStatusAccess::Authorized {
        return (access, SystemSilentState::Unknown);
    }
    // SAFETY: no-argument getters on the live `center` and on the retained
    // INFocusStatus it returns, which lives to the end of the statement.
    let Some(focused) = (unsafe { center.focusStatus().isFocused() }) else {
        return (
            FocusStatusAccess::AuthorizedButUnavailable,
            SystemSilentState::Unknown,
        );
    };
    (access, macos_focus_status(access, Some(focused.as_bool())))
}
