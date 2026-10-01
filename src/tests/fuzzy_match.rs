use super::*;

#[test]
fn characters_in_order_match_and_anything_else_does_not() {
    assert!(score("terminal: paste trimmed", "paste trim").is_some());
    assert!(score("terminal: paste", "missing").is_none());
    assert_eq!(score("anything", ""), Some(0));
}

#[test]
fn word_starts_and_runs_score_higher() {
    let run = score("new tab", "new").unwrap();
    let scattered = score("nerve twin", "new").unwrap();
    assert!(run > scattered);
}

#[test]
fn case_is_ignored_on_request() {
    assert!(score_ignoring_case("Solarized Light", "SL").is_some());
}

#[test]
fn matched_ranges_merge_consecutive_characters() {
    assert_eq!(matched_ranges("New Tab", "new t"), vec![0..5]);
    assert_eq!(matched_ranges("New Tab", "nt"), vec![0..1, 4..5]);
    assert!(matched_ranges("New Tab", "zzz").is_empty());
    assert!(matched_ranges("New Tab", "").is_empty());
}
