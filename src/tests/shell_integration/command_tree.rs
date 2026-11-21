use super::*;

fn every_node() -> Vec<Node<'static>> {
    built_trees()
        .into_iter()
        .flat_map(CommandSpec::nodes)
        .collect()
}

#[test]
fn a_spelling_names_one_option_of_its_node() {
    for node in every_node() {
        let mut seen = Vec::new();
        for spelling in node.spec.options.iter().flat_map(OptionSpec::all_spellings) {
            assert!(
                !seen.contains(&spelling),
                "{} spells {spelling} twice",
                node.id
            );
            seen.push(spelling);
        }
    }
}

#[test]
fn only_long_options_are_offered() {
    for node in every_node() {
        for option in node.spec.options {
            assert!(
                option.long.starts_with("--"),
                "{} offers {}",
                node.id,
                option.long
            );
        }
    }
}

// Every option the zosh launcher parses (`parse_flag`, `takes_value` and
// `parse_attached_value` in crates/zosh/src/launcher/args.rs), which `zetta
// mosh` forwards unmodified. Stock Mosh's short-only switches are left out of
// the tree on purpose, since each has a long spelling.
#[cfg(feature = "zosh-client")]
#[test]
fn mosh_describes_every_option_the_zosh_launcher_accepts() {
    const LAUNCHER: &[&str] = &[
        "--client",
        "--server",
        "--predict",
        "-o",
        "--predict-overwrite",
        "--no-predict-overwrite",
        "-k",
        "--keep-alive",
        "-s",
        "--scrollback",
        "--no-scrollback",
        "--forward-agent",
        "--no-forward-agent",
        "--family",
        "-p",
        "--port",
        "--bind-server",
        "--ssh",
        "--ssh-pty",
        "--no-ssh-pty",
        "--init",
        "--no-init",
        "--local",
        "--experimental-remote-ip",
        "-h",
        "--help",
        "-V",
        "--version",
    ];
    let spellings = trees::ZOSH
        .options
        .iter()
        .flat_map(OptionSpec::all_spellings)
        .collect::<Vec<_>>();
    for spelling in LAUNCHER {
        assert!(spellings.contains(spelling), "zosh lacks {spelling}");
    }
}

/// The spellings a parser takes `SPELLING=VALUE` for, read from its source:
/// the quoted options from `start` up to `end`.
#[cfg(any(feature = "zmux", feature = "zosh-client"))]
fn attaching_spellings(source: &'static str, start: &str, end: &str) -> Vec<&'static str> {
    let from = source.find(start).unwrap() + start.len();
    let to = from + source[from..].find(end).unwrap();
    source[from..to]
        .split('"')
        .skip(1)
        .step_by(2)
        .filter(|text| text.starts_with('-'))
        .collect()
}

/// Whether some option of the tree registered as `command` accepts `spelling`
/// with its value attached.
#[cfg(any(feature = "zmux", feature = "zosh-client"))]
fn attaches(command: &str, spelling: &str) -> bool {
    let registration = REGISTRATIONS
        .iter()
        .find(|registration| registration.command == command)
        .unwrap();
    registration.tree.nodes().iter().any(|node| {
        node.spec.options.iter().any(|option| {
            option
                .attached_prefixes(|_| "helper")
                .iter()
                .any(|(prefix, _)| prefix == &format!("{spelling}=") || prefix == spelling)
        })
    })
}

#[cfg(feature = "zosh-client")]
#[test]
fn mosh_attaches_a_value_wherever_the_zosh_launcher_does() {
    let source = include_str!("../../../crates/zosh/src/launcher/args.rs");
    let spellings = attaching_spellings(source, "fn parse_attached_value", "\"--fake-proxy\"");
    assert!(spellings.contains(&"--port"), "read {spellings:?}");
    for spelling in spellings {
        assert!(attaches("zosh", spelling), "zosh takes {spelling}=VALUE");
    }
}

#[cfg(feature = "zmux")]
#[test]
fn mux_attaches_a_value_wherever_zmux_does() {
    let source = include_str!("../../../crates/zmux/src/cli.rs");
    let spellings = attaching_spellings(source, "const EQUALS_OPTIONS", "];");
    assert!(spellings.contains(&"--ssh-target"), "read {spellings:?}");
    for spelling in spellings {
        assert!(attaches("zmux", spelling), "zmux takes {spelling}=VALUE");
    }
}

#[test]
fn nodes_are_listed_parents_first_with_path_identifiers() {
    let ids = trees::ZNTFY
        .nodes()
        .into_iter()
        .map(|node| node.id)
        .collect::<Vec<_>>();
    assert_eq!(ids, ["zntfy", "zntfy__cleanup"]);
}

#[test]
fn every_placeholder_is_filled() {
    for shell in [
        ShellIntegration::Bash,
        ShellIntegration::Fish,
        ShellIntegration::PowerShell,
        ShellIntegration::Zsh,
    ] {
        let script = shell.completion_script();
        assert!(
            !script.contains("ZETTA_COMMAND_TREE"),
            "{shell:?} kept a command-tree placeholder"
        );
        assert!(script.contains("zhttp"), "{shell:?} lost the zhttp tree");
    }
}

/// The startup script's lazy loader has to stand in for every command the
/// payload completes, or that command completes nothing until something else
/// loads the payload.
#[test]
fn the_lazy_loader_registers_every_completed_command() {
    for (shell, marker) in [
        (ShellIntegration::Bash, "complete -F __zetta_lazy_complete "),
        (ShellIntegration::Fish, "__zetta_completion_commands "),
        (
            ShellIntegration::PowerShell,
            "-ScriptBlock $zettaLazyCompletions",
        ),
        (ShellIntegration::Zsh, "compdef __zetta_lazy_complete "),
    ] {
        let script = shell.script();
        assert!(
            !script.contains("ZETTA_LAZY_COMPLETIONS"),
            "{shell:?} kept the lazy-loader placeholder"
        );
        let registered: Vec<&str> = script
            .lines()
            .filter(|line| line.contains(marker))
            .flat_map(str::split_whitespace)
            .collect();
        for registration in REGISTRATIONS {
            assert!(
                registered.contains(&registration.command),
                "{shell:?}'s lazy loader misses {}",
                registration.command
            );
        }
    }
}

use super::super::tests::{
    bash_available, bash_completion_command, bash_has_compopt, clean_shell_command,
    lock_bash_tests, run_bash_driver,
};

/// What each shell must offer for a command line: the fixes the generated
/// trees brought, pinned in every shell.
const CASES: &[(&str, &[&str])] = &[
    ("zhttp ", &["--help", "server"]),
    // A repeatable option stays on offer; a short spelling marks its long
    // option as used.
    (
        "zhttp server --allow-host x -",
        &[
            "--allow-any-host",
            "--allow-host",
            "--help",
            "--port",
            "--root",
            "--serve-dotfiles",
        ],
    ),
    (
        "zhttp server -r x -",
        &[
            "--allow-any-host",
            "--allow-host",
            "--help",
            "--port",
            "--serve-dotfiles",
        ],
    ),
    // Nothing is offered for a value there is nothing to say about.
    ("zhttp server --port ", &[]),
    // `zntfy` is `zetta notify`: `cleanup` is a subcommand only as the first
    // word.
    (
        "zntfy ",
        &[
            "--app-name",
            "--help",
            "--icon",
            "--sound",
            "--timeout",
            "cleanup",
        ],
    ),
    (
        "zntfy hello ",
        &["--app-name", "--help", "--icon", "--sound", "--timeout"],
    ),
    ("zntfy -t ", &["default", "never"]),
    (
        "zntfy -t never ",
        &["--app-name", "--help", "--icon", "--sound"],
    ),
    ("zntfy -- ", &[]),
    ("zntfy cleanup ", &["--dry-run", "--help"]),
    // The parser's compatibility spellings take values too.
    #[cfg(feature = "clipboard")]
    ("zpaste -Prefer ", &["ps", "rtf", "txt"]),
    #[cfg(feature = "clipboard")]
    ("zcopy -pboard ", &["find", "font", "general", "ruler"]),
    #[cfg(feature = "clipboard")]
    ("zcopy --pboard general -", &["--help"]),
    // `zetta` hands the subcommands its tree describes to the engine.
    ("zetta http ", &["--help", "server"]),
    (
        "zetta http server -c x -r y -",
        &[
            "--allow-any-host",
            "--allow-host",
            "--help",
            "--port",
            "--serve-dotfiles",
        ],
    ),
    (
        "zetta notify ",
        &[
            "--app-name",
            "--help",
            "--icon",
            "--sound",
            "--timeout",
            "cleanup",
        ],
    ),
    ("zetta notify cleanup -", &["--dry-run", "--help"]),
    (
        "zetta attention -n ",
        &["--app-name", "--help", "--icon", "--sound", "--timeout"],
    ),
    ("zetta attention --timeout ", &["default", "never"]),
    #[cfg(feature = "clipboard")]
    ("zetta paste -prefer ", &["ps", "rtf", "txt"]),
    // What `zetta serial list` prints, which `ZETTA_STUB` stands in for.
    (
        "zetta serial console --device ",
        &["/dev/ttyACM0", "/dev/ttyUSB0"],
    ),
    ("zetta serial console -p ", &["even", "none", "odd"]),
    (
        "zetta overlay -c ",
        &[
            "black", "blue", "cyan", "gray", "green", "magenta", "orange", "pink", "purple", "red",
            "white", "yellow",
        ],
    ),
    ("zetta benchmark output -t ", &["repeated", "unique"]),
    ("zetta init -", &["--completions", "--help"]),
    (
        "zetta terminal-size -c 1 -",
        &["--help", "--json", "--resize", "--rows"],
    ),
    ("zetta edit -d -", &["--help"]),
    // Operands taken one by one: a third `get` operand is the local file, and
    // after it there is nothing more to give.
    ("ztftp get host ", &["--help", "--port"]),
    ("ztftp get host remote local ", &["--help", "--port"]),
    ("ztftp ", &["--help", "get", "put", "server"]),
    #[cfg(tftp_setcap)]
    ("zetta tftp ", &["--help", "get", "put", "server", "setcap"]),
    #[cfg(not(tftp_setcap))]
    ("zetta tftp ", &["--help", "get", "put", "server"]),
    (
        "zetta tftp server -",
        &["--config", "--help", "--port", "--root", "--writable"],
    ),
    ("zetta project remove /src/a ", &["--help", "--path"]),
    ("zetta theme ", &["--help", "pane", "tab"]),
    ("zetta tabicon -l -", &[]),
    // `overlay --reset` takes neither text nor a style option.
    (
        "zetta overlay -",
        &[
            "--color",
            "--help",
            "--opacity",
            "--reset",
            "--size",
            "--text",
        ],
    ),
    ("zetta overlay --reset -", &[]),
    // Nothing given after an option that prints and exits is read.
    ("zetta overlay --help -", &[]),
    ("zetta --version -", &[]),
    (
        "zetta overlay -c red -",
        &["--help", "--opacity", "--size", "--text"],
    ),
    ("zvi -", &["--help"]),
    // Operands that end at `--` offer it among the options; an option valid
    // only on its own is offered only first, and ends the command.
    (
        "zetta pane -s ",
        &[
            "--",
            "--direction",
            "--help",
            "--label",
            "--overlay",
            "--overlay-color",
            "--overlay-opacity",
            "--overlay-size",
            "--pane",
        ],
    ),
    ("zetta pane --list ", &[]),
    ("zetta pane wait a ", &["--", "--allow-failure", "--help"]),
    ("zetta cmd -", &["--", "--help", "--list"]),
    ("zetta cmd -- x ", &[]),
    // An operand the command does not take ends completion.
    ("zetta splits x ", &[]),
    #[cfg(feature = "zmux")]
    ("zmux stop -", &["--force", "--help"]),
    #[cfg(feature = "zmux")]
    ("zetta mux attach -P ", &["ssh", "zosh"]),
    // A value attached with `=`, where the parser accepts one.
    #[cfg(feature = "zmux")]
    (
        "zetta mux attach --protocol=",
        &["--protocol=ssh", "--protocol=zosh"],
    ),
    #[cfg(feature = "zmux")]
    ("zetta mux create --retention ", &["disk", "memory", "none"]),
    #[cfg(feature = "worktree")]
    ("zwt new -c x -", &["--copy", "--help", "--path-only"]),
    // The root: `profile` may follow its options, and a configuration file
    // given anywhere is what the profile lists are read with.
    ("zetta --profile ", &["System"]),
    ("zetta -c c.json --profile ", &["Configured"]),
    ("zetta -c c.json profile disable ", &["Configured"]),
    ("zetta profile --config c.json theme ", &["Configured"]),
    (
        "zetta profile ",
        &[
            "--config",
            "--help",
            "add",
            "dark-theme",
            "default",
            "disable",
            "enable",
            "icon",
            "list",
            "remove",
            "theme",
            "themes",
        ],
    ),
    (
        "zetta profile icon x ",
        &["auto", "bash", "fish", "zetta", "zsh"],
    ),
    // `--reset` stands in for the last operand, wherever it is given.
    ("zetta profile theme --reset ", &["System"]),
    ("zetta profile theme x --reset ", &["--config", "--help"]),
    ("zetta tabicon --reset ", &["--help", "--icon", "--queue"]),
    #[cfg(feature = "zmux")]
    (
        "zetta --geometry 1x1 ",
        &[
            "--cell-size",
            "--command",
            "--config",
            "--help",
            "--keymap",
            "--new-window",
            "--no-mux",
            "--profile",
            "--replace-pane",
            "--split",
            "--theme",
            "--version",
            "profile",
        ],
    ),
    #[cfg(not(feature = "zmux"))]
    (
        "zetta --geometry 1x1 ",
        &[
            "--cell-size",
            "--command",
            "--config",
            "--help",
            "--keymap",
            "--new-window",
            "--profile",
            "--replace-pane",
            "--split",
            "--theme",
            "--version",
            "profile",
        ],
    ),
    // Mosh's options end at the host; what follows is the remote command.
    #[cfg(feature = "zosh-client")]
    (
        "zosh -",
        &[
            "--bind-server",
            "--client",
            "--experimental-remote-ip",
            "--family",
            "--forward-agent",
            "--help",
            "--init",
            "--keep-alive",
            "--local",
            "--no-forward-agent",
            "--no-init",
            "--no-predict-overwrite",
            "--no-scrollback",
            "--no-ssh-pty",
            "--port",
            "--predict",
            "--predict-overwrite",
            "--scrollback",
            "--server",
            "--ssh",
            "--ssh-pty",
            "--version",
        ],
    ),
    #[cfg(feature = "zosh-client")]
    (
        "zosh --predict ",
        &["adaptive", "always", "experimental", "never"],
    ),
    #[cfg(feature = "zosh-client")]
    ("zosh host -", &[]),
    #[cfg(feature = "zosh-client")]
    (
        "zosh --predict=a",
        &["--predict=adaptive", "--predict=always"],
    ),
    #[cfg(feature = "zosh-client")]
    (
        "zosh --predict=always --p",
        &["--port", "--predict-overwrite"],
    ),
    // Every form the launcher reads a value from, its option then used.
    #[cfg(feature = "zosh-client")]
    ("zosh -p2222 --p", &["--predict", "--predict-overwrite"]),
    #[cfg(feature = "zosh-client")]
    (
        "zosh -k=5 -",
        &[
            "--bind-server",
            "--client",
            "--experimental-remote-ip",
            "--family",
            "--forward-agent",
            "--help",
            "--init",
            "--local",
            "--no-forward-agent",
            "--no-init",
            "--no-predict-overwrite",
            "--no-scrollback",
            "--no-ssh-pty",
            "--port",
            "--predict",
            "--predict-overwrite",
            "--scrollback",
            "--server",
            "--ssh",
            "--ssh-pty",
            "--version",
        ],
    ),
    #[cfg(feature = "zosh-client")]
    ("zosh --keep-alive=5 --sc", &["--scrollback"]),
    #[cfg(feature = "zosh-client")]
    (
        "zosh --experimental-remote-ip=l",
        &["--experimental-remote-ip=local"],
    ),
    #[cfg(feature = "zosh-client")]
    ("zetta mosh -- host -", &[]),
];

/// Stand in for `zetta`, which the trees run for what is only known at
/// runtime, in bash and zsh, fish and PowerShell.
const ZETTA_STUB: &str = "zetta() { local IFS=' '; case \"$*\" in\n\
    'serial list') printf '%s\\n' /dev/ttyUSB0 /dev/ttyACM0 ;;\n\
    'profile list') printf '%s\\n' System ;;\n\
    'profile list --config c.json') printf '%s\\n' Configured ;;\n\
    esac; }\n";
const ZETTA_STUB_FISH: &str = "function zetta; switch \"$argv\"\n\
    case 'serial list'; printf '%s\\n' /dev/ttyUSB0 /dev/ttyACM0\n\
    case 'profile list'; echo System\n\
    case 'profile list --config c.json'; echo Configured\n\
    end; end\n";
const ZETTA_STUB_POWERSHELL: &str = "function zetta { switch ($args -join ' ') {\n\
    'serial list' { '/dev/ttyUSB0'; '/dev/ttyACM0' }\n\
    'profile list' { 'System' }\n\
    'profile list --config c.json' { 'Configured' }\n\
    } }\n";

/// The sound names come from a helper whose list depends on the platform;
/// that it is reached at all is what matters here.
const SOUND_CASE: &str = "zntfy --sound ";

fn assert_cases(shell: &str, completed: &[Vec<String>]) {
    for ((line, expected), candidates) in CASES.iter().zip(completed) {
        let mut candidates = candidates.clone();
        candidates.sort();
        candidates.dedup();
        assert_eq!(candidates, *expected, "{shell} completed {line:?}");
    }
    let sounds = completed.last().unwrap();
    assert!(
        sounds.iter().any(|sound| sound == "zetta-default"),
        "{shell} completed {SOUND_CASE:?} to {sounds:?}"
    );
}

fn lines() -> impl Iterator<Item = &'static str> {
    CASES
        .iter()
        .map(|(line, _)| *line)
        .chain(std::iter::once(SOUND_CASE))
}

/// Splits `line` into words, with an empty word when it ends in a space.
fn words(line: &str) -> Vec<&str> {
    let mut words = line.split_whitespace().collect::<Vec<_>>();
    if line.ends_with(' ') {
        words.push("");
    }
    words
}

/// The candidates `driver` printed, one `case<N>:` line each.
fn candidates_by_case(output: &str) -> Vec<Vec<String>> {
    let mut cases = vec![Vec::new(); lines().count()];
    for line in output.lines() {
        if let Some((case, candidate)) = line
            .strip_prefix("case")
            .and_then(|rest| rest.split_once(':'))
            && let Ok(case) = case.parse::<usize>()
            && !candidate.is_empty()
        {
            cases[case].push(candidate.split('\t').next().unwrap().to_owned());
        }
    }
    cases
}

/// The function a shell's completion calls for `command`, which is generated
/// for each registered tree.
fn entry(command: &str) -> String {
    format!("_zetta_entry_{command}")
}

fn shell_quote(word: &str) -> String {
    format!("'{word}'")
}

#[test]
fn bash_completes_from_the_trees() {
    let _bash_test_lock = lock_bash_tests();
    if !bash_available() {
        return;
    }
    // The cases give `--option=VALUE` as one word, which bash leaves it as
    // only where `=` does not break words; `zosh --predict = a` is the other.
    let mut driver = ZETTA_STUB.to_owned()
        + "COMP_WORDBREAKS=${COMP_WORDBREAKS//=}\n"
        + &ShellIntegration::Bash.completion_script();
    for (case, line) in lines().enumerate() {
        let words = words(line);
        let _ = writeln!(
            driver,
            "COMP_WORDS=({})\nCOMP_CWORD={}\n{}\nprintf 'case{case}:%s\\n' \"${{COMPREPLY[@]}}\"",
            words
                .iter()
                .map(|word| shell_quote(word))
                .collect::<Vec<_>>()
                .join(" "),
            words.len() - 1,
            entry(words[0]),
        );
    }
    let output = run_bash_driver(bash_completion_command(), &driver);
    assert_cases("bash", &candidates_by_case(&output));
}

#[test]
fn zsh_completes_from_the_trees() {
    if clean_shell_command("zsh")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    // Outside a completion widget the completion builtins are unavailable, so
    // they are stood in for by functions that print what they would offer:
    // what matches the word typed, after the prefix `compset -P` set aside.
    let mut driver = String::from(
        "compdef() { :; }\n\
         compset() { IPREFIX=${PREFIX%%=*}= PREFIX=${PREFIX#*=}; }\n\
         compadd() { [[ $1 == -- ]] && shift; print -rl -- \"case$case:$IPREFIX\"${^${(M)@:#$PREFIX*}}; }\n\
         _describe() { local -a items=(\"${(@M)${(@P)4}:#$PREFIX*}\"); print -rl -- \"case$case:\"${^items%%:*}; }\n\
         _files() { :; }\n",
    );
    driver.push_str(ZETTA_STUB);
    driver.push_str(&ShellIntegration::Zsh.completion_script());
    for (case, line) in lines().enumerate() {
        let words = words(line);
        let _ = writeln!(
            driver,
            "case={case}\nwords=({})\nCURRENT={}\nPREFIX={} IPREFIX=\n{}",
            words
                .iter()
                .map(|word| shell_quote(word))
                .collect::<Vec<_>>()
                .join(" "),
            words.len(),
            shell_quote(words[words.len() - 1]),
            entry(words[0]),
        );
    }
    let output = clean_shell_command("zsh")
        .args(["-f", "-c", &driver])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "zsh failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_cases(
        "zsh",
        &candidates_by_case(&String::from_utf8_lossy(&output.stdout)),
    );
}

/// Fish completes the startup script's `vi` wrapper through its `--wraps
/// 'zetta vi'`, from the `vi` tree, files included — and nothing after
/// `--help`, which exits. Only from that tree: fish 4 builds in completions
/// for `vi` that offer Vim's options, and loaded them alongside until the
/// startup script erased them.
#[test]
fn fish_vi_wrapper_completes_from_the_vi_tree() {
    if clean_shell_command("fish")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    // The startup script's own lines, which it runs only where no `vi`
    // exists: from the wrapper to the registration that keeps fish's own
    // `vi` completions out.
    let startup = ShellIntegration::Fish.script();
    let start = startup
        .find("function vi --wraps 'zetta vi' --description 'Zetta vi editor'")
        .expect("fish lost its vi wrapper");
    let registration = "complete -c vi --wraps 'zetta vi'\n";
    let end = startup[start..]
        .find(registration)
        .expect("fish lost its vi registration")
        + start
        + registration.len();
    let wrapper = &startup[start..end];
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("notes.txt"), "").unwrap();
    let driver = format!(
        "{ZETTA_STUB_FISH}{}\n{wrapper}\
         for line in 'vi -' 'vi no' 'vi --help '\n\
         for candidate in (complete -C $line); echo $line:$candidate; end\nend\n",
        ShellIntegration::Fish.completion_script()
    );
    let script = tempfile::NamedTempFile::with_suffix(".fish")
        .unwrap()
        .into_temp_path();
    std::fs::write(&script, driver).unwrap();
    let output = clean_shell_command("fish")
        .arg("--no-config")
        .arg(&script)
        .current_dir(directory.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fish failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut candidates = stdout
        .lines()
        .map(|line| line.split('\t').next().unwrap_or_default())
        .collect::<Vec<_>>();
    candidates.sort_unstable();
    assert_eq!(candidates, ["vi -:--help", "vi no:notes.txt"]);
}

/// A file name, or a name a runtime list prints, may hold a space: bash must
/// keep each one whole, match it against the word as typed, escapes and all,
/// and have it inserted quoted.
#[test]
fn bash_keeps_names_with_spaces_whole() {
    let _bash_test_lock = lock_bash_tests();
    if !bash_available() {
        return;
    }
    // Where bash can be told the names are file names it quotes them itself,
    // so they are offered as they are. macOS's own bash is 3.2, which cannot.
    if bash_has_compopt() {
        assert_eq!(
            bash_names("", &[]),
            [
                "escaped:Git Bash",
                "files:my dir",
                "files:my notes.txt",
                "globs:notes1.txt",
                "globs:notes[1].txt",
                "quoted:Git Bash",
            ]
        );
        // Not for a profile named like a directory, which would gain a slash.
        assert_eq!(
            bash_names("", &["Git Bash"])[..2],
            ["escaped:Git\\ Bash", "files:my dir"]
        );
    }
    // Nor in bash 3.2, which has no compopt to say it with. Then the names
    // are offered quoted, but for a word that already opens a quote.
    assert_eq!(
        bash_names("enable -n compopt", &[]),
        [
            "escaped:Git\\ Bash",
            "files:my\\ dir/",
            "files:my\\ notes.txt",
            "globs:notes1.txt",
            "globs:notes\\[1\\].txt",
            "quoted:Git Bash",
        ]
    );
}

/// What bash offers, sorted, for file names and profile names holding a
/// space, after `prelude`, and with `directories` created beside the files.
fn bash_names(prelude: &str, directories: &[&str]) -> Vec<String> {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("my dir")).unwrap();
    std::fs::write(directory.path().join("my notes.txt"), "").unwrap();
    // Unquoted, `notes[1].txt` is a glob that matches `notes1.txt` instead.
    std::fs::write(directory.path().join("notes[1].txt"), "").unwrap();
    std::fs::write(directory.path().join("notes1.txt"), "").unwrap();
    for name in directories {
        std::fs::create_dir(directory.path().join(name)).unwrap();
    }
    let mut driver = format!(
        "cd {}\n{}\n{prelude}\nzetta() {{ printf '%s\\n' System 'Git Bash'; }}\n",
        shell_quote(&directory.path().to_string_lossy()),
        ShellIntegration::Bash.completion_script()
    );
    // Each line's words as bash hands them over: escapes and quotes intact.
    for (label, words) in [
        ("files", "zvi my"),
        ("globs", "zvi notes"),
        ("escaped", r"zetta --profile 'Git\ '"),
        ("quoted", r#"zetta --profile '"Git'"#),
    ] {
        let _ = writeln!(
            driver,
            "COMP_WORDS=({words})\nCOMP_CWORD=$(( ${{#COMP_WORDS[@]}} - 1 ))\n\
             _zetta_entry_${{COMP_WORDS[0]}}\nprintf '{label}:%s\\n' \"${{COMPREPLY[@]}}\""
        );
    }
    let output = run_bash_driver(bash_completion_command(), &driver);
    let mut lines = output.lines().map(str::to_owned).collect::<Vec<_>>();
    lines.sort_unstable();
    lines
}

/// Bash splits `--option=VALUE` into three words at its `=`, by default. The
/// value is still completed after the `=`, and replaces only what follows it.
#[cfg(feature = "zosh-client")]
#[test]
fn bash_completes_a_value_split_from_its_option_at_the_equals_sign() {
    let _bash_test_lock = lock_bash_tests();
    if !bash_available() {
        return;
    }
    let mut driver = ShellIntegration::Bash.completion_script();
    for (label, words) in [
        ("empty", "zosh --predict ="),
        ("partial", "zosh --predict = a"),
        ("after", "zosh --predict = always --p"),
        // A switch that may take a value attached, given one.
        ("switch", "zosh --keep-alive = 5 --p"),
        // Bash 3.2 does not split the word, but readline still replaces
        // only what follows the `=`.
        ("joined", "zosh --predict=a"),
    ] {
        let _ = writeln!(
            driver,
            "COMP_WORDS=({words})\nCOMP_CWORD=$(( ${{#COMP_WORDS[@]}} - 1 ))\n\
             _zetta_entry_zosh\nprintf '{label}:%s\\n' \"${{COMPREPLY[@]}}\""
        );
    }
    let output = run_bash_driver(bash_completion_command(), &driver);
    let mut lines = output.lines().collect::<Vec<_>>();
    lines.sort_unstable();
    assert_eq!(
        lines,
        [
            "after:--port",
            "after:--predict-overwrite",
            "empty:adaptive",
            "empty:always",
            "empty:experimental",
            "empty:never",
            "joined:adaptive",
            "joined:always",
            "partial:adaptive",
            "partial:always",
            "switch:--port",
            "switch:--predict",
            "switch:--predict-overwrite",
        ]
    );
}

/// PowerShell completes a path below the current directory with the
/// directory as typed, and marks a directory with a separator.
#[test]
fn powershell_completes_paths_below_the_current_directory() {
    let Some(powershell) = ["pwsh", "powershell"].into_iter().find(|program| {
        clean_shell_command(program)
            .args(["-NoProfile", "-Command", "exit 0"])
            .output()
            .is_ok_and(|output| output.status.success())
    }) else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("my dir")).unwrap();
    std::fs::create_dir_all(directory.path().join("docs")).unwrap();
    std::fs::write(directory.path().join("docs").join("guide.md"), "").unwrap();
    let mut driver = ShellIntegration::PowerShell.completion_script();
    driver.push_str(
        "foreach ($line in 'zvi docs/g', 'zvi my') {\n\
         foreach ($match in (TabExpansion2 -inputScript $line -cursorColumn $line.Length).CompletionMatches) {\n\
         \"${line}:$($match.CompletionText)\" } }\n",
    );
    // A path, not an open file: Windows will not let PowerShell read a file
    // this process still holds open.
    let script = tempfile::NamedTempFile::with_suffix(".ps1")
        .unwrap()
        .into_temp_path();
    std::fs::write(&script, driver).unwrap();
    let output = clean_shell_command(powershell)
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(&script)
        .current_dir(directory.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{powershell} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let separator = std::path::MAIN_SEPARATOR;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .collect::<Vec<_>>(),
        [
            "zvi docs/g:docs/guide.md".to_owned(),
            format!("zvi my:'my dir{separator}'"),
        ]
    );
}

#[test]
fn fish_completes_from_the_trees() {
    if clean_shell_command("fish")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let mut driver = ZETTA_STUB_FISH.to_owned() + &ShellIntegration::Fish.completion_script();
    for (case, line) in lines().enumerate() {
        let _ = writeln!(
            driver,
            "for candidate in (complete -C {}); echo case{case}:$candidate; end",
            shell_quote(line),
        );
    }
    // The payload is larger than one argument may be, so it goes in a file.
    let script = tempfile::NamedTempFile::with_suffix(".fish")
        .unwrap()
        .into_temp_path();
    std::fs::write(&script, driver).unwrap();
    let output = clean_shell_command("fish")
        .arg("--no-config")
        .arg(&script)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fish failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // The payload is sourced on its own here, as `zetta init fish
    // --completions` can be, without the startup script's variables.
    assert!(
        output.stderr.is_empty(),
        "fish complained: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_cases(
        "fish",
        &candidates_by_case(&String::from_utf8_lossy(&output.stdout)),
    );
}

#[test]
fn powershell_completes_from_the_trees() {
    let Some(powershell) = ["pwsh", "powershell"].into_iter().find(|program| {
        clean_shell_command(program)
            .args(["-NoProfile", "-Command", "exit 0"])
            .output()
            .is_ok_and(|output| output.status.success())
    }) else {
        return;
    };
    let mut driver =
        ZETTA_STUB_POWERSHELL.to_owned() + &ShellIntegration::PowerShell.completion_script();
    for (case, line) in lines().enumerate() {
        let _ = writeln!(
            driver,
            "foreach ($match in (TabExpansion2 -inputScript {0} -cursorColumn {0}.Length).CompletionMatches) {{ \"case{case}:$($match.CompletionText)\" }}",
            format!("'{line}'"),
        );
    }
    // A path, not an open file: Windows will not let PowerShell read a file
    // this process still holds open.
    let script = tempfile::NamedTempFile::with_suffix(".ps1")
        .unwrap()
        .into_temp_path();
    std::fs::write(&script, driver).unwrap();
    // PowerShell falls back to path completion when a completer offers
    // nothing, so run where there is no path to offer.
    let empty = tempfile::tempdir().unwrap();
    let output = clean_shell_command(powershell)
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(&script)
        .current_dir(empty.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{powershell} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_cases(
        "powershell",
        &candidates_by_case(&String::from_utf8_lossy(&output.stdout)),
    );
}
