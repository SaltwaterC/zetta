use super::*;

#[test]
fn reading_characters_returns_wide_glyphs_once_or_a_space_when_cut() {
    let lead = CharInfo {
        ch: 0x4e2d,
        attributes: 0x07 | legacy::LEADING_BYTE,
    };
    let trail = CharInfo {
        ch: 0x4e2d,
        attributes: 0x07 | legacy::TRAILING_BYTE,
    };
    let a = CharInfo {
        ch: u16::from(b'a'),
        attributes: 0x07,
    };
    assert_eq!(characters(&[a, lead, trail, a]), [97, 0x4e2d, 97]);
    assert_eq!(characters(&[a, lead]), [97, 0x20]);
    assert_eq!(characters(&[trail, a]), [0x20, 97]);
}

fn server() -> Server {
    Server::new(crate::sink::Sink::new(false, None, 500).unwrap(), false)
}
/// A key-down INPUT_RECORD whose AsciiChar is `byte`.
fn key(byte: u8) -> [u8; 20] {
    let mut record = [0; 20];
    record[0] = 1;
    record[4] = 1;
    record[8] = 1;
    record[14] = byte;
    record
}
fn chars(records: &[[u8; 20]]) -> Vec<u16> {
    records
        .iter()
        .map(|record| u16::from_le_bytes([record[14], record[15]]))
        .collect()
}

#[test]
fn filling_with_a_byte_the_code_page_cannot_convert_fails_as_e_unexpected() {
    let mut server = server();
    let object = server.object(Object::Output(0), Access::SHARED);
    // CONSOLE_FILLCONSOLEOUTPUT_MSG for FillConsoleOutputCharacterA, one cell.
    server.state.output_cp = 50220;
    let mut fill = |byte: u8| {
        server.reply.clear();
        server.reply.resize(16, 0);
        set_u32(&mut server.reply, 4, 1);
        set_u16(&mut server.reply, 8, u16::from(byte));
        set_u32(&mut server.reply, 12, 1);
        let descriptor = Descriptor {
            object,
            ..Default::default()
        };
        server.fill_output(descriptor).unwrap().0
    };
    // GH#4258: in ISO-2022-JP, 14 is a shift with no character of its own.
    assert_eq!(fill(14), E_UNEXPECTED);
    // A byte that is a character gets past the conversion to the fill itself.
    assert_ne!(fill(b'a'), E_UNEXPECTED);
}

#[test]
fn a_lead_byte_ending_one_ansi_input_write_joins_the_next_writes_first_key() {
    let mut server = server();
    // Shift-JIS "あ" is 0x82 0xa0.
    server.state.input_cp = 932;
    assert_eq!(
        chars(&server.widen_input_records(&[key(b'a'), key(0x82)]).unwrap()),
        [97]
    );
    assert_eq!(
        chars(&server.widen_input_records(&[key(0xa0), key(b'b')]).unwrap()),
        [0x3042, 98]
    );
    // A lead byte whose trail is not a key is dropped with it.
    let mut focus = [0; 20];
    focus[0] = 0x10;
    assert_eq!(
        chars(
            &server
                .widen_input_records(&[key(0x82), focus, key(b'c')])
                .unwrap()
        ),
        [99]
    );
}

#[test]
fn ansi_cells_take_lead_and_trail_from_the_code_page_one_row_at_a_time() {
    let mut server = server();
    server.state.output_cp = 932;
    let cell = |byte: u8, attributes: u16| CharInfo {
        ch: u16::from(byte),
        attributes,
    };
    // Two rows of three. The first row's lead byte has no trail in its row;
    // the 'c' claims to be a trail byte, which the caller cannot decide.
    let mut cells = [
        cell(b'a', 0x07),
        cell(b'b', 0x07),
        cell(0x82, 0x07),
        cell(0x82, 0x07),
        cell(0xa0, 0x07),
        cell(b'c', 0x07 | legacy::TRAILING_BYTE),
    ];
    server.cells_from_ansi(&mut cells, 3);
    let lead = 0x07 | legacy::LEADING_BYTE;
    let trail = 0x07 | legacy::TRAILING_BYTE;
    assert_eq!(
        cells.map(|cell| (cell.ch, cell.attributes)),
        [
            (97, 0x07),
            (98, 0x07),
            (0x20, 0x07),
            (0x3042, lead),
            (0x3042, trail),
            (99, 0x07),
        ]
    );
}

/// An 80x24 terminal of 'x' cells that records the ranges it is asked to
/// read and how much VT it is given.
#[derive(Clone, Default)]
struct Probe(std::sync::Arc<std::sync::Mutex<ProbeLog>>);
#[derive(Default)]
struct ProbeLog {
    reads: Vec<(std::ops::Range<i16>, std::ops::Range<i16>)>,
    written: usize,
}
impl Probe {
    fn log(&self) -> std::sync::MutexGuard<'_, ProbeLog> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
impl ConsoleOutput for Probe {
    fn write(&mut self, bytes: &[u8], _: crate::output::OutputMode) -> io::Result<()> {
        self.log().written += bytes.len();
        Ok(())
    }
    fn read_cells(
        &self,
        rows: std::ops::Range<i16>,
        columns: std::ops::Range<i16>,
    ) -> Option<Vec<Cell>> {
        let count = rows.len() * columns.len();
        self.log().reads.push((rows, columns));
        Some(vec![
            Cell {
                ch: 'x',
                ..Cell::BLANK
            };
            count
        ])
    }
}

#[test]
fn a_buffer_sized_at_the_limit_holds_only_the_rows_written_to_it() {
    let mut server = Server::new(Probe::default(), false);
    let object = server.create_screen_buffer(Access::SHARED).unwrap();
    let buffer = server.output_buffer(object).unwrap();
    // About 4.3 GB of cells, which Windows would refuse to commit.
    assert_eq!(
        server.set_off_screen_size(buffer, (32766, 32766)).unwrap(),
        0
    );
    server
        .write_off_screen(buffer, Deferred::Bytes(b"ab".to_vec()))
        .unwrap();
    let screen = &server.buffers[&buffer];
    assert_eq!(screen.cell(1, 0).unwrap().ch, u16::from(b'b'));
    assert_eq!(screen.cell(32765, 32765), Some(CharInfo::blank(0x07)));
    // The row table and the one row written: well under a megabyte.
    assert!(server.budget.used() < 1 << 20, "{}", server.budget.used());
}

#[test]
fn activating_a_buffer_larger_than_the_terminal_reads_and_draws_only_the_terminal() {
    let probe = Probe::default();
    let mut server = Server::new(probe.clone(), false);
    assert!(server.state.set_buffer_size((80, 24), (32766, 32766)));
    // A created buffer is the size of the main one.
    let object = server.create_screen_buffer(Access::SHARED).unwrap();
    server.set_active_screen_buffer(object).unwrap();
    server.flush_output().unwrap();
    let log = probe.log();
    assert_eq!(log.reads, [(0..24, 0..80)]);
    assert!(log.written < 64 * 1024, "{} bytes of VT", log.written);
}

#[test]
fn the_active_buffer_past_the_terminal_reads_blank_and_draws_nothing() {
    let probe = Probe::default();
    let mut server = Server::new(probe.clone(), false);
    assert!(server.state.set_buffer_size((80, 24), (32766, 32766)));
    let rect = Rect {
        left: 70,
        top: 20,
        right: 89,
        bottom: 29,
    };
    let cells = server.read_rect(MAIN, rect).unwrap().unwrap();
    assert_eq!(probe.log().reads, [(20..24, 70..80)]);
    assert_eq!(cells.len(), 200);
    let x = |cell: &CharInfo| cell.ch == u16::from(b'x');
    // Each of rows 20-23 has ten cells on screen, then ten past its edge.
    for (row, cells) in cells.chunks(20).enumerate() {
        assert!(cells[..10].iter().all(|cell| x(cell) == (row < 4)));
        assert!(!cells[10..].iter().any(x));
    }
    let mut out = String::new();
    let cell = CharInfo {
        ch: u16::from(b'y'),
        attributes: 0x07,
    };
    let below = Rect {
        left: 0,
        top: 24,
        right: 9,
        bottom: 24,
    };
    server
        .write_rect(MAIN, below, &[cell; 10], &mut out)
        .unwrap();
    assert_eq!(out, "");
    let edge = Rect {
        left: 78,
        top: 0,
        right: 81,
        bottom: 0,
    };
    server.write_rect(MAIN, edge, &[cell; 4], &mut out).unwrap();
    assert_eq!(out.matches('y').count(), 2);
}

#[test]
fn main_buffer_text_past_the_replay_limit_is_redrawn_from_its_cells() {
    let mut server = Server::new(Probe::default(), false);
    let object = server.create_screen_buffer(Access::SHARED).unwrap();
    server.set_active_screen_buffer(object).unwrap();
    server
        .write_off_screen(MAIN, Deferred::Bytes(vec![b'a'; REPLAY_LIMIT]))
        .unwrap();
    assert_eq!(server.deferred_main.len(), 1);
    server
        .write_off_screen(MAIN, Deferred::Bytes(b"b".to_vec()))
        .unwrap();
    assert!(server.deferred_main.is_empty());
    assert!(server.buffers[&MAIN].dirty);
}
