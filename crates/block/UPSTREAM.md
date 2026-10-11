# Zetta block fork

Source baseline: crates.io `block@0.1.6`, checksum
`0d8c1fef690941d3e7788d328517591fecc684c084084702d6ff1641e993699a`.
Upstream: <https://github.com/SSheldon/rust-block>, MIT licensed.

Rust 1.99 reports `_NSConcreteStackBlock` as an uninhabited static (Rust
issue 74840). The upstream declaration uses an empty enum for an opaque
runtime symbol. This fork declares the symbol and the block's `isa` pointer
with `c_void`, and takes its address with `ptr::addr_of!` without constructing
a reference or reading its contents. This preserves the pointer layout and
the symbol address required by the Apple Blocks ABI:
<https://clang.llvm.org/docs/Block-ABI-Apple.html>.
`uninhabited_static` is denied so this regression fails compilation.

The root, standalone terminal and standalone terminal_view manifests patch
crates.io to this fork. Patch tables apply only at the root of a Cargo build;
the root patch also covers cocoa, core-video and metal's transitive edges.
Their dependency versions and public block API remain unchanged. Static
performance review confirms this only changes the type and address expression;
block layout, allocation, copying and callback dispatch stay the same.

The standalone manifest is unpublished and requires Rust 1.99. Retain the
2015 edition. Foreign declarations explicitly name their existing C ABI,
transmutes name their source and destination types, unsafe API documentation
has Safety headings, and field initializers use shorthand so the fork passes
Clippy with warnings denied. Source formatting follows rustfmt.

The packaged upstream tests depend on `objc_test_utils`, an unpublished path
dependency absent from the package. They are replaced with inline tests of the
native runtime: stack class address and invocation, heap copying with captured
arguments, survival beyond the originating stack frame, and clone/drop
ownership. The two upstream doctests are retained. The Makefile includes this
suite on macOS; other hosts require libBlocksRuntime to link standalone tests.

To synchronize, start from the published package, reapply these changes, and
run its tests, doctests and Clippy, then the root macOS and feature checks.
Remove the fork once the dependency chain no longer needs this upstream
version. Do not suppress the future-incompatibility warning instead.
