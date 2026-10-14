use super::*;

fn w(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}
fn history(commands: &[&str]) -> CommandHistory {
    let mut histories = Histories::default();
    histories.allocate(&w("cmd.exe"), 1);
    let history = histories.find(1).unwrap();
    for command in commands {
        history.add(&w(command), false);
    }
    history.clone()
}

#[test]
fn up_after_enter_recalls_the_last_command_then_walks_back() {
    let mut history = history(&["one", "two", "three"]);
    assert_eq!(history.retrieve(Direction::Previous), w("three"));
    assert_eq!(history.retrieve(Direction::Previous), w("two"));
    assert_eq!(history.retrieve(Direction::Next), w("three"));
    assert!(history.at_last());
}

#[test]
fn consecutive_duplicates_are_kept_once_and_no_dup_moves_them() {
    let mut history = history(&["a", "a", "b"]);
    assert_eq!(history.commands(), [w("a"), w("b")]);
    history.add(&w("a"), true);
    assert_eq!(history.commands(), [w("b"), w("a")]);
}

#[test]
fn the_oldest_command_falls_off_at_capacity() {
    let mut history = history(&[]);
    history.realloc(2);
    for command in ["1", "2", "3"] {
        history.add(&w(command), false);
    }
    assert_eq!(history.commands(), [w("2"), w("3")]);
}

#[test]
fn f8_prefix_search_walks_backwards_and_wraps() {
    let mut history = history(&["dir a", "echo", "dir b"]);
    let start = history.last_displayed;
    assert_eq!(history.find(&w("dir"), start, false, false), Some(2));
    assert_eq!(history.find(&w("dir"), 2, false, false), Some(0));
    assert_eq!(history.find(&w("nope"), 0, false, false), None);
}

#[test]
fn a_freed_list_is_reused_by_the_next_process_of_the_same_name() {
    let mut histories = Histories::default();
    histories.allocate(&w("cmd.exe"), 1);
    histories.find(1).unwrap().add(&w("kept"), false);
    histories.free(1);
    histories.allocate(&w("CMD.EXE"), 2);
    assert_eq!(histories.find(2).unwrap().commands(), [w("kept")]);
    // A different application past the list limit takes a free list, cleared.
    histories.buffers = 1;
    histories.free(2);
    histories.allocate(&w("python.exe"), 3);
    assert!(histories.find(3).unwrap().commands().is_empty());
}
