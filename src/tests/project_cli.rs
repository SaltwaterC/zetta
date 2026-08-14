use super::*;

fn os(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

#[test]
fn parses_every_project_operation_and_both_path_forms() {
    assert_eq!(
        parse_project_args(&os(&["add", "workspace"])).unwrap(),
        ProjectCommand::Add {
            path: Some(PathBuf::from("workspace"))
        }
    );
    assert_eq!(
        parse_project_args(&os(&["remove", "-p", "workspace"])).unwrap(),
        ProjectCommand::Remove {
            path: Some(PathBuf::from("workspace"))
        }
    );
    assert_eq!(
        parse_project_args(&os(&["open", "--path", "workspace"])).unwrap(),
        ProjectCommand::Open {
            path: Some(PathBuf::from("workspace"))
        }
    );
    assert_eq!(
        parse_project_args(&os(&["list"])).unwrap(),
        ProjectCommand::List
    );
}

#[test]
fn project_parser_rejects_ambiguous_or_unknown_arguments() {
    assert!(parse_project_args(&[]).is_err());
    assert!(parse_project_args(&os(&["unknown"])).is_err());
    assert!(parse_project_args(&os(&["list", "extra"])).is_err());
    assert!(parse_project_args(&os(&["add", "one", "two"])).is_err());
    assert!(parse_project_args(&os(&["open", "--unknown"])).is_err());
}

#[test]
fn project_help_documents_registry_and_preserved_configuration() {
    assert!(project_help(None).contains("zetta project <COMMAND>"));
    assert!(project_help(Some("add")).contains(".zetta/config.json"));
    assert!(project_help(Some("remove")).contains("never deleted"));
    assert!(project_help(Some("open")).contains("new active tab"));
}
