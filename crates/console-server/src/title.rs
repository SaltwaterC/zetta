//! Win32 ANSI/Unicode title API translation and ordered title notification.
//! ANSI titles are in the input code page, as OpenConsole converts them.
use super::*;
impl Server {
    pub(super) fn set_title(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
    ) -> io::Result<(i32, usize)> {
        let offset = size + 8;
        let length = d.input_size as usize - offset;
        let unicode = self.reply[0] != 0;
        if length > 65536 || unicode && !length.is_multiple_of(2) {
            return Ok((INVALID_PARAMETER, 0));
        }
        let mut bytes = vec![0; length];
        native::transfer(handle, READ_INPUT, d.id, offset as u32, &mut bytes)?;
        let title = if unicode {
            let wide: Vec<_> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect();
            String::from_utf16_lossy(&wide)
        } else {
            String::from_utf16_lossy(&codepage::to_utf16(self.state.input_cp, &bytes))
        };
        let title = title.trim_end_matches('\0');
        self.flush_output()?;
        self.sink.set_title(title)?;
        title.clone_into(&mut self.state.title);
        Ok((0, 0))
    }
    pub(super) fn get_title(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
    ) -> io::Result<(i32, usize)> {
        let unicode = self.reply[4] != 0;
        let title = if self.reply[5] != 0 {
            self.state.original_title.clone()
        } else {
            self.flush_output()?;
            self.sink
                .title()
                .unwrap_or_else(|| self.state.title.clone())
        };
        let capacity = d.output_size as usize - size;
        let (mut bytes, units) = if unicode {
            let wide: Vec<_> = title.encode_utf16().collect();
            let count = wide.len().min((capacity / 2).saturating_sub(1));
            (
                wide[..count]
                    .iter()
                    .flat_map(|ch| ch.to_le_bytes())
                    .collect::<Vec<_>>(),
                wide.len(),
            )
        } else {
            // Whole characters only, so a DBCS pair is never split.
            let mut bytes = Vec::new();
            let mut needed = 0;
            let mut fits = true;
            for ch in title.chars() {
                let encoded =
                    codepage::from_utf16(self.state.input_cp, ch.encode_utf16(&mut [0; 2]));
                needed += encoded.len();
                // Stop at the first character that does not fit before the NUL.
                fits &= needed < capacity;
                if fits {
                    bytes.extend(encoded);
                }
            }
            (bytes, needed)
        };
        set_u32(&mut self.reply, 0, units as u32);
        let written = bytes.len();
        bytes.extend(std::iter::repeat_n(0, if unicode { 2 } else { 1 }));
        if bytes.len() <= capacity {
            native::transfer(handle, WRITE_OUTPUT, d.id, size as u32, &mut bytes)?;
        }
        Ok((0, written))
    }
}
