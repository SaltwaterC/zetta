//! Zetta: generates the grapheme join tables `src/term/grapheme.rs` reads (see
//! its `JoinClasses`). They are derived from the segmenter the crate links, so
//! they cannot drift from what segmentation does, and generated here because
//! deriving them at run time took 17 ms of the first output that needed them.

use std::fmt::Write as _;
use std::{env, fs, path::Path};

use unicode_segmentation::UnicodeSegmentation;

/// Planes 0 and 1; `grapheme.rs` says why nothing above them needs a table.
const TABLED_SCALARS: u32 = 0x2_0000;

fn one_cluster(scalars: &[char]) -> bool {
    let text = scalars.iter().collect::<String>();
    text.graphemes(true).next().is_some_and(|cluster| cluster.len() == text.len())
}

/// `[previous, next, sequence, takes_marks]`, as `JoinClasses` and `JoinBlock`
/// define them. Each asks the segmenter with an unremarkable letter, a combining
/// mark, or a cluster the class completes, as the other side.
fn classes(c: char) -> [bool; 4] {
    // What can follow a leading jamo or either kind of syllable, which is
    // every Hangul class, and since Unicode 16 Kirat Rai's vowel signs too.
    let hangul =
        ['\u{1100}', '\u{ac00}', '\u{ac01}'].iter().any(|&before| one_cluster(&[before, c]));
    let regional = matches!(c, '\u{1f1e6}'..='\u{1f1ff}');
    let sequence = hangul
        || regional
        || one_cluster(&['\u{915}', '\u{94d}', c])
        || one_cluster(&['\u{1f600}', '\u{200d}', c]);
    [one_cluster(&['a', c]), one_cluster(&[c, 'a']), sequence, one_cluster(&[c, '\u{300}'])]
}

fn main() {
    println!("cargo::rerun-if-changed=build.rs");

    // Most blocks have no joining scalar at all, so each distinct block is
    // stored once and the index maps every block to its copy.
    let mut blocks: Vec<[[u64; 4]; 4]> = Vec::new();
    let mut index = Vec::new();
    for block in 0..TABLED_SCALARS >> 8 {
        let mut bits = [[0u64; 4]; 4];
        for code in block << 8..(block + 1) << 8 {
            let Some(c) = char::from_u32(code) else {
                continue;
            };
            let (word, bit) = (((code >> 6) & 3) as usize, code & 63);
            for (class, member) in classes(c).into_iter().enumerate() {
                bits[class][word] |= u64::from(member) << bit;
            }
        }
        let position = blocks.iter().position(|known| *known == bits).unwrap_or_else(|| {
            blocks.push(bits);
            blocks.len() - 1
        });
        index.push(u8::try_from(position).expect("at most 256 distinct blocks"));
    }

    let mut source = String::new();
    writeln!(source, "const TABLED_SCALARS: u32 = {TABLED_SCALARS:#x};").unwrap();
    writeln!(source, "static JOIN_BLOCK_INDEX: [u8; {}] = {index:?};", index.len()).unwrap();
    writeln!(source, "static JOIN_BLOCKS: [JoinBlock; {}] = [", blocks.len()).unwrap();
    for [previous, next, sequence, takes_marks] in &blocks {
        writeln!(
            source,
            "    JoinBlock {{ previous: {previous:?}, next: {next:?}, sequence: {sequence:?}, \
             takes_marks: {takes_marks:?} }},"
        )
        .unwrap();
    }
    writeln!(source, "];").unwrap();
    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("grapheme_join_tables.rs");
    fs::write(out, source).unwrap();
}
