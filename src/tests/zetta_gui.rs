use super::*;

#[test]
fn cli_executable_is_next_to_the_gui_launcher() {
    assert_eq!(
        cli_executable(Path::new(r"C:\Program Files\Zetta\zetta-gui.exe")),
        Path::new(r"C:\Program Files\Zetta\zetta.exe")
    );
}
