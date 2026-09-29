use super::*;

#[test]
fn the_level_defaults_to_warnings_and_can_be_raised_or_silenced() {
    assert_eq!(level_from(None), LevelFilter::Warn);
    assert_eq!(level_from(Some("nonsense")), LevelFilter::Warn);
    assert_eq!(level_from(Some(" Info")), LevelFilter::Info);
    assert_eq!(level_from(Some("OFF")), LevelFilter::Off);
}

#[test]
fn a_line_names_its_level_and_target() {
    let line = format_line(
        &Record::builder()
            .level(log::Level::Warn)
            .target("zetta::collaboration")
            .args(format_args!("shared stream failed"))
            .build(),
    );
    assert_eq!(line, "WARN  zetta::collaboration: shared stream failed\n");
}
