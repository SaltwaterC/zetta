use super::*;

#[test]
fn web_and_mail_links_open_directly_in_any_case() {
    for url in [
        "http://example.com/",
        "https://example.com/path?q=1",
        "HTTPS://EXAMPLE.COM/",
        "mailto:someone@example.com",
    ] {
        assert_eq!(url_open_policy(url), UrlOpenPolicy::Open, "{url}");
    }
}

#[test]
fn every_other_scheme_needs_confirmation() {
    for url in [
        "javascript:alert(1)",
        "vscode://file/etc/passwd",
        "ms-msdt:/id",
        "smb://host/share",
        "ssh:host",
        "zed://settings",
        // A file URL that was not turned into a path (wrong case, say) must
        // not reach the OS opener unasked either.
        "FILE:///etc/passwd",
        "file:/etc/passwd",
    ] {
        assert_eq!(url_open_policy(url), UrlOpenPolicy::Confirm, "{url}");
    }
}

#[test]
fn a_malformed_scheme_needs_confirmation() {
    for url in [
        "",
        "example.com",
        " https://example.com/",
        "\u{202E}https://example.com/",
        "1http://example.com/",
        "ht tp://example.com/",
        "://example.com/",
    ] {
        assert_eq!(url_open_policy(url), UrlOpenPolicy::Confirm, "{url:?}");
    }
}

#[test]
fn bidi_and_format_characters_are_shown_escaped() {
    assert_eq!(
        display_safe_link_text("https://example.com/\u{202E}gpj.exe"),
        "https://example.com/\\u{202E}gpj.exe"
    );
    assert_eq!(
        display_safe_link_text("a\u{2066}b\u{2069}c\u{200B}d\u{FEFF}e\u{061C}f"),
        "a\\u{2066}b\\u{2069}c\\u{200B}d\\u{FEFF}e\\u{061C}f"
    );
    assert_eq!(
        display_safe_link_text("tag\u{E0041}\u{1B}[31m"),
        "tag\\u{E0041}\\u{001B}[31m"
    );
}

#[test]
fn ordinary_text_is_shown_as_is() {
    for text in [
        "https://example.com/Ῥόδος/",
        "/home/user/notes.txt",
        "mailto:user@example.com",
        "https://例え.jp/",
    ] {
        assert_eq!(display_safe_link_text(text), text);
    }
}
