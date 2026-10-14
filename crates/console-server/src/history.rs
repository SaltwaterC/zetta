// Protocol/native call sequences derived from Microsoft Terminal.
// Copyright (c) Microsoft Corporation. Licensed under the MIT license;
// see LICENSE-MIT-Microsoft for the retained permission notice.
//! Command history as OpenConsole keeps it (history.cpp): one list per
//! attached application, recycled by executable name when the process that
//! owned it exits, at most `buffers` lists of at most `size` commands each.
//! Indices, the reset flag and the wrap-around rules follow the original,
//! because doskey, F7 and the history APIs all expose them.
use std::collections::VecDeque;

pub type Index = i32;

#[derive(Clone, Debug, Default)]
pub struct CommandHistory {
    commands: Vec<Vec<u16>>,
    max_commands: Index,
    application: Vec<u16>,
    /// The client token this list belongs to, while that client lives.
    owner: Option<usize>,
    /// CLE_RESET: the next "previous" retrieves the last displayed command
    /// itself rather than the one before it.
    reset: bool,
    pub last_displayed: Index,
}

pub enum Direction {
    Previous,
    Next,
}

fn same_name(a: &[u16], b: &[u16]) -> bool {
    String::from_utf16_lossy(a).to_lowercase() == String::from_utf16_lossy(b).to_lowercase()
}

impl CommandHistory {
    pub fn len(&self) -> Index {
        self.commands.len() as Index
    }
    pub fn commands(&self) -> &[Vec<u16>] {
        &self.commands
    }
    fn reset_position(&mut self) {
        self.last_displayed = self.len() - 1;
        self.reset = true;
    }
    pub fn add(&mut self, command: &[u16], suppress_duplicates: bool) {
        if self.max_commands == 0 || command.is_empty() {
            return;
        }
        if self.commands.last().is_none_or(|last| last != command) {
            let mut reuse = None;
            if suppress_duplicates
                && let Some(index) = self.find(command, self.last_displayed, true, false)
            {
                reuse = Some(self.remove(index));
            }
            if self.len() == self.max_commands {
                self.commands.remove(0);
                self.last_displayed -= 1;
            }
            self.commands
                .push(reuse.unwrap_or_else(|| command.to_vec()));
            if self.last_displayed == -1 || self.nth(self.last_displayed) != command {
                self.reset_position();
            }
        }
        self.reset = true;
    }
    pub fn nth(&self, index: Index) -> &[u16] {
        if (0..self.len()).contains(&index) {
            &self.commands[index as usize]
        } else {
            &[]
        }
    }
    pub fn retrieve(&mut self, direction: Direction) -> Vec<u16> {
        match direction {
            Direction::Previous if self.reset => self.reset = false,
            Direction::Previous => self.last_displayed -= 1,
            Direction::Next => self.last_displayed += 1,
        }
        self.retrieve_nth(self.last_displayed)
    }
    pub fn retrieve_nth(&mut self, index: Index) -> Vec<u16> {
        if self.commands.is_empty() {
            self.last_displayed = 0;
            return Vec::new();
        }
        self.last_displayed = index.clamp(0, self.len() - 1);
        self.commands[self.last_displayed as usize].clone()
    }
    pub fn last_command(&self) -> &[u16] {
        self.nth(self.last_displayed)
    }
    pub fn empty(&mut self) {
        self.commands.clear();
        self.last_displayed = -1;
        self.reset = true;
    }
    pub fn at_first(&self) -> bool {
        if self.reset {
            return false;
        }
        let mut index = self.last_displayed - 1;
        if index == -1 {
            index = self.len() - 1;
        }
        index == self.len() - 1
    }
    pub fn at_last(&self) -> bool {
        self.last_displayed == self.len() - 1
    }
    pub fn realloc(&mut self, commands: Index) {
        if self.max_commands == commands {
            return;
        }
        self.commands.truncate(commands.max(0) as usize);
        self.reset = true;
        self.last_displayed = self.len() - 1;
        self.max_commands = commands;
    }
    pub fn remove(&mut self, index: Index) -> Vec<u16> {
        if !(0..self.len()).contains(&index) {
            return Vec::new();
        }
        let command = self.commands.remove(index as usize);
        if self.last_displayed == index {
            self.last_displayed = -1;
        } else if self.last_displayed > index {
            // _Dec: wraps at the capacity, as the original does.
            if self.last_displayed <= 0 {
                self.last_displayed = self.max_commands;
            }
            self.last_displayed -= 1;
        }
        command
    }
    pub fn swap(&mut self, a: Index, b: Index) {
        let count = self.len();
        if a != b && (0..count).contains(&a) && (0..count).contains(&b) {
            self.commands.swap(a as usize, b as usize);
        }
    }
    fn previous(&self, index: &mut Index) {
        if *index <= 0 {
            *index = self.len();
        }
        *index -= 1;
    }
    /// FindMatchingCommand: searches backwards from `start` for a command
    /// beginning with `prefix` (or equal to it with `exact`).
    pub fn find(
        &mut self,
        prefix: &[u16],
        start: Index,
        exact: bool,
        just_looking: bool,
    ) -> Option<Index> {
        let mut index = start;
        if self.commands.is_empty() {
            return None;
        }
        if !just_looking && self.reset {
            self.reset = false;
        } else {
            self.previous(&mut index);
        }
        if prefix.is_empty() {
            return Some(index);
        }
        for _ in 0..self.commands.len() {
            let stored = &self.commands[index as usize];
            let length_ok = if exact {
                stored.len() == prefix.len()
            } else {
                prefix.len() <= stored.len()
            };
            if length_ok && stored.starts_with(prefix) {
                return Some(index);
            }
            self.previous(&mut index);
        }
        None
    }
}

/// HISTORY_NO_DUP_FLAG in CONSOLE_HISTORY_INFO.
pub const NO_DUPLICATES: u32 = 0x1;

/// All lists of one console, most recently allocated first.
#[derive(Debug)]
pub struct Histories {
    lists: VecDeque<CommandHistory>,
    pub size: u32,
    pub buffers: u32,
    pub flags: u32,
}
impl Default for Histories {
    fn default() -> Self {
        Self {
            lists: VecDeque::new(),
            size: 50,
            buffers: 4,
            flags: 0,
        }
    }
}
impl Histories {
    pub fn find(&mut self, owner: usize) -> Option<&mut CommandHistory> {
        self.lists.iter_mut().find(|list| list.owner == Some(owner))
    }
    pub fn by_application(&mut self, application: &[u16]) -> Option<&mut CommandHistory> {
        self.lists
            .iter_mut()
            .find(|list| list.owner.is_some() && same_name(&list.application, application))
    }
    /// s_Allocate: the free list of the same application if there is one,
    /// else a new list while under the limit, else the free list that is
    /// emptiest, which is then cleared for its new application.
    pub fn allocate(&mut self, application: &[u16], owner: usize) {
        let same = self
            .lists
            .iter()
            .position(|list| list.owner.is_none() && same_name(&list.application, application));
        if same.is_none() && self.lists.len() < self.buffers as usize {
            self.lists.push_front(CommandHistory {
                application: application.to_vec(),
                owner: Some(owner),
                last_displayed: -1,
                max_commands: self.size as Index,
                ..Default::default()
            });
            return;
        }
        let candidate = same.or_else(|| {
            let mut best: Option<usize> = None;
            for (index, list) in self.lists.iter().enumerate() {
                if list.owner.is_none()
                    && (list.commands.is_empty()
                        || best.is_none_or(|best| !self.lists[best].commands.is_empty()))
                {
                    best = Some(index);
                }
            }
            best
        });
        if let Some(index) = candidate {
            let mut list = self.lists.remove(index).unwrap();
            if same.is_none() {
                list.commands.clear();
                list.last_displayed = -1;
                list.application = application.to_vec();
            }
            list.owner = Some(owner);
            self.lists.push_front(list);
        }
    }
    /// s_Free: the list stays, available to the next process of its name.
    pub fn free(&mut self, owner: usize) {
        if let Some(list) = self.find(owner) {
            list.owner = None;
        }
    }
    /// s_ResizeAll.
    pub fn resize_all(&mut self, size: u32) {
        self.size = size;
        for list in &mut self.lists {
            list.realloc(size as Index);
        }
    }
    /// s_ReallocExeToFront.
    pub fn realloc_application(&mut self, application: &[u16], commands: u32) {
        if let Some(index) = self
            .lists
            .iter()
            .position(|list| list.owner.is_some() && same_name(&list.application, application))
        {
            let mut list = self.lists.remove(index).unwrap();
            list.realloc(commands as Index);
            self.lists.push_front(list);
        }
    }
}

#[cfg(test)]
#[path = "tests/history.rs"]
mod tests;
