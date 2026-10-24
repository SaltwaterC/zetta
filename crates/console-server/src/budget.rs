//! The memory console clients can make the server hold.
//!
//! Conhost can let a client size its allocations freely, because when it
//! runs out only that console goes. This server runs inside Zetta and the
//! zmux daemon, where an allocation that fails anywhere aborts every pane
//! the process hosts. So every allocation whose size a client chooses is
//! charged to its session's budget, and every session's to the process's.
//! A request that would go over either fails with NO_MEMORY, and the memory
//! clients hold together stays well short of what the process needs to
//! keep running.
//!
//! What stays (screen buffer rows, the input queue, text kept for replay)
//! holds a [`Charge`] for as long as it exists. A request's own working
//! memory is a [`Charged`] vector, refunded when it is dropped.
use crate::protocol::{out_of_memory, try_with_capacity};
use std::io;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};

/// One console session. A 32766x2048 buffer written in full, or a few
/// thousand buffers of a full-screen application's size.
pub const SESSION_LIMIT: usize = 256 << 20;
/// Every session in the process together: four sessions at their limit.
pub const PROCESS_LIMIT: usize = 1 << 30;

static PROCESS: LazyLock<Arc<Budget>> =
    LazyLock::new(|| Arc::new(Budget::new(PROCESS_LIMIT, None)));

pub struct Budget {
    limit: usize,
    used: AtomicUsize,
    parent: Option<Arc<Budget>>,
}
impl Budget {
    pub fn new(limit: usize, parent: Option<Arc<Budget>>) -> Self {
        Self {
            limit,
            used: AtomicUsize::new(0),
            parent,
        }
    }
    /// A new session's budget, drawn from the process's.
    pub fn session() -> Arc<Self> {
        Arc::new(Self::new(SESSION_LIMIT, Some(Arc::clone(&PROCESS))))
    }
    #[cfg(test)]
    pub fn used(&self) -> usize {
        self.used.load(Ordering::Acquire)
    }
    fn take(&self, bytes: usize) -> bool {
        let taken = self
            .used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|&total| total <= self.limit)
            })
            .is_ok();
        if taken
            && let Some(parent) = &self.parent
            && !parent.take(bytes)
        {
            self.used.fetch_sub(bytes, Ordering::AcqRel);
            return false;
        }
        taken
    }
    fn give(&self, bytes: usize) {
        self.used.fetch_sub(bytes, Ordering::AcqRel);
        if let Some(parent) = &self.parent {
            parent.give(bytes);
        }
    }
    pub fn empty(self: &Arc<Self>) -> Charge {
        Charge {
            budget: Arc::clone(self),
            bytes: 0,
        }
    }
    pub fn charge(self: &Arc<Self>, bytes: usize) -> io::Result<Charge> {
        let mut charge = self.empty();
        charge.grow(bytes)?;
        Ok(charge)
    }
    /// `Vec::with_capacity`, charged.
    pub fn with_capacity<T>(self: &Arc<Self>, capacity: usize) -> io::Result<Charged<T>> {
        let bytes = capacity
            .checked_mul(size_of::<T>())
            .ok_or(io::ErrorKind::OutOfMemory)?;
        let charge = self.charge(bytes)?;
        Ok(Charged {
            vec: try_with_capacity(capacity)?,
            _charge: charge,
        })
    }
    /// `vec![value; len]`, charged.
    pub fn vec<T: Clone>(self: &Arc<Self>, value: T, len: usize) -> io::Result<Charged<T>> {
        let mut vec = self.with_capacity(len)?;
        vec.resize(len, value);
        Ok(vec)
    }
}

/// Bytes held against a budget until this is dropped.
pub struct Charge {
    budget: Arc<Budget>,
    bytes: usize,
}
impl Charge {
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn budget(&self) -> &Arc<Budget> {
        &self.budget
    }
    pub fn grow(&mut self, bytes: usize) -> io::Result<()> {
        if !self.budget.take(bytes) {
            return Err(io::ErrorKind::OutOfMemory.into());
        }
        self.bytes += bytes;
        Ok(())
    }
    pub fn shrink(&mut self, bytes: usize) {
        let bytes = bytes.min(self.bytes);
        self.budget.give(bytes);
        self.bytes -= bytes;
    }
    /// Grows to `bytes` if it holds less; never shrinks.
    pub fn at_least(&mut self, bytes: usize) -> io::Result<()> {
        self.grow(bytes.saturating_sub(self.bytes))
    }
}
impl Drop for Charge {
    fn drop(&mut self) {
        self.budget.give(self.bytes);
    }
}

/// A vector whose capacity is charged until it is dropped. Code that fills
/// one stays within the capacity it asked for, which is all it paid for.
pub struct Charged<T> {
    vec: Vec<T>,
    _charge: Charge,
}
impl<T> Deref for Charged<T> {
    type Target = Vec<T>;
    fn deref(&self) -> &Vec<T> {
        &self.vec
    }
}
impl<T> DerefMut for Charged<T> {
    fn deref_mut(&mut self) -> &mut Vec<T> {
        &mut self.vec
    }
}

/// `VecDeque::try_reserve_exact`, for a table a [`Charge`] already paid for.
pub fn reserve_exact<T>(
    deque: &mut std::collections::VecDeque<T>,
    additional: usize,
) -> io::Result<()> {
    deque.try_reserve_exact(additional).map_err(out_of_memory)
}

#[cfg(test)]
#[path = "tests/budget.rs"]
mod tests;
