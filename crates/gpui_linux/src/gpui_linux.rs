#![cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux;

pub use linux::{DirectoryPathPrompt, current_platform, current_platform_with_path_prompt};
