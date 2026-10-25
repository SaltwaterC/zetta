//! Paired old and new diff rows parsed from Git's unified patch stream.

use anyhow::Result;
use crossterm::queue;
use crossterm::style::{Attribute, Color, SetAttribute, SetForegroundColor};
use std::io::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineKind {
    File,
    Hunk,
    Added,
    Removed,
    Metadata,
    Context,
}

#[derive(Clone, Debug)]
pub(crate) struct DiffLine {
    pub(crate) text: String,
    pub(crate) kind: LineKind,
}

#[derive(Clone, Debug)]
pub(crate) struct SideBySideLine {
    pub(crate) number: usize,
    pub(crate) text: String,
    pub(crate) kind: LineKind,
}

#[derive(Clone, Debug)]
pub(crate) enum DiffRow {
    Full(DiffLine),
    Sides {
        old: Option<SideBySideLine>,
        new: Option<SideBySideLine>,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct FileStart {
    pub(crate) row: usize,
    pub(crate) label: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DiffDocument {
    pub(crate) rows: Vec<DiffRow>,
    pub(crate) files: Vec<FileStart>,
}

impl DiffDocument {
    pub(crate) fn parse(input: &[u8]) -> Self {
        let text = String::from_utf8_lossy(input);
        let lines = text
            .split_terminator('\n')
            .map(|line| strip_ansi_sequences(line.strip_suffix('\r').unwrap_or(line)))
            .collect::<Vec<_>>();
        let git_format = lines.iter().any(|line| line.starts_with("diff "));
        let mut document = Self::default();
        let mut index = 0;

        while index < lines.len() {
            let line = &lines[index];
            if line.starts_with("diff ") || (!git_format && line.starts_with("--- ")) {
                document.push_file_header(line);
                index += 1;
                continue;
            }
            if line.starts_with("@@") {
                document.push_full(line, LineKind::Hunk);
                index += 1;
                if let Some((old_start, old_count, new_start, new_count)) = parse_hunk_ranges(line)
                {
                    document.push_hunk_rows(
                        &lines, &mut index, old_start, old_count, new_start, new_count,
                    );
                } else {
                    document.push_unparsed_hunk(&lines, &mut index, git_format);
                }
                continue;
            }

            document.push_full(line, classify_full_line(line));
            index += 1;
        }

        document
    }

    fn push_file_header(&mut self, line: &str) {
        self.files.push(FileStart {
            row: self.rows.len(),
            label: line.to_owned(),
        });
        self.push_full(line, LineKind::File);
    }

    fn push_full(&mut self, line: &str, kind: LineKind) {
        self.rows.push(DiffRow::Full(DiffLine {
            text: line.to_owned(),
            kind,
        }));
    }

    fn push_hunk_rows(
        &mut self,
        lines: &[String],
        index: &mut usize,
        mut old_number: usize,
        mut old_remaining: usize,
        mut new_number: usize,
        mut new_remaining: usize,
    ) {
        while *index < lines.len() && (old_remaining > 0 || new_remaining > 0) {
            let line = &lines[*index];
            if line.starts_with("diff ") || line.starts_with("@@") {
                return;
            }

            if line.starts_with(' ') && old_remaining > 0 && new_remaining > 0 {
                let text = line[1..].to_owned();
                self.rows.push(DiffRow::Sides {
                    old: Some(SideBySideLine {
                        number: old_number,
                        text: text.clone(),
                        kind: LineKind::Context,
                    }),
                    new: Some(SideBySideLine {
                        number: new_number,
                        text,
                        kind: LineKind::Context,
                    }),
                });
                old_number += 1;
                new_number += 1;
                old_remaining -= 1;
                new_remaining -= 1;
                *index += 1;
                continue;
            }

            if line.starts_with('-') || line.starts_with('+') {
                let previous_index = *index;
                self.push_change_rows(
                    lines,
                    index,
                    &mut old_number,
                    &mut old_remaining,
                    &mut new_number,
                    &mut new_remaining,
                );
                if *index == previous_index {
                    return;
                }
                continue;
            }

            if line.starts_with("\\ No newline") {
                self.push_full(line, LineKind::Metadata);
            } else {
                self.push_full(line, classify_full_line(line));
            }
            *index += 1;
        }
    }

    fn push_change_rows(
        &mut self,
        lines: &[String],
        index: &mut usize,
        old_number: &mut usize,
        old_remaining: &mut usize,
        new_number: &mut usize,
        new_remaining: &mut usize,
    ) {
        let mut removed = Vec::new();
        let mut added = Vec::new();
        while *index < lines.len() {
            let line = &lines[*index];
            if line.starts_with('-') && *old_remaining > 0 {
                removed.push(SideBySideLine {
                    number: *old_number,
                    text: line[1..].to_owned(),
                    kind: LineKind::Removed,
                });
                *old_number += 1;
                *old_remaining -= 1;
                *index += 1;
            } else if line.starts_with('+') && *new_remaining > 0 {
                added.push(SideBySideLine {
                    number: *new_number,
                    text: line[1..].to_owned(),
                    kind: LineKind::Added,
                });
                *new_number += 1;
                *new_remaining -= 1;
                *index += 1;
            } else {
                break;
            }
        }

        for row in 0..removed.len().max(added.len()) {
            self.rows.push(DiffRow::Sides {
                old: removed.get(row).cloned(),
                new: added.get(row).cloned(),
            });
        }
    }

    fn push_unparsed_hunk(&mut self, lines: &[String], index: &mut usize, git_format: bool) {
        while *index < lines.len() {
            let line = &lines[*index];
            if line.starts_with("diff ") || line.starts_with("@@") {
                return;
            }
            if !git_format && line.starts_with("--- ") {
                return;
            }
            self.push_full(line, classify_full_line(line));
            *index += 1;
        }
    }
}

fn parse_hunk_ranges(line: &str) -> Option<(usize, usize, usize, usize)> {
    let ranges = line.strip_prefix("@@")?.split_once("@@")?.0;
    let mut ranges = ranges.split_whitespace();
    let (old_start, old_count) = parse_range(ranges.next()?, '-')?;
    let (new_start, new_count) = parse_range(ranges.next()?, '+')?;
    Some((old_start, old_count, new_start, new_count))
}

fn parse_range(range: &str, prefix: char) -> Option<(usize, usize)> {
    let range = range.strip_prefix(prefix)?;
    let (start, count) = range.split_once(',').unwrap_or((range, "1"));
    Some((start.parse().ok()?, count.parse().ok()?))
}

pub(crate) fn set_line_style(output: &mut impl Write, kind: LineKind) -> Result<()> {
    match kind {
        LineKind::File => queue!(
            output,
            SetForegroundColor(Color::Cyan),
            SetAttribute(Attribute::Bold)
        )?,
        LineKind::Hunk => queue!(
            output,
            SetForegroundColor(Color::Magenta),
            SetAttribute(Attribute::Bold)
        )?,
        LineKind::Added => queue!(output, SetForegroundColor(Color::Green))?,
        LineKind::Removed => queue!(output, SetForegroundColor(Color::Red))?,
        LineKind::Metadata => queue!(output, SetForegroundColor(Color::DarkGrey))?,
        LineKind::Context => {}
    }
    Ok(())
}

fn classify_full_line(line: &str) -> LineKind {
    if line.starts_with("@@") {
        LineKind::Hunk
    } else if line.starts_with("+") && !line.starts_with("+++") {
        LineKind::Added
    } else if line.starts_with('-') && !line.starts_with("---") {
        LineKind::Removed
    } else if line.starts_with("--- ")
        || line.starts_with("+++ ")
        || line.starts_with("index ")
        || line.starts_with("new file mode ")
        || line.starts_with("deleted file mode ")
        || line.starts_with("similarity index ")
        || line.starts_with("rename from ")
        || line.starts_with("rename to ")
        || line.starts_with("old mode ")
        || line.starts_with("new mode ")
        || line.starts_with("\\ No newline")
    {
        LineKind::Metadata
    } else {
        LineKind::Context
    }
}

fn strip_ansi_sequences(text: &str) -> String {
    #[derive(Clone, Copy)]
    enum State {
        Text,
        Escape,
        Csi,
        Osc,
        OscEscape,
    }

    let mut state = State::Text;
    let mut plain = String::with_capacity(text.len());
    for character in text.chars() {
        state = match state {
            State::Text if character == '\u{1b}' => State::Escape,
            State::Text => {
                plain.push(character);
                State::Text
            }
            State::Escape if character == '[' => State::Csi,
            State::Escape if character == ']' => State::Osc,
            State::Escape => State::Text,
            State::Csi if ('@'..='~').contains(&character) => State::Text,
            State::Csi => State::Csi,
            State::Osc if character == '\u{7}' => State::Text,
            State::Osc if character == '\u{1b}' => State::OscEscape,
            State::Osc => State::Osc,
            State::OscEscape if character == '\\' => State::Text,
            State::OscEscape if character == '\u{1b}' => State::OscEscape,
            State::OscEscape => State::Osc,
        };
    }
    plain
}
