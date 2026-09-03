use super::*;

#[test]
fn retention_modes_parse_and_disk_is_canonical() {
    assert_eq!(Retention::parse("none").unwrap(), Retention::None);
    #[cfg(feature = "scrollback-buffer")]
    assert_eq!(
        Retention::parse("memory").unwrap(),
        Retention::Memory {
            bytes: DEFAULT_RING_BYTES
        }
    );
    #[cfg(not(feature = "scrollback-buffer"))]
    assert!(
        Retention::parse("memory")
            .unwrap_err()
            .to_string()
            .contains("scrollback-buffer")
    );
    assert!(Retention::parse("everything").is_err());

    #[cfg(feature = "session-persistence")]
    assert_eq!(Retention::parse("disk").unwrap(), Retention::Disk);
    #[cfg(not(feature = "session-persistence"))]
    assert!(
        Retention::parse("disk")
            .unwrap_err()
            .to_string()
            .contains("session-persistence")
    );
    let error = Retention::parse("persist").unwrap_err().to_string();
    assert!(error.contains("use \"disk\""), "{error}");
}

#[test]
fn retention_memory_budget_is_bounded_before_a_daemon_starts() {
    #[cfg(feature = "scrollback-buffer")]
    {
        assert!(Retention::Memory { bytes: 4_095 }.validate().is_err());
        assert!(Retention::Memory { bytes: 4_096 }.validate().is_ok());
        assert!(
            Retention::Memory {
                bytes: MAX_RING_BYTES + 1
            }
            .validate()
            .is_err()
        );
    }
}

/// The text a retained screen shows, one line per row, for comparing what a
/// reattach would draw without asserting on escape sequences.
#[cfg(feature = "scrollback-buffer")]
fn shown(retained: &Retained) -> Vec<String> {
    use alacritty_terminal::{
        event::VoidListener,
        grid::Dimensions as _,
        index::{Column, Line},
        term::{Config, Term, cell::LineLength as _},
        vte::ansi::{Processor, StdSyncHandler},
    };

    // Read back the way a window does: a fresh terminal, fed the snapshot.
    let mut term = Term::new(Config::default(), &GridSize::new(40, 5), VoidListener);
    Processor::<StdSyncHandler>::new().advance(&mut term, &retained.snapshot());
    let grid = term.grid();
    (0..grid.screen_lines())
        .map(|line| {
            let row = &grid[Line(line as i32)];
            (0..row.line_length().0)
                .map(|column| row[Column(column)].c)
                .collect()
        })
        .collect()
}

#[cfg(feature = "scrollback-buffer")]
fn decoded_history(retained: &Retained) -> Vec<String> {
    use alacritty_terminal::{
        event::VoidListener,
        grid::Dimensions as _,
        index::{Column, Line},
        term::{Config, Term, cell::LineLength as _},
        vte::ansi::{Processor, StdSyncHandler},
    };

    let mut term = Term::new(Config::default(), &GridSize::new(40, 5), VoidListener);
    Processor::<StdSyncHandler>::new().advance(&mut term, &retained.snapshot());
    let grid = term.grid();
    (grid.topmost_line().0..=grid.bottommost_line().0)
        .map(|line| {
            let row = &grid[Line(line)];
            (0..row.line_length().0)
                .map(|column| row[Column(column)].c)
                .collect()
        })
        .collect()
}

#[test]
fn none_keeps_nothing_at_all() {
    // The mode for a memory-constrained host: the pane is still read, so its
    // child never blocks on a full buffer, but nothing is held.
    let mut retained = Retention::None.new_retained(40, 5);
    retained.seed(b"the screen at detach".to_vec());
    retained.push(b"and everything after it");

    assert!(retained.snapshot().is_empty());
}

#[cfg(feature = "scrollback-buffer")]
#[test]
fn memory_keeps_the_screen_the_output_landed_on() {
    let mut retained = Retention::Memory { bytes: 64 }.new_retained(40, 5);
    retained.seed(b"first line\r\n".to_vec());
    retained.push(b"second line");

    assert_eq!(shown(&retained)[..2], ["first line", "second line"]);
    // And a snapshot does not consume it: two windows asking in turn are shown
    // the same screen.
    assert_eq!(shown(&retained)[..2], ["first line", "second line"]);
}

/// The reason a grid replaced a buffer of bytes.
///
/// A full-screen program's output repaints parts of a screen, so what it printed
/// most recently describes a screen rather than being one. Keeping the recent
/// bytes and dropping the rest left a reattach with fragments over a blank
/// terminal — the reattached `htop` that started this — while a grid holds the
/// screen those fragments were painting on, however long the pane runs.
#[cfg(feature = "scrollback-buffer")]
#[test]
fn a_repainting_program_is_kept_as_the_screen_it_paints() {
    let mut retained = Retention::Memory { bytes: 1024 }.new_retained(40, 5);
    // A screen, entered the way a full-screen program enters one.
    retained.seed(b"\x1b[?1049h\x1b[HHEADER\r\nbody\r\nFOOTER".to_vec());
    // Then thousands of repaints of one field, addressed as such a program
    // addresses them, and far more bytes than any buffer of recent output would
    // have kept.
    for tick in 0..5_000 {
        retained.push(format!("\x1b[2;1Hbody {tick}").as_bytes());
    }

    let shown = shown(&retained);
    assert_eq!(shown[0], "HEADER", "the screen is still there: {shown:?}");
    assert_eq!(shown[1], "body 4999", "with the latest repaint on it");
    assert_eq!(shown[2], "FOOTER", "including what nothing repainted");
}

/// Scrollback is bounded, and it is the oldest lines that go — which is what a
/// terminal does when its own scrollback fills.
#[cfg(feature = "scrollback-buffer")]
#[test]
fn scrollback_is_bounded_by_the_memory_budget() {
    // Two lines of history: 512 bytes at the assumed cost of a line.
    let mut retained = Retention::Memory { bytes: 512 }.new_retained(40, 5);
    for line in 0..50 {
        retained.push(format!("line {line}\r\n").as_bytes());
    }

    let snapshot = String::from_utf8(retained.snapshot()).unwrap();
    assert!(snapshot.contains("line 49"), "{snapshot:?}");
    assert!(
        !snapshot.contains("line 20"),
        "the oldest lines are the ones dropped: {snapshot:?}"
    );
}

#[cfg(feature = "scrollback-buffer")]
#[test]
fn a_retained_snapshot_round_trip_keeps_lines_above_the_viewport() {
    let mut retained = Retention::Memory { bytes: 2_048 }.new_retained(40, 5);
    for line in 0..30 {
        retained.push(format!("line {line}\r\n").as_bytes());
    }

    let history = decoded_history(&retained);
    assert!(history.len() > 5, "the decoded terminal has no scrollback");
    assert!(
        history.iter().any(|line| line == "line 29"),
        "the latest output is missing: {history:?}"
    );
    assert!(
        history.iter().any(|line| line == "line 22"),
        "output above one viewport was not restored: {history:?}"
    );
}

/// Seeding starts from a blank screen: what one window handed over must not be
/// mixed with what another one did.
#[cfg(feature = "scrollback-buffer")]
#[test]
fn seeding_replaces_the_screen_rather_than_adding_to_it() {
    let mut retained = Retention::Memory { bytes: 1024 }.new_retained(40, 5);
    retained.seed(b"an earlier session".to_vec());
    retained.seed(b"the one handed over now".to_vec());

    let shown = shown(&retained);
    assert_eq!(shown[0], "the one handed over now");
    assert!(
        !shown.iter().any(|line| line.contains("earlier")),
        "{shown:?}"
    );
}

/// Clearing is for a pane whose terminal has gone to a client that reads it
/// directly: nothing the daemon holds describes it any more.
#[cfg(feature = "scrollback-buffer")]
#[test]
fn clearing_leaves_no_screen_to_serve() {
    let mut retained = Retention::Memory { bytes: 1024 }.new_retained(40, 5);
    retained.seed(b"a screen".to_vec());
    retained.clear();

    assert!(
        !String::from_utf8_lossy(&retained.snapshot()).contains("a screen"),
        "{:?}",
        String::from_utf8_lossy(&retained.snapshot())
    );
}
