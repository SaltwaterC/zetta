//! Taking a lock whose previous holder panicked.
//!
//! A poisoned `std::sync` lock still holds a usable value; poisoning only
//! records that a thread panicked while holding it. Almost every lock in Zetta
//! guards state a panic cannot leave half-updated, so the rule (AGENTS.md, rule
//! 6) is to recover the value rather than to propagate one thread's panic to
//! every other thread that touches the lock. A lock that guards an invariant a
//! panic *can* break is the exception: it is taken with `unwrap`, and says why
//! at the lock.
//!
//! [`RecoverPoison`] is that recovery, spelled once. It is implemented for
//! [`LockResult`] rather than for each lock type, so the same call follows
//! `Mutex::lock`, `RwLock::read` and `write`, `Mutex::get_mut`, and every
//! `Condvar` wait.

use std::sync::{LockResult, PoisonError};

/// Recovers the value from a lock result, whether or not the lock was
/// poisoned.
pub trait RecoverPoison<T> {
    /// The guard (or value) the lock call produced, taken out of the
    /// `PoisonError` when a previous holder panicked.
    fn recover_poison(self) -> T;
}

impl<T> RecoverPoison<T> for LockResult<T> {
    #[inline]
    fn recover_poison(self) -> T {
        self.unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
#[path = "tests/lib.rs"]
mod tests;
