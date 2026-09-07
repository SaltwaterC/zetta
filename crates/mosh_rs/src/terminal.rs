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

use std::collections::HashMap;

use crate::screen::{DiffScreen, OverlayCell, OverlayCursor, Screen};

/// The client's terminal: the states it can reconstruct, and what is
/// currently displayed.
pub struct ClientTerminal<S: Screen> {
    /// Screens by state number. A diff names the state it starts from,
    /// so the client must be able to reproduce that screen exactly, not
    /// just the newest one.
    states: HashMap<u64, S>,
    /// What the display is currently showing, predictions included.
    displayed: S,
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

impl<S: Screen> ClientTerminal<S> {
    /// Start from a blank screen both sides agree on.
    pub fn new(blank: S) -> Self {
        let mut states = HashMap::new();
        // State 0 is the blank screen before anything is sent.
        states.insert(0, blank.clone());
        Self {
            states,
            displayed: blank,
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
        let mut screen = base.clone();
        screen.feed(bytes);
        self.states.insert(new_num, screen);
        if new_num > self.latest || self.latest == u64::MAX {
            self.latest = new_num;
        }
        true
    }

    /// Record a resize the server reported, so later diffs land on a
    /// screen of the right shape.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        for screen in self.states.values_mut() {
            screen.resize(rows, cols);
        }
        self.displayed.resize(rows, cols);
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
        self.states.get(&self.latest)
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
            .map(Screen::text)
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
        self.states.get(&self.latest).and_then(Screen::title)
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
        let Some(latest) = self.states.get(&self.latest) else {
            return;
        };
        let mut next = latest.clone();
        if !overlay.is_empty() || cursor != OverlayCursor::Unchanged {
            next.draw_overlay(overlay, cursor);
        }
        self.displayed = next;
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
        let Some(latest) = self.states.get(&self.latest) else {
            return Vec::new();
        };
        let mut next = latest.clone();
        if !overlay.is_empty() || cursor != OverlayCursor::Unchanged {
            next.draw_overlay(overlay, cursor);
        }
        let mut out = next.diff_from(&self.displayed);
        // The title travels inside the host diff, and a diff of CELLS
        // cannot carry it: without this it reaches the client and stops
        // there, leaving the local terminal titled whatever it was
        // before the session began.
        out.extend_from_slice(&self.title_escape(next.title(), self.displayed.title()));
        self.displayed = next;
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
        let copy = latest.clone();
        out.extend_from_slice(&self.title_escape(title, None));
        self.displayed = copy;
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
