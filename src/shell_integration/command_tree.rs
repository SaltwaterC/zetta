//! The command trees shell completion is generated from.
//!
//! A command described here is written out once per shell as data — one node
//! per command and subcommand — and completed by a small hand-written engine
//! per shell (`command_tree/engine.*`) that walks the words typed so far
//! through the tree and offers what the node it lands on accepts. Describing a
//! command once is the point: the hand-written completions these replaced had
//! drifted apart, each shell completing a different subset of the same
//! command, and none of them the parser's own spellings.
//!
//! What every engine does, in order:
//!
//! - After an option that takes a value, offer that value and nothing else,
//!   as for `--option=` where the parser accepts the value attached.
//!   An option that takes a command line, as `--command` does, takes every
//!   word after it.
//! - After `--`, offer only operands — or, for a command whose operands end
//!   at `--`, what follows it, which may be a command line of its own.
//! - For a command whose options come first, the first operand ends them, as
//!   `--` does.
//! - On a word starting with `-`, offer the node's long options that are not
//!   used yet (a repeatable one stays). Short and compatibility spellings are
//!   recognised but never offered.
//! - A switch given in place of the last operand, such as `--reset` for a
//!   theme name, leaves one fewer operand to give.
//! - An option valid only on its own, such as `--list`, is offered only before
//!   anything else is given, and once given, nothing is offered after it.
//!   Nor is anything after one that prints and exits, such as `--help`.
//! - After an operand the node does not take, offer nothing.
//! - Otherwise offer the node's subcommands (only as the first word after
//!   their parent, as every parser here reads them, unless one may follow the
//!   parent's options) and its operands, and its options too when the
//!   operands offer nothing.
//! - A value naming a configuration file is remembered, and the runtime lists
//!   that depend on it are read with it.

use super::ShellIntegration;
use crate::pane::OVERLAY_COLOR_PRESETS;
use std::fmt::Write as _;

mod bash;
mod fish;
mod powershell;
mod trees;
mod zsh;

use trees::REGISTRATIONS;

/// A command or subcommand.
pub(super) struct CommandSpec {
    /// The word that selects it; for a tree's root, the executable.
    name: &'static str,
    about: &'static str,
    options: &'static [OptionSpec],
    subcommands: &'static [CommandSpec],
    /// What the words that are neither options nor subcommands complete to.
    operands: Values,
    /// Offered only where a local multiplexer daemon can run it.
    needs_daemon: bool,
    /// Recognised after its parent's options too, as `zetta profile` is
    /// after a `--config`, rather than only as the first word.
    after_options: bool,
    /// Its options come only before the first operand, which ends them as
    /// `--` would: Mosh's words after the host are the remote command.
    options_first: bool,
}

/// A command with nothing below it, for the others to update from.
const LEAF: CommandSpec = CommandSpec {
    name: "",
    about: "",
    options: &[],
    subcommands: &[],
    operands: Values::Nothing,
    needs_daemon: false,
    after_options: false,
    options_first: false,
};

pub(super) struct OptionSpec {
    long: &'static str,
    /// Every other spelling the parser accepts — the short form, and any
    /// compatibility spelling. Recognised, never offered.
    spellings: &'static [&'static str],
    about: &'static str,
    /// [`Values::Nothing`] for a switch.
    value: Values,
    repeatable: bool,
    /// Valid only on its own, as `--list` usually is.
    alone: bool,
    /// Which spellings also take the value attached, which only some parsers
    /// accept.
    attached: Attached,
    /// Offered only where a local multiplexer daemon can serve it.
    needs_daemon: bool,
}

#[derive(Clone, Copy)]
pub(super) enum Values {
    /// Takes no argument: a switch, or a command without operands.
    Nothing,
    /// Takes an argument with nothing worth offering for it.
    Any,
    Files,
    /// A configuration file, which [`Values::ConfiguredLines`] after it are
    /// read with.
    Config,
    Directories,
    Words(&'static [&'static str]),
    /// The lines `zetta` prints when run with these arguments — what is only
    /// known at runtime, such as the serial devices plugged in.
    ZettaLines(&'static str),
    /// [`Values::ZettaLines`] that depend on the configuration, read with the
    /// configuration file given so far.
    ConfiguredLines(&'static str),
    /// The overlay color presets.
    OverlayColors,
    /// For operands only: what each operand in turn completes to, after which
    /// there are none.
    Each(&'static [Values]),
    /// For a switch only: given in place of the last of [`Values::Each`]'s
    /// operands, as `--reset` is in place of a theme name.
    InsteadOfOperand,
    /// For a switch only: one that prints something and exits, as `--help`
    /// does, so that nothing given after it is ever read.
    Exits,
    /// What one of each shell's own helpers offers.
    Call(Helper),
    /// For operands only: operands that end at `--`, which the engines offer
    /// among the options until it is typed.
    Delimited(&'static Delimiter),
    /// A command line of its own, completed as the shell completes that
    /// command.
    CommandLine,
}

/// Operands that end at `--`, and what follows it.
pub(super) struct Delimiter {
    before: Values,
    /// What `--` does, offered beside it.
    about: &'static str,
    after: Values,
}

/// A completion helper each shell writes itself, because what it offers
/// depends on the platform or on running something only the shell can.
#[derive(Clone, Copy)]
pub(super) enum Helper {
    /// The notification sound names, which depend on the platform.
    SoundNames,
    /// The source-branch commits a worktree can sync to, from Git.
    #[cfg_attr(
        not(feature = "worktree"),
        allow(dead_code, reason = "only `wt` uses it")
    )]
    WorktreeCommits,
    /// Pane labels as a comma-separated list, each label offered once.
    PaneLabelList,
    /// The SSH hosts named in `~/.ssh/config`.
    #[cfg_attr(
        not(any(feature = "zmux", feature = "zosh-client")),
        allow(dead_code, reason = "only `mux` and `mosh` use it")
    )]
    SshTargets,
    /// The local multiplexer's session IDs, from `mux list`.
    #[cfg_attr(not(feature = "zmux"), allow(dead_code, reason = "only `mux` uses it"))]
    MuxSessionIds,
    /// The local multiplexer's encrypted disk records, from `mux list`.
    #[cfg_attr(not(feature = "zmux"), allow(dead_code, reason = "only `mux` uses it"))]
    MuxRestorableIds,
    /// `mux attach`'s operands: an SSH host until one is given, by operand
    /// or `--ssh-target`, then that host's session IDs over SSH.
    #[cfg_attr(not(feature = "zmux"), allow(dead_code, reason = "only `mux` uses it"))]
    MuxAttach,
}

impl Values {
    /// The engines' spelling of the kind; `helper` names the shell's own
    /// function for each [`Helper`].
    fn kind(self, helper: fn(Helper) -> &'static str) -> String {
        match self {
            Self::Nothing => "nothing".to_owned(),
            Self::Any => "any".to_owned(),
            Self::Files => "files".to_owned(),
            Self::Config => "config".to_owned(),
            Self::Directories => "directories".to_owned(),
            Self::Words(words) => format!("words:{}", words.join(" ")),
            Self::ZettaLines(arguments) => format!("zetta:{arguments}"),
            Self::ConfiguredLines(arguments) => format!("configured:{arguments}"),
            Self::OverlayColors => {
                let names = OVERLAY_COLOR_PRESETS.map(|preset| preset.name);
                format!("words:{}", names.join(" "))
            }
            Self::Call(called) => format!("call:{}", helper(called)),
            Self::CommandLine => "command".to_owned(),
            Self::InsteadOfOperand => "instead".to_owned(),
            Self::Exits => "exits".to_owned(),
            Self::Each(_) | Self::Delimited(_) => {
                unreachable!("only a command's operands are taken one by one")
            }
        }
    }

    /// The engines' spelling of what each operand completes to: the last kind
    /// listed is what every later operand completes to.
    fn operand_kinds(self, helper: fn(Helper) -> &'static str) -> Vec<String> {
        match self {
            Self::Each(each) => each
                .iter()
                .map(|values| values.kind(helper))
                .chain(std::iter::once(Self::Nothing.kind(helper)))
                .collect(),
            Self::Delimited(delimiter) => delimiter.before.operand_kinds(helper),
            // An operand given where none is taken is a mistake nothing
            // can follow, which the engines offer nothing for.
            Self::Nothing => vec![self.kind(helper), "stray".to_owned()],
            values => vec![values.kind(helper)],
        }
    }
}

impl OptionSpec {
    /// Every spelling the parser accepts, the long one first.
    fn all_spellings(&self) -> impl Iterator<Item = &'static str> + '_ {
        std::iter::once(self.long).chain(self.spellings.iter().copied())
    }

    /// What a word giving this option its value attached starts with — each
    /// spelling that takes one, then `=` unless the value may follow the
    /// spelling directly — each with the kind of that value. A switch that
    /// takes one takes a value there is nothing to offer for.
    fn attached_prefixes(&self, helper: fn(Helper) -> &'static str) -> Vec<(String, String)> {
        let kind = match self.value {
            Values::Nothing => Values::Any.kind(helper),
            value => value.kind(helper),
        };
        let short = self.spellings.iter().filter(|spelling| spelling.len() == 2);
        let mut prefixes = Vec::new();
        if self.attached != Attached::No {
            prefixes.push(format!("{}=", self.long));
        }
        if self.attached == Attached::Every {
            prefixes.extend(short.map(|spelling| format!("{spelling}=")));
        } else if self.attached == Attached::Joined {
            prefixes.extend(short.map(|spelling| (*spelling).to_owned()));
        }
        prefixes
            .into_iter()
            .map(|prefix| (prefix, kind.clone()))
            .collect()
    }
}

/// Which of an option's spellings take its value attached.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Attached {
    No,
    /// The long spelling, as `--port=2222`.
    #[cfg_attr(
        not(any(feature = "zmux", feature = "zosh-client")),
        allow(dead_code, reason = "only `mux` and `mosh` parse attached values")
    )]
    Long,
    /// Every spelling, as `--port=2222` and `-p=2222`.
    Every,
    /// Every spelling, a short one with or without the `=`, as `-p2222`.
    Joined,
}

/// A command name completion is registered for, and the tree it completes
/// from. A command this build does not ship is left out of `REGISTRATIONS`
/// with `#[cfg]`, along with any tree only it completes from, so every
/// registration there is one to generate.
struct Registration {
    command: &'static str,
    tree: &'static CommandSpec,
    /// Registered only outside macOS, where the name would otherwise shadow
    /// the system's own command.
    outside_macos: bool,
    /// An executable rather than a function the startup script defines,
    /// which PowerShell registers a completer for with `-Native`.
    native: bool,
}

const fn option(
    long: &'static str,
    spellings: &'static [&'static str],
    about: &'static str,
    value: Values,
) -> OptionSpec {
    OptionSpec {
        long,
        spellings,
        about,
        value,
        repeatable: false,
        alone: false,
        attached: Attached::No,
        needs_daemon: false,
    }
}

/// `option`, whose value its spellings `attached` also take attached to them.
#[cfg_attr(
    not(any(feature = "zmux", feature = "zosh-client")),
    allow(dead_code, reason = "only `mux` and `mosh` parse attached values")
)]
const fn attached(attached: Attached, option: OptionSpec) -> OptionSpec {
    OptionSpec { attached, ..option }
}

/// A switch valid only on its own.
const fn alone(
    long: &'static str,
    spellings: &'static [&'static str],
    about: &'static str,
) -> OptionSpec {
    OptionSpec {
        alone: true,
        ..switch(long, spellings, about)
    }
}

/// A switch given in place of the last operand.
const fn instead(
    long: &'static str,
    spellings: &'static [&'static str],
    about: &'static str,
) -> OptionSpec {
    option(long, spellings, about, Values::InsteadOfOperand)
}

/// A switch that prints something and exits, after which nothing is offered.
const fn exits(
    long: &'static str,
    spellings: &'static [&'static str],
    about: &'static str,
) -> OptionSpec {
    option(long, spellings, about, Values::Exits)
}

const fn switch(
    long: &'static str,
    spellings: &'static [&'static str],
    about: &'static str,
) -> OptionSpec {
    option(long, spellings, about, Values::Nothing)
}

/// One node of a tree, with the identifier the engines know it by: the
/// path from the root, joined with `__`.
struct Node<'a> {
    id: String,
    spec: &'a CommandSpec,
}

impl CommandSpec {
    /// This command and every subcommand below it, parents first.
    fn nodes(&self) -> Vec<Node<'_>> {
        let mut nodes = Vec::new();
        self.collect_nodes(identifier(self.name), &mut nodes);
        nodes
    }

    fn collect_nodes<'a>(&'a self, id: String, nodes: &mut Vec<Node<'a>>) {
        for subcommand in self.subcommands {
            subcommand.collect_nodes(format!("{id}__{}", identifier(subcommand.name)), nodes);
        }
        nodes.insert(0, Node { id, spec: self });
    }

    /// Where this command's operands end at `--`, what that is offered as.
    fn delimiter(&self) -> Option<&'static Delimiter> {
        match self.operands {
            Values::Delimited(delimiter) => Some(delimiter),
            _ => None,
        }
    }

    /// The engines' spelling of what the words after `--` complete to, or
    /// nothing where `--` only ends the options.
    fn rest_kind(&self, helper: fn(Helper) -> &'static str) -> String {
        self.delimiter()
            .map(|delimiter| delimiter.after.kind(helper))
            .unwrap_or_default()
    }

    /// The subcommands offered, as name and description: those that need a
    /// local multiplexer daemon, or those that do not.
    fn offered_subcommands(
        &self,
        needs_daemon: bool,
    ) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
        self.subcommands
            .iter()
            .filter(move |command| command.needs_daemon == needs_daemon)
            .map(|command| (command.name, command.about))
    }

    /// The long options offered, as name and description: those that need a
    /// local multiplexer daemon, or those that do not — with `--` among the
    /// latter where the operands end at one.
    fn offered_options(
        &self,
        needs_daemon: bool,
    ) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
        let delimiter = self.delimiter().filter(|_| !needs_daemon);
        self.options
            .iter()
            .filter(move |option| option.needs_daemon == needs_daemon)
            .map(|option| (option.long, option.about))
            .chain(delimiter.map(|delimiter| ("--", delimiter.about)))
    }

    /// Whether anything here is offered only with a daemon, which the
    /// generators then write a test of `ZETTA_NO_MUX` for.
    fn has_daemon_only(&self) -> bool {
        self.offered_subcommands(true)
            .chain(self.offered_options(true))
            .next()
            .is_some()
    }

    fn repeatable(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.options
            .iter()
            .filter(|option| option.repeatable)
            .map(|option| option.long)
    }

    /// The subcommands recognised after this command's options too.
    fn late(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.subcommands
            .iter()
            .filter(|command| command.after_options)
            .map(|command| command.name)
    }

    /// The long options valid only on their own.
    fn alone(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.options
            .iter()
            .filter(|option| option.alone)
            .map(|option| option.long)
    }
}

/// A command name as it can appear in a function or variable name.
fn identifier(name: &str) -> String {
    name.replace('-', "_")
}

/// The trees this build writes out, each once however many names complete
/// from it.
fn built_trees() -> Vec<&'static CommandSpec> {
    let mut trees: Vec<&'static CommandSpec> = Vec::new();
    for registration in REGISTRATIONS {
        if !trees
            .iter()
            .any(|tree| std::ptr::eq(*tree, registration.tree))
        {
            trees.push(registration.tree);
        }
    }
    trees
}

/// Fills in a rendered completion template's placeholders with the engine,
/// every built tree and the registrations, in `shell`'s syntax.
///
/// Bash, Fish and Zsh take all of it at one `# ZETTA_COMMAND_TREE` line.
/// PowerShell takes it in the three parts [`powershell`] explains.
pub(super) fn expand(shell: ShellIntegration, template: &str) -> String {
    let parts = match shell {
        ShellIntegration::Bash => vec![("# ZETTA_COMMAND_TREE\n", bash::render())],
        ShellIntegration::Fish => vec![("# ZETTA_COMMAND_TREE\n", fish::render())],
        ShellIntegration::Zsh => vec![("# ZETTA_COMMAND_TREE\n", zsh::render())],
        ShellIntegration::PowerShell => vec![
            ("# ZETTA_COMMAND_TREE_NODES\n", powershell::render()),
            (
                "    # ZETTA_COMMAND_TREE_ENGINE\n",
                powershell::ENGINE.to_owned(),
            ),
            (
                "# ZETTA_COMMAND_TREE_REGISTRATIONS\n",
                powershell::registrations("zettaCompletions"),
            ),
        ],
    };
    let mut script = template.to_owned();
    for (placeholder, generated) in parts {
        debug_assert_eq!(
            script.matches(placeholder).count(),
            1,
            "{shell:?} completion template needs exactly one {placeholder:?}"
        );
        script = script.replacen(placeholder, &generated, 1);
    }
    script
}

/// Fills a rendered startup script's `# ZETTA_LAZY_COMPLETIONS` line with
/// the commands its lazy loader registers for, in `shell`'s syntax.
///
/// The loader stands in for the payload [`expand`] generates until the first
/// completion loads it, so it has to name exactly the commands that payload
/// registers; generating both from [`REGISTRATIONS`] is what keeps them equal.
/// The placeholder sits inside a block in every script, so the lines take its
/// indentation.
pub(super) fn expand_lazy(shell: ShellIntegration, template: &str) -> String {
    const PLACEHOLDER: &str = "# ZETTA_LAZY_COMPLETIONS\n";
    debug_assert_eq!(
        template.matches(PLACEHOLDER).count(),
        1,
        "{shell:?} startup script needs exactly one {PLACEHOLDER:?}"
    );
    let Some(at) = template.find(PLACEHOLDER) else {
        return template.to_owned();
    };
    let line_start = template[..at].rfind('\n').map_or(0, |newline| newline + 1);
    let indent = &template[line_start..at];
    let lines = match shell {
        ShellIntegration::Bash => posix_lazy("complete -F"),
        ShellIntegration::Zsh => posix_lazy("compdef"),
        ShellIntegration::Fish => fish::lazy(),
        ShellIntegration::PowerShell => powershell::registrations("zettaLazyCompletions"),
    };
    let mut script = template[..line_start].to_owned();
    for line in lines.lines() {
        let _ = writeln!(script, "{indent}{line}");
    }
    script.push_str(&template[at + PLACEHOLDER.len()..]);
    script
}

/// The built commands' names: those registered everywhere, then those
/// registered only outside macOS.
fn lazy_commands() -> (String, String) {
    let (outside_macos, everywhere): (Vec<_>, Vec<_>) = REGISTRATIONS
        .iter()
        .partition(|registration| registration.outside_macos);
    let names = |registrations: Vec<&Registration>| {
        let names = registrations
            .iter()
            .map(|registration| registration.command);
        names.collect::<Vec<_>>().join(" ")
    };
    (names(everywhere), names(outside_macos))
}

/// Bash's and Zsh's lazy registrations, which differ only in the command that
/// registers a completion function.
fn posix_lazy(register: &str) -> String {
    let (everywhere, outside_macos) = lazy_commands();
    let mut script = format!("{register} __zetta_lazy_complete {everywhere}\n");
    if !outside_macos.is_empty() {
        let _ = writeln!(
            script,
            "case \"$OSTYPE\" in darwin*) ;; *) {register} __zetta_lazy_complete {outside_macos} ;; esac"
        );
    }
    script
}

/// What each operand completes to, as POSIX shell words, for the shells that
/// list them in an array.
fn posix_operand_kinds(spec: &CommandSpec, helper: fn(Helper) -> &'static str) -> String {
    let kinds = spec.operands.operand_kinds(helper);
    let quoted = kinds.iter().map(|kind| posix_quote(kind));
    quoted.collect::<Vec<_>>().join(" ")
}

/// `text` as a single-quoted POSIX shell word.
fn posix_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
#[path = "../tests/shell_integration/command_tree.rs"]
mod tests;
