//! Standalone HTTP server engine shared with Zetta server panes.
pub mod cli;
mod server;

pub use server::{
    HttpServerOptions, OpenHttpServer, http_server_limits_help, normalize_allowed_host,
    start_http_server,
};

fn format_help_table<'a>(rows: impl AsRef<[(&'a str, &'a str)]>) -> String {
    let rows = rows
        .as_ref()
        .iter()
        .map(|&(label, description)| (label.trim_end(), description))
        .collect::<Vec<_>>();
    let label_width = rows
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    rows.into_iter()
        .map(|(label, description)| {
            let mut lines = description.split('\n');
            let first_line = lines.next().unwrap_or("").trim_end();
            let mut formatted = String::new();
            formatted.push_str("  ");
            formatted.push_str(label);
            formatted.push_str(&" ".repeat(label_width - label.chars().count()));
            if !first_line.is_empty() {
                formatted.push_str("  ");
                formatted.push_str(first_line);
            }
            for line in lines {
                formatted.push('\n');
                let line = line.trim_end();
                if !line.is_empty() {
                    formatted.push_str("  ");
                    formatted.push_str(&" ".repeat(label_width));
                    formatted.push_str("  ");
                    formatted.push_str(line);
                }
            }
            formatted
        })
        .collect::<Vec<_>>()
        .join("\n")
}
