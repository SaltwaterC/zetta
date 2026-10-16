# Zosh resize content loss

The bundled Rust server had a platform-independent destructive resize.
`TerminalState::resize` called vt100's fixed-grid `set_size`: `Row::resize`
discarded right-hand cells, and the row vector discarded bottom rows.
Growing the pane again could not restore either. Splitting a session pane
exercises this path because its Mosh resize also resizes the server's emulator.

The same emulator serves plain Zosh, independently of zmux. Windows uses
ConPTY between the relay and server, while Unix uses a PTY; both feed this
emulator and both went through the destructive operation. SSH session panes
bypass this emulator, and zmux's retained Alacritty grid already reflows.
This identifies a shared Zosh defect; it does not establish that every
possible Windows console application preserves its own output on resize.

The fix gives the authoritative server an explicit normal-screen reflow.
It preserves hard line breaks, soft wraps, attributes, wide and combining
characters, and cursor placement. Rows displaced above the smaller screen
enter the existing acknowledged scrollback transport. The client reconstructs
those rows before repainting, because its previous protocol grid and the
physical terminal can have different contents after resizing. Alternate screens
and partial scroll regions retain fixed-coordinate behavior for applications
that redraw themselves. Protocol replicas retain the ordinary resize and use
the server's authoritative repaint.

Regression coverage includes width and height reductions, shrink/grow cycles,
pending wraps, Unicode and attributes, the scrollback acknowledgement boundary,
and server-to-client-to-display preservation. A live UDP loopback test runs a
plain shell that prints once and does not redraw after resizing, through the
same repaint function used by standalone Zosh and embedded pane sessions.
The existing end-to-end relay suite checks the multiplexer transport separately.

Both the client and remote server need updating. Old servers have already
discarded the missing cells before the client receives a screen state.
Stock external Mosh servers are not changed by this patch.

The resize work is linear in the cells being reflowed, carried history being
normalized to the final width, and the resulting rows; it runs only when
geometry changes. Ordinary output, input and frame diffing
gain no new loops or allocations. The existing bounded eviction queue and
scrollback budget still apply. A graphical rendering benchmark is not needed
to establish this resize-only cost.

Validation on macOS includes the root suite, vt100 and zosh suites, live UDP
interoperability tests, all five end-to-end relay tests, and Clippy. Reinstating
the old server resize makes the new content-preservation regression fail at
"resize must carry history". Repeated resizes before a frame are also covered,
including a final width of one column and widening after an earlier shrink.
The Windows cross-check could not run because `x86_64-pc-windows-gnu` is not
installed. No native Windows host was exercised, so ConPTY-specific runtime
behavior still needs validation there.
