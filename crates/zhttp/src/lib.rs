//! Standalone HTTP server engine shared with Zetta server panes.
pub mod cli;
mod log_reader;
mod server;

pub use server::{
    HttpServerOptions, OpenHttpServer, http_server_limits_help, normalize_allowed_host,
    start_http_server,
};

use zetta_cli_help::format_help_table;
