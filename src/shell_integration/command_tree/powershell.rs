//! The command trees as PowerShell: a table of nodes the completer's closure
//! captures, and the engine that runs inside that closure.
//!
//! Unlike the other shells this comes in three parts, because the completion
//! payload is evaluated inside a function: the nodes have to exist before the
//! closure captures them, the engine has to run inside the closure, and the
//! registrations can only name the closure once it exists.

use super::{Helper, Node, REGISTRATIONS, built_trees};
use std::fmt::Write as _;

pub(super) const ENGINE: &str = include_str!("engine.ps1");
/// This shell's function for each helper.
fn helper(helper: Helper) -> &'static str {
    match helper {
        Helper::SoundNames => "zettaSoundNames",
        Helper::WorktreeCommits => "zettaWorktreeCommits",
        Helper::PaneLabelList => "zettaRunPaneLabels",
        Helper::SshTargets => "zettaSshTargets",
        Helper::MuxSessionIds => "zmuxSessionIds",
        Helper::MuxRestorableIds => "zmuxRestorableIds",
        Helper::MuxAttach => "zettaMuxAttach",
    }
}

/// The node table, and which tree each registered command completes from.
pub(super) fn render() -> String {
    let mut script = String::from("$zettaTreeNodes = @{}\n$zettaTreeRoots = @{\n");
    for registration in REGISTRATIONS {
        let _ = writeln!(
            script,
            "    {} = {}",
            quote(registration.command),
            quote(&super::identifier(registration.tree.name))
        );
    }
    script.push_str("}\n");
    for tree in built_trees() {
        for node in tree.nodes() {
            write_node(&mut script, &node);
        }
    }
    script
}

/// A completer for every registered command, run by the script block
/// `$script_block`: the completer itself, or the startup script's lazy loader.
pub(super) fn registrations(script_block: &str) -> String {
    let mut script = String::new();
    for registration in REGISTRATIONS {
        let register = format!(
            "Register-ArgumentCompleter{} -CommandName {} -ScriptBlock ${script_block}\n",
            if registration.native { " -Native" } else { "" },
            registration.command
        );
        if registration.outside_macos {
            let _ = write!(script, "if (-not $IsMacOS) {{\n    {register}}}\n");
        } else {
            script.push_str(&register);
        }
    }
    script
}

fn write_node(script: &mut String, node: &Node<'_>) {
    let spec = node.spec;
    let list = |items: Vec<&str>| {
        let quoted = items.into_iter().map(quote).collect::<Vec<_>>();
        format!("@({})", quoted.join(", "))
    };
    let _ = writeln!(script, "$zettaTreeNodes[{}] = @{{", quote(&node.id));
    let names = |items: Vec<(&'static str, &'static str)>| {
        list(items.into_iter().map(|(name, _)| name).collect())
    };
    let _ = writeln!(
        script,
        "    Subcommands = {}\n    Options = {}",
        names(spec.offered_subcommands(false).collect()),
        names(spec.offered_options(false).collect())
    );
    // What needs a local multiplexer daemon, which the engine adds unless
    // `ZETTA_NO_MUX` is 1.
    if spec.has_daemon_only() {
        let _ = writeln!(
            script,
            "    Daemon = @{{ Subcommands = {}; Options = {} }}",
            names(spec.offered_subcommands(true).collect()),
            names(spec.offered_options(true).collect())
        );
    }
    let _ = writeln!(
        script,
        "    Late = {}\n    Repeatable = {}\n    Alone = {}\n    OptionsFirst = ${}",
        list(spec.late().collect()),
        list(spec.repeatable().collect()),
        list(spec.alone().collect()),
        spec.options_first
    );
    let _ = writeln!(
        script,
        "    Operands = {}",
        list(
            spec.operands
                .operand_kinds(helper)
                .iter()
                .map(String::as_str)
                .collect()
        )
    );
    let _ = writeln!(script, "    Rest = {}", quote(&spec.rest_kind(helper)));
    // What a word giving an option its value attached starts with, the
    // option, and the value's kind, as `PREFIX|--long|KIND` strings: an exact
    // switch cannot match a prefix, and PowerShell's tables ignore case.
    let attached = spec
        .options
        .iter()
        .flat_map(|option| {
            option
                .attached_prefixes(helper)
                .into_iter()
                .map(|(prefix, kind)| quote(&format!("{prefix}|{}|{kind}", option.long)))
        })
        .collect::<Vec<_>>();
    if !attached.is_empty() {
        let _ = writeln!(script, "    Attached = @({})", attached.join(", "));
    }
    script.push_str(
        "    Option = {\n        param($word)\n        switch -CaseSensitive -Exact ($word) {\n",
    );
    for option in spec.options {
        for spelling in option.all_spellings() {
            let _ = writeln!(
                script,
                "            {} {{ {}, {} }}",
                quote(spelling),
                quote(option.long),
                quote(&option.value.kind(helper))
            );
        }
    }
    script.push_str("            default { $word, 'nothing' }\n        }\n    }\n}\n");
}

/// `text` as a single-quoted PowerShell string.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}
