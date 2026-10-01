use super::*;

fn profile(name: &str, program: &str, arguments: &[&str]) -> ProfileForm {
    ProfileForm {
        name: TextField::new(name),
        program: TextField::new(program),
        arguments: arguments
            .iter()
            .map(|argument| TextField::new(*argument))
            .collect(),
        ..ProfileForm::blank()
    }
}

/// One file entry for both forms: name and program trimmed, arguments as
/// typed, unset overrides left out. The user configuration's form used to keep
/// surrounding whitespace that the project form trimmed.
#[test]
fn a_profile_is_written_as_one_entry_shape() {
    let mut form = profile(" Work ", " /bin/sh ", &["-c", "echo a,b", ""]);
    form.icon = Some(ProfileIcon::Fish);

    assert_eq!(
        form.to_entry(),
        json!({
            "name": "Work",
            "program": "/bin/sh",
            "args": ["-c", "echo a,b"],
            "icon": "fish",
        })
    );
}

#[test]
fn an_entry_reads_back_into_the_same_form() {
    let entry = json!({
        "name": "Work",
        "program": "/bin/sh",
        "args": ["-l"],
        "theme": "One Dark",
        "hidden": true,
    });

    let form = ProfileForm::from_entry(&entry, ProfileIcon::Zetta).unwrap();

    assert_eq!(form.to_entry(), entry);
    assert!(!form.detected);
}

/// An override accepts exactly the fields a user profile does.
#[test]
fn an_entry_with_a_field_profiles_do_not_have_is_refused() {
    let error = ProfileForm::from_entry(
        &json!({"name": "Work", "colour": "red"}),
        ProfileIcon::Zetta,
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("unknown field `colour`"), "{error}");
}

#[test]
fn a_profile_list_names_the_profile_and_field_at_fault() {
    assert_eq!(check_profiles(&[profile("Work", "/bin/sh", &[])]), Ok(()));
    assert_eq!(
        check_profiles(&[profile(" ", "/bin/sh", &[])]),
        Err(ProfileProblem::EmptyName(0))
    );
    let problem = check_profiles(&[
        profile("Work", "/bin/sh", &[]),
        profile("work", "/bin/zsh", &[]),
    ])
    .unwrap_err();
    assert_eq!(problem.profile(), 1);
    assert!(!problem.is_in_program());
    let problem = check_profiles(&[profile("Work", "", &["-l"])]).unwrap_err();
    assert!(problem.is_in_program());
}
