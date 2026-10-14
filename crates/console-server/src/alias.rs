// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! Console aliases, which doskey manages (alias.cpp): per-executable maps
//! with case-insensitive keys, expanded when a cooked line is entered.
use std::collections::BTreeMap;

/// Alias source to target, both as typed except for lowercased sources.
type AliasMap = BTreeMap<Vec<u16>, Vec<u16>>;

/// Keys are lowercased, as AddConsoleAliasWImpl stores them; executable
/// names are compared case-insensitively too, in insertion order.
#[derive(Debug, Default)]
pub struct Aliases {
    exes: Vec<(Vec<u16>, AliasMap)>,
}

fn lower(text: &[u16]) -> Vec<u16> {
    String::from_utf16_lossy(text)
        .to_lowercase()
        .encode_utf16()
        .collect()
}

impl Aliases {
    fn exe(&self, exe: &[u16]) -> Option<&AliasMap> {
        let key = lower(exe);
        self.exes
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, map)| map)
    }
    /// AddConsoleAlias: an empty target removes the alias.
    pub fn add(&mut self, source: &[u16], target: &[u16], exe: &[u16]) -> bool {
        if source.is_empty() {
            return false;
        }
        let exe = lower(exe);
        let source = lower(source);
        let position = self.exes.iter().position(|(name, _)| *name == exe);
        if target.is_empty() {
            if let Some(index) = position {
                self.exes[index].1.remove(&source);
            }
            return true;
        }
        let index = position.unwrap_or_else(|| {
            self.exes.push((exe, BTreeMap::new()));
            self.exes.len() - 1
        });
        self.exes[index].1.insert(source, target.to_vec());
        true
    }
    pub fn get(&self, source: &[u16], exe: &[u16]) -> Option<&[u16]> {
        self.exe(exe)?
            .get(&lower(source))
            .map(Vec::as_slice)
            .filter(|target| !target.is_empty())
    }
    /// GetConsoleAliases: "source=target\0" for each alias.
    pub fn list(&self, exe: &[u16]) -> Vec<u16> {
        let mut out = Vec::new();
        if let Some(map) = self.exe(exe) {
            for (source, target) in map {
                out.extend_from_slice(source);
                out.push(u16::from(b'='));
                out.extend_from_slice(target);
                out.push(0);
            }
        }
        out
    }
    /// GetConsoleAliasExes: "exe\0" for each executable with aliases.
    pub fn exes(&self) -> Vec<u16> {
        let mut out = Vec::new();
        for (exe, _) in &self.exes {
            out.extend_from_slice(exe);
            out.push(0);
        }
        out
    }
    /// Alt+F10 in cmd clears cmd.exe's aliases.
    pub fn clear_cmd(&mut self) {
        let cmd = lower(&"cmd.exe".encode_utf16().collect::<Vec<_>>());
        if let Some((_, map)) = self.exes.iter_mut().find(|(name, _)| *name == cmd) {
            map.clear();
        }
    }
    /// s_MatchAndCopyAlias: expands the alias the line starts with, if any,
    /// into CRLF-terminated lines, returning them and how many there are.
    /// $1-$9, $* (everything after the alias), $L $G $B (< > |) and $T (a
    /// line break) are macro escapes; anything else after $ stays as is.
    pub fn expand(&self, line: &[u16], exe: &[u16]) -> Option<(Vec<u16>, usize)> {
        let map = self.exe(exe)?;
        if map.is_empty() {
            return None;
        }
        let space = u16::from(b' ');
        let mut args: Vec<(usize, &[u16])> = Vec::new();
        let mut start = 0;
        while start < line.len() && args.len() < 10 {
            let end = line[start..]
                .iter()
                .position(|&unit| unit == space)
                .map_or(line.len(), |offset| start + offset);
            if end == start {
                break;
            }
            args.push((start, &line[start..end]));
            match line[end..].iter().position(|&unit| unit != space) {
                Some(offset) => start = end + offset,
                None => break,
            }
        }
        let (_, name) = *args.first()?;
        let target = map.get(&lower(name)).filter(|target| !target.is_empty())?;
        let mut out = Vec::new();
        let mut lines = 0;
        let mut units = target.iter().copied();
        while let Some(unit) = units.next() {
            if unit != u16::from(b'$') {
                out.push(unit);
                continue;
            }
            let Some(next) = units.next() else {
                out.push(unit);
                continue;
            };
            let lowered = if (u16::from(b'A')..=u16::from(b'Z')).contains(&next) {
                next + 32
            } else {
                next
            };
            match lowered {
                digit if (u16::from(b'1')..=u16::from(b'9')).contains(&digit) => {
                    let index = usize::from(digit - u16::from(b'0'));
                    if let Some((_, arg)) = args.get(index) {
                        out.extend_from_slice(arg);
                    }
                }
                star if star == u16::from(b'*') => {
                    if let Some(&(offset, _)) = args.get(1) {
                        out.extend_from_slice(&line[offset..]);
                    }
                }
                l if l == u16::from(b'l') => out.push(u16::from(b'<')),
                g if g == u16::from(b'g') => out.push(u16::from(b'>')),
                b if b == u16::from(b'b') => out.push(u16::from(b'|')),
                t if t == u16::from(b't') => {
                    out.extend_from_slice(&[13, 10]);
                    lines += 1;
                }
                _ => out.extend_from_slice(&[unit, next]),
            }
        }
        out.extend_from_slice(&[13, 10]);
        Some((out, lines + 1))
    }
}

#[cfg(test)]
#[path = "tests/alias.rs"]
mod tests;
