use super::*;
#[test]
fn empty_clipboard_yields_no_output() {
    assert_eq!(
        clipboard_text(Err(arboard::Error::ContentNotAvailable)).unwrap(),
        None
    );
}
