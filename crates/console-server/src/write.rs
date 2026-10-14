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
        let text = if self.state.output_mode & mode::PROCESSED_OUTPUT != 0 {
            let code_page = self.state.output_cp;
            legacy::processed(
                wide,
                self.state.output_mode & mode::DISABLE_NEWLINE_AUTO_RETURN == 0,
                |byte| codepage::glyph(code_page, byte),
            )
        } else {
            legacy::unprocessed(wide)
        };
        self.consume(text.as_bytes(), process)
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
