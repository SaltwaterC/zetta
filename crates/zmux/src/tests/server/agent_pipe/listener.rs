use super::*;

/// A name nobody holds, so tests never touch a real pane's pipe.
fn test_name() -> PathBuf {
    crate::paths::new_pane_forwarded_agent_pipe(u64::MAX).unwrap()
}

fn is_free(name: &Path) -> bool {
    match create_instance(&wide(name), &user_only_descriptor().unwrap(), true) {
        Ok(handle) => {
            let _ = unsafe { CloseHandle(handle) };
            true
        }
        Err(_) => false,
    }
}

/// What another account would do: create the name first, with a DACL that
/// lets anybody add an instance and so join it.
fn squat(name: &Path) -> HANDLE {
    create_instance(&wide(name), "D:(A;;GA;;;WD)", true).expect("squatting the name")
}

#[test]
fn a_name_somebody_already_holds_is_refused_rather_than_joined() {
    let pane_id = u64::MAX - 1;
    let name = test_name();
    let squatter = squat(&name);

    assert!(
        start(pane_id, name, FirstInstance::Create, None).is_none(),
        "the daemon joined a pipe somebody else created"
    );
    assert!(
        !LISTENERS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(&pane_id)
    );

    let _ = unsafe { CloseHandle(squatter) };
}

#[test]
fn a_served_name_stays_held_across_connections() {
    let pane_id = u64::MAX - 2;
    let name = start(pane_id, test_name(), FirstInstance::Create, None).expect("serving");
    assert!(!is_free(&name));

    for _ in 0..20 {
        let client = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name)
            .expect("connecting");
        drop(client);
        assert!(!is_free(&name), "the name was left without an instance");
    }

    stop(pane_id);
}

#[test]
fn an_adopted_instance_must_be_private_to_this_account() {
    let descriptor = user_only_descriptor().unwrap();
    let name = test_name();
    let permissive = squat(&name);
    assert!(
        first_instance(
            &wide(&name),
            &descriptor,
            FirstInstance::Adopted(permissive.0 as usize),
        )
        .is_err(),
        "an instance anybody could join was adopted"
    );

    let name = test_name();
    let private = create_instance(&wide(&name), &descriptor, true).unwrap();
    let adopted = first_instance(
        &wide(&name),
        &descriptor,
        FirstInstance::Adopted(private.0 as usize),
    )
    .expect("this daemon's own instance is adopted");
    let _ = unsafe { CloseHandle(adopted) };
}

#[test]
fn a_handed_over_instance_keeps_the_name_after_the_listener_stops() {
    // A listener without its thread, so the test decides when the old daemon's
    // own instance goes, rather than racing `stop`'s wake-up connection.
    let pane_id = u64::MAX - 3;
    let name = test_name();
    let old_instance = create_instance(&wide(&name), &user_only_descriptor().unwrap(), true)
        .expect("the old daemon's instance");
    LISTENERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(
            pane_id,
            PaneListener {
                name: name.clone(),
                stop: Arc::new(AtomicBool::new(false)),
            },
        );

    let handed_over = hand_over(pane_id, unsafe { GetCurrentProcess() });
    LISTENERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&pane_id);
    let (handed_name, instance) = handed_over.expect("the pane's name is handed over");
    let instance = instance.expect("an instance is handed over with it");
    assert_eq!(handed_name, name);

    let _ = unsafe { CloseHandle(old_instance) };
    assert!(!is_free(&name), "the name was free between the two daemons");

    let _ = unsafe { CloseHandle(HANDLE(instance as usize as _)) };
    assert!(is_free(&name));
}
