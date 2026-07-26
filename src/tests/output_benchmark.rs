use super::*;

#[derive(Default)]
struct InspectingWriter {
    bytes: usize,
    newlines: usize,
    invalid_bytes: usize,
    flushes: usize,
}

impl io::Write for InspectingWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes += buffer.len();
        for (offset, byte) in buffer.iter().enumerate() {
            let index = self.bytes - buffer.len() + offset;
            let column = index % OUTPUT_BENCHMARK_LINE_BYTES;
            if column == OUTPUT_BENCHMARK_LINE_BYTES - 1 {
                self.newlines += usize::from(*byte == b'\n');
                self.invalid_bytes += usize::from(*byte != b'\n');
            } else {
                let expected = OUTPUT_BENCHMARK_TEXT[column % OUTPUT_BENCHMARK_TEXT.len()];
                self.invalid_bytes += usize::from(*byte != expected);
            }
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

#[test]
fn output_benchmark_writes_requested_amount_of_deterministic_text() {
    let mut output = InspectingWriter::default();
    let bytes = MIB_BYTES;
    let result = write_output_benchmark(&mut output, bytes).unwrap();

    assert_eq!(result.bytes, bytes);
    assert_eq!(output.bytes, bytes);
    assert_eq!(output.newlines, bytes / OUTPUT_BENCHMARK_LINE_BYTES);
    assert_eq!(output.invalid_bytes, 0);
    assert_eq!(output.flushes, 1);
}

#[test]
fn default_output_benchmark_uses_complete_lines() {
    let bytes = DEFAULT_OUTPUT_BENCHMARK_MIB * MIB_BYTES;
    assert_eq!(bytes % OUTPUT_BENCHMARK_LINE_BYTES, 0);
    assert_eq!(
        output_benchmark_payload(OUTPUT_BENCHMARK_LINE_BYTES - 1, 1)
            .last()
            .copied(),
        Some(b'\n')
    );
}
