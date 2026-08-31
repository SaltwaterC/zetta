//! State shared by `zetta pane wait` clients and terminal lifecycle events.
//!
//! The registry deliberately has no GPUI dependency.  Terminal event handlers
//! feed it from the UI thread, while process-control workers wait on the small
//! per-run channels it owns.  This keeps waiting out of rendering and makes the
//! dependency graph useful to every window in one Zetta process.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    hash::Hash,
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{Receiver, RecvTimeoutError, Sender, channel},
    },
};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// Stable identity exposed to a shell running in a pane.  `pane_id` is a
/// window-local layout identifier and may change when a session is moved;
/// `routing_id` is retained by the pane for the lifetime of the session.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub(crate) struct RunPaneIdentity {
    pub(crate) attention_id: u64,
    pub(crate) routing_id: u64,
}

impl RunPaneIdentity {
    pub(crate) const fn new(attention_id: u64, routing_id: u64) -> Self {
        Self {
            attention_id,
            routing_id,
        }
    }
}

/// The parsed `pane wait` operation. The command is kept as argv rather than a
/// shell string so the wrapper can execute it without another round of
/// quoting or interpretation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct PaneWaitCommand {
    pub(crate) dependencies: Vec<String>,
    pub(crate) allow_failure: bool,
    pub(crate) command: Vec<String>,
}

/// Payload transported over the private process-control socket.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct RunWaitRequest {
    pub(crate) owner: RunPaneIdentity,
    pub(crate) dependencies: Vec<String>,
    pub(crate) allow_failure: bool,
    pub(crate) command: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RunResolution {
    Ready,
    Failed,
}

/// A registered wrapper waiting for its dependency decision.
pub(crate) struct RunRegistration {
    pub(crate) id: u64,
    receiver: Receiver<RunResolutionMessage>,
}

impl RunRegistration {
    #[cfg(test)]
    pub(crate) fn wait(self) -> RunResolutionMessage {
        self.receiver
            .recv()
            .unwrap_or_else(|_| RunResolutionMessage::failed("the run registry stopped"))
    }

    pub(crate) fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> std::result::Result<RunResolutionMessage, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RunResolutionMessage {
    pub(crate) resolution: RunResolution,
    pub(crate) message: Option<String>,
}

impl RunResolutionMessage {
    fn ready() -> Self {
        Self {
            resolution: RunResolution::Ready,
            message: None,
        }
    }

    pub(crate) fn failed(message: impl Into<String>) -> Self {
        Self {
            resolution: RunResolution::Failed,
            message: Some(message.into()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LastResult {
    Success(i32),
    Failure(i32),
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ActiveOperation {
    Shell(String),
    Managed(u64),
    Ignored,
}

#[derive(Debug)]
struct PaneState {
    available: bool,
    tracking_ready: bool,
    active: Option<ActiveOperation>,
    last_result: Option<LastResult>,
    /// The shell's command-finished marker for a managed wrapper is emitted
    /// after the wrapper has reported its result over process control. Keep
    /// duplicate markers from replacing the authoritative managed result until
    /// the next real command-start event.
    managed_shell_result_pending: bool,
}

impl Default for PaneState {
    fn default() -> Self {
        Self {
            available: true,
            tracking_ready: false,
            active: None,
            last_result: None,
            managed_shell_result_pending: false,
        }
    }
}

#[derive(Debug)]
struct RunNode {
    owner: RunPaneIdentity,
    dependencies: Vec<RunDependency>,
    allow_failure: bool,
    ready: bool,
    terminal: Option<RunResolutionMessage>,
    waiter: Option<Sender<RunResolutionMessage>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RunDependency {
    pane: RunPaneIdentity,
    observation: DependencyObservation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DependencyObservation {
    Missing,
    Closed,
    WaitingForCommand,
    ActiveShell,
    ActiveManaged(u64),
    Last(LastResult),
    Unknown,
}

#[derive(Default)]
struct RegistryState {
    next_id: u64,
    panes: HashMap<RunPaneIdentity, PaneState>,
    runs: HashMap<u64, RunNode>,
    dependents_by_pane: HashMap<RunPaneIdentity, HashSet<u64>>,
    dependents_by_run: HashMap<u64, HashSet<u64>>,
}

/// Process-wide run dependency registry.
#[derive(Clone, Default)]
pub(crate) struct RunCommandRegistry {
    state: Arc<Mutex<RegistryState>>,
}

static PROCESS_RUN_REGISTRY: OnceLock<RunCommandRegistry> = OnceLock::new();

pub(crate) fn process_run_registry() -> RunCommandRegistry {
    PROCESS_RUN_REGISTRY
        .get_or_init(RunCommandRegistry::default)
        .clone()
}

impl RunCommandRegistry {
    pub(crate) fn register(
        &self,
        owner: RunPaneIdentity,
        dependencies: Vec<RunPaneIdentity>,
        allow_failure: bool,
        command: Vec<String>,
    ) -> Result<RunRegistration> {
        ensure!(
            owner.attention_id != 0,
            "run owner attention ID must be positive"
        );
        ensure!(
            owner.routing_id != 0,
            "run owner routing ID must be positive"
        );
        ensure!(
            !dependencies.is_empty(),
            "a run requires at least one dependency"
        );
        ensure!(!command.is_empty(), "a run requires a command");
        ensure!(
            command.first().is_none_or(|program| !program.is_empty()),
            "a run requires a non-empty command name"
        );
        let mut unique = HashSet::with_capacity(dependencies.len());
        for dependency in &dependencies {
            ensure!(
                dependency.attention_id != 0 && dependency.routing_id != 0,
                "run dependency identity must be positive"
            );
            ensure!(*dependency != owner, "a run cannot depend on its own pane");
            ensure!(
                unique.insert(*dependency),
                "a run cannot list the same dependency more than once"
            );
        }

        let (receiver, id) = {
            let mut state = self.lock_state();
            {
                let owner_state = state.panes.entry(owner).or_default();
                ensure!(
                    owner_state.available,
                    "the originating pane is no longer available"
                );
                ensure!(
                    !matches!(owner_state.active, Some(ActiveOperation::Managed(_))),
                    "the originating pane already owns a waiting run"
                );
            }

            let dependencies = dependencies
                .into_iter()
                .map(|pane| RunDependency {
                    pane,
                    observation: snapshot_dependency(&state, pane),
                })
                .collect::<Vec<_>>();

            state.next_id = state.next_id.wrapping_add(1).max(1);
            let id = state.next_id;
            let (sender, receiver) = channel();
            for dependency in &dependencies {
                state
                    .dependents_by_pane
                    .entry(dependency.pane)
                    .or_default()
                    .insert(id);
                if let DependencyObservation::ActiveManaged(dependency_id) = dependency.observation
                {
                    state
                        .dependents_by_run
                        .entry(dependency_id)
                        .or_default()
                        .insert(id);
                }
            }
            state.runs.insert(
                id,
                RunNode {
                    owner,
                    dependencies,
                    allow_failure,
                    ready: false,
                    terminal: None,
                    waiter: Some(sender),
                },
            );
            let owner_state = state
                .panes
                .get_mut(&owner)
                .expect("owner state inserted above");
            let owner_was_shell = matches!(owner_state.active, Some(ActiveOperation::Shell(_)));
            owner_state.active = Some(ActiveOperation::Managed(id));
            owner_state.managed_shell_result_pending = owner_was_shell;
            // The shell's command-start event and the process-control request
            // travel through different queues. Even though the wrapper was
            // launched from that shell, the request can reach the UI first.
            // Rebind any dependents that were waiting for the owner's next
            // command, or that already saw it as an active shell command, so
            // they cannot wait forever on the wrapper's later completion
            // marker.
            let mut roots = vec![id];
            roots.extend(replace_pending_command_observations(
                &mut state,
                owner,
                DependencyObservation::ActiveManaged(id),
            ));
            resolve_locked(&mut state, roots);
            (receiver, id)
        };

        Ok(RunRegistration { id, receiver })
    }

    /// Marks the shell integration as available.  Repeated markers are safe.
    pub(crate) fn tracking_ready(&self, pane: RunPaneIdentity) {
        let mut state = self.lock_state();
        let first_ready = {
            let pane_state = state.panes.entry(pane).or_default();
            if !pane_state.available {
                return;
            }
            let first_ready = !pane_state.tracking_ready;
            pane_state.available = true;
            pane_state.tracking_ready = true;
            // The integration command itself can run through a shell before it
            // emits this marker. It is setup, not the first user command that
            // a newly registered wait should observe. Never disturb a managed
            // wrapper, though: its connection owns the authoritative state.
            if first_ready && matches!(pane_state.active, Some(ActiveOperation::Shell(_))) {
                pane_state.active = None;
            }
            first_ready
        };
        if first_ready {
            let roots = replace_shell_observations(
                &mut state,
                pane,
                DependencyObservation::WaitingForCommand,
            );
            resolve_locked(&mut state, roots);
        }
    }

    /// Reopens an identity after a daemon handoff has been attached again.
    /// Existing dependency observations remain snapshots; only registrations
    /// made after the new terminal reports tracking readiness may use it.
    pub(crate) fn pane_reopened(&self, pane: RunPaneIdentity) {
        let mut state = self.lock_state();
        let pane_state = state.panes.entry(pane).or_default();
        if pane_state.available {
            return;
        }
        pane_state.available = true;
        pane_state.tracking_ready = false;
        pane_state.active = None;
        pane_state.last_result = None;
        pane_state.managed_shell_result_pending = false;
    }

    /// Records a command that is currently executing in a pane.  A managed
    /// wrapper owns its pane state once registration has completed, so a late
    /// shell marker cannot hide the graph node.
    pub(crate) fn command_started(&self, pane: RunPaneIdentity, command: String) {
        let mut state = self.lock_state();
        let roots = {
            let pane_state = state.panes.entry(pane).or_default();
            if !pane_state.available {
                return;
            }
            pane_state.available = true;
            match pane_state.active {
                Some(ActiveOperation::Managed(id)) => {
                    pane_state.active = Some(ActiveOperation::Managed(id));
                    Vec::new()
                }
                _ if is_internal_startup_command(&command) => {
                    pane_state.active = Some(ActiveOperation::Ignored);
                    Vec::new()
                }
                _ => {
                    pane_state.managed_shell_result_pending = false;
                    pane_state.active = Some(ActiveOperation::Shell(command));
                    // An idle dependency is deliberately represented as a
                    // wait for the next command rather than as its previous
                    // result. Only a real command-start event may move that
                    // observation to the active-command state.
                    replace_waiting_observations(
                        &mut state,
                        pane,
                        DependencyObservation::ActiveShell,
                    )
                }
            }
        };
        resolve_locked(&mut state, roots);
    }

    /// Records a shell command result.  `None` is deliberately retained as an
    /// unavailable result: it is not a success and cannot satisfy a later wait.
    pub(crate) fn command_finished(&self, pane: RunPaneIdentity, exit_code: Option<i32>) {
        let mut state = self.lock_state();
        let active = {
            let pane_state = state.panes.entry(pane).or_default();
            if !pane_state.available {
                return;
            }
            pane_state.available = true;
            pane_state.tracking_ready = true;

            // A managed wrapper has a second, authoritative completion path:
            // `run_complete` on its process-control connection.  The shell's
            // prompt marker can race that message, especially when the child
            // exits by signal or when the shell is slow to redraw.  Never let
            // that duplicate marker complete the graph node early.
            if matches!(pane_state.active, Some(ActiveOperation::Managed(_))) {
                return;
            }

            let active = pane_state.active.take();

            // Ignore every shell marker corresponding to a managed wrapper
            // after the process-control result has already been recorded. The
            // next command-start event clears this guard, which also handles
            // duplicate subscriptions without losing the next command.
            if pane_state.managed_shell_result_pending {
                return;
            }

            active
        };
        match active {
            // The state check above makes this unreachable. Keep the arm so
            // a future event source cannot accidentally bypass the two-phase
            // protocol if the state transition is changed.
            Some(ActiveOperation::Managed(_) | ActiveOperation::Ignored) => (),
            _ => {
                let result = last_result_from_exit_code(exit_code);
                state
                    .panes
                    .get_mut(&pane)
                    .expect("pane state inserted above")
                    .last_result = Some(result);
                let roots = replace_shell_observations(
                    &mut state,
                    pane,
                    DependencyObservation::Last(result),
                );
                resolve_locked(&mut state, roots);
            }
        }
    }

    /// Explicitly records a result for registry tests.
    #[cfg(test)]
    pub(crate) fn record_result(&self, pane: RunPaneIdentity, exit_code: Option<i32>) {
        let mut state = self.lock_state();
        {
            let pane_state = state.panes.entry(pane).or_default();
            pane_state.available = true;
            pane_state.tracking_ready = true;
            pane_state.active = None;
            pane_state.managed_shell_result_pending = false;
        }
        let result = last_result_from_exit_code(exit_code);
        state
            .panes
            .get_mut(&pane)
            .expect("pane state inserted above")
            .last_result = Some(result);
        let roots =
            replace_shell_observations(&mut state, pane, DependencyObservation::Last(result));
        resolve_locked(&mut state, roots);
    }

    /// Marks a pane unavailable and fails any managed wrapper that owned it.
    pub(crate) fn pane_closed(&self, pane: RunPaneIdentity) {
        let mut state = self.lock_state();
        let active = {
            let pane_state = state.panes.entry(pane).or_default();
            pane_state.available = false;
            pane_state.tracking_ready = false;
            pane_state.active.take()
        };
        state
            .panes
            .get_mut(&pane)
            .expect("pane state inserted above")
            .managed_shell_result_pending = false;
        let mut roots = replace_pane_observations(&mut state, pane, DependencyObservation::Closed);
        let mut failure = None;
        if let Some(ActiveOperation::Managed(id)) = active {
            failure = fail_locked(&mut state, id, "the dependency pane closed");
            if let Some((_, _, dependents)) = &failure {
                roots.push(id);
                roots.extend(dependents.iter().copied());
            }
        }
        resolve_locked(&mut state, roots);
        drop(state);
        send_failure(failure);
    }

    pub(crate) fn terminal_lost(&self, pane: RunPaneIdentity) {
        self.pane_closed(pane);
    }

    /// Called when the two-phase process-control connection disappears before
    /// the wrapper reports its child result.
    pub(crate) fn connection_lost(&self, id: u64) {
        self.fail(id, "the run wrapper connection was lost");
    }

    /// Fails all unresolved wrappers when the owning Zetta process shuts down.
    pub(crate) fn shutdown(&self) {
        let ids = {
            let state = self.lock_state();
            state
                .runs
                .iter()
                .filter_map(|(id, node)| node.terminal.is_none().then_some(*id))
                .collect::<Vec<_>>()
        };
        for id in ids {
            self.fail(id, "the Zetta process is shutting down");
        }
    }

    pub(crate) fn complete(&self, id: u64, exit_code: Option<i32>) {
        let mut state = self.lock_state();
        let roots = std::iter::once(id)
            .chain(complete_locked(&mut state, id, exit_code))
            .collect::<Vec<_>>();
        resolve_locked(&mut state, roots);
    }

    fn fail(&self, id: u64, reason: impl Into<String>) {
        let mut state = self.lock_state();
        let failure = fail_locked(&mut state, id, reason);
        let roots = failure
            .as_ref()
            .map(|(_, _, dependents)| {
                std::iter::once(id)
                    .chain(dependents.iter().copied())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        resolve_locked(&mut state, roots);
        drop(state);
        send_failure(failure);
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, RegistryState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Clone, Debug)]
enum Evaluation {
    Pending,
    Ready,
    Failed { message: String, structural: bool },
}

fn evaluate_run(state: &RegistryState, id: u64, stack: &mut Vec<u64>) -> Option<Evaluation> {
    let node = state.runs.get(&id)?;
    if let Some(result) = &node.terminal {
        return Some(if result.resolution == RunResolution::Ready {
            Evaluation::Ready
        } else {
            Evaluation::Failed {
                message: result
                    .message
                    .clone()
                    .unwrap_or_else(|| "managed dependency failed".to_owned()),
                structural: false,
            }
        });
    }
    // A ready wrapper has been released to its child process, but its actual
    // exit status is not known yet.  Downstream runs must wait for that
    // status rather than treating the dependency decision as completion.
    if node.ready {
        return Some(Evaluation::Pending);
    }
    if stack.contains(&id) {
        return Some(Evaluation::Failed {
            message: "run dependency cycle detected".to_owned(),
            structural: true,
        });
    }
    stack.push(id);
    let mut pending = false;
    for dependency in &node.dependencies {
        let evaluation = match dependency.observation {
            DependencyObservation::Missing => Evaluation::Failed {
                message: "a dependency pane is no longer available".to_owned(),
                structural: true,
            },
            DependencyObservation::Closed => Evaluation::Failed {
                message: "a dependency pane closed".to_owned(),
                structural: true,
            },
            DependencyObservation::WaitingForCommand | DependencyObservation::ActiveShell => {
                Evaluation::Pending
            }
            DependencyObservation::ActiveManaged(dependency_id) => {
                let dependency_was_terminal = state
                    .runs
                    .get(&dependency_id)
                    .is_some_and(|node| node.terminal.is_some());
                evaluate_run(state, dependency_id, stack)
                    .map(|evaluation| match evaluation {
                        // A managed dependency is not satisfied merely because
                        // its own prerequisites are ready. Its wrapper still
                        // has to run its child and report the child's result.
                        Evaluation::Ready if !dependency_was_terminal => Evaluation::Pending,
                        evaluation => evaluation,
                    })
                    .unwrap_or_else(|| {
                        // A terminal managed node is normally replaced in every
                        // dependent before it is retired. Keep this fallback for
                        // an already-snapshotted dependent whose link was
                        // cleaned up first: the pane's recorded result is still
                        // authoritative, while an active or unknown pane must
                        // remain a hard failure rather than being released.
                        match state
                            .panes
                            .get(&dependency.pane)
                            .map(recorded_result)
                            .unwrap_or(DependencyObservation::Missing)
                        {
                            DependencyObservation::Last(result) => evaluate_last_result(result),
                            _ => Evaluation::Failed {
                                message: "managed dependency result is unavailable".to_owned(),
                                structural: true,
                            },
                        }
                    })
            }
            DependencyObservation::Last(result) => evaluate_last_result(result),
            DependencyObservation::Unknown => Evaluation::Failed {
                message: "dependency has no trustworthy command result".to_owned(),
                structural: true,
            },
        };
        match evaluation {
            Evaluation::Ready => {}
            Evaluation::Pending => pending = true,
            Evaluation::Failed {
                message,
                structural,
            } => {
                if structural || !node.allow_failure {
                    stack.pop();
                    return Some(Evaluation::Failed {
                        message,
                        structural,
                    });
                }
            }
        }
    }
    stack.pop();
    Some(if pending {
        Evaluation::Pending
    } else {
        Evaluation::Ready
    })
}

fn last_result_from_exit_code(exit_code: Option<i32>) -> LastResult {
    match exit_code {
        Some(0) => LastResult::Success(0),
        Some(code) => LastResult::Failure(code),
        None => LastResult::Unavailable,
    }
}

fn evaluate_last_result(result: LastResult) -> Evaluation {
    match result {
        LastResult::Success(_) => Evaluation::Ready,
        LastResult::Failure(code) => Evaluation::Failed {
            message: format!("dependency exited with status {code}"),
            structural: false,
        },
        LastResult::Unavailable => Evaluation::Failed {
            message: "dependency exit status was unavailable".to_owned(),
            structural: true,
        },
    }
}

fn recorded_result(pane_state: &PaneState) -> DependencyObservation {
    if pane_state.tracking_ready {
        pane_state
            .last_result
            .map(DependencyObservation::Last)
            .unwrap_or(DependencyObservation::Unknown)
    } else {
        DependencyObservation::Unknown
    }
}

fn snapshot_dependency(state: &RegistryState, pane: RunPaneIdentity) -> DependencyObservation {
    match state.panes.get(&pane) {
        None => DependencyObservation::Missing,
        Some(pane_state) if !pane_state.available => DependencyObservation::Closed,
        Some(pane_state) => match pane_state.active {
            Some(ActiveOperation::Managed(id)) if state.runs.contains_key(&id) => {
                DependencyObservation::ActiveManaged(id)
            }
            Some(ActiveOperation::Managed(_)) => recorded_result(pane_state),
            Some(ActiveOperation::Ignored) => DependencyObservation::WaitingForCommand,
            Some(ActiveOperation::Shell(_)) => DependencyObservation::ActiveShell,
            // A previous result is intentionally not a valid snapshot for a
            // newly registered wait. The dependency must observe a command
            // that starts after registration (or one already active when the
            // snapshot is taken).
            None => DependencyObservation::WaitingForCommand,
        },
    }
}

fn is_internal_startup_command(command: &str) -> bool {
    command.contains("__zed_init_command_ready_")
        || command.contains("__ZETTA_LIFECYCLE_TRACKING_")
        || (command.contains("ZETTA_HOST_EXECUTABLE") && command.contains(" init "))
}

fn complete_locked(state: &mut RegistryState, id: u64, exit_code: Option<i32>) -> Vec<u64> {
    let Some(node) = state.runs.get_mut(&id) else {
        return Vec::new();
    };
    if node.terminal.is_some() {
        return Vec::new();
    }
    let result = if exit_code == Some(0) {
        RunResolutionMessage::ready()
    } else {
        RunResolutionMessage::failed(match exit_code {
            Some(code) => format!("managed run exited with status {code}"),
            None => "managed run exited without a status".to_owned(),
        })
    };
    let owner = node.owner;
    let waiter = node.waiter.take();
    node.terminal = Some(result.clone());
    if let Some(pane) = state.panes.get_mut(&owner) {
        pane.active = None;
        pane.tracking_ready = true;
        pane.last_result = Some(last_result_from_exit_code(exit_code));
        // The shell emits its prompt completion after the wrapper has sent
        // `run_complete`. Consume that duplicate without replacing the
        // managed result with a shell-observed status.
        pane.managed_shell_result_pending = true;
    }
    let mut dependents = replace_managed_observations(
        state,
        id,
        DependencyObservation::Last(last_result_from_exit_code(exit_code)),
    );
    // A downstream registration can race the managed wrapper's own
    // registration and snapshot its owner as an active shell command. The
    // process-control completion is authoritative for both observations.
    dependents.extend(replace_shell_observations(
        state,
        owner,
        DependencyObservation::Last(last_result_from_exit_code(exit_code)),
    ));
    if let Some(waiter) = waiter {
        let _ = waiter.send(result);
    }
    dependents
}

fn fail_locked(
    state: &mut RegistryState,
    id: u64,
    reason: impl Into<String>,
) -> Option<(
    Option<Sender<RunResolutionMessage>>,
    RunResolutionMessage,
    Vec<u64>,
)> {
    let node = state.runs.get_mut(&id)?;
    if node.terminal.is_some() {
        return None;
    }
    let result = RunResolutionMessage::failed(reason);
    let owner = node.owner;
    let waiter = node.waiter.take();
    node.terminal = Some(result.clone());
    if let Some(pane) = state.panes.get_mut(&owner) {
        pane.active = None;
        pane.tracking_ready = true;
        pane.last_result = Some(LastResult::Unavailable);
        pane.managed_shell_result_pending = true;
    }
    let mut dependents = replace_managed_observations(
        state,
        id,
        DependencyObservation::Last(LastResult::Unavailable),
    );
    dependents.extend(replace_shell_observations(
        state,
        owner,
        DependencyObservation::Last(LastResult::Unavailable),
    ));
    Some((waiter, result, dependents))
}

fn send_failure(
    failure: Option<(
        Option<Sender<RunResolutionMessage>>,
        RunResolutionMessage,
        Vec<u64>,
    )>,
) {
    if let Some((Some(waiter), result, _)) = failure {
        let _ = waiter.send(result);
    }
}

fn resolve_locked(state: &mut RegistryState, roots: impl IntoIterator<Item = u64>) {
    let mut queue = VecDeque::new();
    let mut queued = HashSet::new();
    for root in roots {
        if queued.insert(root) {
            queue.push_back(root);
        }
    }
    let mut terminal_ids = HashSet::new();

    while let Some(id) = queue.pop_front() {
        queued.remove(&id);
        if state
            .runs
            .get(&id)
            .is_some_and(|node| node.terminal.is_some())
        {
            terminal_ids.insert(id);
            continue;
        }
        let Some(decision) = evaluate_run(state, id, &mut Vec::new()) else {
            continue;
        };
        match decision {
            Evaluation::Pending => {}
            Evaluation::Ready => {
                let Some(node) = state.runs.get_mut(&id) else {
                    continue;
                };
                if node.terminal.is_some() || node.ready {
                    continue;
                }
                node.ready = true;
                if let Some(waiter) = node.waiter.take() {
                    let _ = waiter.send(RunResolutionMessage::ready());
                }
            }
            Evaluation::Failed {
                message,
                structural,
            } => {
                let Some(node) = state.runs.get_mut(&id) else {
                    continue;
                };
                if node.terminal.is_some() {
                    continue;
                }
                let result = RunResolutionMessage::failed(message);
                let owner = node.owner;
                let waiter = node.waiter.take();
                node.terminal = Some(result.clone());
                let last_result = if structural {
                    LastResult::Unavailable
                } else {
                    // A wrapper that cannot release because a known nonzero
                    // dependency failed exits with the normal error status
                    // used by `run_wait_command`.
                    LastResult::Failure(1)
                };
                if let Some(pane) = state.panes.get_mut(&owner) {
                    pane.active = None;
                    pane.tracking_ready = true;
                    pane.last_result = Some(last_result);
                    pane.managed_shell_result_pending = true;
                }
                let mut dependents = replace_managed_observations(
                    state,
                    id,
                    DependencyObservation::Last(last_result),
                );
                dependents.extend(replace_shell_observations(
                    state,
                    owner,
                    DependencyObservation::Last(last_result),
                ));
                for dependent in dependents {
                    if queued.insert(dependent) {
                        queue.push_back(dependent);
                    }
                }
                if let Some(waiter) = waiter {
                    let _ = waiter.send(result);
                }
                terminal_ids.insert(id);
            }
        }
    }

    for id in terminal_ids {
        remove_run(state, id);
    }
}

fn replace_shell_observations(
    state: &mut RegistryState,
    pane: RunPaneIdentity,
    observation: DependencyObservation,
) -> Vec<u64> {
    replace_pane_observations_with(state, pane, observation, |current| {
        current == DependencyObservation::ActiveShell
    })
}

fn replace_pending_command_observations(
    state: &mut RegistryState,
    pane: RunPaneIdentity,
    observation: DependencyObservation,
) -> Vec<u64> {
    replace_pane_observations_with(state, pane, observation, |current| {
        matches!(
            current,
            DependencyObservation::WaitingForCommand | DependencyObservation::ActiveShell
        )
    })
}

fn replace_waiting_observations(
    state: &mut RegistryState,
    pane: RunPaneIdentity,
    observation: DependencyObservation,
) -> Vec<u64> {
    replace_pane_observations_with(state, pane, observation, |current| {
        current == DependencyObservation::WaitingForCommand
    })
}

fn replace_pane_observations(
    state: &mut RegistryState,
    pane: RunPaneIdentity,
    observation: DependencyObservation,
) -> Vec<u64> {
    replace_pane_observations_with(state, pane, observation, |current| {
        matches!(
            current,
            DependencyObservation::WaitingForCommand
                | DependencyObservation::ActiveShell
                | DependencyObservation::ActiveManaged(_)
        )
    })
}

fn replace_managed_observations(
    state: &mut RegistryState,
    run_id: u64,
    observation: DependencyObservation,
) -> Vec<u64> {
    let dependent_ids = state
        .dependents_by_run
        .get(&run_id)
        .cloned()
        .unwrap_or_default();
    replace_observations(state, dependent_ids, observation, |dependency| {
        dependency.observation == DependencyObservation::ActiveManaged(run_id)
    })
}

fn replace_pane_observations_with(
    state: &mut RegistryState,
    pane: RunPaneIdentity,
    observation: DependencyObservation,
    predicate: impl Fn(DependencyObservation) -> bool,
) -> Vec<u64> {
    let dependent_ids = state
        .dependents_by_pane
        .get(&pane)
        .cloned()
        .unwrap_or_default();
    replace_observations(state, dependent_ids, observation, |dependency| {
        dependency.pane == pane && predicate(dependency.observation)
    })
}

fn replace_observations(
    state: &mut RegistryState,
    dependent_ids: HashSet<u64>,
    observation: DependencyObservation,
    predicate: impl Fn(&RunDependency) -> bool,
) -> Vec<u64> {
    let mut changed = HashSet::new();
    for dependent_id in dependent_ids {
        let indices = state
            .runs
            .get(&dependent_id)
            .map(|node| {
                node.dependencies
                    .iter()
                    .enumerate()
                    .filter_map(|(index, dependency)| predicate(dependency).then_some(index))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for index in indices {
            if set_dependency_observation(state, dependent_id, index, observation) {
                changed.insert(dependent_id);
            }
        }
    }
    changed.into_iter().collect()
}

fn set_dependency_observation(
    state: &mut RegistryState,
    dependent_id: u64,
    dependency_index: usize,
    observation: DependencyObservation,
) -> bool {
    let Some(current) = state
        .runs
        .get(&dependent_id)
        .and_then(|node| node.dependencies.get(dependency_index))
        .map(|dependency| dependency.observation)
    else {
        return false;
    };
    if current == observation {
        return false;
    }
    if let DependencyObservation::ActiveManaged(run_id) = current {
        remove_dependency_link(&mut state.dependents_by_run, run_id, dependent_id);
    }
    if let DependencyObservation::ActiveManaged(run_id) = observation {
        state
            .dependents_by_run
            .entry(run_id)
            .or_default()
            .insert(dependent_id);
    }
    let Some(node) = state.runs.get_mut(&dependent_id) else {
        return false;
    };
    let Some(dependency) = node.dependencies.get_mut(dependency_index) else {
        return false;
    };
    dependency.observation = observation;
    true
}

fn remove_run(state: &mut RegistryState, id: u64) {
    let Some(node) = state.runs.remove(&id) else {
        return;
    };
    // Retiring a node is the final graph cleanup step. Normally its owner was
    // cleared by the transition that made it terminal, but clear the identity
    // here as well so a late/reordered terminal event cannot leave a pane
    // pointing at a node that no longer exists.
    for pane in state.panes.values_mut() {
        if matches!(pane.active, Some(ActiveOperation::Managed(active_id)) if active_id == id) {
            pane.active = None;
            pane.managed_shell_result_pending = true;
        }
    }
    for dependency in node.dependencies {
        remove_dependency_link(&mut state.dependents_by_pane, dependency.pane, id);
        if let DependencyObservation::ActiveManaged(run_id) = dependency.observation {
            remove_dependency_link(&mut state.dependents_by_run, run_id, id);
        }
    }
    state.dependents_by_run.remove(&id);
}

fn remove_dependency_link<K: Eq + Hash>(
    links: &mut HashMap<K, HashSet<u64>>,
    key: K,
    dependent_id: u64,
) {
    let Some(dependents) = links.get_mut(&key) else {
        return;
    };
    dependents.remove(&dependent_id);
    if dependents.is_empty() {
        links.remove(&key);
    }
}

#[cfg(test)]
#[path = "tests/run_command.rs"]
mod tests;
