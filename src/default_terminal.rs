//! Making Zetta the user's default terminal, and asking whether it is.
//!
//! Each platform's mechanism lives in its own module; this one only routes to
//! it and owns the menu action. Windows' lives in `windows_integration.rs`.

use anyhow::Result;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
use linux::{linux_default_terminal_is_set, set_linux_default_terminal};
#[cfg(target_os = "macos")]
pub(crate) use macos::script_paths_from_urls;
#[cfg(target_os = "macos")]
use macos::{macos_script_handlers_are_zetta, register_macos_script_handlers};

/// Selects the platform's user-level default-terminal integration. Installing
/// Zetta only registers the application; this function is called by the
/// explicit action exposed in the application menu.
pub(crate) fn set_default_terminal() -> Result<String> {
    #[cfg(windows)]
    {
        crate::windows_integration::set_default_terminal()
    }
    #[cfg(target_os = "macos")]
    {
        register_macos_script_handlers()?;
        Ok("Zetta is now the default terminal for shell-script files".to_owned())
    }
    #[cfg(target_os = "linux")]
    {
        set_linux_default_terminal()
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    anyhow::bail!("default-terminal integration is not supported on this platform")
}

/// Returns whether the platform reports Zetta as the currently selected
/// default terminal. This is intentionally a read-only query so menus can
/// show their checked state without changing the user's configuration.
pub(crate) fn is_default_terminal() -> bool {
    #[cfg(windows)]
    {
        crate::windows_integration::is_default_terminal()
    }
    #[cfg(target_os = "macos")]
    {
        macos_script_handlers_are_zetta()
    }
    #[cfg(target_os = "linux")]
    {
        linux_default_terminal_is_set()
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

impl crate::Zetta {
    /// Whether a macOS script-open confirmation is waiting for an answer; the
    /// prompt itself lives in `script_open_ui.rs`, which only macOS compiles.
    pub(crate) fn script_open_is_open(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.script_open.is_open()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    pub(crate) fn set_default_terminal(
        &mut self,
        _: &crate::SetDefaultTerminal,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        match set_default_terminal() {
            Ok(message) => {
                #[cfg(target_os = "macos")]
                {
                    let effective = self.effective_config();
                    crate::startup::update_native_macos_menus(
                        cx,
                        &self.profiles,
                        &effective.hidden_profiles,
                        effective.default_profile,
                    );
                    let profiles = self.profiles.clone();
                    let hidden_profiles = effective.hidden_profiles.clone();
                    let default_profile = effective.default_profile;
                    let executor = cx.background_executor().clone();
                    cx.spawn(async move |this, cx| {
                        // Launch Services can finish committing the handler after the
                        // setter returns, especially when it has shown confirmation
                        // dialogs. Refresh until it reports the new handler, but bound
                        // the polling so a cancelled prompt does not leave a task alive.
                        for _ in 0..20 {
                            executor.timer(std::time::Duration::from_millis(50)).await;
                            if macos_script_handlers_are_zetta() {
                                this.update(cx, |_this, cx| {
                                    crate::startup::update_native_macos_menus(
                                        cx,
                                        &profiles,
                                        &hidden_profiles,
                                        default_profile,
                                    );
                                })
                                .ok();
                                break;
                            }
                        }
                    })
                    .detach();
                }
                self.show_notice(message, cx);
            }
            Err(error) => self.show_error_notice(
                format!("Could not set Zetta as the default terminal: {error:#}"),
                cx,
            ),
        }
    }
}
