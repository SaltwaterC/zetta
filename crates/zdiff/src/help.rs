//! Command help text for the diff pager.

pub(crate) fn command_help() -> String {
    let options = format_help_table([
        (
            "-c, --columns COLUMNS",
            "Minimum terminal width per side before stacking (default: 80; ZDIFF_COLUMNS)",
        ),
        (
            "-w, --width COLUMNS",
            "Cap each pane's width (default: use available width; ZDIFF_WIDTH)",
        ),
        (
            "-n, --no-color",
            "Disable syntax colors in the terminal view",
        ),
        ("-h, --help", "Print this help"),
    ]);
    format!(
        "View diff text from standard input in a scrollable terminal view. It uses side-by-side panes when the terminal is at least twice the configured minimum width plus the center gutter, and stacks old/new lines otherwise. Side-by-side panes use all available width unless capped with -w/--width or ZDIFF_WIDTH. Long lines wrap at word boundaries in each panel.\n\nUsage: zdiff [OPTIONS]\n\nUse `GIT_PAGER=zdiff git --paginate diff` to select zdiff for one Git\ncommand, or pipe diff output directly with `git diff | zdiff`. When standard\noutput is redirected, zdiff passes the input through unchanged.\n\nOptions:\n{options}\n\nViewer keys:\n  Up/Down or j/k   Scroll one line\n  Page Up/Down     Scroll one page\n  n/p              Jump to the next/previous file\n  Home/End or g/G  Jump to the start/end\n  q or Escape      Quit\n"
    )
}

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
