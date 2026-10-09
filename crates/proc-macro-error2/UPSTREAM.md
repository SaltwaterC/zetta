# Zetta proc-macro-error2 fork

Source baseline: crates.io `proc-macro-error2@2.0.1`, checksum
`11ec05c52be0a07b08061f7dd003e7d7092e0472bc731b4af7bb1ef876109802`.
Upstream: <https://github.com/GnomedDev/proc-macro-error-2>.

`age`'s `i18n-embed-fl` dependency uses this crate. Rust 1.99 reports its
private `extern crate proc_macro` being publicly re-exported through
`__export` as a future incompatibility (Rust issue 127909). The retained
source fix makes that declaration public; macro behavior is unchanged.

The root and standalone `zmux` manifests patch crates.io to this fork.
Patch tables apply only to the manifest at the root of a Cargo build, so
both are required. The encryption dependencies retain their existing versions.

The manifest is standalone, unpublished, and requires Rust 1.99. The runtime
tests and doctests are retained. The packaged `ok.rs` and UI tests refer to an
unpublished `test-crate` absent from the package. Those test entry points,
UI fixtures, and their unused dependencies are omitted.
Upstream's pedantic lint configuration is omitted; the fork passes the default
Clippy lints with warnings denied. Documentation list indentation is adjusted
for Clippy, and source formatting follows the repository's rustfmt.

Synchronize from the published package, reapply these manifest and test
adjustments, make the `proc_macro` declaration public, and run formatting,
tests, and Clippy. Remove the fork once the dependency chain no longer needs
the affected upstream version.
