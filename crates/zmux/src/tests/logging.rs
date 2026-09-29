use super::*;

use std::time::Duration;

#[test]
fn timestamps_are_utc_iso_8601_with_milliseconds() {
    assert_eq!(utc_timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
    // 2026-09-29T19:37:53.123Z, a date past a leap day and a century rule.
    let moment = UNIX_EPOCH + Duration::from_millis(1_790_710_673_123);
    assert_eq!(utc_timestamp(moment), "2026-09-29T19:37:53.123Z");
    // 2000-02-29: the leap day of a year divisible by 400.
    let leap = UNIX_EPOCH + Duration::from_secs(951_782_400);
    assert_eq!(utc_timestamp(leap), "2000-02-29T00:00:00.000Z");
}

#[test]
fn the_level_defaults_to_warnings_and_can_be_raised_or_silenced() {
    assert_eq!(level_from(None), LevelFilter::Warn);
    assert_eq!(level_from(Some("")), LevelFilter::Warn);
    assert_eq!(level_from(Some("nonsense")), LevelFilter::Warn);
    assert_eq!(level_from(Some(" Debug ")), LevelFilter::Debug);
    assert_eq!(level_from(Some("off")), LevelFilter::Off);
}

#[test]
fn a_full_log_is_rotated_so_two_files_bound_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("daemon.log");
    let logger = FileLogger::open(path.clone(), LevelFilter::Warn).unwrap();
    let message = "x".repeat(4096);
    for _ in 0..(MAX_LOG_BYTES as usize / 4096 + 8) {
        logger.log(
            &Record::builder()
                .level(log::Level::Warn)
                .target("zmux::test")
                .args(format_args!("{message}"))
                .build(),
        );
    }
    let current = std::fs::metadata(&path).unwrap().len();
    let rotated = std::fs::metadata(path.with_extension("log.1"))
        .unwrap()
        .len();
    assert!(current <= MAX_LOG_BYTES, "{current}");
    assert!(rotated <= MAX_LOG_BYTES, "{rotated}");
    assert!(current > 0);
}

#[test]
fn lines_below_the_level_are_not_written() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("daemon.log");
    let logger = FileLogger::open(path.clone(), LevelFilter::Warn).unwrap();
    for level in [log::Level::Debug, log::Level::Warn] {
        logger.log(
            &Record::builder()
                .level(level)
                .target("zmux::test")
                .args(format_args!("at {level}"))
                .build(),
        );
    }
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("WARN  zmux::test: at WARN"), "{written}");
    assert!(!written.contains("at DEBUG"), "{written}");
}
