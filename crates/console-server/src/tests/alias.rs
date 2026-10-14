use super::*;

fn w(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

#[test]
fn macros_expand_arguments_and_split_lines() {
    let mut aliases = Aliases::default();
    assert!(aliases.add(
        &w("ZZ"),
        &w("echo macro-$1 [$*]$Tdir $g out"),
        &w("CMD.EXE")
    ));
    let (text, lines) = aliases.expand(&w("zz works  fine"), &w("cmd.exe")).unwrap();
    assert_eq!(
        String::from_utf16(&text).unwrap(),
        "echo macro-works [works  fine]\r\ndir > out\r\n"
    );
    assert_eq!(lines, 2);
    assert!(aliases.expand(&w("other"), &w("cmd.exe")).is_none());
    assert!(aliases.expand(&w("zz"), &w("python.exe")).is_none());
}

#[test]
fn listing_removal_and_empty_sources() {
    let mut aliases = Aliases::default();
    assert!(!aliases.add(&[], &w("x"), &w("cmd.exe")));
    aliases.add(&w("ll"), &w("dir"), &w("cmd.exe"));
    assert_eq!(
        aliases.get(&w("LL"), &w("cmd.exe")),
        Some(w("dir").as_slice())
    );
    assert_eq!(
        String::from_utf16(&aliases.list(&w("cmd.exe"))).unwrap(),
        "ll=dir\0"
    );
    assert_eq!(String::from_utf16(&aliases.exes()).unwrap(), "cmd.exe\0");
    aliases.add(&w("ll"), &[], &w("cmd.exe"));
    assert_eq!(aliases.get(&w("ll"), &w("cmd.exe")), None);
}
