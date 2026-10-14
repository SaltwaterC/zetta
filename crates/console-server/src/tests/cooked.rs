use super::*;
use crate::history::Histories;

fn w(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}
const SIZE: Size = Size {
    width: 80,
    height: 24,
};
struct Editor {
    read: CookedRead,
    history: CommandHistory,
    clear: bool,
}
impl Editor {
    fn new(commands: &[&str]) -> Self {
        let mut histories = Histories::default();
        histories.allocate(&w("cmd.exe"), 1);
        let history = histories.find(1).unwrap();
        for command in commands {
            history.add(&w(command), false);
        }
        Self {
            read: CookedRead::new(&[], 0, true, SIZE, Point::default()),
            history: history.clone(),
            clear: false,
        }
    }
    fn type_text(&mut self, text: &str) {
        for unit in w(text) {
            self.read.handle_char(
                unit,
                0,
                &mut Context {
                    history: Some(&mut self.history),
                    processed: true,
                    clear_cmd_aliases: &mut self.clear,
                },
            );
        }
    }
    fn key(&mut self, vkey: u16, modifiers: u32) {
        self.read.handle_vkey(
            vkey,
            modifiers,
            &mut Context {
                history: Some(&mut self.history),
                processed: true,
                clear_cmd_aliases: &mut self.clear,
            },
        );
    }
    fn line(&mut self) -> String {
        let buffer = self.read.buffer.clone();
        String::from_utf16(&buffer).unwrap()
    }
}

#[test]
fn editing_keys_move_and_delete_by_grapheme_in_insert_mode() {
    let mut editor = Editor::new(&[]);
    editor.type_text("echo Xbc");
    editor.key(vk::LEFT, 0);
    editor.key(vk::LEFT, 0);
    editor.type_text("\x08a");
    editor.key(vk::END, 0);
    editor.type_text("d");
    assert_eq!(editor.line(), "echo abcd");
    // Ctrl+Left goes back a word, Ctrl+End deletes to the end of the line,
    // and Ctrl+Home deletes from the start to the cursor.
    editor.key(vk::LEFT, CTRL);
    editor.key(vk::END, CTRL);
    assert_eq!(editor.line(), "echo ");
    editor.key(vk::LEFT, 0);
    editor.key(vk::HOME, CTRL);
    assert_eq!(editor.line(), " ");
    editor.key(vk::ESCAPE, 0);
    assert_eq!(editor.line(), "");
}

#[test]
fn overwrite_mode_replaces_and_ctrl_backspace_erases_a_word() {
    let mut editor = Editor::new(&[]);
    editor.type_text("one two");
    editor.key(vk::INSERT, 0);
    editor.key(vk::HOME, 0);
    editor.type_text("ON");
    assert_eq!(editor.line(), "ONe two");
    editor.key(vk::END, 0);
    editor.type_text("\x7f");
    assert_eq!(editor.line(), "ONe ");
}

#[test]
fn history_keys_recall_commands_and_f8_completes_a_prefix() {
    let mut editor = Editor::new(&["dir a", "echo one", "dir b"]);
    editor.key(vk::UP, 0);
    assert_eq!(editor.line(), "dir b");
    editor.key(vk::UP, 0);
    assert_eq!(editor.line(), "echo one");
    editor.key(vk::DOWN, 0);
    assert_eq!(editor.line(), "dir b");
    editor.key(vk::ESCAPE, 0);
    editor.type_text("dir");
    editor.key(vk::F8, 0);
    assert!(editor.line().starts_with("dir "));
    assert_eq!(
        editor.read.cursor, 3,
        "F8 keeps the cursor after the prefix"
    );
}

#[test]
fn f1_and_f3_copy_from_the_previous_command() {
    let mut editor = Editor::new(&["echo hello"]);
    editor.key(vk::F1, 0);
    editor.key(vk::RIGHT, 0);
    assert_eq!(editor.line(), "ec");
    editor.key(vk::F3, 0);
    assert_eq!(editor.line(), "echo hello");
}

#[test]
fn enter_finishes_and_the_wakeup_mask_returns_the_key_unprinted() {
    let mut editor = Editor::new(&[]);
    editor.type_text("abc\r");
    assert_eq!(editor.read.state, State::DoneWithCarriageReturn);
    let mut read = CookedRead::new(&[], 1 << 9, true, SIZE, Point::default());
    let mut clear = false;
    for unit in w("cd C:\\Win\t") {
        read.handle_char(
            unit,
            0,
            &mut Context {
                history: None,
                processed: true,
                clear_cmd_aliases: &mut clear,
            },
        );
    }
    assert_eq!(read.state, State::DoneWithWakeupMask);
    read.redisplay(SIZE, Point { x: 2, y: 0 }, true, "", None);
    read.complete_wakeup();
    assert_eq!(
        String::from_utf16(&read.take_buffer()).unwrap(),
        "cd C:\\Win\t"
    );
}

#[test]
fn redisplay_writes_only_what_changed_and_parks_the_cursor() {
    let mut editor = Editor::new(&[]);
    editor.type_text("abc");
    let first = editor
        .read
        .redisplay(SIZE, Point { x: 5, y: 2 }, true, "", None)
        .unwrap();
    assert_eq!(first, "\x1b[3;6Habc\x1b[3;9H");
    editor.type_text("d");
    let second = editor
        .read
        .redisplay(SIZE, Point { x: 99, y: 99 }, true, "", None)
        .unwrap();
    assert_eq!(
        second, "\x1b[3;9Hd\x1b[3;10H",
        "the origin is measured once"
    );
    editor.type_text("\x08\x08");
    let third = editor
        .read
        .redisplay(SIZE, Point::default(), true, "", None)
        .unwrap();
    assert_eq!(
        third, "\x1b[3;8H  \x1b[3;8H",
        "shrinking erases the tail with spaces"
    );
    assert!(
        editor
            .read
            .redisplay(SIZE, Point::default(), true, "", None)
            .is_none()
    );
    editor.type_text("x");
    assert!(
        editor
            .read
            .redisplay(SIZE, Point::default(), false, "", None)
            .is_none()
    );
}

#[test]
fn layout_shows_controls_as_carets_and_wraps_wide_glyphs_whole() {
    let mut out = String::new();
    assert_eq!(layout_line(&mut out, &w("a\x01\tb"), 0, 0, 80), (4, 9));
    assert_eq!(out, "a^A     b");
    let mut out = String::new();
    // "中" is two columns; it does not fit after "ab" in three.
    assert_eq!(layout_line(&mut out, &w("ab中c"), 0, 0, 3), (2, 3));
    assert_eq!(out, "ab");
}

#[test]
fn initial_data_is_placed_so_that_it_ends_at_the_cursor() {
    let initial = w("dir C:\\Windows");
    let mut read = CookedRead::new(&initial, 1 << 9, true, SIZE, Point { x: 40, y: 5 });
    assert_eq!(read.origin, Some(Point { x: 26, y: 5 }));
    assert_eq!(read.take_buffer(), initial);
}

#[test]
fn the_command_list_popup_selects_and_runs_a_command() {
    let mut editor = Editor::new(&["one", "two"]);
    editor.key(vk::F7, 0);
    assert!(editor.read.has_popup());
    let drawn = editor
        .read
        .redisplay(
            SIZE,
            Point::default(),
            true,
            "\x1b[0;95;47m",
            Some(&editor.history),
        )
        .unwrap();
    assert!(
        drawn.contains("0: one") && drawn.contains("▸1: two"),
        "{drawn:?}"
    );
    editor.read.handle_popup(
        0,
        vk::UP,
        0,
        &mut Context {
            history: Some(&mut editor.history),
            processed: true,
            clear_cmd_aliases: &mut editor.clear,
        },
    );
    editor.read.handle_popup(
        u16::from(b'\r'),
        0,
        0,
        &mut Context {
            history: Some(&mut editor.history),
            processed: true,
            clear_cmd_aliases: &mut editor.clear,
        },
    );
    assert!(!editor.read.has_popup());
    assert_eq!(editor.line(), "one");
    assert_eq!(editor.read.state, State::DoneWithCarriageReturn);
}
