//! Compact storage for sealed history, and the cache that reads it back.
//!
//! A row of history costs a full `Row<Cell>` (24 bytes a cell) while it is a `Row`, which made
//! unlimited scrollback grow about 28 bytes for every byte of plain output; on a stream of
//! distinct lines the allocations and page faults behind that growth cost more than parsing.
//! `Storage` therefore encodes a sealed chunk that is not one row repeated into [`CompactRows`]:
//! each row is runs of attributes, each followed by that run's characters as UTF-8, and a
//! trailing fill cell repeated to the row's end. The encoded chunk's rows go back to the grid as the next lines scrolled
//! in, so a stream of distinct output stops allocating altogether.
//!
//! Readers borrow history rows as `&Row` for as long as they borrow the grid, and hold several
//! at once, so a compact row is read through [`DecodedChunks`]: the whole chunk is decoded once
//! into rows whose address stays fixed until the storage is next borrowed mutably. That is the
//! only time decoded chunks are released, which keeps every handed-out reference valid. A reader
//! that wants only the characters, such as scrollback search, reads the encoding directly through
//! [`super::text`] instead, and never decodes.

use std::collections::HashMap;
use std::fmt;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;

use parking_lot::Mutex;

use super::text::{RowText, SPACERS, has_zerowidth};
use super::{GridCell, Row};
use crate::index::{Column, Line, Point};
use crate::term::cell::{Cell, Flags};

/// Above this many distinct attributes a chunk stays as rows. Looking them up is linear, and
/// output colourful enough to need more is too rare to be worth a hash table on every run.
const MAX_ATTRIBUTES: usize = 64;

/// A sealed chunk of history rows, newest first, in compact form.
#[derive(Clone)]
pub(super) struct CompactRows<T> {
    bytes: Box<[u8]>,
    /// Where each row starts in `bytes`.
    offsets: Box<[u32]>,
    /// Rows still retained; the oldest past it were dropped from history.
    len: usize,
    /// Distinct attributes, as cells whose character is ignored.
    attributes: Box<[T]>,
    /// Column resizes applied since encoding, without decoding: a row keeps at most
    /// `kept_columns` of its encoded cells and is then padded with default cells to `columns`.
    /// This is exactly what shrinking and growing each row in turn would have left.
    resized: Option<Resize>,
    /// Decoding needs `T: GridCell + Default`, which readers of the grid do not promise.
    decode: fn(&Self, usize) -> Row<T>,
}

#[derive(Clone, Copy, Debug)]
struct Resize {
    kept_columns: usize,
    columns: usize,
}

impl<T> fmt::Debug for CompactRows<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompactRows")
            .field("len", &self.len)
            .field("bytes", &self.bytes.len())
            .field("attributes", &self.attributes.len())
            .field("resized", &self.resized)
            .finish()
    }
}

impl<T> CompactRows<T> {
    pub(super) fn len(&self) -> usize {
        self.len
    }

    /// Forget the `count` oldest rows.
    pub(super) fn truncate_oldest(&mut self, count: usize) {
        debug_assert!(count <= self.len);
        self.len -= count;
    }

    pub(super) fn row(&self, index: usize) -> Row<T> {
        debug_assert!(index < self.len);
        (self.decode)(self, index)
    }

    pub(super) fn rows(&self) -> Vec<Row<T>> {
        (0..self.len).map(|index| self.row(index)).collect()
    }

    /// A value identifying row `index` for as long as this chunk exists, distinct from every
    /// other row's: an address inside `bytes`, which holds at least one byte per row.
    pub(super) fn row_id(&self, index: usize) -> usize {
        debug_assert!(index < self.offsets.len() && self.offsets.len() <= self.bytes.len());
        self.bytes.as_ptr() as usize + index
    }

    /// Resize every row to `columns` the way `Row::grow` and `Row::shrink` would.
    pub(super) fn resize_columns(&mut self, columns: usize) {
        let kept_columns = self.resized.map_or(columns, |resize| resize.kept_columns.min(columns));
        self.resized = Some(Resize { kept_columns, columns });
    }
}

impl<T: GridCell + Default + PartialEq> CompactRows<T> {
    /// Encode `rows`, or `None` when they are not worth or able to be encoded.
    pub(super) fn encode(rows: &[Row<T>]) -> Option<Self> {
        let mut encoder = Encoder {
            // Plain text is about one byte a cell; a little headroom avoids regrowing.
            bytes: Vec::with_capacity(rows.iter().map(|row| row.len() + 8).sum()),
            attributes: Vec::new(),
            last_attribute: 0,
        };
        let mut offsets = Vec::with_capacity(rows.len());
        for row in rows {
            offsets.push(u32::try_from(encoder.bytes.len()).ok()?);
            encoder.row(&row[..])?;
        }

        Some(Self {
            bytes: encoder.bytes.into_boxed_slice(),
            offsets: offsets.into_boxed_slice(),
            len: rows.len(),
            attributes: encoder.attributes.into_boxed_slice(),
            resized: None,
            decode: decode_row::<T>,
        })
    }
}

struct Encoder<T> {
    bytes: Vec<u8>,
    attributes: Vec<T>,
    last_attribute: usize,
}

impl<T: GridCell + PartialEq> Encoder<T> {
    fn row(&mut self, cells: &[T]) -> Option<()> {
        let fill = cells.last()?;
        let mut explicit = cells.len() - 1;
        while explicit > 0 && cells[explicit - 1] == *fill {
            explicit -= 1;
        }
        write_varint(&mut self.bytes, cells.len());
        write_varint(&mut self.bytes, explicit);

        let mut start = 0;
        while start < explicit {
            let attribute = self.attribute(&cells[start])?;
            write_varint(&mut self.bytes, attribute);
            // The run's length is only known once its characters are written; rows are at most
            // `u16::MAX` cells wide, so it fits a fixed slot written afterwards.
            let length_at = self.bytes.len();
            self.bytes.extend_from_slice(&[0; 2]);
            let template = &self.attributes[attribute];
            let mut end = start;
            while end < explicit && cells[end].same_archive_attributes(template) {
                write_char(&mut self.bytes, cells[end].archive_char()?);
                end += 1;
            }
            let run = u16::try_from(end - start).ok()?;
            self.bytes[length_at..length_at + 2].copy_from_slice(&run.to_le_bytes());
            start = end;
        }

        let attribute = self.attribute(fill)?;
        write_varint(&mut self.bytes, attribute);
        write_char(&mut self.bytes, fill.archive_char()?);
        Some(())
    }

    /// The index of `cell`'s attributes, added if new.
    fn attribute(&mut self, cell: &T) -> Option<usize> {
        if self
            .attributes
            .get(self.last_attribute)
            .is_some_and(|attributes| attributes.same_archive_attributes(cell))
        {
            return Some(self.last_attribute);
        }
        let index = match self
            .attributes
            .iter()
            .position(|attributes| attributes.same_archive_attributes(cell))
        {
            Some(index) => index,
            None if self.attributes.len() < MAX_ATTRIBUTES => {
                cell.archive_char()?;
                self.attributes.push(cell.clone());
                self.attributes.len() - 1
            },
            None => return None,
        };
        self.last_attribute = index;
        Some(index)
    }
}

fn decode_row<T: GridCell + Default>(rows: &CompactRows<T>, index: usize) -> Row<T> {
    let mut reader = Reader { bytes: &rows.bytes, position: rows.offsets[index] as usize };
    let len = reader.varint();
    let explicit = reader.varint();
    let (kept, columns) =
        rows.resized.map_or((len, len), |resize| (resize.kept_columns.min(len), resize.columns));

    let mut cells = Vec::with_capacity(columns);
    let runs = explicit.min(kept);
    while cells.len() < runs {
        let attributes = &rows.attributes[reader.varint()];
        let run = reader.run_length();
        for _ in 0..run {
            let c = reader.char();
            if cells.len() < runs {
                cells.push(attributes.with_archive_char(c));
            }
        }
    }
    // Every run was read when any of the fill is kept, so the fill is next.
    if kept > explicit {
        let fill = rows.attributes[reader.varint()].with_archive_char(reader.char());
        cells.resize(kept, fill);
    }
    cells.resize_with(columns, T::default);

    // Decoded rows are only read, or handed back to the grid as whole rows; marking every cell
    // occupied is always a correct bound.
    let occupied = cells.len();
    Row::from_vec(cells, occupied)
}

/// The fill's attribute index is the varint written just before the row's last character, which
/// is only readable backwards because it is always one byte.
const _: () = assert!(MAX_ATTRIBUTES < 0x80);

/// Reading rows as text; see [`super::text`]. Each mirrors [`decode_row`] without building cells.
impl CompactRows<Cell> {
    /// A reader past row `index`'s header, the row's explicit cell count, and how many of its
    /// cells are kept out of how many columns it now has.
    fn header(&self, index: usize) -> (Reader<'_>, usize, usize, usize) {
        debug_assert!(index < self.len);
        let mut reader = Reader { bytes: &self.bytes, position: self.offsets[index] as usize };
        let len = reader.varint();
        let explicit = reader.varint();
        let (kept, columns) = self
            .resized
            .map_or((len, len), |resize| (resize.kept_columns.min(len), resize.columns));
        (reader, explicit, kept, columns)
    }

    /// Append row `index`, as `line`, to `text`.
    pub(super) fn append_text(&self, index: usize, line: Line, text: &mut RowText) {
        let (mut reader, explicit, kept, columns) = self.header(index);
        // The text ends after the last occupied cell, unless the row wraps.
        let mut occupied_end = text.len();
        let mut last_flags = Flags::empty();
        let runs = explicit.min(kept);
        let mut column = 0;
        while column < runs {
            let attributes = &self.attributes[reader.varint()];
            let run = reader.run_length().min(runs - column);
            let spacer = attributes.flags.intersects(SPACERS);
            let point = Point::new(line, Column(column));
            if let Some(ascii) = reader.ascii(run) {
                let start = text.len();
                if !spacer {
                    text.push_ascii(ascii, point);
                }
                if has_zerowidth(attributes) {
                    occupied_end = text.len();
                } else if let Some(last) = ascii.bytes().rposition(|byte| byte != b' ') {
                    occupied_end = if spacer { text.len() } else { start + last + 1 };
                }
            } else {
                for offset in 0..run {
                    let c = reader.char();
                    if !spacer {
                        text.push_char(c, Point::new(line, point.column + offset));
                    }
                    if c != ' ' || has_zerowidth(attributes) {
                        occupied_end = text.len();
                    }
                }
            }
            last_flags = attributes.flags;
            column += run;
        }

        let fill = (kept > explicit).then(|| {
            let attributes = &self.attributes[reader.varint()];
            last_flags = attributes.flags;
            (attributes, reader.char())
        });
        // A row narrower than its columns ends in default cells, which never wrap.
        let wraps = columns == kept && last_flags.contains(Flags::WRAPLINE);
        if let Some((attributes, c)) = fill
            && (wraps || c != ' ' || has_zerowidth(attributes))
        {
            if !attributes.flags.intersects(SPACERS) {
                text.push_repeated(c, kept - explicit, Point::new(line, Column(explicit)));
            }
            occupied_end = text.len();
        }
        if wraps {
            occupied_end = text.len();
        }
        text.truncate(occupied_end);
    }

    /// Whether row `index`'s last cell carries [`Flags::WRAPLINE`].
    pub(super) fn wraps(&self, index: usize) -> bool {
        let (mut reader, explicit, kept, columns) = self.header(index);
        if columns > kept {
            return false;
        }
        let last = kept - 1;
        let attribute = if last >= explicit {
            self.fill_attribute(index)
        } else {
            // Only a row narrowed below its explicit cells ends inside a run.
            let mut start = 0;
            loop {
                let attribute = reader.varint();
                let run = reader.run_length();
                if last < start + run {
                    break attribute;
                }
                reader.skip_chars(run);
                start += run;
            }
        };
        self.attributes[attribute].flags.contains(Flags::WRAPLINE)
    }

    /// The attribute index of row `index`'s fill, read back from the row's end.
    fn fill_attribute(&self, index: usize) -> usize {
        let end = self.offsets.get(index + 1).map_or(self.bytes.len(), |&end| end as usize);
        let mut fill = end - 1;
        while self.bytes[fill] & 0xc0 == 0x80 {
            fill -= 1;
        }
        usize::from(self.bytes[fill - 1])
    }
}

fn write_varint(bytes: &mut Vec<u8>, mut value: usize) {
    while value >= 0x80 {
        bytes.push(value as u8 | 0x80);
        value >>= 7;
    }
    bytes.push(value as u8);
}

fn write_char(bytes: &mut Vec<u8>, c: char) {
    if c.is_ascii() {
        bytes.push(c as u8);
    } else {
        bytes.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    /// The next `count` characters when they are all ASCII, else `None` without moving.
    #[inline]
    fn ascii(&mut self, count: usize) -> Option<&'a str> {
        let bytes = self.bytes.get(self.position..self.position + count)?;
        if !bytes.is_ascii() {
            return None;
        }
        self.position += count;
        std::str::from_utf8(bytes).ok()
    }

    fn skip_chars(&mut self, count: usize) {
        if self.ascii(count).is_none() {
            for _ in 0..count {
                self.char();
            }
        }
    }

    fn run_length(&mut self) -> usize {
        let length = u16::from_le_bytes([self.bytes[self.position], self.bytes[self.position + 1]]);
        self.position += 2;
        usize::from(length)
    }

    fn varint(&mut self) -> usize {
        let mut value = 0;
        let mut shift = 0;
        loop {
            let byte = self.bytes[self.position];
            self.position += 1;
            value |= usize::from(byte & 0x7f) << shift;
            if byte < 0x80 {
                return value;
            }
            shift += 7;
        }
    }

    /// The next character. The encoder wrote valid UTF-8.
    fn char(&mut self) -> char {
        let first = self.bytes[self.position];
        let width = match first {
            0x00..=0x7f => {
                self.position += 1;
                return char::from(first);
            },
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            _ => 4,
        };
        let encoded = &self.bytes[self.position..self.position + width];
        self.position += width;
        std::str::from_utf8(encoded).ok().and_then(|text| text.chars().next()).unwrap_or('\u{fffd}')
    }
}

/// Compact chunks decoded for reading, owned by one `Storage`.
///
/// Entries are added through a shared borrow and only removed through a mutable one, and each
/// entry's rows live in their own allocation, so a reference handed out stays valid for as long
/// as the storage stays borrowed. An entry also holds its chunk: a chunk cannot be freed and its
/// address reused while it is cached, so a stale entry can never be found for a new chunk.
pub(super) struct DecodedChunks<C, T> {
    entries: Mutex<Entries<C, T>>,
}

/// Every cell read from a compact row looks its chunk up here, mostly the same chunk as the
/// last read, so that is checked before the map.
struct Entries<C, T> {
    by_chunk: HashMap<usize, DecodedChunk<C, T>, BuildHasherDefault<ChunkHasher>>,
    /// The last chunk found, and the address of its rows.
    last: Option<(usize, usize)>,
}

struct DecodedChunk<C, T> {
    _chunk: Arc<C>,
    rows: Box<[Row<T>]>,
}

/// Keys are chunk addresses: distinct, and only needing their alignment bits mixed in.
#[derive(Default)]
struct ChunkHasher(u64);

impl Hasher for ChunkHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0.rotate_left(8) ^ u64::from(byte)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        }
    }

    fn write_usize(&mut self, value: usize) {
        self.0 = (value as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(29);
    }
}

impl<C, T> Default for DecodedChunks<C, T> {
    fn default() -> Self {
        Self { entries: Mutex::new(Entries { by_chunk: HashMap::default(), last: None }) }
    }
}

/// Decoded rows are a cache of the chunks, which a clone shares; the clone decodes its own.
impl<C, T> Clone for DecodedChunks<C, T> {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl<C, T> fmt::Debug for DecodedChunks<C, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecodedChunks")
            .field("chunks", &self.entries.lock().by_chunk.len())
            .finish()
    }
}

impl<C, T> DecodedChunks<C, T> {
    /// Row `index` of `chunk`, which `decode` decodes if it has not been.
    pub(super) fn row(
        &self,
        chunk: &Arc<C>,
        index: usize,
        decode: impl FnOnce() -> Vec<Row<T>>,
    ) -> &Row<T> {
        let key = Arc::as_ptr(chunk) as usize;
        let mut entries = self.entries.lock();
        let rows = match entries.last {
            Some((last, rows)) if last == key => rows as *const Row<T>,
            _ => {
                let rows = entries
                    .by_chunk
                    .entry(key)
                    .or_insert_with(|| DecodedChunk {
                        _chunk: chunk.clone(),
                        rows: decode().into_boxed_slice(),
                    })
                    .rows
                    .as_ptr();
                entries.last = Some((key, rows as usize));
                rows
            },
        };
        drop(entries);
        // SAFETY: `rows` points into a boxed slice owned by an entry, which is only removed
        // through `&mut self` and does not move when the map does, so it outlives `&self`. The
        // chunk's row count bounds `index`, which the caller took from that chunk.
        unsafe { &*rows.add(index) }
    }

    /// Release every decoded chunk.
    #[inline]
    pub(super) fn clear(&mut self) {
        let entries = self.entries.get_mut();
        if entries.last.is_some() {
            *entries = Entries { by_chunk: HashMap::default(), last: None };
        }
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.lock().by_chunk.len()
    }
}

#[cfg(test)]
#[path = "../tests/grid/archive.rs"]
mod tests;
