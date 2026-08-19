use super::*;
use std::path::Path;

#[test]
fn the_configuration_directory_never_falls_back_to_the_working_directory() {
    // This directory holds the process control token and the session catalogs.
    // A relative path would place them under whatever directory Zetta was
    // started in, which may be writable by another user.
    let fallback = Path::new("/tmp/zetta-1000");

    assert_eq!(
        unix_config_dir(Some("/home/u/.config".into()), None, fallback),
        Path::new("/home/u/.config/zetta")
    );
    assert_eq!(
        unix_config_dir(None, Some("/home/u".into()), fallback),
        Path::new("/home/u/.config/zetta")
    );
    // An empty variable is as good as an unset one.
    assert_eq!(
        unix_config_dir(Some("".into()), Some("".into()), fallback),
        Path::new("/tmp/zetta-1000/zetta")
    );
    assert_eq!(
        unix_config_dir(None, None, fallback),
        Path::new("/tmp/zetta-1000/zetta")
    );

    assert_eq!(
        windows_config_dir(Some(r"C:\Users\u\AppData\Roaming".into()), fallback),
        Path::new(r"C:\Users\u\AppData\Roaming").join("Zetta")
    );
    assert_eq!(
        windows_config_dir(None, fallback),
        Path::new("/tmp/zetta-1000/Zetta")
    );

    for resolved in [
        unix_config_dir(None, None, &private_fallback_dir()),
        windows_config_dir(None, &private_fallback_dir()),
    ] {
        assert!(
            resolved.is_absolute(),
            "{resolved:?} must not be relative to the working directory"
        );
    }
}

#[test]
fn the_session_directory_sits_inside_the_configuration_directory() {
    assert_eq!(
        session_catalog_dir(),
        platform_config_dir().join("sessions")
    );
}
