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
    let result =
        write_output_benchmark(&mut output, bytes, OutputBenchmarkType::RepeatedLines).unwrap();

    assert_eq!(result.bytes, bytes);
    assert_eq!(result.output_type, OutputBenchmarkType::RepeatedLines);
    assert_eq!(output.bytes, bytes);
    assert_eq!(output.newlines, bytes / OUTPUT_BENCHMARK_LINE_BYTES);
    assert_eq!(output.invalid_bytes, 0);
    assert_eq!(output.flushes, 1);
}

#[test]
fn unique_output_benchmark_writes_deterministic_non_repeating_lines() {
    let bytes = OUTPUT_BENCHMARK_LINE_BYTES * 4;
    let first = unique_output_benchmark_payload(bytes);
    let second = unique_output_benchmark_payload(bytes);
    let lines = first
        .split_inclusive(|byte| *byte == b'\n')
        .collect::<Vec<_>>();

    assert_eq!(first, second);
    assert_eq!(lines.len(), 4);
    assert!(
        lines
            .iter()
            .all(|line| line.len() == OUTPUT_BENCHMARK_LINE_BYTES)
    );
    assert!(lines.windows(2).all(|pair| pair[0] != pair[1]));
    assert!(lines.iter().all(|line| line.last() == Some(&b'\n')));
}

#[test]
fn unique_output_benchmark_reports_its_selected_type() {
    let mut output = Vec::new();
    let result = write_output_benchmark(&mut output, 1, OutputBenchmarkType::UniqueLines).unwrap();

    assert_eq!(result.output_type, OutputBenchmarkType::UniqueLines);
    assert_eq!(output, b"l");
}

#[test]
fn terminal_size_summary_includes_columns_and_rows() {
    assert_eq!(
        terminal_size_summary(Some(TerminalSize {
            columns: 120,
            rows: 40,
        })),
        "120 columns x 40 rows"
    );
    assert_eq!(terminal_size_summary(None), "terminal size unavailable");
}

#[test]
fn terminal_size_json_includes_columns_and_rows() {
    assert_eq!(
        terminal_size_json(Some(TerminalSize {
            columns: 120,
            rows: 40,
        })),
        r#"{"columns":120,"rows":40}"#
    );
    assert_eq!(terminal_size_json(None), r#"{"columns":null,"rows":null}"#);
}

#[test]
fn terminal_resize_sequence_uses_rows_before_columns() {
    assert_eq!(terminal_resize_sequence(120, 40), "\x1b[8;40;120t");
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
