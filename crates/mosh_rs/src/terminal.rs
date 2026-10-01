//! The client's own copy of the screen, and how it reaches whatever is
//! displaying it (`completeterminal.cc`, `terminaldisplay.cc`).
//!
//! Without this, a client can only write what the server sends straight
//! through, and that is wrong for a reason worth stating: a host diff
//! describes the screen as of the state the SERVER believes the client
//! holds. While an acknowledgement is in flight the server recomputes
//! the same frame from that older state, so writing every diff through
//! paints the same output twice.
//!
//! mosh's answer, and this module's, is to keep the screen locally.
//! Each received state is a screen the client can reconstruct, a diff
//! is applied to the state it was computed FROM (not to whatever is
//! currently displayed), and what actually reaches the display is the
//! difference between what is on it now and the newest state. Repeated
//! frames collapse to nothing.
//!
//! What a screen IS lives behind [`Screen`], so an application can
//! bring the emulator it already has instead of carrying a second one.
//!
//! Screens are held behind [`Arc`], and that is a cost decision rather
//! than a sharing one. Most states on an idle link change nothing: a
//! keep-alive's answer and a heartbeat are new state numbers with empty
//! diffs, and each used to cost a whole screen copy. Shared, they cost a
//! reference count. The same sharing is what lets [`ClientTerminal::render`]
//! recognise a pass with nothing new to paint without diffing anything.

use std::collections::HashMap;
use std::sync::Arc;

use crate::screen::{DiffScreen, OverlayCell, OverlayCursor, Screen};

/// The client's terminal: the states it can reconstruct, and what is
/// currently displayed.
pub struct ClientTerminal<S: Screen> {
    /// Screens by state number. A diff names the state it starts from,
    /// so the client must be able to reproduce that screen exactly, not
    /// just the newest one. States whose diff changed nothing share
    /// their base's screen.
    states: HashMap<u64, Arc<S>>,
    /// What the display is currently showing, predictions included.
    displayed: Arc<S>,
    /// What `displayed` was last brought up to: the confirmed screen it
    /// was built from and what was painted over it. While neither has
    /// changed the display is already right, which is how a pass with
    /// nothing new costs a comparison instead of a copy and a diff.
    /// `None` whenever `displayed` may differ from what this describes.
    shown: Option<Shown<S>>,
    /// Highest state applied, which is what `displayed` is caught up to
    /// after a render.
    latest: u64,
    /// Prepended to every window title sent onward. Empty by default:
    /// this decides the MECHANISM, never the text. mosh prefixes
    /// `[mosh] ` so a taskbar shows which windows are sessions that
    /// survive a disconnect, and then had to add a way to turn it off,
    /// which is the whole argument for the choice living with whoever
    /// looks at the window.
    title_prefix: String,
}

/// The inputs `displayed` was last built from.
struct Shown<S> {
    confirmed: Arc<S>,
    overlay: Vec<OverlayCell>,
    cursor: OverlayCursor,
}

impl<S> Shown<S> {
    fn matches(&self, confirmed: &Arc<S>, overlay: &[OverlayCell], cursor: OverlayCursor) -> bool {
        Arc::ptr_eq(&self.confirmed, confirmed) && self.cursor == cursor && self.overlay == overlay
    }
}

impl<S: Screen> ClientTerminal<S> {
    /// Start from a blank screen both sides agree on.
    pub fn new(blank: S) -> Self {
        let blank = Arc::new(blank);
        let mut states = HashMap::new();
        // State 0 is the blank screen before anything is sent.
        states.insert(0, Arc::clone(&blank));
        Self {
            states,
            displayed: blank,
            shown: None,
            latest: 0,
            title_prefix: String::new(),
        }
    }

    /// Apply a host diff that takes state `old_num` to `new_num`.
    ///
    /// `false` when `old_num` names a screen no longer held, which the
    /// transport layer already filters; keeping the check here means a
    /// diff can never be applied to the wrong screen.
    pub fn apply_diff(&mut self, old_num: u64, new_num: u64, bytes: &[u8]) -> bool {
        let Some(base) = self.states.get(&old_num) else {
            return false;
        };
        // Feeding nothing changes nothing (see `Screen::feed`), so the
        // new state IS its base and shares it.
        let screen = if bytes.is_empty() {
            Arc::clone(base)
        } else {
            let mut screen = S::clone(base);
            screen.feed(bytes);
            Arc::new(screen)
        };
        self.states.insert(new_num, screen);
        if new_num > self.latest || self.latest == u64::MAX {
            self.latest = new_num;
        }
        true
    }

    /// Record a resize the server reported, so later diffs land on a
    /// screen of the right shape.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        // Every held state is resized on its own, so states that shared
        // a screen stop sharing it here; a resize is rare enough that
        // the copies do not matter.
        for screen in self.states.values_mut() {
            Arc::make_mut(screen).resize(rows, cols);
        }
        Arc::make_mut(&mut self.displayed).resize(rows, cols);
        self.shown = None;
    }

    /// Forget states the server promised never to diff from again.
    /// Without this a long session keeps a screen per state forever.
    pub fn forget_before(&mut self, throwaway: u64) {
        self.states
            .retain(|num, _| *num >= throwaway || *num == self.latest);
    }

    /// The newest state the server has sent: the screen every
    /// prediction must be judged against, and nothing else.
    pub fn confirmed(&self) -> Option<&S> {
        self.states.get(&self.latest).map(Arc::as_ref)
    }

    /// What the display is showing right now, predictions included.
    pub fn displayed(&self) -> &S {
        &self.displayed
    }

    /// The text of the newest state, as the client believes the screen
    /// reads. The authority a correctly painted display must agree with.
    pub fn screen_text(&self) -> String {
        self.states
            .get(&self.latest)
            .map(|screen| screen.text())
            .unwrap_or_default()
    }

    /// The text the display is showing, predictions included.
    pub fn displayed_text(&self) -> String {
        self.displayed.text()
    }

    /// What to prepend to a window title before passing it on.
    pub fn set_title_prefix(&mut self, prefix: impl Into<String>) {
        self.title_prefix = prefix.into();
    }

    /// The window title the host has asked for, unprefixed.
    pub fn title(&self) -> Option<String> {
        self.states
            .get(&self.latest)
            .and_then(|screen| screen.title())
    }

    /// Highest state applied.
    pub fn latest(&self) -> u64 {
        self.latest
    }

    /// How many screens are held; a session that never prunes would
    /// grow this without bound.
    pub fn held_states(&self) -> usize {
        self.states.len()
    }

    /// Bring the displayed screen up to the newest state, with the
    /// given predictions painted over it.
    ///
    /// The overlay lands on a COPY of the confirmed state, and that copy
    /// becomes what the display is believed to be showing. Both halves
    /// matter: the confirmed state stays clean so the next host diff
    /// still lands on the screen the server computed it from, and
    /// `displayed` tracks what was really painted so the next difference
    /// is taken against reality rather than against a screen the user
    /// never saw.
    pub fn advance(&mut self, overlay: &[OverlayCell], cursor: OverlayCursor) {
        if let Some((next, shown)) = self.next_display(overlay, cursor) {
            self.displayed = next;
            self.shown = Some(shown);
        }
    }

    /// The screen the display should show next, and what it is built
    /// from, or `None` when that is exactly what it already shows (or
    /// there is no state to show).
    ///
    /// With nothing painted over it, the display is the confirmed screen
    /// itself, shared rather than copied; only an overlay needs a copy of
    /// its own to paint on.
    fn next_display(
        &self,
        overlay: &[OverlayCell],
        cursor: OverlayCursor,
    ) -> Option<(Arc<S>, Shown<S>)> {
        let confirmed = self.states.get(&self.latest)?;
        if self
            .shown
            .as_ref()
            .is_some_and(|shown| shown.matches(confirmed, overlay, cursor))
        {
            return None;
        }
        let next = if overlay.is_empty() && cursor == OverlayCursor::Unchanged {
            Arc::clone(confirmed)
        } else {
            let mut painted = S::clone(confirmed);
            painted.draw_overlay(overlay, cursor);
            Arc::new(painted)
        };
        let shown = Shown {
            confirmed: Arc::clone(confirmed),
            overlay: overlay.to_vec(),
            cursor,
        };
        Some((next, shown))
    }
}

impl<S: DiffScreen> ClientTerminal<S> {
    /// The bytes that turn what the display shows into the newest state
    /// with `overlay` painted on it, and nothing more. Empty when it
    /// already matches, which is exactly what makes a repeated frame
    /// free.
    ///
    /// Only a caller writing to a real terminal needs this. One that
    /// owns the grid it draws from calls [`Self::advance`] and then
    /// draws [`Self::displayed`].
    pub fn render(&mut self, overlay: &[OverlayCell], cursor: OverlayCursor) -> Vec<u8> {
        // Nothing has changed since the last render: what the display
        // shows is already this, so the diff would be empty and the
        // title already sent.
        let Some((next, shown)) = self.next_display(overlay, cursor) else {
            return Vec::new();
        };
        let mut out = next.diff_from(&self.displayed);
        // The title travels inside the host diff, and a diff of CELLS
        // cannot carry it: without this it reaches the client and stops
        // there, leaving the local terminal titled whatever it was
        // before the session began.
        out.extend_from_slice(&self.title_escape(next.title(), self.displayed.title()));
        self.displayed = next;
        self.shown = Some(shown);
        out
    }

    /// The escape that sets a window title, when it has changed.
    ///
    /// mosh sends OSC 0, which sets the icon name and the title
    /// together, terminated by BEL rather than the more correct ST
    /// because BEL is more widely supported.
    fn title_escape(&self, next: Option<String>, shown: Option<String>) -> Vec<u8> {
        match next {
            Some(title) if Some(&title) != shown.as_ref() => {
                format!("\x1b]0;{}{title}\x07", self.title_prefix).into_bytes()
            }
            _ => Vec::new(),
        }
    }

    /// The whole screen, for a terminal whose state cannot be known
    /// (the first paint, or coming back from a suspend).
    pub fn repaint(&mut self) -> Vec<u8> {
        let Some(latest) = self.states.get(&self.latest) else {
            return Vec::new();
        };
        let mut out = latest.repaint();
        // A repaint is for a terminal whose state cannot be known, so
        // the title is restated rather than diffed.
        let title = latest.title();
        let shown = Shown {
            confirmed: Arc::clone(latest),
            overlay: Vec::new(),
            cursor: OverlayCursor::Unchanged,
        };
        self.displayed = Arc::clone(latest);
        out.extend_from_slice(&self.title_escape(title, None));
        self.shown = Some(shown);
        out
    }
}

#[cfg(all(test, feature = "vt100-screen"))]
mod tests {
    use super::*;
    use crate::screen::Vt100Screen;

    fn terminal(rows: u16, cols: u16) -> ClientTerminal<Vt100Screen> {
        ClientTerminal::new(Vt100Screen::new(rows, cols))
    }

    fn text_of(term: &ClientTerminal<Vt100Screen>) -> String {
        term.screen_text()
    }

    #[test]
    fn a_diff_lands_on_the_state_it_was_computed_from() {
        let mut t = terminal(3, 20);
        assert!(t.apply_diff(0, 1, b"hello"));
        assert_eq!(text_of(&t).trim(), "hello");

        // The server had not heard our ack yet, so this second diff
        // starts from state 0 again and repeats the frame.
        assert!(t.apply_diff(0, 2, b"hello world"));
        // Applied to state 0, NOT to state 1: no doubled text.
        assert_eq!(text_of(&t).trim(), "hello world");
    }

    #[test]
    fn a_repeated_frame_renders_to_nothing() {
        let mut t = terminal(3, 20);
        t.apply_diff(0, 1, b"hello");
        let first = t.render(&[], OverlayCursor::Unchanged);
        assert!(!first.is_empty(), "the first paint has to say something");

        // The same screen arriving again under a new state number is
        // what a retransmission looks like from up here.
        t.apply_diff(0, 2, b"hello");
        assert!(
            t.render(&[], OverlayCursor::Unchanged).is_empty(),
            "nothing changed on screen, so nothing should reach the terminal"
        );
    }

    #[test]
    fn rendering_twice_produces_nothing_the_second_time() {
        let mut t = terminal(3, 20);
        t.apply_diff(0, 1, b"abc");
        assert!(!t.render(&[], OverlayCursor::Unchanged).is_empty());
        assert!(t.render(&[], OverlayCursor::Unchanged).is_empty());
    }

    #[test]
    fn a_diff_from_an_unknown_state_is_refused() {
        let mut t = terminal(3, 20);
        assert!(!t.apply_diff(9, 10, b"nope"));
        assert_eq!(t.latest(), 0);
    }

    #[test]
    fn the_rendered_bytes_actually_reproduce_the_screen() {
        // The point of the diff: replaying it over the previous screen
        // must give the same result as painting the new one outright.
        let mut t = terminal(3, 20);
        t.apply_diff(0, 1, b"first line");
        let mut mirror = Vt100Screen::new(3, 20);
        mirror.feed(&t.render(&[], OverlayCursor::Unchanged));

        t.apply_diff(1, 2, b"\r\nsecond");
        mirror.feed(&t.render(&[], OverlayCursor::Unchanged));

        assert_eq!(mirror.text(), t.screen_text());
    }

    #[test]
    fn forgetting_prunes_but_keeps_the_latest() {
        let mut t = terminal(3, 20);
        t.apply_diff(0, 1, b"a");
        t.apply_diff(1, 2, b"b");
        t.apply_diff(2, 3, b"c");
        assert_eq!(t.held_states(), 4);
        t.forget_before(3);
        assert_eq!(t.held_states(), 1);
        // And the screen is still intact.
        assert_eq!(text_of(&t).trim(), "abc");
    }

    #[test]
    fn an_overlay_paints_without_touching_the_confirmed_state() {
        let mut t = terminal(3, 20);
        t.apply_diff(0, 1, b"ab");
        let overlay = [OverlayCell {
            row: 0,
            col: 2,
            cell: crate::screen::Cell {
                contents: "c".into(),
                rendition: Default::default(),
            },
            underline: false,
        }];
        let painted = t.render(&overlay, OverlayCursor::At(0, 3));
        assert!(!painted.is_empty());
        assert_eq!(t.displayed_text().trim(), "abc");
        // The server still believes the screen reads "ab", and the next
        // host diff will be computed from that.
        assert_eq!(t.screen_text().trim(), "ab");
    }

    #[test]
    fn a_title_the_host_sets_reaches_the_terminal() {
        // The half that was missing. The title travels inside the host
        // diff and a diff of cells cannot carry it, so without this it
        // arrives at the client and stops there.
        let mut t = terminal(3, 20);
        t.apply_diff(0, 1, b"\x1b]0;deploy@prod\x07hi");
        let painted =
            String::from_utf8_lossy(&t.render(&[], OverlayCursor::Unchanged)).into_owned();
        assert!(painted.contains("\x1b]0;deploy@prod\x07"), "{painted:?}");
    }

    #[test]
    fn an_unchanged_title_is_not_sent_again() {
        // Every frame would otherwise carry it, which is what the diff
        // exists to avoid.
        let mut t = terminal(3, 20);
        t.apply_diff(0, 1, b"\x1b]0;same\x07a");
        assert!(
            String::from_utf8_lossy(&t.render(&[], OverlayCursor::Unchanged)).contains("]0;same")
        );
        t.apply_diff(1, 2, b"b");
        let second = String::from_utf8_lossy(&t.render(&[], OverlayCursor::Unchanged)).into_owned();
        assert!(!second.contains("]0;"), "sent twice: {second:?}");
    }

    #[test]
    fn the_prefix_is_applied_but_never_chosen_here() {
        let mut t = terminal(3, 20);
        t.set_title_prefix("[mosh] ");
        t.apply_diff(0, 1, b"\x1b]0;deploy@prod\x07");
        let painted =
            String::from_utf8_lossy(&t.render(&[], OverlayCursor::Unchanged)).into_owned();
        assert!(
            painted.contains("\x1b]0;[mosh] deploy@prod\x07"),
            "{painted:?}"
        );
        // And the title itself is reported unprefixed, so a caller that
        // wants to do something else with it can.
        assert_eq!(t.title().as_deref(), Some("deploy@prod"));
    }

    #[test]
    fn a_repaint_restates_the_title() {
        // A repaint is for a terminal whose state cannot be known, so
        // nothing may be assumed to already be set.
        let mut t = terminal(3, 20);
        t.apply_diff(0, 1, b"\x1b]0;after suspend\x07x");
        let _ = t.render(&[], OverlayCursor::Unchanged);
        let again = String::from_utf8_lossy(&t.repaint()).into_owned();
        assert!(again.contains("]0;after suspend"), "{again:?}");
    }

    /// A screen that counts the diffs taken of it, so a test can tell a
    /// render that was skipped from one that compared and found nothing.
    #[derive(Clone)]
    struct Counted {
        inner: Vt100Screen,
        diffs: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Screen for Counted {
        fn feed(&mut self, bytes: &[u8]) {
            self.inner.feed(bytes);
        }
        fn resize(&mut self, rows: u16, cols: u16) {
            self.inner.resize(rows, cols);
        }
        fn rows(&self) -> u16 {
            self.inner.rows()
        }
        fn cols(&self) -> u16 {
            self.inner.cols()
        }
        fn cursor(&self) -> (u16, u16) {
            self.inner.cursor()
        }
        fn cell(&self, row: u16, col: u16) -> crate::screen::Cell {
            self.inner.cell(row, col)
        }
        fn text(&self) -> String {
            self.inner.text()
        }
    }

    impl DiffScreen for Counted {
        fn diff_from(&self, previous: &Self) -> Vec<u8> {
            self.diffs
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.inner.diff_from(&previous.inner)
        }
        fn repaint(&self) -> Vec<u8> {
            self.inner.repaint()
        }
    }

    fn counted() -> (
        ClientTerminal<Counted>,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let diffs = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let blank = Counted {
            inner: Vt100Screen::new(3, 20),
            diffs: std::sync::Arc::clone(&diffs),
        };
        (ClientTerminal::new(blank), diffs)
    }

    fn diffs_taken(diffs: &std::sync::atomic::AtomicUsize) -> usize {
        diffs.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn predicted(col: u16, contents: &str) -> OverlayCell {
        OverlayCell {
            row: 0,
            col,
            cell: crate::screen::Cell {
                contents: contents.into(),
                rendition: Default::default(),
            },
            underline: false,
        }
    }

    #[test]
    fn a_state_whose_diff_is_empty_shares_its_base_screen() {
        // A keep-alive's answer: a new state number that changes nothing.
        let mut t = terminal(3, 20);
        t.apply_diff(0, 1, b"abc");
        t.apply_diff(1, 2, b"");
        assert!(Arc::ptr_eq(&t.states[&1], &t.states[&2]));
        assert_eq!(t.latest(), 2);
        assert_eq!(text_of(&t).trim(), "abc");
    }

    #[test]
    fn a_pass_with_nothing_new_is_not_diffed_at_all() {
        let (mut t, diffs) = counted();
        t.apply_diff(0, 1, b"hello");
        assert!(!t.render(&[], OverlayCursor::Unchanged).is_empty());
        assert_eq!(diffs_taken(&diffs), 1);

        // Neither the state nor the overlay moved, and an empty diff
        // keeps the same screen, so there is nothing to compare.
        assert!(t.render(&[], OverlayCursor::Unchanged).is_empty());
        t.apply_diff(1, 2, b"");
        assert!(t.render(&[], OverlayCursor::Unchanged).is_empty());
        assert_eq!(diffs_taken(&diffs), 1, "an unchanged pass took a diff");

        // A real change still paints.
        t.apply_diff(2, 3, b" world");
        assert!(!t.render(&[], OverlayCursor::Unchanged).is_empty());
        assert_eq!(t.displayed_text().trim(), "hello world");
    }

    #[test]
    fn a_changed_overlay_paints_even_on_an_unchanged_state() {
        let (mut t, diffs) = counted();
        t.apply_diff(0, 1, b"ab");
        let _ = t.render(&[], OverlayCursor::Unchanged);

        let guess = [predicted(2, "c")];
        assert!(!t.render(&guess, OverlayCursor::At(0, 3)).is_empty());
        assert_eq!(t.displayed_text().trim(), "abc");
        // The same guess again is already on screen.
        let before = diffs_taken(&diffs);
        assert!(t.render(&guess, OverlayCursor::At(0, 3)).is_empty());
        assert_eq!(diffs_taken(&diffs), before);
        // And taking it away is a change too.
        assert!(!t.render(&[], OverlayCursor::Unchanged).is_empty());
        assert_eq!(t.displayed_text().trim(), "ab");
    }

    #[test]
    fn a_resize_makes_the_next_render_look_again() {
        let (mut t, diffs) = counted();
        t.apply_diff(0, 1, b"ab");
        let _ = t.render(&[], OverlayCursor::Unchanged);
        t.resize(4, 20);
        let before = diffs_taken(&diffs);
        let _ = t.render(&[], OverlayCursor::Unchanged);
        assert_eq!(diffs_taken(&diffs), before + 1);
    }

    #[test]
    fn a_resize_does_not_reach_a_state_another_shared() {
        // Shared screens are copied apart by a resize, not resized twice
        // or left behind.
        let mut t = terminal(3, 20);
        t.apply_diff(0, 1, b"ab");
        t.apply_diff(1, 2, b"");
        t.resize(5, 30);
        for screen in t.states.values() {
            assert_eq!((screen.rows(), screen.cols()), (5, 30));
        }
        assert_eq!((t.displayed().rows(), t.displayed().cols()), (5, 30));
    }

    #[test]
    fn a_render_after_advance_or_repaint_has_nothing_left_to_do() {
        let (mut t, diffs) = counted();
        t.apply_diff(0, 1, b"xy");
        t.advance(&[], OverlayCursor::Unchanged);
        assert!(t.render(&[], OverlayCursor::Unchanged).is_empty());
        t.apply_diff(1, 2, b"z");
        let _ = t.repaint();
        assert!(t.render(&[], OverlayCursor::Unchanged).is_empty());
        assert_eq!(diffs_taken(&diffs), 0);
    }

    #[test]
    fn advance_updates_the_display_without_producing_bytes() {
        // The path an application that owns its grid takes: no diff, no
        // escape bytes, just the newest state with the overlay on it.
        let mut t = terminal(3, 20);
        t.apply_diff(0, 1, b"xy");
        t.advance(&[], OverlayCursor::Unchanged);
        assert_eq!(t.displayed().text().trim(), "xy");
    }
}
