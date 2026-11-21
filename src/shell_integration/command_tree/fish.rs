//! The command trees as Fish: one function per node, answering the questions
//! `engine.fish` asks of it.

use super::{Helper, Node, REGISTRATIONS, built_trees, posix_operand_kinds, posix_quote};
use std::fmt::Write as _;

const ENGINE: &str = include_str!("engine.fish");
/// This shell's function for each helper.
fn helper(helper: Helper) -> &'static str {
    match helper {
        Helper::SoundNames => "__zetta_sound_names",
        Helper::WorktreeCommits => "__zetta_worktree_commits",
        Helper::PaneLabelList => "__zetta_run_pane_labels",
        Helper::SshTargets => "__zetta_ssh_targets",
        Helper::MuxSessionIds => "__zetta_mux_session_ids",
        Helper::MuxRestorableIds => "__zetta_mux_restorable_ids",
        Helper::MuxAttach => "__zetta_mux_attach",
    }
}

pub(super) fn render() -> String {
    let mut script = String::from(ENGINE);
    for tree in built_trees() {
        for node in tree.nodes() {
            write_node(&mut script, &node);
        }
    }
    for registration in REGISTRATIONS {
        let command = registration.command;
        let complete = format!(
            "complete -c {command} -f -a '(__zetta_tree_complete {})'\n",
            super::identifier(registration.tree.name)
        );
        if registration.outside_macos {
            let _ = write!(
                script,
                "if test \"$__zetta_platform\" != Darwin\n    {complete}end\n"
            );
        } else {
            script.push_str(&complete);
        }
    }
    script
}

/// The commands the startup script's lazy loader registers for, as the list
/// it registers from.
pub(super) fn lazy() -> String {
    let (everywhere, outside_macos) = super::lazy_commands();
    let mut script = format!("set -g __zetta_completion_commands {everywhere}\n");
    if !outside_macos.is_empty() {
        let _ = write!(
            script,
            "if test \"$__zetta_platform\" != Darwin\n    set -a __zetta_completion_commands {outside_macos}\nend\n"
        );
    }
    script
}

fn write_node(script: &mut String, node: &Node<'_>) {
    let spec = node.spec;
    let described = |items: Vec<(&str, &str)>| -> String {
        items
            .into_iter()
            .map(|(name, about)| format!(" {name} {}", posix_quote(about)))
            .collect::<String>()
    };
    // `printf` with no arguments still prints its format once, which would
    // offer an empty candidate.
    let printed = |format: &str, arguments: String| -> String {
        if arguments.is_empty() {
            String::new()
        } else {
            format!("\n            printf '{format}'{arguments}")
        }
    };
    let _ = writeln!(script, "function __zetta_node_{}", node.id);
    script.push_str("    switch $argv[1]\n");
    // What needs a local multiplexer daemon is printed only with one.
    let gated = |format: &str, arguments: String| -> String {
        if arguments.is_empty() {
            String::new()
        } else {
            format!("\n            test \"$ZETTA_NO_MUX\" = 1; or printf '{format}'{arguments}")
        }
    };
    let subcommands = printed(
        "%s\\t%s\\n",
        described(spec.offered_subcommands(false).collect()),
    ) + &gated(
        "%s\\t%s\\n",
        described(spec.offered_subcommands(true).collect()),
    );
    let options = printed(
        "%s\\t%s\\n",
        described(spec.offered_options(false).collect()),
    ) + &gated(
        "%s\\t%s\\n",
        described(spec.offered_options(true).collect()),
    );
    let listed = |options: Vec<&str>| -> String {
        options.iter().map(|option| format!(" {option}")).collect()
    };
    let _ = writeln!(
        script,
        "        case subcommands{}\n        case late{}\n        case options{}\n        case repeatable{}\n        case alone{}\n        case options-first\n            echo {}",
        subcommands,
        printed("%s\\n", listed(spec.late().collect())),
        options,
        printed("%s\\n", listed(spec.repeatable().collect())),
        printed("%s\\n", listed(spec.alone().collect())),
        u8::from(spec.options_first),
    );
    let _ = writeln!(
        script,
        "        case operands\n            printf '%s\\n' {}\n        case rest\n            printf '%s\\n' {}",
        posix_operand_kinds(spec, helper),
        posix_quote(&spec.rest_kind(helper))
    );
    script.push_str("        case option\n            switch $argv[2]\n");
    for option in spec.options {
        let _ = writeln!(
            script,
            "                case {}\n                    printf '%s\\n' {} {}",
            option.all_spellings().collect::<Vec<_>>().join(" "),
            option.long,
            posix_quote(&option.value.kind(helper)),
        );
    }
    // After every exact spelling, which a pattern such as `-p*` would
    // otherwise take.
    for option in spec.options {
        for (prefix, kind) in option.attached_prefixes(helper) {
            let _ = writeln!(
                script,
                "                case '{prefix}*'\n                    printf '%s\\n' {} {}",
                option.long,
                posix_quote(&format!("attached:{kind}")),
            );
        }
    }
    script.push_str(
        "                case '*'\n                    printf '%s\\n' $argv[2] nothing\n            end\n    end\nend\n",
    );
}
