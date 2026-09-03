use super::*;

#[test]
fn terminal_rendering_profiler_launches_the_current_executable() {
    let executable = Path::new(if cfg!(windows) {
        r"C:\tools\zetta.exe"
    } else {
        "/usr/local/bin/zetta"
    });
    let config = terminal_rendering_profile_config(executable, PerformanceWorkload::Standard);

    assert_eq!(config.profiles.len(), 1);
    assert_eq!(config.default_profile, 0);
    assert_eq!(
        config.profiles[0].command,
        Shell::WithArguments {
            program: executable.to_string_lossy().into_owned(),
            args: vec![
                "benchmark".to_owned(),
                "--terminal-render-workload".to_owned(),
            ],
            title_override: Some("Terminal rendering profiler".to_owned()),
        }
    );
}

#[test]
fn checkerboard_profiler_launches_the_background_workload() {
    let executable = Path::new("/path/to/zetta");
    let config =
        terminal_rendering_profile_config(executable, PerformanceWorkload::CheckerboardBackground);

    assert_eq!(
        config.profiles[0].command,
        Shell::WithArguments {
            program: executable.to_string_lossy().into_owned(),
            args: vec![
                "benchmark".to_owned(),
                "--terminal-checkerboard-workload".to_owned(),
            ],
            title_override: Some("Terminal rendering profiler".to_owned()),
        }
    );
}

#[test]
fn sparse_update_profiler_launches_the_sparse_workload() {
    let executable = Path::new("/path/to/zetta");
    let config = terminal_rendering_profile_config(executable, PerformanceWorkload::SparseUpdates);

    assert_eq!(
        config.profiles[0].command,
        Shell::WithArguments {
            program: executable.to_string_lossy().into_owned(),
            args: vec![
                "benchmark".to_owned(),
                "--terminal-sparse-update-workload".to_owned(),
            ],
            title_override: Some("Terminal rendering profiler".to_owned()),
        }
    );
}

#[test]
fn linux_desktop_entry_matches_app_id() {
    let desktop_entry = include_str!("../../resources/linux/Zetta.desktop");
    let makefile = include_str!("../../Makefile");
    assert!(desktop_entry.contains(&format!("\nIcon={ZETTA_APP_ID}\n")));
    assert!(desktop_entry.contains(&format!("\nStartupWMClass={ZETTA_APP_ID}\n")));
    assert!(desktop_entry.contains("# ZETTA MANAGED PROFILE ACTIONS BEGIN"));
    assert!(desktop_entry.contains("# ZETTA MANAGED PROFILE ACTIONS END"));
    assert!(desktop_entry.contains("\nActions=new-window;\n"));
    assert!(desktop_entry.contains("\nExec=zetta\n"));
    assert!(
        desktop_entry
            .contains("[Desktop Action new-window]\nName=New Window\nExec=zetta --new-window\n")
    );
    assert!(makefile.contains(
        "-e \"1,/^\\[Desktop Action / s|^Exec=[^[:space:]]*\\(.*\\)$$|Exec=$(BINDIR)/zetta\\1|\""
    ));
    assert!(makefile.contains("desktop_source=\"$$desktop_entry\""));
    assert!(makefile.contains("ZETTA MANAGED PROFILE GROUPS BEGIN"));
    assert!(
        makefile.contains(
            "test -f \"$$desktop_entry\" && cmp -s \"$$desktop_tmp\" \"$$desktop_entry\""
        )
    );
    assert!(makefile.contains("mv -f \"$$desktop_tmp\" \"$$desktop_entry\""));
    assert!(!makefile.contains("-e 's|^Exec=zetta$$|Exec=$(BINDIR)/zetta|'"));
    assert!(!makefile.contains("-e 's|^Exec=.*|Exec=$(BINDIR)/zetta|'"));
}
