//! The fuzzy matcher Zetta's lists filter with: the command palette, the theme
//! picker, and the settings and remote-session dropdowns.
//!
//! The palette and the dropdowns each carried a copy of this scorer. The tab
//! icon picker deliberately does not use it: icon names are short and its grid
//! shows every match at once, so scattered subsequence matches would flood it. A query matches when its characters
//! appear in the candidate in order; consecutive characters and characters at
//! the start of a word score higher, and so does a shorter candidate.

/// How well `query` matches `candidate`, both already lowercased; `None` when
/// it does not. An empty query matches everything equally.
pub(crate) fn score(candidate: &str, query: &str) -> Option<i32> {
    walk(candidate, query, |_| {})
}

/// [`score`] for text in any case, for callers that do not keep a lowercased
/// copy of their candidates.
pub(crate) fn score_ignoring_case(candidate: &str, query: &str) -> Option<i32> {
    score(&candidate.to_lowercase(), &query.to_lowercase())
}

/// The byte ranges of `candidate` that `query` matched, for highlighting them.
///
/// Matching is done on the lowercased text, and a range is only meaningful in
/// the original when lowercasing kept every byte where it was — true for the
/// ASCII names the lists hold. Where it did not, nothing is highlighted rather
/// than the wrong characters.
pub(crate) fn matched_ranges(candidate: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let lower = candidate.to_lowercase();
    let query = query.trim().to_lowercase();
    if query.is_empty() || lower.len() != candidate.len() {
        return Vec::new();
    }
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    let matched = walk(&lower, &query, |range| match ranges.last_mut() {
        Some(last) if last.end == range.start => last.end = range.end,
        _ => ranges.push(range),
    });
    if matched.is_none()
        || ranges
            .iter()
            .any(|range| !candidate.is_char_boundary(range.end))
    {
        return Vec::new();
    }
    ranges
}

/// The scorer itself, reporting each matched character's byte range to
/// `on_match` as it goes.
fn walk(
    candidate: &str,
    query: &str,
    mut on_match: impl FnMut(std::ops::Range<usize>),
) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let mut characters = query.chars();
    let mut wanted = characters.next()?;
    let mut score = 0;
    let mut previous_match: Option<usize> = None;
    for (index, character) in candidate.char_indices() {
        if character != wanted {
            continue;
        }
        score += 10;
        if previous_match.is_some_and(|previous| previous + character.len_utf8() == index) {
            score += 8;
        }
        if index == 0
            || candidate[..index]
                .chars()
                .next_back()
                .is_some_and(|previous| matches!(previous, ' ' | ':' | '_' | '-'))
        {
            score += 5;
        }
        previous_match = Some(index);
        on_match(index..index + character.len_utf8());
        match characters.next() {
            Some(next) => wanted = next,
            None => return Some(score - candidate.len() as i32 / 8),
        }
    }
    None
}

#[cfg(test)]
#[path = "tests/fuzzy_match.rs"]
mod tests;
