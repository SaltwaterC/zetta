//! Cross-platform desktop notification CLI and its optional Zetta tab routing.
//!
//! Native notification delivery, worker lifetime, icon caching and sound
//! playback live here so `zntfy` can run without the Zetta GUI. The only
//! connections back to Zetta are the process-control client for tab state and,
//! on macOS, the main app executable for its Notification Center identity.
//!
//! All of that is the default `backends` feature. Without it the crate is only
//! [`request`]: what a notification request is and how a command line spells
//! one, which Zetta parses itself on every platform but delivers through this
//! crate only on macOS.

pub mod request;

#[cfg(notify_cleanup_enabled)]
pub use request::NotifyCleanupCommand;
pub use request::{
    NotificationOptions, NotificationRequest, NotificationTarget, NotificationTimeout,
    notification_reexec_args, notification_target_from_environment, parse_notification_target,
    parse_notification_timeout, requests_help,
};

#[cfg(all(feature = "backends", notify_cleanup_enabled))]
mod cleanup;
#[cfg(feature = "backends")]
mod notify;
#[cfg(feature = "backends")]
mod process_control;
#[cfg(feature = "backends")]
mod sounds;

#[cfg(all(feature = "backends", notify_cleanup_enabled))]
pub use cleanup::{notify_cleanup_help, parse_notify_cleanup_args, run_notify_cleanup};
#[cfg(feature = "backends")]
pub use notify::*;
