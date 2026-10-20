//! Streaming grapheme placement and width changes at the terminal cursor.
//!
//! Existing cells provide the cluster across parser batches, so snapshots and
//! asynchronous reflow need no second copy of Unicode state. ASCII runs retain
//! their bulk writer; only their first scalar can extend a preceding cluster.
//!
//! Segmenting is far too slow to do for every scalar: output of mostly
//! unrelated characters (`cat` of a binary file) spent two fifths of its parse
//! time here. [`JoinClasses`] settles nearly every scalar without even looking
//! at the cell before it, [`may_join`] most of the rest, and segmentation runs
//! only for the pairs neither can.
//!
//! The one rule that depends on the cell before alone, GB9b (a Prepend joins
//! whatever follows it), is looked for only right after a Prepend is printed:
//! moving the cursor back beside an older one does not draw an ordinary
//! character into its cell. Every other rule reads the cell, so a combining
//! mark still joins whatever it is printed after.

use super::*;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const MAX_CLUSTER_SCALARS: usize = 24;

/// How a scalar can join a grapheme cluster, as far as the scalar alone says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct JoinClasses {
    /// Joins whatever cluster it follows (Extend, ZWJ and SpacingMark: GB9, GB9a).
    previous: bool,
    /// A cluster it starts joins whatever follows (Prepend: GB9b).
    next: bool,
    /// Joins a cluster that ends in a particular way: a Hangul jamo or
    /// syllable (GB6-8), a regional indicator (GB12-13), an Indic consonant
    /// after a linker (GB9c), or a pictograph after a ZWJ (GB11).
    sequence: bool,
}

/// [`JoinClasses`] for each scalar of one 256-scalar block, a bit per scalar,
/// and whether a mark printed after the scalar joins it (see [`takes_marks`]).
struct JoinBlock {
    previous: [u64; 4],
    next: [u64; 4],
    sequence: [u64; 4],
    takes_marks: [u64; 4],
}

// `TABLED_SCALARS`, `JOIN_BLOCK_INDEX` and `JOIN_BLOCKS`, which `build.rs`
// derives from the segmenter. The table covers planes 0 and 1. Above them
// there is nothing to look up: planes 2 and 3 are ideographs, and the rest is
// unassigned or private use, all class Other, but for plane 14's tags and
// variation selectors, which are Extend.
include!(concat!(env!("OUT_DIR"), "/grapheme_join_tables.rs"));

/// The Hangul syllable type of `c`, as GB6-8 use it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hangul {
    Leading,
    Vowel,
    Trailing,
    Lv,
    Lvt,
}

fn hangul(c: char) -> Option<Hangul> {
    Some(match c {
        '\u{1100}'..='\u{115f}' | '\u{a960}'..='\u{a97c}' => Hangul::Leading,
        // Kirat Rai's vowel signs break as Hangul vowels do (Unicode 16).
        '\u{1160}'..='\u{11a7}'
        | '\u{d7b0}'..='\u{d7c6}'
        | '\u{16d63}'
        | '\u{16d67}'..='\u{16d6a}' => Hangul::Vowel,
        '\u{11a8}'..='\u{11ff}' | '\u{d7cb}'..='\u{d7fb}' => Hangul::Trailing,
        '\u{ac00}'..='\u{d7a3}' if (c as u32 - 0xac00).is_multiple_of(28) => Hangul::Lv,
        '\u{ac00}'..='\u{d7a3}' => Hangul::Lvt,
        _ => return None,
    })
}

/// GB6-8: whether a Hangul `c` joins a Hangul `previous`.
fn hangul_joins(previous: char, c: char) -> bool {
    use Hangul::{Leading, Lv, Lvt, Trailing, Vowel};
    matches!(
        (hangul(previous), hangul(c)),
        (Some(Leading), Some(Leading | Vowel | Lv | Lvt))
            | (Some(Lv | Vowel), Some(Vowel | Trailing))
            | (Some(Lvt | Trailing), Some(Trailing))
    )
}

fn regional(c: char) -> bool {
    matches!(c, '\u{1f1e6}'..='\u{1f1ff}')
}

/// Whether `c` is below U+0300 and so joins nothing, as all of Latin-1 and its
/// extensions but © and ® do not: those are pictographs, and an emoji ZWJ
/// sequence can end with one (GB11).
#[inline]
fn latin_joins_nothing(c: char) -> bool {
    (c as u32) < 0x300 && c != '\u{a9}' && c != '\u{ae}'
}

#[inline]
fn join_classes(c: char) -> JoinClasses {
    let code = c as u32;
    // Nor does U+FFFD, which is what undecodable output prints as.
    if latin_joins_nothing(c) || c == '\u{fffd}' {
        return JoinClasses::default();
    }
    if code >= TABLED_SCALARS {
        let previous = matches!(code, 0xe_0000..=0xe_0fff);
        return JoinClasses { previous, ..JoinClasses::default() };
    }
    let (block, word, bit) = join_block(code);
    JoinClasses {
        previous: block.previous[word] >> bit & 1 != 0,
        next: block.next[word] >> bit & 1 != 0,
        sequence: block.sequence[word] >> bit & 1 != 0,
    }
}

/// The block holding a tabled scalar's bits, the word in it and the bit.
#[inline]
fn join_block(code: u32) -> (&'static JoinBlock, usize, u32) {
    let block = &JOIN_BLOCKS[usize::from(JOIN_BLOCK_INDEX[(code >> 8) as usize])];
    (block, ((code >> 6) & 3) as usize, code & 63)
}

/// Whether an Extend, ZWJ or SpacingMark printed after `c` joins it, which is
/// so of anything but a control, CR or LF (GB4, GB9, GB9a). Above the table it
/// says false, which only means segmentation has to decide.
fn takes_marks(c: char) -> bool {
    let code = c as u32;
    if code >= TABLED_SCALARS {
        return false;
    }
    let (block, word, bit) = join_block(code);
    block.takes_marks[word] >> bit & 1 != 0
}

/// Whether `c`, of `classes`, can join a cluster that is the single scalar
/// `previous`. False is certain; true only means segmentation has to decide.
/// Of UAX #29's rules against a break, GB9 and GB9a depend on `c` alone, GB9b
/// on `previous` alone, and GB6-8 (Hangul) and GB12-13 (regional indicators) on
/// both being of one script. GB9c and GB11 need a second scalar already in the
/// cluster.
fn may_join(previous: char, c: char, classes: JoinClasses) -> bool {
    classes.previous
        || join_classes(previous).next
        || hangul_joins(previous, c)
        || (regional(previous) && regional(c))
}

/// Whether `c` is a Prepend, which starts a cluster with whatever follows it.
#[inline]
pub(super) fn is_prepend(c: char) -> bool {
    join_classes(c).next
}

impl<T: EventListener> Term<T> {
    /// Adds `c` to the cluster before the cursor if it belongs there, and says
    /// whether it did.
    #[inline]
    pub(super) fn extend_grapheme(&mut self, c: char) -> bool {
        // Most text is below U+0300, where nothing joins but after a Prepend;
        // classifying even that inline cost a sixth of the time Latin text
        // took to print.
        if latin_joins_nothing(c) && !self.prepend_pending {
            return false;
        }
        self.extend_classified_grapheme(c)
    }

    #[inline(never)]
    fn extend_classified_grapheme(&mut self, c: char) -> bool {
        // Unless it follows a Prepend, a scalar of none of the classes starts
        // a cluster, so the cell before it need not even be found. Every
        // printed scalar passes here first, so the flag cannot miss one.
        let classes = join_classes(c);
        let after_prepend = mem::replace(&mut self.prepend_pending, classes.next);
        if !classes.previous && !classes.sequence && !after_prepend {
            return false;
        }
        self.extend_cluster_before_cursor(c, classes)
    }

    #[inline(never)]
    fn extend_cluster_before_cursor(&mut self, c: char, classes: JoinClasses) -> bool {
        if !self.protocol.grapheme_width && UnicodeWidthChar::width(c).unwrap_or(0) != 0 {
            return false;
        }
        let cursor = &self.grid.cursor;
        let mut point = cursor.point;
        if !cursor.input_needs_wrap {
            if point.column == 0 {
                return false;
            }
            point.column -= 1;
        }
        if self.grid[point].flags.contains(Flags::WIDE_CHAR_SPACER) && point.column > 0 {
            point.column -= 1;
        }
        let cell = &self.grid[point];
        if cell.zerowidth().is_none_or(<[char]>::is_empty)
            && ((c.is_ascii() && cell.c.is_ascii()) || !may_join(cell.c, c, classes))
        {
            return false;
        }

        // Bound both segmentation work and retained combining data. The buffer
        // fits the current cluster plus one incoming four-byte scalar.
        let mut bytes = [0; (MAX_CLUSTER_SCALARS + 1) * 4];
        let mut length = cell.c.encode_utf8(&mut bytes).len();
        let mut count = 1;
        for &extra in cell.zerowidth().unwrap_or_default().iter().take(MAX_CLUSTER_SCALARS - 1) {
            length += extra.encode_utf8(&mut bytes[length..]).len();
            count += 1;
        }
        length += c.encode_utf8(&mut bytes[length..]).len();
        // SAFETY: the bytes are whole scalars, each written by `encode_utf8`.
        let text = unsafe { std::str::from_utf8_unchecked(&bytes[..length]) };
        // A mark after a lone scalar is the common case that needs segmenting,
        // and the classes settle it. Plane 14's `previous` is not exact.
        let certain =
            count == 1 && classes.previous && (c as u32) < TABLED_SCALARS && takes_marks(cell.c);
        if !certain && text.graphemes(true).next().is_none_or(|cluster| cluster.len() != text.len())
        {
            return false;
        }
        if count >= MAX_CLUSTER_SCALARS {
            return true;
        }
        let right = self.input_right_edge();
        let old_width = if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
        let width = if self.protocol.grapheme_width {
            UnicodeWidthStr::width(text).clamp(1, 2).min(right - self.horizontal_bounds().start.0)
        } else {
            old_width
        };
        if point.column + width > right && !self.mode.contains(TermMode::LINE_WRAP) {
            return true;
        }
        // Taken rather than cloned: a clone shares `extra`, so extending it
        // would copy that, and storing it back would free the original.
        let mut base = mem::take(&mut self.grid[point]);
        base.push_zerowidth(c);
        base.flags.remove(Flags::WIDE_CHAR | Flags::WIDE_CHAR_SPACER | Flags::WRAPLINE);

        if point.column + width > right {
            // The placeholder ordinary wide input leaves, so the row still
            // reads as continuing onto the next one without a gap.
            self.grid[point] =
                Cell { bg: base.bg, flags: Flags::LEADING_WIDE_CHAR_SPACER, ..Cell::default() };
            self.grid.cursor.point = point;
            self.wrapline();
            point = self.grid.cursor.point;
        } else if old_width == 2 && width == 1 && point.column < self.last_column() {
            self.grid[point.line][point.column + 1] = Cell { bg: base.bg, ..Cell::default() };
            self.damage.damage_line(point.line.0 as usize, point.column.0, point.column.0 + 1);
        }
        self.clear_edit_boundary(point.line, (point.column + width).0);
        if width == 2 {
            let mut spacer = base.clone();
            spacer.clear_wide();
            spacer.flags.insert(Flags::WIDE_CHAR_SPACER);
            self.grid[point.line][point.column + 1] = spacer;
            base.flags.insert(Flags::WIDE_CHAR);
        }
        self.grid[point] = base;
        self.damage.damage_line(point.line.0 as usize, point.column.0, point.column.0 + width - 1);
        self.grid.cursor.point =
            Point::new(point.line, (point.column + width).min(Column(right - 1)));
        self.grid.cursor.input_needs_wrap = point.column + width >= right;
        true
    }
}

#[cfg(test)]
#[path = "../tests/term/grapheme.rs"]
mod tests;
