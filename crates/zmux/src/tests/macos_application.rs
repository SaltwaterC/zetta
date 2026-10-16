use super::*;

#[test]
fn only_a_bundled_installation_uses_the_application_host() {
    let directory = tempfile::tempdir().unwrap();
    let macos = directory.path().join("Zetta.app/Contents/MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    let helper = macos.join("zmux");
    assert_eq!(executable(&helper), None);
    std::fs::write(macos.join("zetta"), b"app").unwrap();
    assert_eq!(executable(&helper), None);
    std::fs::write(macos.parent().unwrap().join("Info.plist"), b"plist").unwrap();
    assert_eq!(executable(&helper), Some(macos.join("zetta")));
    assert!(is_application(&macos.join("zetta")));
    assert!(!is_application(&helper));
    assert_eq!(executable(&directory.path().join("zmux")), None);
    let unpacked = directory.path().join("unbundled/Contents/MacOS");
    std::fs::create_dir_all(&unpacked).unwrap();
    std::fs::write(unpacked.join("zetta"), b"app").unwrap();
    std::fs::write(unpacked.parent().unwrap().join("Info.plist"), b"plist").unwrap();
    assert_eq!(executable(&unpacked.join("zmux")), None);
}
