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
        if !unicode {
            return self.reply_title_ansi(handle, d, size, &title, capacity);
        }
        let wide: Vec<_> = title.encode_utf16().collect();
        let count = wide.len().min((capacity / 2).saturating_sub(1));
        let mut bytes: Vec<u8> = wide[..count]
            .iter()
            .flat_map(|ch| ch.to_le_bytes())
            .collect();
        set_u32(&mut self.reply, 0, wide.len() as u32);
        let written = bytes.len();
        bytes.extend([0, 0]);
        if bytes.len() <= capacity {
            native::transfer(handle, WRITE_OUTPUT, d.id, size as u32, &mut bytes)?;
        }
        Ok((0, written))
    }

    /// GetConsoleTitleAImplHelper's legacy rule: a buffer too small for the
    /// whole converted title gets only a NUL and a needed length of 0; one
    /// exactly its length gets every byte and no terminator.
    fn reply_title_ansi(
        &mut self,
        handle: &Handle,
        d: Descriptor,
        size: usize,
        title: &str,
        capacity: usize,
    ) -> io::Result<(i32, usize)> {
        let wide: Vec<u16> = title.encode_utf16().collect();
        let converted = codepage::from_utf16(self.state.input_cp, &wide);
        let (mut bytes, needed, written) = if capacity == 0 {
            (Vec::new(), 0, 0)
        } else if converted.is_empty() || capacity < converted.len() {
            (vec![0], 0, usize::from(!converted.is_empty()))
        } else {
            let mut bytes = converted[..converted.len().min(capacity - 1)].to_vec();
            bytes.push(if capacity == converted.len() {
                converted[converted.len() - 1]
            } else {
                0
            });
            let written = capacity.min(converted.len() + 1);
            (bytes, converted.len(), written)
        };
        set_u32(&mut self.reply, 0, needed as u32);
        if !bytes.is_empty() {
            native::transfer(handle, WRITE_OUTPUT, d.id, size as u32, &mut bytes)?;
        }
        Ok((0, written))
    }
}
