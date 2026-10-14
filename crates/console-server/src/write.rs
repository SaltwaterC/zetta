//! Text written to the console, on its way to the embedder's VT parser.
//! With VT processing on, UTF-8 bytes pass through untouched: that path is
//! the throughput this crate exists for. Other code pages are converted, and
//! with VT processing off the legacy console semantics are expressed as VT.
use super::*;
use crate::legacy;
use crate::output::OutputMode;
use crate::state::mode;

impl Server {
    pub(super) fn output_mode(&self) -> OutputMode {
        OutputMode::from_console_flags(self.state.output_mode)
    }
    /// DoWriteConsole: VT parsing needs both VT processing and processed
    /// output; without either, the legacy console semantics apply.
    fn vt_path(&self) -> bool {
        self.state.vt_output() && self.state.output_mode & mode::PROCESSED_OUTPUT != 0
    }
    /// WriteFile and WriteConsoleA: bytes in the output code page.
    pub(super) fn write_bytes(&mut self, bytes: &[u8], process: usize) -> io::Result<()> {
        if self.vt_path()
            && self.decoder.is_idle()
            && (self.state.output_cp == codepage::UTF8 || self.ascii_output && bytes.is_ascii())
        {
            return self.consume(bytes, process);
        }
        let wide = self.decoder.decode(self.state.output_cp, bytes);
        self.write_wide(&wide, process)
    }
    /// WriteConsoleW, and converted bytes.
    pub(super) fn write_wide(&mut self, wide: &[u16], process: usize) -> io::Result<()> {
        if self.vt_path() {
            return self.consume(String::from_utf16_lossy(wide).as_bytes(), process);
        }
        let mut column = self.legacy_column(wide)?;
        let text = if self.state.output_mode & mode::PROCESSED_OUTPUT != 0 {
            let code_page = self.state.output_cp;
            legacy::processed(
                wide,
                self.state.output_mode & mode::DISABLE_NEWLINE_AUTO_RETURN == 0,
                &mut column,
                |byte| codepage::glyph(code_page, byte),
            )
        } else {
            legacy::unprocessed(wide, &mut column)
        };
        self.consume(text.as_bytes(), process)?;
        self.legacy_column =
            (column != legacy::Column::UNKNOWN).then_some((self.sink_bytes, column));
        Ok(())
    }
    /// The cursor column a legacy write needs: with wrap at EOL on, for every
    /// write, and otherwise only for a TAB. Reading it from the terminal costs
    /// a flush, so the column a legacy write ends at is reused while nothing
    /// else has been written and the width is unchanged; a legacy write never
    /// leaves a wrap pending, so only a fresh read can find one.
    fn legacy_column(&mut self, wide: &[u16]) -> io::Result<legacy::Column> {
        let wrap = self.state.output_mode & mode::WRAP_AT_EOL_OUTPUT != 0;
        if !wrap && !wide.contains(&u16::from(b'\t')) {
            return Ok(legacy::Column::UNKNOWN);
        }
        let width = self.sink.size().0.max(1) as usize;
        if let Some((bytes, column)) = self.legacy_column
            && bytes == self.sink_bytes
            && column.width == width
        {
            return Ok(legacy::Column { wrap, ..column });
        }
        let screen = self.screen()?;
        let mut at = screen.cursor.0.max(0) as usize;
        // WriteCharsLegacy: the console APIs have no deferred wrap, so one a
        // VT write left pending happens before any legacy text.
        if wrap && screen.wrap_pending {
            self.sink_write(b"\r\n")?;
            at = 0;
        }
        Ok(legacy::Column {
            at,
            width: screen.columns.max(1) as usize,
            wrap,
        })
    }
    /// VT the server generates for a console API call, ordered with output.
    pub(super) fn write_vt(&mut self, vt: &str) -> io::Result<()> {
        self.sink_write(vt.as_bytes())
    }
    /// The output code page changed: the ASCII shortcut is per code page.
    pub(super) fn output_code_page_changed(&mut self) {
        self.ascii_output = codepage::ascii_compatible(self.state.output_cp);
    }
}
