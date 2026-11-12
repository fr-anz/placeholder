//! Orchestrator's reviewed plans create real native sessions. In a Git
//! checkout every task gets its own worktree cut from the run's integration
//! branch, is reviewed by the Reviewer role, and merges back behind the
//! project's check command. The app drives that loop without model calls; the
//! orchestrator is only asked when a task needs judgment. Outside Git, workers
//! share the checkout and overlapping scopes are serialized.
use super::*;
use monocode_core::{
    agent::Settings,
    orchestration::{
        self,
        pipeline::{self, Decision, Escalate, Reason, Verdict},
        worktrees::{self, Merge, ProjectSetup, RunTree, TaskTree},
        Plan, Task,
    },
    roles,
};
use std::path::Path;

/// Agents (workers and reviewers) running at once in one run.
const PARALLEL: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Pending,
    Starting,
    Working,
    /// The app is committing the worker's changes.
    Committing,
    Reviewing,
    /// Approved and queued for the integration branch.
    Merging,
    /// Waiting for the orchestrator's decision.
    Escalated,
    Done,
    Skipped,
    /// Replaced by smaller tasks, appended to the run so indices stay stable.
    Split,
    Failed,
    Blocked,
}
impl State {
    fn settled(self) -> bool {
        matches!(
            self,
            State::Done | State::Skipped | State::Split | State::Failed | State::Blocked
        )
    }
    fn active(self) -> bool {
        matches!(
            self,
            State::Starting | State::Working | State::Committing | State::Reviewing
        )
    }
}

/// What the user can do with a failed task from the sidebar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum TaskAction {
    Retry,
    Rerun,
    Skip,
}

struct Assignment {
    task: Task,
    /// Resolved from the task's worker role when the run starts.
    provider: String,
    settings: Settings,
    role: Option<roles::Role>,
    session: Option<i64>,
    reviewer: Option<i64>,
    tree: Option<TaskTree>,
    state: State,
    /// The worker's latest report.
    report: String,
    /// The latest review issues or failure detail.
    note: String,
    /// `git diff --stat` against the integration branch.
    stat: String,
    /// Automatic fix rounds used, on review feedback or a failed merge or check.
    attempts: u32,
    /// The worker is fixing a failed merge or check; the fix is reviewed again.
    fixing_merge: bool,
    /// Created by a split, so it is not split again.
    piece: bool,
    /// Fresh restarts, which also name the worktree.
    reruns: u32,
    /// Extra instructions for a rerun.
    extra: String,
    /// An escalation waiting for the orchestrator to be free.
    packet: Option<String>,
    reason: Option<Reason>,
    /// A follow-up prompt went to this session; its idle state means nothing
    /// until it has been seen busy with the new turn.
    sent_to: Option<i64>,
}
impl Assignment {
    fn status(&self) -> &'static str {
        match self.state {
            State::Pending | State::Starting => "queued",
            State::Working | State::Committing => "running",
            State::Reviewing => "reviewing",
            State::Merging => "merging",
            State::Escalated => "escalated",
            State::Done => "completed",
            State::Skipped => "skipped",
            State::Split => "split",
            State::Failed => "failed",
            State::Blocked => "blocked",
        }
    }
}
pub(super) struct Run {
    /// The checkout the run started from.
    cwd: String,
    /// The lead's project, where worker sessions are listed.
    project: String,
    title: String,
    tasks: Vec<Assignment>,
    stopped: bool,
    /// The integration worktree is still being created.
    preparing: bool,
    git: Option<RunTree>,
    setup: ProjectSetup,
    reviewer: Option<(String, Settings, roles::Role)>,
    escalate: Escalate,
    roles: Vec<roles::Role>,
    /// The lead's provider and worker settings, for reruns on another role.
    provider: String,
    settings: Settings,
    summary: String,
    /// Answers escalations in its own small session, created on the first one.
    decider: Option<i64>,
    decider_role: roles::Role,
    /// The task whose escalation the decider is answering.
    asking: Option<usize>,
    asked_busy: bool,
    merging: Option<usize>,
    reported: bool,
    notice: Option<String>,
}
impl Run {
    fn settled(&self) -> bool {
        !self.preparing
            && self.asking.is_none()
            && self.merging.is_none()
            && self.tasks.iter().all(|t| t.state.settled())
    }
}

enum Job {
    Dispatch(usize),
    Commit(usize),
    Review(usize),
    Merge(usize),
    /// An escalation for the decider, about the task at this index.
    Ask(usize, String),
    Send(i64, String),
    Remove(TaskTree),
}

impl Shell {
    pub(super) fn observe_orchestration(
        &mut self,
        view: &Entity<Conversation>,
        id: i64,
        cx: &mut Context<Self>,
    ) {
        use crate::conversation::composer_features::{StartOrchestration, StopOrchestration};
        self.observations.push(cx.subscribe(
            view,
            move |this, _, event: &StartOrchestration, cx| {
                this.start_orchestration(id, event.0.clone(), cx)
            },
        ));
        self.observations.push(
            cx.subscribe(view, move |this, _, _: &StopOrchestration, cx| {
                this.stop_orchestration(id, cx)
            }),
        );
    }
    pub(super) fn start_orchestration(&mut self, lead: i64, plan: Plan, cx: &mut Context<Self>) {
        if self
            .orchestrations
            .get(&lead)
            .is_some_and(|run| !run.settled())
        {
            return;
        }
        let (worker_roles, escalate, decider_role) = locked(&self.store)
            .map(|store| {
                (
                    roles::load(&store),
                    pipeline::load_escalate(&store),
                    roles::load_decider(&store),
                )
            })
            .unwrap_or_else(|_| (roles::defaults(), Escalate::default(), roles::Role::default()));
        if let Err(error) = orchestration::validate(&plan)
            .and_then(|()| orchestration::validate_roles(&plan, &worker_roles))
        {
            self.error = Some(error);
            cx.notify();
            return;
        }
        let Some(view) = self.sessions.get(&lead) else {
            return;
        };
        let conversation = view.read(cx);
        if conversation.busy {
            return;
        }
        let project = conversation.summary.cwd.clone();
        let cwd = conversation
            .settings
            .worktree_cwd
            .clone()
            .unwrap_or_else(|| project.clone());
        let git = worktrees::supported(Path::new(&cwd));
        if !git
            && self
                .orchestrations
                .iter()
                .any(|(l, run)| *l != lead && !run.settled() && run.git.is_none() && run.cwd == cwd)
        {
            self.error = Some("An orchestration is already using this checkout.".into());
            cx.notify();
            return;
        }
        let mut settings = conversation.settings.clone();
        settings.plan = false;
        settings.orchestrator = false;
        settings.operator = false;
        settings.operator_enabled = false;
        settings.handoff = None;
        settings.turns.clear();
        settings.environment.clear();
        settings.attachments.clear();
        let provider = conversation.summary.provider.clone();
        let setup = locked(&self.store)
            .map(|store| worktrees::load_setup(&store, &project))
            .unwrap_or_default();
        let reviewer = roles::find(&worker_roles, Some(pipeline::REVIEW_ROLE)).map(|role| {
            let (provider, settings) = roles::apply(Some(role), &provider, &settings);
            (provider, settings, role.clone())
        });
        // A settled run being replaced can no longer retry its failed tasks.
        if let Some(old) = self.orchestrations.get(&lead).and_then(|run| run.git.clone()) {
            cx.background_executor()
                .spawn(async move { worktrees::finish(&old) })
                .detach();
        }
        let tasks = plan
            .tasks
            .into_iter()
            .map(|task| {
                let role = roles::find(&worker_roles, task.role.as_deref()).cloned();
                let (provider, settings) = roles::apply(role.as_ref(), &provider, &settings);
                Assignment {
                    task,
                    provider,
                    settings,
                    role,
                    session: None,
                    reviewer: None,
                    tree: None,
                    state: State::Pending,
                    report: String::new(),
                    note: String::new(),
                    stat: String::new(),
                    attempts: 0,
                    fixing_merge: false,
                    piece: false,
                    reruns: 0,
                    extra: String::new(),
                    packet: None,
                    reason: None,
                    sent_to: None,
                }
            })
            .collect();
        self.orchestrations.insert(
            lead,
            Run {
                cwd: cwd.clone(),
                project,
                title: plan.title.clone(),
                summary: plan.summary.clone(),
                tasks,
                stopped: false,
                preparing: git,
                git: None,
                setup,
                reviewer,
                escalate,
                roles: worker_roles,
                provider,
                settings,
                decider: None,
                decider_role,
                asking: None,
                asked_busy: false,
                merging: None,
                reported: false,
                notice: None,
            },
        );
        if let Some(view) = self.sessions.get(&lead) {
            view.update(cx, |v, _| v.orchestration_report = None);
        }
        if git {
            let title = plan.title;
            let work = cx
                .background_executor()
                .spawn(async move { worktrees::start(Path::new(&cwd), &title) });
            cx.spawn(async move |this, cx| {
                let result = work.await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(run) = this.orchestrations.get_mut(&lead) {
                        run.preparing = false;
                        match result {
                            Ok(tree) => run.git = Some(tree),
                            Err(error) => {
                                run.notice = Some(format!(
                                    "Worktrees unavailable, so agents share the checkout: {error}"
                                ))
                            }
                        }
                    }
                    this.schedule_orchestration(lead, cx);
                });
            })
            .detach();
        }
        self.schedule_orchestration(lead, cx);
    }
    fn stop_orchestration(&mut self, lead: i64, cx: &mut Context<Self>) {
        let mut stop = Vec::new();
        if let Some(run) = self.orchestrations.get_mut(&lead) {
            run.stopped = true;
            if run.asking.take().is_some() {
                stop.extend(run.decider);
            }
            let merging = run.merging;
            for (ix, task) in run.tasks.iter_mut().enumerate() {
                match task.state {
                    // A Starting task is still being created; its dispatch
                    // settles it, so the run outlives every in-flight start.
                    State::Pending => task.state = State::Blocked,
                    State::Merging if merging != Some(ix) => {
                        task.state = State::Failed;
                        task.note = "Stopped before merging".into();
                    }
                    State::Working => stop.extend(task.session),
                    State::Reviewing => stop.extend(task.reviewer),
                    State::Escalated => {
                        task.state = State::Failed;
                        task.packet = None;
                    }
                    _ => {}
                }
            }
        }
        for id in stop {
            if let Some(view) = self.sessions.get(&id) {
                view.update(cx, |v, cx| v.stop_if_busy(cx));
            }
        }
        self.schedule_orchestration(lead, cx);
    }
    /// Whether a sidebar task can be retried, rerun or skipped.
    pub(super) fn orchestration_task_actionable(&self, lead: i64, ix: usize) -> bool {
        self.orchestrations.get(&lead).is_some_and(|run| {
            !run.stopped && run.tasks.get(ix).is_some_and(|t| t.state == State::Failed)
        })
    }
    pub(super) fn orchestration_task_action(
        &mut self,
        lead: i64,
        ix: usize,
        action: TaskAction,
        cx: &mut Context<Self>,
    ) {
        if !self.orchestration_task_actionable(lead, ix) {
            return;
        }
        let decision = match action {
            TaskAction::Retry => Decision::Retry(String::new()),
            TaskAction::Rerun => Decision::Rerun {
                role: None,
                notes: String::new(),
            },
            TaskAction::Skip => Decision::Skip,
        };
        let jobs = self.apply_decision(lead, ix, decision, cx);
        self.run_jobs(lead, jobs, cx);
        self.schedule_orchestration(lead, cx);
    }
    pub(super) fn orchestration_session_changed(&mut self, id: i64, cx: &mut Context<Self>) {
        let leads: Vec<_> = self
            .orchestrations
            .iter()
            .filter(|(_, run)| {
                run.decider == Some(id)
                    || run
                        .tasks
                        .iter()
                        .any(|t| t.session == Some(id) || t.reviewer == Some(id))
            })
            .map(|(lead, _)| *lead)
            .collect();
        let busy = self.sessions.get(&id).is_some_and(|v| v.read(cx).busy);
        for lead in leads {
            let mut jobs = Vec::new();
            if let Some(run) = self.orchestrations.get_mut(&lead) {
                for task in &mut run.tasks {
                    if busy && task.sent_to == Some(id) {
                        task.sent_to = None;
                    }
                }
            }
            let fresh = |run: &Run, ix: usize| run.tasks[ix].sent_to != Some(id);
            let idle = |this: &Self, session: i64| {
                this.sessions
                    .get(&session)
                    .map(|view| (!view.read(cx).busy).then(|| view.read(cx).external_outcome(cx)))
            };
            let worker = self.orchestrations.get(&lead).and_then(|run| {
                run.tasks
                    .iter()
                    .position(|t| t.session == Some(id) && t.state == State::Working)
                    .filter(|&ix| fresh(run, ix))
            });
            if let Some(ix) = worker
                && let Some(Some((success, report))) = idle(self, id)
            {
                jobs.extend(self.worker_finished(lead, ix, success, report));
            }
            let reviewer = self.orchestrations.get(&lead).and_then(|run| {
                run.tasks
                    .iter()
                    .position(|t| t.reviewer == Some(id) && t.state == State::Reviewing)
                    .filter(|&ix| fresh(run, ix))
            });
            if let Some(ix) = reviewer
                && let Some(Some((success, review))) = idle(self, id)
            {
                jobs.extend(self.review_finished(lead, ix, success, review));
            }
            if self.orchestrations.get(&lead).is_some_and(|r| r.decider == Some(id)) {
                let answered = self.orchestrations.get_mut(&lead).and_then(|run| {
                    let ix = run.asking?;
                    if busy {
                        run.asked_busy = true;
                        return None;
                    }
                    if !run.asked_busy {
                        return None;
                    }
                    run.asking = None;
                    Some(ix)
                });
                if let Some(ix) = answered {
                    let reply = self
                        .sessions
                        .get(&id)
                        .map(|v| v.read(cx).latest_reply(cx))
                        .unwrap_or_default();
                    let decision = pipeline::parse_decision(&reply);
                    jobs.extend(self.apply_decision(lead, ix, decision, cx));
                }
            }
            self.run_jobs(lead, jobs, cx);
            self.schedule_orchestration(lead, cx);
        }
    }

    fn worker_finished(&mut self, lead: i64, ix: usize, success: bool, report: String) -> Vec<Job> {
        let Some(run) = self.orchestrations.get_mut(&lead) else {
            return Vec::new();
        };
        let stopped = run.stopped;
        let git = run.git.is_some();
        let task = &mut run.tasks[ix];
        task.report = report.clone();
        if stopped {
            task.state = State::Failed;
            task.note = "Stopped".into();
            return Vec::new();
        }
        if !success {
            return self.escalate(lead, ix, Reason::Worker, report);
        }
        if git && task.tree.is_some() {
            task.state = State::Committing;
            return vec![Job::Commit(ix)];
        }
        self.start_review(lead, ix)
    }
    /// After a worker's change is committed: review it, or merge a conflict fix.
    fn committed(&mut self, lead: i64, ix: usize, changed: bool, stat: String) -> Vec<Job> {
        let Some(run) = self.orchestrations.get_mut(&lead) else {
            return Vec::new();
        };
        let stopped = run.stopped;
        let task = &mut run.tasks[ix];
        task.stat = stat;
        if stopped {
            task.state = State::Failed;
            task.note = "Stopped".into();
            return Vec::new();
        }
        if !changed {
            // Report-only work (a scout) has nothing to review or merge.
            task.state = State::Done;
            return task.tree.take().map(Job::Remove).into_iter().collect();
        }
        // A merge or check fix is reviewed like any other change: resolving a
        // conflict can silently drop another task's work.
        task.fixing_merge = false;
        self.start_review(lead, ix)
    }
    fn start_review(&mut self, lead: i64, ix: usize) -> Vec<Job> {
        let Some(run) = self.orchestrations.get_mut(&lead) else {
            return Vec::new();
        };
        if run.reviewer.is_none() {
            return self.approved(lead, ix, String::new());
        }
        let task = &mut run.tasks[ix];
        task.state = State::Reviewing;
        match task.reviewer {
            // Later rounds reuse the reviewer, which keeps its context.
            Some(reviewer) => {
                let prompt = pipeline::review_prompt(
                    &task.task,
                    run.git.as_ref().map(|g| g.branch.as_str()),
                    &task.stat,
                    &task.report,
                    task.attempts,
                );
                task.sent_to = Some(reviewer);
                vec![Job::Send(reviewer, prompt)]
            }
            None => vec![Job::Review(ix)],
        }
    }
    fn review_finished(&mut self, lead: i64, ix: usize, success: bool, review: String) -> Vec<Job> {
        let Some(run) = self.orchestrations.get_mut(&lead) else {
            return Vec::new();
        };
        if run.stopped {
            run.tasks[ix].state = State::Failed;
            run.tasks[ix].note = "Stopped".into();
            return Vec::new();
        }
        if !success {
            return self.escalate(lead, ix, Reason::Worker, format!("The reviewer stopped:\n{review}"));
        }
        match pipeline::parse_verdict(&review) {
            Verdict::Approve => self.approved(lead, ix, review),
            Verdict::Replan(issues) => self.escalate(lead, ix, Reason::Replan, issues),
            Verdict::Changes(issues) => {
                let task = &mut run.tasks[ix];
                match task.session {
                    Some(worker) if task.attempts < pipeline::MAX_ATTEMPTS => {
                        task.attempts += 1;
                        task.note = issues.clone();
                        task.state = State::Working;
                        task.sent_to = Some(worker);
                        vec![Job::Send(worker, pipeline::fix_prompt(&issues))]
                    }
                    _ => self.escalate(lead, ix, Reason::StillFailing, issues),
                }
            }
        }
    }
    fn approved(&mut self, lead: i64, ix: usize, review: String) -> Vec<Job> {
        let Some(run) = self.orchestrations.get_mut(&lead) else {
            return Vec::new();
        };
        if run.escalate == Escalate::Always && run.tasks[ix].reason != Some(Reason::Approved) {
            return self.escalate(lead, ix, Reason::Approved, review);
        }
        let task = &mut run.tasks[ix];
        task.note.clear();
        if run.git.is_some() && task.tree.is_some() {
            task.state = State::Merging;
        } else {
            task.state = State::Done;
        }
        Vec::new()
    }
    fn merged(&mut self, lead: i64, ix: usize, result: Result<Merge, String>) -> Vec<Job> {
        let Some(run) = self.orchestrations.get_mut(&lead) else {
            return Vec::new();
        };
        run.merging = None;
        let integration = run.git.as_ref().map(|g| g.branch.clone()).unwrap_or_default();
        let task = &mut run.tasks[ix];
        let problem = match result {
            Ok(Merge::Merged) => {
                task.state = State::Done;
                return task.tree.take().map(Job::Remove).into_iter().collect();
            }
            Ok(Merge::Conflict(files)) => {
                format!("Merge conflict in:\n{}", files.join("\n"))
            }
            Ok(Merge::CheckFailed(output)) => output,
            Err(error) => {
                task.state = State::Failed;
                task.note = error;
                return Vec::new();
            }
        };
        match task.session {
            Some(worker) if task.attempts < pipeline::MAX_ATTEMPTS && !run.stopped => {
                task.attempts += 1;
                task.fixing_merge = true;
                task.note = problem.clone();
                task.state = State::Working;
                task.sent_to = Some(worker);
                vec![Job::Send(worker, pipeline::conflict_prompt(&integration, &problem))]
            }
            _ => self.escalate(lead, ix, Reason::Merge, problem),
        }
    }
    fn escalate(&mut self, lead: i64, ix: usize, reason: Reason, detail: String) -> Vec<Job> {
        let Some(run) = self.orchestrations.get_mut(&lead) else {
            return Vec::new();
        };
        if run.stopped {
            // A stopped run asks nobody; the task just ends.
            let task = &mut run.tasks[ix];
            task.state = State::Failed;
            task.note = detail;
            return Vec::new();
        }
        let roles = run.roles.clone();
        let live = run.tasks.iter().filter(|t| t.state != State::Split).count();
        let can_split = !run.tasks[ix].piece && live < 12;
        let task = &mut run.tasks[ix];
        task.packet = Some(pipeline::escalation(pipeline::Packet {
            task: &task.task,
            reason: reason.clone(),
            attempts: task.attempts,
            stat: &task.stat,
            detail: &detail,
            roles: &roles,
            can_split,
        }));
        task.note = detail;
        task.reason = Some(reason);
        task.state = State::Escalated;
        Vec::new()
    }
    fn apply_decision(
        &mut self,
        lead: i64,
        ix: usize,
        decision: Decision,
        _cx: &mut Context<Self>,
    ) -> Vec<Job> {
        let Some(run) = self.orchestrations.get_mut(&lead) else {
            return Vec::new();
        };
        let integration = run.git.as_ref().map(|g| g.branch.clone()).unwrap_or_default();
        let git = run.git.is_some();
        let task = &mut run.tasks[ix];
        task.packet = None;
        let reason = task.reason.take();
        match decision {
            Decision::Merge => {
                task.state = if git && task.tree.is_some() {
                    State::Merging
                } else {
                    State::Done
                };
                Vec::new()
            }
            Decision::Retry(notes) if task.session.is_some() && (task.tree.is_some() || !git) => {
                let worker = task.session.unwrap_or_default();
                task.attempts = 0;
                task.state = State::Working;
                let fix = if reason == Some(Reason::Merge) {
                    task.fixing_merge = true;
                    pipeline::conflict_prompt(&integration, &task.note)
                } else if task.note.trim().is_empty() {
                    "Continue the assignment from where you stopped, then report your changes and validation.".into()
                } else {
                    pipeline::fix_prompt(&task.note)
                };
                let prompt = if notes.trim().is_empty() {
                    fix
                } else {
                    format!("{}\n\n{fix}", notes.trim())
                };
                task.sent_to = Some(worker);
                vec![Job::Send(worker, prompt)]
            }
            // Without a live worker or worktree, a retry has to start fresh.
            Decision::Retry(notes) => self.rerun(lead, ix, None, notes),
            Decision::Rerun { role, notes } => self.rerun(lead, ix, role, notes),
            Decision::Skip => {
                task.state = State::Skipped;
                task.tree.take().map(Job::Remove).into_iter().collect()
            }
            Decision::Split(pieces) => self.split(lead, ix, pieces),
            Decision::Ask => {
                task.state = State::Failed;
                if task.note.trim().is_empty() {
                    task.note = "Left for you by the orchestrator".into();
                }
                Vec::new()
            }
        }
    }
    /// A fresh worker restarts the task in a new worktree, optionally on
    /// another role, carrying the latest issues forward.
    fn rerun(&mut self, lead: i64, ix: usize, role: Option<String>, notes: String) -> Vec<Job> {
        let Some(run) = self.orchestrations.get_mut(&lead) else {
            return Vec::new();
        };
        if let Some(role) = role.as_deref().and_then(|id| roles::find(&run.roles, Some(id))) {
            let (provider, settings) = roles::apply(Some(role), &run.provider, &run.settings);
            let task = &mut run.tasks[ix];
            task.role = Some(role.clone());
            task.provider = provider;
            task.settings = settings;
        }
        let task = &mut run.tasks[ix];
        let mut extra = notes.trim().to_owned();
        if !task.note.trim().is_empty() {
            extra = format!(
                "{extra}\n\nA previous attempt ran into:\n{}",
                monocode_core::response::limit(task.note.trim(), 4_000)
            )
            .trim()
            .to_owned();
        }
        task.extra = extra;
        task.reruns += 1;
        task.session = None;
        task.reviewer = None;
        task.attempts = 0;
        task.fixing_merge = false;
        task.state = State::Pending;
        task.tree.take().map(Job::Remove).into_iter().collect()
    }
    /// Replaces a task with the decider's smaller pieces. The original stays
    /// in place as Split and the pieces are appended, so the indices that
    /// in-flight work holds stay valid.
    fn split(&mut self, lead: i64, ix: usize, pieces: Vec<Task>) -> Vec<Job> {
        let Some(run) = self.orchestrations.get_mut(&lead) else {
            return Vec::new();
        };
        let current: Vec<Task> = run
            .tasks
            .iter()
            .filter(|t| t.state != State::Split)
            .map(|t| t.task.clone())
            .collect();
        let original = run.tasks[ix].task.id.clone();
        let at = current.iter().position(|t| t.id == original).unwrap_or_default();
        let result = if run.tasks[ix].piece {
            Err("Pieces of a split are not split again.".to_owned())
        } else {
            pipeline::apply_split(&current, at, pieces)
        };
        let next = match result {
            Ok(next) => next,
            Err(error) => {
                let task = &mut run.tasks[ix];
                task.state = State::Failed;
                task.note = format!("The orchestrator's split was rejected: {error}\n\n{}", task.note);
                return Vec::new();
            }
        };
        let by_id: HashMap<&str, &Task> = next.iter().map(|t| (t.id.as_str(), t)).collect();
        for task in run.tasks.iter_mut().filter(|t| t.state != State::Split) {
            if let Some(updated) = by_id.get(task.task.id.as_str()) {
                task.task.depends_on = updated.depends_on.clone();
            }
        }
        let known: HashSet<String> = run.tasks.iter().map(|t| t.task.id.clone()).collect();
        let new: Vec<Task> = next.into_iter().filter(|t| !known.contains(&t.id)).collect();
        for task in new {
            let role = roles::find(&run.roles, task.role.as_deref()).cloned();
            let (provider, settings) = roles::apply(role.as_ref(), &run.provider, &run.settings);
            run.tasks.push(Assignment {
                task,
                provider,
                settings,
                role,
                session: None,
                reviewer: None,
                tree: None,
                state: State::Pending,
                report: String::new(),
                note: String::new(),
                stat: String::new(),
                attempts: 0,
                fixing_merge: false,
                piece: true,
                reruns: 0,
                extra: String::new(),
                packet: None,
                reason: None,
                sent_to: None,
            });
        }
        let task = &mut run.tasks[ix];
        task.state = State::Split;
        task.tree.take().map(Job::Remove).into_iter().collect()
    }

    fn schedule_orchestration(&mut self, lead: i64, cx: &mut Context<Self>) {
        let decider = self.orchestrations.get(&lead).and_then(|run| run.decider);
        let decider_ready = decider.is_none_or(|id| {
            self.sessions
                .get(&id)
                .is_some_and(|v| v.read(cx).ready_for_external(cx))
        });
        let Some(run) = self.orchestrations.get_mut(&lead) else {
            return;
        };
        let mut jobs = Vec::new();
        if !run.preparing {
            let unfinished = |run: &Run, states: &[State]| -> HashSet<String> {
                run.tasks
                    .iter()
                    .filter(|t| states.contains(&t.state))
                    .map(|t| t.task.id.clone())
                    .collect()
            };
            // A retried dependency releases the tasks it blocked.
            if !run.stopped {
                let failed = unfinished(run, &[State::Failed, State::Blocked]);
                for task in &mut run.tasks {
                    if task.state == State::Blocked
                        && !task.task.depends_on.iter().any(|id| failed.contains(id))
                    {
                        task.state = State::Pending;
                    }
                }
            }
            // Failed dependencies transitively block their dependants.
            loop {
                let failed = unfinished(run, &[State::Failed, State::Blocked]);
                let mut changed = false;
                for task in &mut run.tasks {
                    if task.state == State::Pending
                        && task.task.depends_on.iter().any(|id| failed.contains(id))
                    {
                        task.state = State::Blocked;
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
            let satisfied = unfinished(run, &[State::Done, State::Skipped]);
            let shared = run.git.is_none();
            let mut occupied: Vec<Vec<String>> = run
                .tasks
                .iter()
                .filter(|t| t.state.active())
                .map(|t| t.task.files.clone())
                .collect();
            if !run.stopped {
                for (ix, task) in run.tasks.iter_mut().enumerate() {
                    if occupied.len() >= PARALLEL {
                        break;
                    }
                    if task.state != State::Pending
                        || !task.task.depends_on.iter().all(|id| satisfied.contains(id))
                        || (shared
                            && occupied
                                .iter()
                                .any(|files| orchestration::overlaps(files, &task.task.files)))
                    {
                        continue;
                    }
                    task.state = State::Starting;
                    occupied.push(task.task.files.clone());
                    jobs.push(Job::Dispatch(ix));
                }
                // One merge at a time keeps the integration branch linear.
                if run.merging.is_none()
                    && let Some(ix) = run.tasks.iter().position(|t| t.state == State::Merging)
                {
                    run.merging = Some(ix);
                    jobs.push(Job::Merge(ix));
                }
            }
            if run.asking.is_none()
                && decider_ready
                && let Some(ix) = run.tasks.iter().position(|t| t.packet.is_some())
            {
                run.asking = Some(ix);
                run.asked_busy = false;
                jobs.push(Job::Ask(ix, run.tasks[ix].packet.take().unwrap_or_default()));
            }
        }
        let settled = run.settled() && !jobs.iter().any(|j| matches!(j, Job::Ask(..)));
        let progress = std::iter::once(if run.preparing {
            Some("Preparing worktrees…".to_owned())
        } else {
            run.notice.clone()
        })
        .flatten()
        .chain(run.git.as_ref().map(|g| format!("Integration branch: {}", g.branch)))
        .chain(run.tasks.iter().map(|t| {
            format!(
                "{}: {}{}{}",
                t.task.title,
                match t.state {
                    State::Pending => "waiting",
                    State::Starting => "starting",
                    State::Working if t.fixing_merge => "fixing merge",
                    State::Working if t.attempts > 0 => "fixing review",
                    State::Working => "working",
                    State::Committing => "committing",
                    State::Reviewing => "in review",
                    State::Merging => "merging",
                    State::Escalated => "asking orchestrator",
                    State::Done => "done",
                    State::Skipped => "skipped",
                    State::Split => "split up",
                    State::Failed => "failed",
                    State::Blocked => "blocked",
                },
                if t.attempts > 0 {
                    format!(" (round {})", t.attempts + 1)
                } else {
                    String::new()
                },
                t.session
                    .map(|id| format!(" (session {id})"))
                    .unwrap_or_default()
            )
        }))
        .collect::<Vec<_>>()
        .join("\n");
        // The sidebar lists the lead's agents from this, during and after the run.
        let summary = orchestration::Summary {
            live: !settled,
            tasks: run
                .tasks
                .iter()
                .map(|t| orchestration::SummaryTask {
                    session: t.session,
                    title: t.task.title.clone(),
                    provider: t.provider.clone(),
                    model: t.settings.model.clone(),
                    status: t.status().into(),
                })
                .collect(),
        };
        let report = (settled && !run.reported).then(|| {
            run.reported = true;
            let outcomes: Vec<_> = run
                .tasks
                .iter()
                .map(|t| pipeline::Outcome {
                    title: &t.task.title,
                    status: t.status(),
                    attempts: t.attempts,
                    note: &t.note,
                })
                .collect();
            pipeline::final_report(
                &run.title,
                run.git.as_ref().map(|g| g.branch.as_str()),
                &outcomes,
            )
        });
        if !settled {
            run.reported = false;
        }
        let stopped_and_settled = settled && run.stopped;
        let cleanup = stopped_and_settled.then(|| run.git.clone()).flatten();
        if self.orchestration_summaries.get(&lead) != Some(&summary) {
            let store = self.store.clone();
            let saved = summary.clone();
            cx.background_executor()
                .spawn(async move {
                    let _ = locked(&store).and_then(|s| s.save_orchestration_summary(lead, &saved));
                })
                .detach();
            self.orchestration_summaries.insert(lead, summary);
            self.invalidate_chrome(cx);
        }
        if let Some(view) = self.sessions.get(&lead) {
            view.update(cx, |v, cx| {
                if settled {
                    v.orchestration_plan = None;
                    if report.is_some() {
                        v.orchestration_report = report;
                    }
                    v.set_orchestration_progress(None, cx);
                } else {
                    v.set_orchestration_progress(Some(progress), cx);
                }
            });
        }
        if stopped_and_settled {
            self.orchestrations.remove(&lead);
            if let Some(tree) = cleanup {
                cx.background_executor()
                    .spawn(async move { worktrees::finish(&tree) })
                    .detach();
            }
        }
        self.run_jobs(lead, jobs, cx);
    }

    fn run_jobs(&mut self, lead: i64, jobs: Vec<Job>, cx: &mut Context<Self>) {
        for job in jobs {
            match job {
                Job::Dispatch(ix) => self.dispatch_task(lead, ix, cx),
                Job::Commit(ix) => self.commit_task(lead, ix, cx),
                Job::Review(ix) => self.review_task(lead, ix, cx),
                Job::Merge(ix) => self.merge_task(lead, ix, cx),
                Job::Ask(ix, packet) => self.ask_decider(lead, ix, packet, cx),
                Job::Send(session, prompt) => {
                    if let Some(view) = self.sessions.get(&session) {
                        view.update(cx, |v, cx| v.send_coordinator_report(prompt, cx));
                    }
                }
                Job::Remove(tree) => {
                    if let Some(run) = self.orchestrations.get(&lead).and_then(|r| r.git.clone()) {
                        cx.background_executor()
                            .spawn(async move { worktrees::remove_task_tree(&run, &tree) })
                            .detach();
                    }
                }
            }
        }
    }
    /// Runs blocking work for a task off the UI thread, then feeds its result
    /// back into the state machine.
    fn task_work<T: Send + 'static>(
        &mut self,
        lead: i64,
        work: impl std::future::Future<Output = T> + Send + 'static,
        then: impl FnOnce(&mut Self, T, &mut Context<Self>) -> Vec<Job> + 'static,
        cx: &mut Context<Self>,
    ) {
        let work = cx.background_executor().spawn(work);
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                let jobs = then(this, result, cx);
                this.run_jobs(lead, jobs, cx);
                this.invalidate_chrome(cx);
                this.schedule_orchestration(lead, cx);
                cx.notify();
            });
        })
        .detach();
    }
    /// Sends an escalation to the run's decider, creating its session on the
    /// first one. It starts with only the plan, not the lead's conversation,
    /// so each decision resends a few thousand tokens instead of the whole
    /// planning thread.
    fn ask_decider(&mut self, lead: i64, ix: usize, packet: String, cx: &mut Context<Self>) {
        let Some(run) = self.orchestrations.get(&lead) else {
            return;
        };
        if let Some(id) = run.decider {
            if let Some(view) = self.sessions.get(&id) {
                view.update(cx, |v, cx| v.send_coordinator_report(packet, cx));
            }
            return;
        }
        let (provider, mut settings) =
            roles::apply(Some(&run.decider_role), &run.provider, &run.settings);
        if run.decider_role.access.is_none() {
            settings.access = monocode_core::agent::AccessMode::ReadOnly;
        }
        settings.environment.clear();
        settings.worktree_cwd = Some(
            run.git
                .as_ref()
                .map(|g| g.path.to_string_lossy().into_owned())
                .unwrap_or_else(|| run.cwd.clone()),
        );
        let tasks: Vec<Task> = run
            .tasks
            .iter()
            .filter(|t| t.state != State::Split)
            .map(|t| t.task.clone())
            .collect();
        let prompt = format!(
            "{}\n\n{packet}",
            pipeline::decider_intro(&run.title, &run.summary, &tasks)
        );
        let title = format!("Decider · {}", run.title);
        let project = run.project.clone();
        let store = self.store.clone();
        self.task_work(
            lead,
            async move {
                let store = locked(&store)?;
                let mut summary = store.create(&provider, &project)?;
                summary.title = title;
                store.title(summary.id, &summary.title)?;
                store.save_agent_settings(summary.id, &settings)?;
                Ok::<_, String>((summary, settings))
            },
            move |this, result, cx| match result {
                Ok((summary, settings)) => {
                    let id = summary.id;
                    this.add(summary, cx);
                    let asking = this.orchestrations.get_mut(&lead).is_some_and(|run| {
                        run.decider = Some(id);
                        run.asking == Some(ix)
                    });
                    if asking {
                        this.sessions[&id]
                            .update(cx, |v, cx| v.install_transfer(settings, Some(prompt), cx));
                    }
                    Vec::new()
                }
                Err(error) => {
                    if let Some(run) = this.orchestrations.get_mut(&lead) {
                        run.asking = None;
                        let task = &mut run.tasks[ix];
                        task.state = State::Failed;
                        task.note = format!("Could not start the decider: {error}\n\n{}", task.note);
                    }
                    Vec::new()
                }
            },
            cx,
        );
    }
    fn commit_task(&mut self, lead: i64, ix: usize, cx: &mut Context<Self>) {
        let Some(run) = self.orchestrations.get(&lead) else {
            return;
        };
        let (Some(git), Some(tree)) = (run.git.clone(), run.tasks[ix].tree.clone()) else {
            return;
        };
        let message = run.tasks[ix].task.title.clone();
        self.task_work(
            lead,
            async move {
                worktrees::commit_all(&git, &tree, &message)
                    .map(|changed| (changed, worktrees::diff_stat(&git, &tree)))
            },
            move |this, result, _| match result {
                Ok((changed, stat)) => this.committed(lead, ix, changed, stat),
                Err(error) => this.escalate(lead, ix, Reason::Worker, format!("Could not commit the worker's changes: {error}")),
            },
            cx,
        );
    }
    fn merge_task(&mut self, lead: i64, ix: usize, cx: &mut Context<Self>) {
        let Some(run) = self.orchestrations.get(&lead) else {
            return;
        };
        let (Some(git), Some(tree)) = (run.git.clone(), run.tasks[ix].tree.clone()) else {
            return;
        };
        let title = run.tasks[ix].task.title.clone();
        let check = run.setup.check.clone();
        self.task_work(
            lead,
            async move { worktrees::merge(&git, &tree, &title, &check) },
            move |this, result, _| this.merged(lead, ix, result),
            cx,
        );
    }
    /// Opens the task's reviewer session in the task's worktree.
    fn review_task(&mut self, lead: i64, ix: usize, cx: &mut Context<Self>) {
        let Some(run) = self.orchestrations.get(&lead) else {
            return;
        };
        let Some((provider, mut settings, role)) = run.reviewer.clone() else {
            return;
        };
        let task = &run.tasks[ix];
        let cwd = task
            .tree
            .as_ref()
            .map(|t| t.path.to_string_lossy().into_owned())
            .unwrap_or_else(|| run.cwd.clone());
        let prompt = format!(
            "{}\n\n{}",
            role.instructions.trim(),
            pipeline::review_prompt(
                &task.task,
                run.git.as_ref().map(|g| g.branch.as_str()),
                &task.stat,
                &task.report,
                task.attempts,
            )
        );
        let title = format!("{} · {}", role.label, task.task.title);
        let project = run.project.clone();
        let store = self.store.clone();
        settings.worktree_cwd = Some(cwd);
        self.task_work(
            lead,
            async move {
                let store = locked(&store)?;
                let mut summary = store.create(&provider, &project)?;
                summary.title = title;
                store.title(summary.id, &summary.title)?;
                store.save_agent_settings(summary.id, &settings)?;
                Ok::<_, String>((summary, settings))
            },
            move |this, result, cx| match result {
                Ok((summary, settings)) => {
                    let id = summary.id;
                    this.add(summary, cx);
                    let live = this.orchestrations.get_mut(&lead).is_some_and(|run| {
                        let task = &mut run.tasks[ix];
                        let ok = !run.stopped && task.state == State::Reviewing;
                        if ok {
                            task.reviewer = Some(id);
                        } else if task.state == State::Reviewing {
                            task.state = State::Failed;
                            task.note = "Stopped".into();
                        }
                        ok
                    });
                    if live {
                        this.sessions[&id]
                            .update(cx, |v, cx| v.install_transfer(settings, Some(prompt), cx));
                    }
                    Vec::new()
                }
                Err(error) => this.escalate(lead, ix, Reason::Worker, format!("Could not start the reviewer: {error}")),
            },
            cx,
        );
    }
    fn dispatch_task(&mut self, lead: i64, ix: usize, cx: &mut Context<Self>) {
        let Some(run) = self.orchestrations.get(&lead) else {
            return;
        };
        let reports = run
            .tasks
            .iter()
            .filter(|t| t.state == State::Done)
            .map(|t| format!("{}: {}", t.task.title, t.report))
            .collect::<Vec<_>>()
            .join("\n\n");
        let assignment = &run.tasks[ix];
        let task = assignment.task.clone();
        let provider = assignment.provider.clone();
        let mut settings = assignment.settings.clone();
        let role = assignment.role.clone();
        let extra = assignment.extra.clone();
        let reruns = assignment.reruns;
        let git = run.git.clone();
        let setup = run.setup.clone();
        let cwd = run.cwd.clone();
        let project = run.project.clone();
        let store = self.store.clone();
        self.task_work(
            lead,
            async move {
                let mut prompt = orchestration::worker_prompt(&task, &reports, role.as_ref());
                let tree = match &git {
                    Some(git) => {
                        let tree = worktrees::task_tree(git, &task.id, reruns)?;
                        if let Err(error) = worktrees::prepare(git, &tree.path, &setup) {
                            worktrees::remove_task_tree(git, &tree);
                            return Err(error);
                        }
                        settings.environment = worktrees::worker_env(&tree.path, PARALLEL);
                        settings.worktree_cwd = Some(tree.path.to_string_lossy().into_owned());
                        prompt = format!(
                            "{}\n\n{prompt}",
                            pipeline::worktree_note(&tree.branch, &git.branch)
                        );
                        Some(tree)
                    }
                    None => {
                        let root = Path::new(&cwd).canonicalize().map_err(|e| e.to_string())?;
                        // Existing symlink parents must not redirect an assignment outside the checkout.
                        for file in &task.files {
                            let mut path = root.join(file);
                            while !path.exists() {
                                if !path.pop() {
                                    break;
                                }
                            }
                            if !path
                                .canonicalize()
                                .map_err(|e| e.to_string())?
                                .starts_with(&root)
                            {
                                return Err(format!("Assignment scope escapes the checkout: {file}"));
                            }
                        }
                        settings.worktree_cwd = Some(cwd);
                        None
                    }
                };
                if !extra.trim().is_empty() {
                    prompt = format!("{prompt}\n\nNotes for this attempt:\n{}", extra.trim());
                }
                let store = locked(&store)?;
                let mut summary = store.create(&provider, &project)?;
                let who = role.as_ref().map_or("Agent", |r| r.label.as_str());
                summary.title = format!("{who} · {}", task.title);
                store.title(summary.id, &summary.title)?;
                store.save_agent_settings(summary.id, &settings)?;
                Ok::<_, String>((summary, settings, prompt, tree))
            },
            move |this, result, cx| {
                match result {
                    Ok((summary, settings, prompt, tree)) => {
                        let id = summary.id;
                        this.add(summary, cx);
                        let start = this.orchestrations.get_mut(&lead).is_some_and(|run| {
                            let task = &mut run.tasks[ix];
                            task.tree = tree.clone();
                            let start = !run.stopped && task.state == State::Starting;
                            if start {
                                task.session = Some(id);
                                task.state = State::Working;
                            } else if task.state == State::Starting {
                                task.state = State::Blocked;
                            }
                            start
                        });
                        if start {
                            this.sessions[&id]
                                .update(cx, |v, cx| v.install_transfer(settings, Some(prompt), cx));
                        } else if let Some(tree) = tree {
                            return vec![Job::Remove(tree)];
                        }
                    }
                    Err(error) => {
                        if let Some(run) = this.orchestrations.get_mut(&lead) {
                            run.tasks[ix].state = State::Failed;
                            run.tasks[ix].note = error;
                        }
                    }
                }
                Vec::new()
            },
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    #[gpui::test]
    fn reviewed_dispatch_caps_workers_serializes_scopes_and_stops_before_launch(
        cx: &mut TestAppContext,
    ) {
        let (window, _, lead) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&lead].update(cx, |v, cx| {
                    v.install_transfer(Settings::default(), None, cx)
                });
                let tasks = ["src", "src/main.rs", "tests", "docs", "examples", "scripts"]
                    .into_iter()
                    .enumerate()
                    .map(|(ix, file)| Task {
                        id: format!("task-{ix}"),
                        title: format!("Assignment {ix}"),
                        prompt: "Inspect and report".into(),
                        files: vec![file.into()],
                        depends_on: Vec::new(),
                        role: None,
                    })
                    .collect();
                shell.start_orchestration(
                    lead,
                    Plan {
                        title: "Review".into(),
                        summary: String::new(),
                        tasks,
                    },
                    cx,
                );
                let run = shell.orchestrations.get(&lead).unwrap();
                assert!(run.git.is_none() && !run.preparing, "not a Git checkout");
                assert_eq!(
                    run.tasks
                        .iter()
                        .filter(|t| t.state == State::Starting)
                        .count(),
                    4
                );
                assert!(
                    run.tasks[1].state == State::Pending,
                    "overlapping src/main.rs waits for src"
                );
                shell.stop_orchestration(lead, cx);
                assert!(
                    shell.orchestrations.contains_key(&lead),
                    "the run waits for the four in-flight starts"
                );
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                assert!(!shell.orchestrations.contains_key(&lead));
                assert!(shell.sessions.values().all(|v| !v.read(cx).busy))
            })
            .unwrap();
    }

    #[gpui::test]
    fn reviews_loop_twice_then_escalate_and_failed_tasks_can_be_retried(cx: &mut TestAppContext) {
        let (window, _, lead) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&lead].update(cx, |v, cx| {
                    v.install_transfer(Settings::default(), None, cx)
                });
                let task = |id: &str, deps: &[&str]| Task {
                    id: id.into(),
                    title: id.into(),
                    prompt: "Change it".into(),
                    files: vec![format!("{id}.rs")],
                    depends_on: deps.iter().map(|d| d.to_string()).collect(),
                    role: None,
                };
                shell.start_orchestration(
                    lead,
                    Plan {
                        title: "Loop".into(),
                        summary: String::new(),
                        tasks: vec![task("a", &[]), task("b", &["a"])],
                    },
                    cx,
                );
                let run = shell.orchestrations.get_mut(&lead).unwrap();
                assert!(run.reviewer.is_some(), "the default roles include a reviewer");
                // Pretend the worker launched; the dispatch then leaves it alone.
                run.tasks[0].state = State::Working;
                run.tasks[0].session = Some(9001);

                let jobs = shell.worker_finished(lead, 0, true, "Ready\nDid it".into());
                assert!(matches!(jobs[..], [Job::Review(0)]));
                shell.orchestrations.get_mut(&lead).unwrap().tasks[0].reviewer = Some(9002);

                for round in 1..=pipeline::MAX_ATTEMPTS {
                    let jobs = shell.review_finished(
                        lead,
                        0,
                        true,
                        "VERDICT: changes needed\n1. a.rs:1 wrong".into(),
                    );
                    assert!(matches!(jobs[..], [Job::Send(9001, _)]), "round {round} goes back to the worker");
                    let t = &shell.orchestrations[&lead].tasks[0];
                    assert_eq!((t.state, t.attempts, t.sent_to), (State::Working, round, Some(9001)));
                    let jobs = shell.worker_finished(lead, 0, true, "Ready\nFixed".into());
                    assert!(matches!(jobs[..], [Job::Send(9002, _)]), "the same reviewer looks again");
                }
                let jobs =
                    shell.review_finished(lead, 0, true, "VERDICT: changes needed\n1. still".into());
                assert!(jobs.is_empty());
                let t = &shell.orchestrations[&lead].tasks[0];
                assert_eq!(t.state, State::Escalated);
                assert!(t.packet.as_deref().is_some_and(|p| p.contains("DECISION: retry")));

                // The orchestrator leaves it to the user, who retries it.
                shell.apply_decision(lead, 0, Decision::Ask, cx);
                assert!(shell.orchestration_task_actionable(lead, 0));
                assert!(!shell.orchestration_task_actionable(lead, 1));
                let jobs = shell.apply_decision(lead, 0, Decision::Retry(String::new()), cx);
                assert!(matches!(&jobs[..], [Job::Send(9001, p)] if p.contains("still")));
                assert_eq!(shell.orchestrations[&lead].tasks[0].attempts, 0, "a retry gets a fresh budget");

                // An approval finishes it in a shared checkout; a skip would too.
                shell.worker_finished(lead, 0, true, "Ready\nFixed".into());
                shell.review_finished(lead, 0, true, "VERDICT: approve".into());
                assert_eq!(shell.orchestrations[&lead].tasks[0].state, State::Done);
                let jobs = shell.review_finished(lead, 0, true, "VERDICT: replan".into());
                assert!(jobs.is_empty());
                shell.apply_decision(lead, 0, Decision::Skip, cx);
                assert_eq!(shell.orchestrations[&lead].tasks[0].state, State::Skipped);

                // A rerun starts over with the issues carried forward.
                shell.orchestrations.get_mut(&lead).unwrap().tasks[0].note = "broke x".into();
                shell.rerun(lead, 0, Some("implement".into()), "Use y".into());
                let t = &shell.orchestrations[&lead].tasks[0];
                assert_eq!((t.state, t.session, t.reruns), (State::Pending, None, 1));
                assert!(t.extra.contains("Use y") && t.extra.contains("broke x"));
                shell.stop_orchestration(lead, cx);
            })
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn escalations_go_to_a_decider_splits_rewire_dependants_and_merge_fixes_are_reviewed(
        cx: &mut TestAppContext,
    ) {
        let (window, _, lead) = crate::shell::tests::window(cx);
        let task = |id: &str, deps: &[&str]| Task {
            id: id.into(),
            title: id.into(),
            prompt: "Change it".into(),
            files: vec![format!("{id}.rs")],
            depends_on: deps.iter().map(|d| d.to_string()).collect(),
            role: None,
        };
        window
            .update(cx, |shell, _, cx| {
                shell.sessions[&lead].update(cx, |v, cx| {
                    v.install_transfer(Settings::default(), None, cx)
                });
                shell.start_orchestration(
                    lead,
                    Plan {
                        title: "Split".into(),
                        summary: "Two steps".into(),
                        tasks: vec![task("a", &[]), task("b", &["a"])],
                    },
                    cx,
                );
                let run = shell.orchestrations.get_mut(&lead).unwrap();
                run.tasks[0].state = State::Working;
                run.tasks[0].session = Some(9001);
                shell.worker_finished(lead, 0, true, "Ready\nDid it".into());
                shell.orchestrations.get_mut(&lead).unwrap().tasks[0].reviewer = Some(9002);
                shell.review_finished(lead, 0, true, "VERDICT: replan\nToo big.".into());
                let t = &shell.orchestrations[&lead].tasks[0];
                assert_eq!(t.state, State::Escalated);
                assert!(t.packet.as_deref().is_some_and(|p| p.contains("DECISION: split")));
                shell.schedule_orchestration(lead, cx);
                assert_eq!(shell.orchestrations[&lead].asking, Some(0));
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                let decider = shell.orchestrations[&lead].decider.expect("a decider session");
                assert_ne!(decider, lead, "escalations never resend the lead's conversation");
                assert!(shell.session_summaries[&decider].title.starts_with("Decider · Split"));

                shell.orchestrations.get_mut(&lead).unwrap().asking = None;
                shell.apply_decision(
                    lead,
                    0,
                    Decision::Split(vec![task("x", &[]), task("y", &["x"])]),
                    cx,
                );
                let run = &shell.orchestrations[&lead];
                let ids: Vec<_> = run.tasks.iter().map(|t| t.task.id.as_str()).collect();
                assert_eq!(ids, ["a", "b", "a-1", "a-2"], "pieces are appended");
                assert_eq!(run.tasks[0].state, State::Split);
                assert_eq!(run.tasks[1].task.depends_on, ["a-1", "a-2"]);
                assert_eq!(run.tasks[3].task.depends_on, ["a-1"]);
                assert!(run.tasks[2].piece && run.tasks[3].piece);

                // A piece is not offered another split.
                shell.escalate(lead, 2, Reason::Replan, "Still too big".into());
                let packet = shell.orchestrations[&lead].tasks[2].packet.clone().unwrap();
                assert!(!packet.contains("DECISION: split"));

                // Merge and check fixes share the review budget and are reviewed again.
                let run = shell.orchestrations.get_mut(&lead).unwrap();
                let piece = &mut run.tasks[2];
                piece.packet = None;
                piece.state = State::Merging;
                piece.session = Some(9003);
                run.merging = Some(2);
                let jobs = shell.merged(lead, 2, Ok(Merge::Conflict(vec!["a.rs".into()])));
                assert!(matches!(&jobs[..], [Job::Send(9003, p)] if p.contains("a.rs")));
                assert_eq!(shell.orchestrations[&lead].tasks[2].attempts, 1);
                let jobs = shell.worker_finished(lead, 2, true, "Ready\nMerged".into());
                assert!(matches!(jobs[..], [Job::Review(2)]), "a conflict fix is reviewed");
                let run = shell.orchestrations.get_mut(&lead).unwrap();
                run.tasks[2].attempts = pipeline::MAX_ATTEMPTS;
                run.tasks[2].state = State::Merging;
                run.merging = Some(2);
                shell.merged(lead, 2, Ok(Merge::CheckFailed("error[E0425]".into())));
                let t = &shell.orchestrations[&lead].tasks[2];
                assert_eq!((t.state, t.reason.clone()), (State::Escalated, Some(Reason::Merge)));
                shell.stop_orchestration(lead, cx);
            })
            .unwrap();
        cx.run_until_parked();
    }
}
