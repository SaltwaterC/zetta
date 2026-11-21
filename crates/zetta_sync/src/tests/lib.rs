use super::*;

use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread;
use std::time::Duration;

fn poisoned_mutex() -> Arc<Mutex<u32>> {
    let mutex = Arc::new(Mutex::new(7));
    let held = Arc::clone(&mutex);
    let _ = thread::spawn(move || {
        let _guard = held.lock().unwrap();
        panic!("poison the lock");
    })
    .join();
    assert!(mutex.is_poisoned());
    mutex
}

#[test]
fn a_poisoned_mutex_still_yields_its_value() {
    let mutex = poisoned_mutex();
    *mutex.lock().recover_poison() += 1;
    assert_eq!(*mutex.lock().recover_poison(), 8);
}

#[test]
fn an_unpoisoned_lock_is_taken_as_usual() {
    let lock = RwLock::new(String::from("value"));
    lock.write().recover_poison().push('!');
    assert_eq!(*lock.read().recover_poison(), "value!");
    let mut mutex = Mutex::new(1);
    *mutex.get_mut().recover_poison() = 2;
    assert_eq!(mutex.into_inner().recover_poison(), 2);
}

#[test]
fn a_condvar_wait_on_a_poisoned_mutex_returns_the_guard() {
    let mutex = poisoned_mutex();
    let condvar = Condvar::new();
    let guard = mutex.lock().recover_poison();
    let (guard, timeout) = condvar
        .wait_timeout(guard, Duration::from_millis(1))
        .recover_poison();
    assert!(timeout.timed_out());
    assert_eq!(*guard, 7);
}
