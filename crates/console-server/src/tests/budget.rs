use super::*;

#[test]
fn a_charge_over_its_budget_fails_and_takes_nothing() {
    let budget = Arc::new(Budget::new(100, None));
    let held = budget.charge(60).unwrap();
    assert_eq!(
        budget.charge(41).err().map(|error| error.kind()),
        Some(io::ErrorKind::OutOfMemory)
    );
    assert_eq!(budget.used(), 60);
    let mut rest = budget.charge(40).unwrap();
    assert!(rest.grow(1).is_err());
    assert_eq!((budget.used(), rest.bytes()), (100, 40));
    drop(held);
    assert_eq!(budget.used(), 40);
}

#[test]
fn a_session_cannot_take_what_the_process_has_given_to_others() {
    let process = Arc::new(Budget::new(100, None));
    let one = Arc::new(Budget::new(80, Some(Arc::clone(&process))));
    let two = Arc::new(Budget::new(80, Some(Arc::clone(&process))));
    let held = one.charge(70).unwrap();
    // Within the session's own limit, but past the process's.
    assert!(two.charge(31).is_err());
    assert_eq!((two.used(), process.used()), (0, 70));
    let mut taken = two.charge(30).unwrap();
    assert_eq!(process.used(), 100);
    taken.shrink(10);
    assert_eq!((two.used(), process.used()), (20, 90));
    drop((held, taken));
    assert_eq!((one.used(), two.used(), process.used()), (0, 0, 0));
}

#[test]
fn a_charge_grows_to_at_least_what_it_is_asked_and_never_shrinks_by_it() {
    let budget = Arc::new(Budget::new(100, None));
    let mut charge = budget.empty();
    charge.at_least(30).unwrap();
    charge.at_least(10).unwrap();
    assert_eq!(budget.used(), 30);
    charge.at_least(50).unwrap();
    assert_eq!(budget.used(), 50);
    charge.shrink(usize::MAX);
    assert_eq!((charge.bytes(), budget.used()), (0, 0));
}

#[test]
fn a_charged_vector_is_paid_for_by_its_capacity_until_it_is_dropped() {
    let budget = Arc::new(Budget::new(100, None));
    let vec = budget.vec(7_u32, 10).unwrap();
    assert_eq!(vec[..], [7; 10]);
    assert_eq!(budget.used(), 40);
    assert!(budget.with_capacity::<u32>(16).is_err());
    assert!(budget.with_capacity::<u64>(usize::MAX).is_err());
    drop(vec);
    assert_eq!(budget.used(), 0);
}
