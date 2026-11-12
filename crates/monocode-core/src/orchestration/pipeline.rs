//! The per-task review loop and its escalations. The app drives the routine
//! path (review, merge, one round of fixes) without any model call; the
//! orchestrator is only asked when a task needs judgment.
use serde::{Deserialize, Serialize};

/// Automatic fix rounds a task gets before it is escalated. Review fixes and
/// merge or check fixes share this budget.
pub const MAX_ATTEMPTS: u32 = 2;
/// How many pieces a split may produce.
pub const SPLIT_PIECES: std::ops::RangeInclusive<usize> = 2..=4;
/// The worker role that reviews each task, when it exists.
pub const REVIEW_ROLE: &str = "review";
pub const ESCALATE_KEY: &str = "orchestration.escalate";

/// When the orchestrator is asked about a task.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Escalate {
    /// Only when the automatic loop could not finish the task.
    #[default]
    OnFailure,
    /// After every review, before anything merges.
    Always,
}
pub fn load_escalate(store: &crate::store::Store) -> Escalate {
    store
        .setting(ESCALATE_KEY)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}
pub fn save_escalate(store: &crate::store::Store, value: Escalate) -> Result<(), String> {
    let json = serde_json::to_string(&value).map_err(|e| e.to_string())?;
    store.save_setting(ESCALATE_KEY, &json)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Approve,
    /// The issues the worker should fix.
    Changes(String),
    /// The task or the plan itself is wrong; fixing the code will not help.
    Replan(String),
}
/// Reads the reviewer's last `VERDICT:` line. A review without one is treated
/// as a request for changes, carrying the whole review.
pub fn parse_verdict(review: &str) -> Verdict {
    let Some((ix, line)) = review
        .lines()
        .enumerate()
        .filter(|(_, l)| {
            l.trim()
                .trim_matches(['*', '`', '_'])
                .to_ascii_lowercase()
                .starts_with("verdict:")
        })
        .last()
    else {
        return Verdict::Changes(review.trim().to_owned());
    };
    let value = line
        .trim()
        .trim_matches(['*', '`', '_'])
        .split_once(':')
        .map(|(_, v)| v.trim().trim_matches(['*', '`', '_']).to_ascii_lowercase())
        .unwrap_or_default();
    let rest: String = review
        .lines()
        .skip(ix + 1)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    let issues = if rest.is_empty() {
        review.trim().to_owned()
    } else {
        rest
    };
    if value.starts_with("approve") {
        Verdict::Approve
    } else if value.starts_with("replan") {
        Verdict::Replan(issues)
    } else {
        Verdict::Changes(issues)
    }
}

/// What the worker is told about its isolated checkout.
pub fn worktree_note(branch: &str, integration: &str) -> String {
    format!(
        "You are working in your own Git worktree on branch `{branch}`; other assignments run in separate worktrees. Do not switch branches, push, or edit the integration branch `{integration}`. Commit when you finish if you like; the app commits anything left uncommitted. A reviewer checks your diff before it is merged."
    )
}
/// Sent to the reviewer, after its role instructions.
pub fn review_prompt(
    task: &super::Task,
    diff_base: Option<&str>,
    stat: &str,
    worker_report: &str,
    attempt: u32,
) -> String {
    let diff = match diff_base {
        Some(base) => format!("Inspect the change with `git diff {base}...HEAD` in this worktree."),
        None => format!(
            "The change is in the shared checkout; inspect it with `git diff -- {}`.",
            task.files.join(" ")
        ),
    };
    format!(
        "Review assignment {id}: {title}{round}\n\nThe assignment was:\n{prompt}\n\nIts scopes: {scopes}\n{diff}\n\nChanged files:\n{stat}\n\nThe worker reported:\n{report}\n\nReview only this change. Do not edit files. End with exactly one verdict line, then the issues as a numbered list with file and line:\n`VERDICT: approve` — correct and complete for this assignment.\n`VERDICT: changes needed` — the worker can fix it.\n`VERDICT: replan` — the assignment itself is wrong or impossible as written.",
        id = task.id,
        title = task.title,
        round = if attempt > 0 {
            format!(" (round {} after fixes)", attempt + 1)
        } else {
            String::new()
        },
        prompt = task.prompt,
        scopes = task.files.join(", "),
        stat = if stat.trim().is_empty() { "(no diff summary)" } else { stat.trim() },
        report = crate::response::limit(worker_report.trim(), 4_000),
    )
}
pub fn fix_prompt(issues: &str) -> String {
    format!(
        "The reviewer asked for changes before this can be merged. Fix these issues, run focused checks, and report what you changed:\n\n{}",
        issues.trim()
    )
}
pub fn conflict_prompt(integration: &str, problem: &str) -> String {
    format!(
        "Your approved change could not be merged into `{integration}`:\n\n{}\n\nRun `git merge {integration}` in this worktree, resolve the result so both your change and the merged work are kept, make sure it builds, commit, and report what you did.",
        problem.trim()
    )
}

/// Why a task needs the orchestrator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The automatic fix rounds ran out.
    StillFailing,
    /// The reviewer said the assignment itself is wrong.
    Replan,
    /// The merge or check still fails and the fix rounds ran out.
    Merge,
    /// The worker stopped with an error or a question.
    Worker,
    /// Escalation is set to always; the review approved it.
    Approved,
}
pub struct Packet<'a> {
    pub task: &'a super::Task,
    pub reason: Reason,
    pub attempts: u32,
    pub stat: &'a str,
    pub detail: &'a str,
    pub roles: &'a [crate::roles::Role],
    /// Whether this task may still be split: it is not itself a piece of a
    /// split, and the run has room for more tasks.
    pub can_split: bool,
}
/// A short brief for the orchestrator, never a transcript.
pub fn escalation(packet: Packet) -> String {
    let why = match packet.reason {
        Reason::StillFailing => format!(
            "The reviewer still asks for changes after {} automatic fix rounds.",
            packet.attempts
        ),
        Reason::Replan => "The reviewer says the assignment itself is wrong.".to_owned(),
        Reason::Merge => format!(
            "The change still cannot be merged or fails the check after {} automatic fix rounds.",
            packet.attempts
        ),
        Reason::Worker => "The worker stopped with an error or a question.".to_owned(),
        Reason::Approved => "The reviewer approved it; decide whether to merge.".to_owned(),
    };
    let roles = packet
        .roles
        .iter()
        .map(|r| r.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let merge = if packet.reason == Reason::Approved {
        "\n`DECISION: merge` — merge it."
    } else {
        ""
    };
    let split = if packet.can_split {
        "\n`DECISION: split` — replace it with 2 to 4 smaller assignments, given after the line as <monocode_split>{\"tasks\":[{\"id\":\"a\",\"title\":\"...\",\"prompt\":\"complete self-contained instructions\",\"files\":[\"src/path\"],\"depends_on\":[],\"role\":\"role id or omit\"}]}</monocode_split>. Pieces start fresh from the integration branch; `depends_on` may name other pieces. Keep their scopes from overlapping."
    } else {
        ""
    };
    format!(
        "Orchestration escalation for assignment {id}: {title}\n{why}\n\nScopes: {scopes}\nChanged files:\n{stat}\n\nDetails:\n{detail}\n\nDecide without editing files yourself; you may inspect the code read-only. Reply with exactly one decision line, then any instructions for the worker:{merge}\n`DECISION: retry` — the same worker continues with your instructions.\n`DECISION: rerun` — a fresh worker restarts the assignment; add `ROLE: <id>` to use another role ({roles}).{split}\n`DECISION: skip` — give up on it and let its dependents run.\n`DECISION: ask` — leave it for the user.",
        id = packet.task.id,
        title = packet.task.title,
        scopes = packet.task.files.join(", "),
        stat = if packet.stat.trim().is_empty() {
            "(none)"
        } else {
            packet.stat.trim()
        },
        detail = crate::response::limit(packet.detail.trim(), 6_000),
        roles = if roles.is_empty() { "none" } else { &roles },
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Merge,
    Retry(String),
    Rerun {
        role: Option<String>,
        notes: String,
    },
    /// Smaller assignments that replace the task.
    Split(Vec<super::Task>),
    Skip,
    Ask,
}
/// Reads the orchestrator's last `DECISION:` line; anything else is `Ask`.
pub fn parse_decision(reply: &str) -> Decision {
    let clean = |l: &str| l.trim().trim_matches(['*', '`', '_']).to_owned();
    let lines: Vec<String> = reply.lines().map(clean).collect();
    let Some(ix) = lines
        .iter()
        .rposition(|l| l.to_ascii_lowercase().starts_with("decision:"))
    else {
        return Decision::Ask;
    };
    let value = lines[ix]
        .split_once(':')
        .map(|(_, v)| v.trim().trim_matches(['*', '`', '_']).to_ascii_lowercase())
        .unwrap_or_default();
    let mut role = None;
    let notes = lines[ix + 1..]
        .iter()
        .filter(|l| {
            match l.split_once(':') {
                Some((key, value)) if key.trim().eq_ignore_ascii_case("role") => {
                    let value = value.trim().trim_matches(['*', '`', '_', '<', '>']);
                    if !value.is_empty() {
                        role = Some(value.to_owned());
                    }
                    false
                }
                _ => true,
            }
        })
        .cloned()
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    match value.split_whitespace().next().unwrap_or("") {
        "merge" => Decision::Merge,
        "retry" => Decision::Retry(notes),
        "rerun" => Decision::Rerun { role, notes },
        "skip" => Decision::Skip,
        "split" => parse_split(reply).map_or(Decision::Ask, Decision::Split),
        _ => Decision::Ask,
    }
}
fn parse_split(reply: &str) -> Option<Vec<super::Task>> {
    #[derive(Deserialize)]
    struct Pieces {
        tasks: Vec<super::Task>,
    }
    let body = reply
        .rsplit_once("<monocode_split>")?
        .1
        .split_once("</monocode_split>")?
        .0
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    serde_json::from_str::<Pieces>(body).ok().map(|p| p.tasks)
}

/// The run's task list with `tasks[ix]` replaced by `pieces`. Pieces get IDs
/// derived from the original's, inherit its dependencies, and every task that
/// waited on the original waits on all of them. The result is validated like
/// a plan, which also enforces the 12-task cap.
pub fn apply_split(
    tasks: &[super::Task],
    ix: usize,
    pieces: Vec<super::Task>,
) -> Result<Vec<super::Task>, String> {
    if !SPLIT_PIECES.contains(&pieces.len()) {
        return Err(format!(
            "A split needs {} to {} assignments.",
            SPLIT_PIECES.start(),
            SPLIT_PIECES.end()
        ));
    }
    let original = tasks.get(ix).ok_or("Unknown assignment.")?;
    let base: String = original.id.chars().take(56).collect();
    let renamed: std::collections::HashMap<String, String> = pieces
        .iter()
        .enumerate()
        .map(|(n, p)| (p.id.clone(), format!("{base}-{}", n + 1)))
        .collect();
    let piece_ids: Vec<String> = (1..=pieces.len()).map(|n| format!("{base}-{n}")).collect();
    let pieces = pieces.into_iter().enumerate().map(|(n, mut piece)| {
        piece.id = piece_ids[n].clone();
        let mut deps = original.depends_on.clone();
        for dep in piece.depends_on.drain(..) {
            let dep = renamed.get(&dep).cloned().unwrap_or(dep);
            if !deps.contains(&dep) {
                deps.push(dep);
            }
        }
        piece.depends_on = deps;
        piece
    });
    let mut out: Vec<super::Task> = tasks[..ix].to_vec();
    out.extend(pieces);
    out.extend(tasks[ix + 1..].iter().cloned());
    for task in &mut out {
        if let Some(at) = task.depends_on.iter().position(|d| *d == original.id) {
            task.depends_on.remove(at);
            for id in &piece_ids {
                if !task.depends_on.contains(id) {
                    task.depends_on.push(id.clone());
                }
            }
        }
    }
    super::validate(&super::Plan {
        title: "Split".into(),
        summary: String::new(),
        tasks: out.clone(),
    })?;
    Ok(out)
}

/// The first message of a run's decider session: the plan it decides for.
/// Later escalations arrive as plain packets in the same session, so it
/// remembers what it already decided.
pub fn decider_intro(plan_title: &str, summary: &str, tasks: &[super::Task]) -> String {
    let list = tasks
        .iter()
        .map(|t| format!("- {} {}: {} [{}]", t.id, t.title, crate::response::limit(t.prompt.trim(), 300), t.files.join(", ")))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "You make decisions for an orchestration run that other agents carry out. The app runs workers in separate worktrees, has a reviewer check each change, and merges approved work. It only asks you when a task needs judgment. Do not edit files or start work yourself; you may read the code to decide.\n\nPlan: {plan_title}\n{summary}\n\nAssignments:\n{list}"
    )
}

/// One line per task for the app-built final report.
pub struct Outcome<'a> {
    pub title: &'a str,
    pub status: &'a str,
    pub attempts: u32,
    pub note: &'a str,
}
/// The run's final report, written by the app from the reviews; no model call.
pub fn final_report(title: &str, integration: Option<&str>, outcomes: &[Outcome]) -> String {
    let done = outcomes.iter().filter(|o| o.status == "completed").count();
    let mut out = format!(
        "Orchestration finished: {title}\n{done} of {} assignments merged.",
        outcomes.len()
    );
    for o in outcomes {
        out.push_str(&format!(
            "\n- {} — {}{}",
            o.title,
            o.status,
            if o.attempts > 0 {
                format!(" after {} fix round{}", o.attempts, if o.attempts == 1 { "" } else { "s" })
            } else {
                String::new()
            }
        ));
        let note = o.note.trim();
        if !note.is_empty() && o.status != "completed" {
            out.push_str(&format!(": {}", note.lines().next().unwrap_or(note)));
        }
    }
    if let Some(branch) = integration {
        out.push_str(&format!(
            "\n\nThe merged work is on `{branch}`. Review it, then merge it yourself; nothing was merged into your branch."
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts_take_the_last_line_and_default_to_changes() {
        assert_eq!(parse_verdict("Looks good.\n**VERDICT: approve**"), Verdict::Approve);
        assert_eq!(
            parse_verdict("VERDICT: changes needed\n1. a.rs:3 off by one"),
            Verdict::Changes("1. a.rs:3 off by one".into())
        );
        assert_eq!(
            parse_verdict("`Verdict: replan`\nThe file does not exist."),
            Verdict::Replan("The file does not exist.".into())
        );
        assert_eq!(
            parse_verdict("I could not tell."),
            Verdict::Changes("I could not tell.".into())
        );
        assert_eq!(
            parse_verdict("VERDICT: approve\nnit\nVERDICT: changes needed\n1. fix"),
            Verdict::Changes("1. fix".into())
        );
    }

    #[test]
    fn decisions_parse_roles_and_notes_and_default_to_ask() {
        assert_eq!(parse_decision("Hmm.\nDECISION: skip"), Decision::Skip);
        assert_eq!(parse_decision("**DECISION: merge**"), Decision::Merge);
        assert_eq!(
            parse_decision("DECISION: retry\nUse the existing helper in git.rs."),
            Decision::Retry("Use the existing helper in git.rs.".into())
        );
        assert_eq!(
            parse_decision("DECISION: rerun\nROLE: <implement>\nStart from the tests."),
            Decision::Rerun {
                role: Some("implement".into()),
                notes: "Start from the tests.".into()
            }
        );
        assert_eq!(parse_decision("I think we should retry."), Decision::Ask);
        assert_eq!(parse_decision("DECISION: maybe"), Decision::Ask);
    }

    fn task(id: &str, files: &[&str], deps: &[&str]) -> crate::orchestration::Task {
        crate::orchestration::Task {
            id: id.into(),
            title: format!("Task {id}"),
            prompt: "Do it".into(),
            files: files.iter().map(|f| f.to_string()).collect(),
            depends_on: deps.iter().map(|d| d.to_string()).collect(),
            role: None,
        }
    }

    #[test]
    fn split_decisions_parse_their_pieces_and_bad_blocks_ask_the_user() {
        let reply = "Too big.\nDECISION: split\n<monocode_split>\n```json\n{\"tasks\":[{\"id\":\"a\",\"title\":\"A\",\"prompt\":\"p\",\"files\":[\"src/a.rs\"]},{\"id\":\"b\",\"title\":\"B\",\"prompt\":\"p\",\"files\":[\"src/b.rs\"],\"depends_on\":[\"a\"]}]}\n```\n</monocode_split>";
        let Decision::Split(pieces) = parse_decision(reply) else { panic!("not a split") };
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[1].depends_on, vec!["a".to_string()]);
        assert_eq!(parse_decision("DECISION: split\n<monocode_split>nope</monocode_split>"), Decision::Ask);
        assert_eq!(parse_decision("DECISION: split"), Decision::Ask);
    }

    #[test]
    fn splits_rename_pieces_inherit_dependencies_and_rewire_dependants() {
        let tasks = vec![task("scout", &["."], &[]), task("big", &["src"], &["scout"]), task("docs", &["docs"], &["big"])];
        let pieces = vec![task("x", &["src/a.rs"], &[]), task("y", &["src/b.rs"], &["x"])];
        let out = apply_split(&tasks, 1, pieces).unwrap();
        let ids: Vec<_> = out.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["scout", "big-1", "big-2", "docs"]);
        assert_eq!(out[1].depends_on, ["scout"]);
        assert_eq!(out[2].depends_on, ["scout", "big-1"]);
        assert_eq!(out[3].depends_on, ["big-1", "big-2"]);
        assert!(apply_split(&tasks, 1, vec![task("x", &["src"], &[])]).is_err(), "one piece is not a split");
        let many: Vec<_> = (0..11).map(|n| task(&format!("t{n}"), &["src"], &[])).collect();
        let four: Vec<_> = (0..4).map(|n| task(&format!("p{n}"), &["src"], &[])).collect();
        assert!(apply_split(&many, 0, four).is_err(), "the run stays within 12 tasks");
    }

    #[test]
    fn final_report_lists_outcomes_without_a_model() {
        let report = final_report(
            "Rename",
            Some("orch/rename"),
            &[
                Outcome { title: "Core", status: "completed", attempts: 1, note: "" },
                Outcome { title: "Docs", status: "failed", attempts: 2, note: "still broken\nmore" },
            ],
        );
        assert!(report.contains("1 of 2 assignments merged"));
        assert!(report.contains("- Core — completed after 1 fix round"));
        assert!(report.contains("- Docs — failed after 2 fix rounds: still broken"));
        assert!(!report.contains("more"));
        assert!(report.contains("`orch/rename`"));
    }
}
