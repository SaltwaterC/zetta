use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use bitflags::bitflags;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::grid::{self, GridCell};
use crate::index::Column;
use crate::vte::ansi::{Attr, Color, Hyperlink as VteHyperlink, NamedColor};

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    #[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
    #[repr(transparent)]
    pub struct Flags: u32 {
        const INVERSE                   = 0b0000_0000_0000_0001;
        const BOLD                      = 0b0000_0000_0000_0010;
        const ITALIC                    = 0b0000_0000_0000_0100;
        const BOLD_ITALIC               = 0b0000_0000_0000_0110;
        const UNDERLINE                 = 0b0000_0000_0000_1000;
        const WRAPLINE                  = 0b0000_0000_0001_0000;
        const WIDE_CHAR                 = 0b0000_0000_0010_0000;
        const WIDE_CHAR_SPACER          = 0b0000_0000_0100_0000;
        const DIM                       = 0b0000_0000_1000_0000;
        const DIM_BOLD                  = 0b0000_0000_1000_0010;
        const HIDDEN                    = 0b0000_0001_0000_0000;
        const STRIKEOUT                 = 0b0000_0010_0000_0000;
        const LEADING_WIDE_CHAR_SPACER  = 0b0000_0100_0000_0000;
        const DOUBLE_UNDERLINE          = 0b0000_1000_0000_0000;
        const UNDERCURL                 = 0b0001_0000_0000_0000;
        const DOTTED_UNDERLINE          = 0b0010_0000_0000_0000;
        const DASHED_UNDERLINE          = 0b0100_0000_0000_0000;
        const BLINK                     = 1 << 15;
        const OVERLINE                  = 1 << 16;
        const PROTECTED                 = 1 << 17;
        const GUARDED                   = 1 << 18;
        /// Explicitly written, including spaces; distinct from erased cells.
        const DRAWN                     = 1 << 19;
        const ALL_UNDERLINES            = Self::UNDERLINE.bits() | Self::DOUBLE_UNDERLINE.bits()
                                        | Self::UNDERCURL.bits() | Self::DOTTED_UNDERLINE.bits()
                                        | Self::DASHED_UNDERLINE.bits();
    }
}

/// Counter for hyperlinks without explicit ID.
static HYPERLINK_ID_SUFFIX: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Hyperlink {
    inner: Arc<HyperlinkInner>,
}

impl Hyperlink {
    pub fn new<T: ToString>(id: Option<T>, uri: String) -> Self {
        let inner = Arc::new(HyperlinkInner::new(id, uri));
        Self { inner }
    }

    pub fn id(&self) -> &str {
        &self.inner.id
    }

    pub fn uri(&self) -> &str {
        &self.inner.uri
    }
}

impl From<VteHyperlink> for Hyperlink {
    fn from(value: VteHyperlink) -> Self {
        Self::new(value.id, value.uri)
    }
}

impl From<Hyperlink> for VteHyperlink {
    fn from(val: Hyperlink) -> Self {
        VteHyperlink { id: Some(val.id().to_owned()), uri: val.uri().to_owned() }
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
struct HyperlinkInner {
    /// Identifier for the given hyperlink.
    id: String,

    /// Resource identifier of the hyperlink.
    uri: String,
}

impl HyperlinkInner {
    pub fn new<T: ToString>(id: Option<T>, uri: String) -> Self {
        let id = match id {
            Some(id) => id.to_string(),
            None => {
                let mut id = HYPERLINK_ID_SUFFIX.fetch_add(1, Ordering::Relaxed).to_string();
                id.push_str("_alacritty");
                id
            },
        };

        Self { id, uri }
    }
}

/// Trait for determining if a reset should be performed.
pub trait ResetDiscriminant<T> {
    /// Value based on which equality for the reset will be determined.
    fn discriminant(&self) -> T;
}

impl<T: Copy> ResetDiscriminant<T> for T {
    fn discriminant(&self) -> T {
        *self
    }
}

impl ResetDiscriminant<(Color, Color)> for Cell {
    fn discriminant(&self) -> (Color, Color) {
        (self.fg, self.bg)
    }
}

/// Dynamically allocated cell content.
///
/// This storage is reserved for cell attributes which are rarely set. This allows reducing the
/// allocation required ahead of time for every cell, with some additional overhead when the extra
/// storage is actually required.
#[derive(Default, Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct CellExtra {
    zerowidth: Vec<char>,
    underline_color: Option<Color>,
    hyperlink: Option<Hyperlink>,
}

/// Content and attributes of a single cell in the terminal grid.
///
/// Zetta patch: `c` and `flags` are laid out side by side, within one aligned eight bytes, so
/// writing a character and its `DRAWN` flag, or resetting both, is one store: plain output is
/// bound by stores to cells.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[repr(C)]
pub struct Cell {
    pub c: char,
    pub flags: Flags,
    pub fg: Color,
    pub bg: Color,
    pub extra: Option<Arc<CellExtra>>,
}

impl Default for Cell {
    #[inline]
    fn default() -> Cell {
        Cell {
            c: ' ',
            bg: Color::Named(NamedColor::Background),
            fg: Color::Named(NamedColor::Foreground),
            flags: Flags::empty(),
            extra: None,
        }
    }
}

impl Cell {
    /// Zerowidth characters stored in this cell.
    #[inline]
    pub fn zerowidth(&self) -> Option<&[char]> {
        self.extra.as_ref().map(|extra| extra.zerowidth.as_slice())
    }

    /// Write a new zerowidth character to this cell.
    #[inline]
    pub fn push_zerowidth(&mut self, character: char) {
        let extra = self.extra.get_or_insert(Default::default());
        Arc::make_mut(extra).zerowidth.push(character);
    }

    /// Remove all wide char data from a cell.
    #[inline(never)]
    pub fn clear_wide(&mut self) {
        self.flags.remove(Flags::WIDE_CHAR | Flags::WIDE_CHAR_SPACER | Flags::DRAWN);
        if let Some(extra) = self.extra.as_mut() {
            Arc::make_mut(extra).zerowidth = Vec::new();
        }
        self.c = ' ';
    }

    /// Set underline color on the cell.
    pub fn set_underline_color(&mut self, color: Option<Color>) {
        // If we reset color and we don't have zerowidth we should drop extra storage.
        if color.is_none()
            && self
                .extra
                .as_ref()
                .is_none_or(|extra| extra.zerowidth.is_empty() && extra.hyperlink.is_none())
        {
            self.extra = None;
        } else {
            let extra = self.extra.get_or_insert(Default::default());
            Arc::make_mut(extra).underline_color = color;
        }
    }

    /// Underline color stored in this cell.
    #[inline]
    pub fn underline_color(&self) -> Option<Color> {
        self.extra.as_ref()?.underline_color
    }

    /// Set hyperlink.
    pub fn set_hyperlink(&mut self, hyperlink: Option<Hyperlink>) {
        let should_drop = hyperlink.is_none()
            && self
                .extra
                .as_ref()
                .is_none_or(|extra| extra.zerowidth.is_empty() && extra.underline_color.is_none());

        if should_drop {
            self.extra = None;
        } else {
            let extra = self.extra.get_or_insert(Default::default());
            Arc::make_mut(extra).hyperlink = hyperlink;
        }
    }

    /// Hyperlink stored in this cell.
    #[inline]
    pub fn hyperlink(&self) -> Option<Hyperlink> {
        self.extra.as_ref()?.hyperlink.clone()
    }
}

impl GridCell for Cell {
    #[inline]
    fn is_empty(&self) -> bool {
        (self.c == ' ' || self.c == '\t')
            && self.bg == Color::Named(NamedColor::Background)
            && self.fg == Color::Named(NamedColor::Foreground)
            && !self.flags.intersects(
                Flags::INVERSE
                    | Flags::ALL_UNDERLINES
                    | Flags::STRIKEOUT
                    | Flags::WRAPLINE
                    | Flags::WIDE_CHAR_SPACER
                    | Flags::LEADING_WIDE_CHAR_SPACER,
            )
            && self.extra.as_ref().map(|extra| extra.zerowidth.is_empty()) != Some(false)
    }

    #[inline]
    fn clear_truncated_wide(&mut self) {
        if self.flags.contains(Flags::WIDE_CHAR) {
            self.clear_wide();
        }
    }

    #[inline]
    fn flags(&self) -> &Flags {
        &self.flags
    }

    #[inline]
    fn flags_mut(&mut self) -> &mut Flags {
        &mut self.flags
    }

    #[inline]
    fn reset(&mut self, template: &Self) {
        *self = template.erased();
    }

    /// Every scrolled-in line resets a whole row, and storing a cell field by field made that a
    /// fifth of the time spent on plain text. Copying one blank cell whole is two wide stores.
    #[inline]
    fn reset_all(cells: &mut [Self], template: &Self) {
        let blank = template.erased();
        for cell in cells {
            cell.extra = None;
            // SAFETY: `extra` is the only field that owns anything, and it was just dropped.
            unsafe { std::ptr::copy_nonoverlapping(&blank, cell, 1) };
        }
    }

    /// A row that only ever had characters written since its last full reset holds erased cells
    /// but for their characters and `DRAWN`, and an erased cell has no flags, so the flags are
    /// set rather than read.
    #[inline]
    fn reset_characters(cells: &mut [Self], _template: &Self) {
        for cell in cells {
            debug_assert!(cell.flags.difference(Flags::DRAWN).is_empty());
            cell.c = ' ';
            cell.flags = Flags::empty();
        }
    }

    #[inline]
    fn archive_char(&self) -> Option<char> {
        Some(self.c)
    }

    #[inline]
    fn archive_wrap(&self) -> bool {
        self.flags.contains(Flags::WRAPLINE)
    }

    /// Shared `extra` storage counts as the same only when it is the same allocation, which is
    /// how the cells written under one cursor template share it.
    #[inline]
    fn same_archive_attributes(&self, other: &Self) -> bool {
        self.fg == other.fg
            && self.bg == other.bg
            && self.flags == other.flags
            && match (&self.extra, &other.extra) {
                (None, None) => true,
                (Some(extra), Some(other)) => Arc::ptr_eq(extra, other),
                _ => false,
            }
    }

    #[inline]
    fn with_archive_char(&self, c: char) -> Self {
        Cell { c, ..self.clone() }
    }
}

impl From<Color> for Cell {
    #[inline]
    fn from(color: Color) -> Self {
        Self { bg: color, ..Cell::default() }
    }
}

/// Get the length of occupied cells in a line.
pub trait LineLength {
    /// Calculate the occupied line length.
    fn line_length(&self) -> Column;
}

impl LineLength for grid::Row<Cell> {
    fn line_length(&self) -> Column {
        let mut length = Column(0);

        if self[Column(self.len() - 1)].flags.contains(Flags::WRAPLINE) {
            return Column(self.len());
        }

        for (index, cell) in self[..].iter().rev().enumerate() {
            if cell.c != ' '
                || cell.extra.as_ref().map(|extra| extra.zerowidth.is_empty()) == Some(false)
            {
                length = Column(self.len() - index);
                break;
            }
        }

        length
    }
}

// `Cell::write_drawn_ascii` writes `c` and `flags` as one eight-byte value.
const _: () = assert!(
    std::mem::offset_of!(Cell, c) == 0
        && std::mem::offset_of!(Cell, flags) == 4
        && std::mem::size_of::<Flags>() == 4
        && std::mem::align_of::<Cell>() >= 8
);

impl Cell {
    /// Writes `c` and sets the flags to exactly `DRAWN`, as plain output does to a blank cell.
    /// Two stores per cell would bind that loop to its stores, so they are made one.
    #[inline(always)]
    pub(crate) fn write_drawn_ascii(&mut self, c: char) {
        let mut bytes = [0; 8];
        bytes[..4].copy_from_slice(&u32::from(c).to_ne_bytes());
        bytes[4..].copy_from_slice(&Flags::DRAWN.bits().to_ne_bytes());
        // SAFETY: the assertion above places `c` and the `repr(transparent)` `u32` of `flags` in
        // the cell's first eight bytes, which are aligned for a `u64`. A `char` read back from
        // them is `c`, and any `u32` is valid flags.
        unsafe { std::ptr::from_mut(self).cast::<u64>().write(u64::from_ne_bytes(bytes)) };
    }

    /// Apply SGR without changing layout metadata or the rest of a grapheme.
    pub(crate) fn apply_attribute(&mut self, attr: Attr) {
        match attr {
            Attr::Foreground(color) => self.fg = color,
            Attr::Background(color) => self.bg = color,
            Attr::UnderlineColor(color) => self.set_underline_color(color),
            Attr::Reset => {
                self.fg = Color::Named(NamedColor::Foreground);
                self.bg = Color::Named(NamedColor::Background);
                self.flags &= Flags::PROTECTED | Flags::GUARDED;
                self.set_underline_color(None);
            },
            Attr::Reverse => self.flags.insert(Flags::INVERSE),
            Attr::CancelReverse => self.flags.remove(Flags::INVERSE),
            Attr::Bold => self.flags.insert(Flags::BOLD),
            Attr::CancelBold => self.flags.remove(Flags::BOLD),
            Attr::Dim => self.flags.insert(Flags::DIM),
            Attr::CancelBoldDim => self.flags.remove(Flags::BOLD | Flags::DIM),
            Attr::Italic => self.flags.insert(Flags::ITALIC),
            Attr::CancelItalic => self.flags.remove(Flags::ITALIC),
            Attr::Underline => {
                self.flags.remove(Flags::ALL_UNDERLINES);
                self.flags.insert(Flags::UNDERLINE);
            },
            Attr::DoubleUnderline => {
                self.flags.remove(Flags::ALL_UNDERLINES);
                self.flags.insert(Flags::DOUBLE_UNDERLINE);
            },
            Attr::Undercurl => {
                self.flags.remove(Flags::ALL_UNDERLINES);
                self.flags.insert(Flags::UNDERCURL);
            },
            Attr::DottedUnderline => {
                self.flags.remove(Flags::ALL_UNDERLINES);
                self.flags.insert(Flags::DOTTED_UNDERLINE);
            },
            Attr::DashedUnderline => {
                self.flags.remove(Flags::ALL_UNDERLINES);
                self.flags.insert(Flags::DASHED_UNDERLINE);
            },
            Attr::CancelUnderline => self.flags.remove(Flags::ALL_UNDERLINES),
            Attr::Hidden => self.flags.insert(Flags::HIDDEN),
            Attr::CancelHidden => self.flags.remove(Flags::HIDDEN),
            Attr::BlinkSlow | Attr::BlinkFast => self.flags.insert(Flags::BLINK),
            Attr::CancelBlink => self.flags.remove(Flags::BLINK),
            Attr::Overline => self.flags.insert(Flags::OVERLINE),
            Attr::CancelOverline => self.flags.remove(Flags::OVERLINE),
            Attr::Strike => self.flags.insert(Flags::STRIKEOUT),
            Attr::CancelStrike => self.flags.remove(Flags::STRIKEOUT),
        }
    }

    /// A freshly erased cell retains the current foreground and background.
    pub(crate) fn erased(&self) -> Self {
        Self { fg: self.fg, bg: self.bg, ..Self::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::mem;

    use crate::grid::Row;
    use crate::index::Column;

    #[test]
    fn cell_size_is_below_cap() {
        // Expected cell size on 64-bit architectures.
        const EXPECTED_CELL_SIZE: usize = 24;

        // Ensure that cell size isn't growing by accident.
        assert!(mem::size_of::<Cell>() <= EXPECTED_CELL_SIZE);
    }

    #[test]
    fn line_length_works() {
        let mut row = Row::<Cell>::new(10);
        row[Column(5)].c = 'a';

        assert_eq!(row.line_length(), Column(6));
    }

    #[test]
    fn line_length_works_with_wrapline() {
        let mut row = Row::<Cell>::new(10);
        row[Column(9)].flags.insert(super::Flags::WRAPLINE);

        assert_eq!(row.line_length(), Column(10));
    }

    #[test]
    fn resetting_cells_together_matches_resetting_each() {
        let mut cells = vec![Cell::default(); 6];
        cells[0].c = 'a';
        cells[1].fg = Color::Named(NamedColor::Red);
        cells[1].bg = Color::Indexed(4);
        cells[2].flags = Flags::WIDE_CHAR | Flags::BOLD;
        cells[3].flags = Flags::WIDE_CHAR_SPACER;
        cells[4].push_zerowidth('\u{301}');
        cells[5].set_underline_color(Some(Color::Spec(crate::vte::ansi::Rgb { r: 1, g: 2, b: 3 })));
        let shared = cells[5].extra.clone().unwrap();
        let template = Cell { bg: Color::Named(NamedColor::Green), ..Cell::default() };

        let mut each = cells.clone();
        for cell in &mut each {
            cell.reset(&template);
        }
        let mut together = cells;
        Cell::reset_all(&mut together, &template);

        assert_eq!(together, each);
        assert!(together.iter().all(|cell| *cell == template.erased()));
        // Both resets dropped their reference to the shared extra.
        assert_eq!(Arc::strong_count(&shared), 1);
    }
}
