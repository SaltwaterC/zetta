#![cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux;

pub use linux::{DirectoryPathPrompt, current_platform, current_platform_with_path_prompt};

/// Uses a compositor-issued activation token for the next window activation.
///
/// The application calls this immediately before activating a newly opened
/// window. It is intentionally kept in the local platform fork so GPUI's
/// upstream `PlatformWindow` trait does not need to change.
pub fn set_next_activation_token(activation_token: &str) {
    linux::set_next_activation_token(activation_token);
}
