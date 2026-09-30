// Cargo does not run build-script unit tests. Run this sidecar through its
// production module:
// rustc --edition=2024 --test build_support/conpty_tools.rs -o /tmp/zetta-conpty-tests
// /tmp/zetta-conpty-tests
use super::*;

const HASH: &str = "2C57CB7DA7E19FA06C86487C8D9B5C307D65695429FA15A854BF5F3CDDCA9E1D";

#[test]
fn parses_native_and_cross_host_checksum_output() {
    for output in [
        format!(
            "SHA256 hash of conpty.nupkg.zip:\r\n{HASH}\r\nCertUtil: completed successfully.\r\n"
        ),
        format!("{HASH}  /tmp/a path/conpty.nupkg.zip\n"),
        format!("{}  conpty.nupkg.zip\n", HASH.to_ascii_lowercase()),
    ] {
        assert!(parse_sha256(&output).unwrap().eq_ignore_ascii_case(HASH));
    }
}

#[test]
fn rejects_missing_truncated_and_non_hex_checksums() {
    for output in ["", "CertUtil: failed", &HASH[..63], &"z".repeat(64)] {
        assert_eq!(parse_sha256(output), None);
    }
}

#[cfg(not(windows))]
#[test]
fn verifies_a_file_with_spaces_and_rejects_a_mismatch() {
    let directory = std::env::temp_dir().join(format!("zetta conpty test {}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("archive ' test.zip");
    std::fs::write(&path, b"abc").unwrap();
    verify_sha256(
        &path,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
    let mismatch = std::panic::catch_unwind(|| verify_sha256(&path, HASH));
    std::fs::remove_dir_all(directory).unwrap();
    assert!(mismatch.is_err());
}
