//! The command trees as Bash: one function per node, setting the variables
//! `engine.bash` reads. Bash 3.2, which macOS still ships, has no associative
//! arrays, so a node is a function rather than a table.

use super::{Helper, Node, REGISTRATIONS, built_trees, posix_operand_kinds, posix_quote};
use std::fmt::Write as _;

const ENGINE: &str = include_str!("engine.bash");

pub(super) fn render() -> String {
    let mut script = String::from(ENGINE);
    for tree in built_trees() {
        for node in tree.nodes() {
            write_node(&mut script, &node);
        }
    }
    for registration in REGISTRATIONS {
        let entry = format!(
            "_zetta_entry_{}() {{ _zetta_tree_complete {}; }}\n",
            super::identifier(registration.command),
            super::identifier(registration.tree.name),
        );
        let complete = format!(
            "complete -F _zetta_entry_{} {}\n",
            super::identifier(registration.command),
            registration.command
        );
        if registration.outside_macos {
            let _ = write!(
                script,
                "case \"$OSTYPE\" in darwin*) ;; *)\n{entry}{complete}esac\n"
            );
        } else {
            script.push_str(&entry);
            script.push_str(&complete);
        }
    }
    script
}

fn write_node(script: &mut String, node: &Node<'_>) {
    let spec = node.spec;
    let words = |items: Vec<&str>| posix_quote(&items.join(" "));
    let _ = writeln!(script, "_zetta_node_{}() {{", node.id);
    let _ = writeln!(
        script,
        "    spec_subcommands={}\n    spec_options={}",
        words(names(spec.offered_subcommands(false))),
        words(names(spec.offered_options(false)))
    );
    if spec.has_daemon_only() {
        // Appended after a space, so each list stays space-separated.
        let appended = |items: Vec<&str>| posix_quote(&format!(" {}", items.join(" ")));
        let _ = writeln!(
            script,
            "    [[ ${{ZETTA_NO_MUX:-0}} == 1 ]] || spec_subcommands+={} spec_options+={}",
            appended(names(spec.offered_subcommands(true))),
            appended(names(spec.offered_options(true)))
        );
    }
    let _ = writeln!(
        script,
        "    spec_late={}\n    spec_repeatable={}\n    spec_alone={}\n    spec_options_first={}",
        words(spec.late().collect()),
        words(spec.repeatable().collect()),
        words(spec.alone().collect()),
        u8::from(spec.options_first)
    );
    let _ = writeln!(
        script,
        "    spec_operands=({})\n    spec_rest={}",
        posix_operand_kinds(spec, helper),
        posix_quote(&spec.rest_kind(helper))
    );
    script.push_str("    case $1 in\n");
    for option in spec.options {
        let _ = writeln!(
            script,
            "        {}) spec_option={} spec_value={} ;;",
            option.all_spellings().collect::<Vec<_>>().join("|"),
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
                "        {prefix}*) spec_option={} spec_value={} ;;",
                option.long,
                posix_quote(&format!("attached:{kind}")),
            );
        }
    }
    script.push_str("        *) spec_option=$1 spec_value=nothing ;;\n    esac\n}\n");
}

/// The names of what a node offers, without their descriptions.
fn names(items: impl Iterator<Item = (&'static str, &'static str)>) -> Vec<&'static str> {
    items.map(|(name, _)| name).collect()
}

/// This shell's function for each helper.
fn helper(helper: Helper) -> &'static str {
    match helper {
        Helper::SoundNames => "_zetta_complete_sound_names",
        Helper::WorktreeCommits => "_zetta_complete_worktree_commits",
        Helper::PaneLabelList => "_zetta_complete_run_pane_labels",
        Helper::SshTargets => "_zetta_complete_ssh_targets",
        Helper::MuxSessionIds => "_zetta_complete_mux_session_ids",
        Helper::MuxRestorableIds => "_zetta_complete_mux_restorable_ids",
        Helper::MuxAttach => "_zetta_complete_mux_attach",
    }
}
