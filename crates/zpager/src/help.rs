//! Command help text for the diff pager.

pub(crate) fn command_help() -> String {
    let options = format_help_table([
        (
            "-c, --columns COLUMNS",
            "Minimum terminal width per side before stacking (default: 80; ZPAGER_COLUMNS)",
        ),
        (
            "-w, --width COLUMNS",
            "Cap each pane's width (default: use available width; ZPAGER_WIDTH)",
        ),
        (
            "-n, --no-color",
            "Disable syntax colors in the terminal view",
        ),
        ("-h, --help", "Print this help"),
    ]);
    format!(
        "View diff text from standard input in a scrollable terminal view. It uses side-by-side panes when the terminal is at least twice the configured minimum width plus the center gutter, and stacks old/new lines otherwise. Side-by-side panes use all available width unless capped with -w/--width or ZPAGER_WIDTH. Long lines wrap at word boundaries in each panel.\n\nUsage: zpager [OPTIONS]\n\nUse `GIT_PAGER=zpager git --paginate diff` to select zpager for one Git\ncommand, or pipe diff output directly with `git diff | zpager`. When standard\noutput is redirected, zpager passes the input through unchanged.\n\nOptions:\n{options}\n\nViewer keys:\n  Up/Down or j/k   Scroll one line\n  Page Up/Down     Scroll one page\n  n/p              Jump to the next/previous file\n  Home/End or g/G  Jump to the start/end\n  q or Escape      Quit\n"
    )
}

pub(crate) use zetta_cli_help::format_help_table;
