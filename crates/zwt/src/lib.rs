mod process_control;
mod worktree_cli;
mod worktree_copy;

pub(crate) fn format_help_table<'a>(rows: impl AsRef<[(&'a str, &'a str)]>) -> String {
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

pub use process_control::{WorktreeNameRequest, request_process_worktree_name};
pub use worktree_cli::{
    WorktreeCommand, WorktreeInvocation, parse_worktree_args, parse_worktree_args_for, run,
    run_for, worktree_done_help, worktree_done_help_for, worktree_help, worktree_help_for,
    worktree_new_help, worktree_new_help_for, worktree_rerere_help, worktree_rerere_help_for,
    worktree_status_help, worktree_status_help_for,
};

/// Runs the standalone `zwt` command with the supplied arguments.
pub fn run_standalone<I>(arguments: I) -> anyhow::Result<()>
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    let command = parse_worktree_args(arguments.into_iter().collect::<Vec<_>>().as_slice())?;
    run(&command)
}

/// Entry point used by the standalone and root-produced `zwt` binaries.
pub fn standalone_main() {
    if let Err(error) = run_standalone(std::env::args_os().skip(1)) {
        eprintln!("zwt failed: {error:#}");
        std::process::exit(1);
    }
}
