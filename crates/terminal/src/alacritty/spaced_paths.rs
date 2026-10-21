//! Readings of a hovered path that run across single spaces.
//!
//! The path regexes end a path at a space, so `/tmp/Q3 report.xlsx` matches as
//! `/tmp/Q3` and `report.xlsx`. Which reading was meant cannot be told from
//! the text, and hovering must never touch the filesystem, so this module only
//! lists the readings: the hovered match joined with the neighbouring tokens
//! that are each separated from it by exactly one space. The view chooses the
//! longest one that exists, off the foreground thread (`standalone.rs` in
//! `terminal_view`).

use std::{cmp::Reverse, ops::Range};

/// The most space-separated tokens one reading joins, the hovered one
/// included. Bounds what the view stats for one hover.
pub(super) const MAX_TOKENS: usize = 16;

/// Where readings of a hovered path match may start and end, as byte offsets
/// into its line, nearest first. The first of each is the match's own edge.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Boundaries {
    pub(super) starts: Vec<usize>,
    pub(super) ends: Vec<usize>,
}

impl Boundaries {
    /// Every reading other than the match itself, longest first.
    pub(super) fn readings(&self) -> Vec<Range<usize>> {
        let mut readings = Vec::new();
        for (left, &start) in self.starts.iter().enumerate() {
            for (right, &end) in self.ends.iter().enumerate() {
                if left + right > 0 && left + right < MAX_TOKENS {
                    readings.push(start..end);
                }
            }
        }
        readings.sort_by_key(|reading| Reverse(reading.len()));
        readings
    }
}

/// The boundaries of `link`'s readings in `line`, or `None` when no
/// single-space neighbour can extend it.
pub(super) fn boundaries(line: &str, link: Range<usize>) -> Option<Boundaries> {
    if line[link.clone()].contains(' ') {
        return None;
    }
    let starts = starts(line, link.start);
    let ends = ends(line, link.end);
    (starts.len() + ends.len() > 2).then_some(Boundaries { starts, ends })
}

fn starts(line: &str, mut start: usize) -> Vec<usize> {
    let mut starts = vec![start];
    while starts.len() < MAX_TOKENS {
        let Some(before) = line[..start].strip_suffix(' ') else {
            break;
        };
        if before.is_empty() || before.ends_with(' ') {
            break;
        }
        let token_start = before.rfind(' ').map_or(0, |space| space + 1);
        let token = &before[token_start..];
        let trimmed = trim_opening(token);
        if trimmed.is_empty() {
            break;
        }
        let edge = before.len() - trimmed.len();
        starts.push(edge);
        // An opening quote or bracket begins the path; nothing before it joins.
        if edge != token_start {
            break;
        }
        start = token_start;
    }
    starts
}

fn ends(line: &str, mut end: usize) -> Vec<usize> {
    let mut ends = vec![end];
    while ends.len() < MAX_TOKENS {
        let Some(after) = line[end..].strip_prefix(' ') else {
            break;
        };
        if after.is_empty() || after.starts_with(' ') {
            break;
        }
        let token_start = end + 1;
        let token = &after[..after.find(' ').unwrap_or(after.len())];
        let trimmed = trim_closing(token);
        if trimmed.is_empty() {
            break;
        }
        ends.push(token_start + trimmed.len());
        if trimmed.len() != token.len() {
            break;
        }
        end = token_start + token.len();
    }
    ends
}

/// What the default path regex refuses to start a path with. A `(` stays when
/// the token closes it, as in `(1).pdf`.
fn trim_opening(token: &str) -> &str {
    let closes = token.contains(')');
    token.trim_start_matches(|c| {
        matches!(c, '{' | '[' | '<' | '"' | '\'' | '`') || (c == '(' && !closes)
    })
}

/// What the default path regex refuses to end a path with. A `)` stays when
/// the token opens it, as in `report (1)`.
fn trim_closing(token: &str) -> &str {
    let opens = token.contains('(');
    token.trim_end_matches(|c| {
        matches!(
            c,
            '}' | ']' | '>' | '"' | '\'' | '`' | '.' | ',' | ';' | ':'
        ) || (c == ')' && !opens)
    })
}

#[cfg(test)]
#[path = "../tests/alacritty/spaced_paths.rs"]
mod tests;
