//! The command trees as Zsh: one function per node, setting the variables
//! `engine.zsh` reads.

use super::{Helper, Node, REGISTRATIONS, built_trees, posix_operand_kinds, posix_quote};
use std::fmt::Write as _;

const ENGINE: &str = include_str!("engine.zsh");
/// This shell's function for each helper.
fn helper(helper: Helper) -> &'static str {
    match helper {
        Helper::SoundNames => "_zetta_sound_names",
        Helper::WorktreeCommits => "_zetta_worktree_commits",
        Helper::PaneLabelList => "_zetta_run_pane_labels",
        Helper::SshTargets => "_zetta_ssh_targets",
        Helper::MuxSessionIds => "_zmux_session_ids",
        Helper::MuxRestorableIds => "_zmux_restorable_ids",
        Helper::MuxAttach => "_zmux_attach",
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
        let entry = format!(
            "_zetta_entry_{}() {{ _zetta_tree_complete {}; }}\ncompdef _zetta_entry_{0} {command}\n",
            super::identifier(command),
            super::identifier(registration.tree.name),
        );
        if registration.outside_macos {
            let _ = write!(
                script,
                "case \"$OSTYPE\" in darwin*) ;; *)\n{entry};; esac\n"
            );
        } else {
            script.push_str(&entry);
        }
    }
    script
}

fn write_node(script: &mut String, node: &Node<'_>) {
    let spec = node.spec;
    let described = |items: Vec<(&str, &str)>| {
        items
            .into_iter()
            .map(|(name, about)| posix_quote(&format!("{name}:{about}")))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let _ = writeln!(script, "_zetta_node_{}() {{", node.id);
    let _ = writeln!(
        script,
        "    spec_subcommands=({})\n    spec_options=({})",
        described(spec.offered_subcommands(false).collect()),
        described(spec.offered_options(false).collect())
    );
    if spec.has_daemon_only() {
        let _ = writeln!(
            script,
            "    [[ ${{ZETTA_NO_MUX:-0}} == 1 ]] || spec_subcommands+=({}) spec_options+=({})",
            described(spec.offered_subcommands(true).collect()),
            described(spec.offered_options(true).collect())
        );
    }
    let _ = writeln!(
        script,
        "    spec_late=({})\n    spec_repeatable=({})\n    spec_alone=({})\n    spec_options_first={}",
        spec.late().collect::<Vec<_>>().join(" "),
        spec.repeatable().collect::<Vec<_>>().join(" "),
        spec.alone().collect::<Vec<_>>().join(" "),
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
            "        ({}) spec_option={} spec_value={} ;;",
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
                "        ({prefix}*) spec_option={} spec_value={} ;;",
                option.long,
                posix_quote(&format!("attached:{kind}")),
            );
        }
    }
    script.push_str("        (*) spec_option=$1 spec_value=nothing ;;\n    esac\n}\n");
}
