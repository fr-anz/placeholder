//! Worktree-per-task orchestration. A run snapshots the checkout (including
//! uncommitted work) onto an integration branch; each task works on its own
//! branch and worktree cut from that branch, and approved work merges back in
//! dependency order behind an optional check command. Nothing touches the
//! user's checkout, index or `main`.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
};

/// Per-project settings, keyed by project path.
pub const SETTING_PREFIX: &str = "orchestration.project:";
/// How much output from a failed setup or check the worker gets back.
const OUTPUT_TAIL: usize = 6_000;

/// What a new task worktree needs that Git does not carry, and how to tell
/// whether merged work still builds. Empty fields are skipped.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectSetup {
    /// Shell command run in each new worktree, e.g. `pnpm install`.
    pub setup: String,
    /// Ignored files to copy from the main checkout, e.g. `.env*`.
    pub copy: Vec<String>,
    /// Shell command run after each merge, e.g. `cargo check`.
    pub check: String,
}
/// The project's file, which a team can commit so everyone's runs match.
pub const PROJECT_FILE: &str = ".placeholder/orchestration.json";
/// This machine's setting for the project, else the project's committed file.
pub fn load_setup(store: &crate::store::Store, project: &str) -> ProjectSetup {
    local_setup(store, project)
        .or_else(|| file_setup(Path::new(project)))
        .unwrap_or_default()
}
/// This machine's setting for the project. An empty one counts as unset, so
/// clearing every field falls back to the project's file.
pub fn local_setup(store: &crate::store::Store, project: &str) -> Option<ProjectSetup> {
    store
        .setting(&format!("{SETTING_PREFIX}{project}"))
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str::<ProjectSetup>(&json).ok())
        .filter(|setup| *setup != ProjectSetup::default())
}
/// The project's committed `PROJECT_FILE`, when it has one that parses.
pub fn file_setup(project: &Path) -> Option<ProjectSetup> {
    std::fs::read_to_string(project.join(PROJECT_FILE))
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
}
pub fn save_setup(
    store: &crate::store::Store,
    project: &str,
    setup: &ProjectSetup,
) -> Result<(), String> {
    let json = serde_json::to_string(setup).map_err(|e| e.to_string())?;
    store.save_setting(&format!("{SETTING_PREFIX}{project}"), &json)
}

/// A run's integration branch and its worktree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunTree {
    /// The user's main checkout.
    pub repo: PathBuf,
    /// The run's slug, unique among existing branches.
    pub slug: String,
    /// `orch/<slug>`: approved work lands here, never on the user's branch.
    pub branch: String,
    /// Where `branch` is checked out; merges and checks run here.
    pub path: PathBuf,
    /// The snapshot every task was originally cut from.
    pub base: String,
}
/// A task's own branch and worktree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskTree {
    pub branch: String,
    pub path: PathBuf,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Merge {
    Merged,
    /// Files Git could not merge; the integration branch is unchanged.
    Conflict(Vec<String>),
    /// The check failed on the merged result, which was rolled back.
    CheckFailed(String),
}

/// Worktrees need a local Git checkout; anything else keeps the shared checkout.
pub fn supported(cwd: &Path) -> bool {
    !crate::files::remote::is_remote(cwd)
        && crate::git::git(cwd, &["rev-parse", "--is-inside-work-tree"])
            .is_ok_and(|out| out.trim() == "true")
}

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    crate::git::git(cwd, args).map(|out| out.trim().to_owned())
}
/// A commit of the working tree as it is now, untracked files included,
/// built on a private index so the user's staging area is untouched.
pub fn snapshot(cwd: &Path) -> Result<String, String> {
    let head = git(cwd, &["rev-parse", "--verify", "HEAD^{commit}"])
        .map_err(|_| "Commit at least once before starting a worktree orchestration.")?;
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let index = dir.path().join("index");
    let run = |args: &[&str]| -> Result<String, String> {
        let mut command = Command::new("git");
        command
            .current_dir(cwd)
            .args(args)
            .env("GIT_INDEX_FILE", &index)
            .env("GIT_TERMINAL_PROMPT", "0");
        crate::process::configure_child(&mut command);
        let out = command.output().map_err(|e| format!("Could not run Git: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    run(&["read-tree", &head])?;
    run(&["add", "-A", "--", "."])?;
    let tree = run(&["write-tree"])?;
    if git(cwd, &["rev-parse", &format!("{head}^{{tree}}")])? == tree {
        return Ok(head);
    }
    run(&[
        "-c",
        "user.name=Placeholder",
        "-c",
        "user.email=orchestration@placeholder.invalid",
        "commit-tree",
        &tree,
        "-p",
        &head,
        "-m",
        "Orchestration snapshot of uncommitted changes",
    ])
}

fn slugify(text: &str) -> String {
    let mut slug = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() >= 40 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-').to_owned();
    if slug.is_empty() { "run".into() } else { slug }
}
fn branch_exists(cwd: &Path, branch: &str) -> bool {
    git(cwd, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok()
}
fn trees_root(repo: &Path) -> PathBuf {
    crate::worktree_management::default_root(repo)
}
fn main_checkout(cwd: &Path) -> Result<PathBuf, String> {
    let trees = crate::git::worktrees(cwd)?;
    trees
        .into_iter()
        .find(|tree| tree.main)
        .map(|tree| PathBuf::from(tree.path))
        .ok_or_else(|| "No working copies found".into())
}

/// Snapshots `cwd` onto a fresh `orch/<slug>` branch with its own worktree.
pub fn start(cwd: &Path, title: &str) -> Result<RunTree, String> {
    let repo = main_checkout(cwd)?;
    let base = snapshot(cwd)?;
    let stem = slugify(title);
    let mut slug = stem.clone();
    for n in 2.. {
        if !branch_exists(&repo, &format!("orch/{slug}"))
            && !trees_root(&repo).join(format!("orch-{slug}")).exists()
        {
            break;
        }
        slug = format!("{stem}-{n}");
    }
    let branch = format!("orch/{slug}");
    let path = trees_root(&repo).join(format!("orch-{slug}"));
    std::fs::create_dir_all(trees_root(&repo)).map_err(|e| e.to_string())?;
    git(
        &repo,
        &["worktree", "add", "--no-track", "-b", &branch, "--", &path.to_string_lossy(), &base],
    )?;
    Ok(RunTree {
        repo,
        slug,
        branch,
        path,
        base,
    })
}

/// A worktree for one attempt at a task, cut from the integration branch as it
/// stands now, so it already contains every merged dependency. A previous
/// attempt's worktree and branch are replaced.
pub fn task_tree(run: &RunTree, task: &str, attempt: u32) -> Result<TaskTree, String> {
    let name = if attempt == 0 {
        slugify(task)
    } else {
        format!("{}-r{attempt}", slugify(task))
    };
    let branch = format!("orch-task/{}/{name}", run.slug);
    let path = trees_root(&run.repo).join(format!("orch-{}--{name}", run.slug));
    let tree = TaskTree { branch, path };
    remove_task_tree(run, &tree);
    let tip = git(&run.path, &["rev-parse", "HEAD"])?;
    git(
        &run.repo,
        &[
            "worktree",
            "add",
            "--no-track",
            "-b",
            &tree.branch,
            "--",
            &tree.path.to_string_lossy(),
            &tip,
        ],
    )?;
    Ok(tree)
}

/// Copies the project's ignored files and runs its setup command in a new
/// worktree. Returns the setup output for the run log.
pub fn prepare(run: &RunTree, tree: &Path, setup: &ProjectSetup) -> Result<String, String> {
    for pattern in setup.copy.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        for relative in matches(&run.repo, pattern)? {
            let from = run.repo.join(&relative);
            let to = tree.join(&relative);
            if to.exists() {
                continue;
            }
            copy_path(&from, &to).map_err(|e| format!("Could not copy {relative}: {e}"))?;
        }
    }
    if setup.setup.trim().is_empty() {
        return Ok(String::new());
    }
    let (ok, output) = shell(tree, &setup.setup)?;
    if ok {
        Ok(output)
    } else {
        Err(format!("Setup command `{}` failed:\n{output}", setup.setup.trim()))
    }
}
/// Paths under `root` matching `pattern`: a relative path whose last
/// component may contain `*` and `?`. Patterns never leave `root`.
fn matches(root: &Path, pattern: &str) -> Result<Vec<String>, String> {
    let path = Path::new(pattern);
    if path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(format!("Copy pattern must stay inside the project: {pattern}"));
    }
    let (parent, name) = match pattern.rsplit_once('/') {
        Some((parent, name)) => (parent, name),
        None => ("", pattern),
    };
    if parent.contains(['*', '?']) {
        return Err(format!("Only the last part of a copy pattern may use wildcards: {pattern}"));
    }
    if !name.contains(['*', '?']) {
        return Ok(root
            .join(pattern)
            .exists()
            .then(|| pattern.to_owned())
            .into_iter()
            .collect());
    }
    let Ok(entries) = std::fs::read_dir(root.join(parent)) else {
        return Ok(Vec::new());
    };
    let mut found: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|file| file != ".git" && wildcard(name, file))
        .map(|file| {
            if parent.is_empty() {
                file
            } else {
                format!("{parent}/{file}")
            }
        })
        .collect();
    found.sort();
    Ok(found)
}
fn wildcard(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}
fn copy_path(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_path(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}
/// Runs a project command through the platform shell.
fn shell(cwd: &Path, command: &str) -> Result<(bool, String), String> {
    let mut child = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    };
    child.current_dir(cwd).stdin(std::process::Stdio::null());
    crate::process::configure_child(&mut child);
    let out = child
        .output()
        .map_err(|e| format!("Could not run `{command}`: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok((out.status.success(), tail(&text, OUTPUT_TAIL)))
}
fn tail(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.len() <= max {
        return text.to_owned();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &text[start..])
}

/// Commits whatever the worker left uncommitted. Returns false when the branch
/// has no changes against the integration branch at all.
pub fn commit_all(run: &RunTree, tree: &TaskTree, message: &str) -> Result<bool, String> {
    git(&tree.path, &["add", "-A", "--", "."])?;
    if git(&tree.path, &["diff", "--cached", "--quiet"]).is_err() {
        git(
            &tree.path,
            &[
                "-c",
                "user.name=Placeholder",
                "-c",
                "user.email=orchestration@placeholder.invalid",
                "commit",
                "--no-verify",
                "-m",
                message,
            ],
        )?;
    }
    let ahead = git(
        &tree.path,
        &["rev-list", "--count", &format!("{}..HEAD", run.branch)],
    )?;
    Ok(ahead != "0")
}
/// `git diff --stat` of the task's work against the integration branch, for
/// reviewers and escalations.
pub fn diff_stat(run: &RunTree, tree: &TaskTree) -> String {
    git(
        &tree.path,
        &["diff", "--stat", &format!("{}...HEAD", run.branch)],
    )
    .unwrap_or_default()
}

/// Merges an approved task into the integration branch, then runs the check.
/// A conflict or failed check leaves the integration branch as it was.
pub fn merge(run: &RunTree, tree: &TaskTree, title: &str, check: &str) -> Result<Merge, String> {
    let before = git(&run.path, &["rev-parse", "HEAD"])?;
    let message = format!("Merge orchestration task: {title}");
    let merged = git(
        &run.path,
        &[
            "-c",
            "user.name=Placeholder",
            "-c",
            "user.email=orchestration@placeholder.invalid",
            "merge",
            "--no-ff",
            "--no-edit",
            "-m",
            &message,
            &tree.branch,
        ],
    );
    if merged.is_err() {
        let files = git(&run.path, &["diff", "--name-only", "--diff-filter=U"])
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let _ = git(&run.path, &["merge", "--abort"]);
        let _ = git(&run.path, &["reset", "--hard", &before]);
        if files.is_empty() {
            return Err(merged.unwrap_err());
        }
        return Ok(Merge::Conflict(files));
    }
    if check.trim().is_empty() {
        return Ok(Merge::Merged);
    }
    let (ok, output) = shell(&run.path, check)?;
    if ok {
        return Ok(Merge::Merged);
    }
    git(&run.path, &["reset", "--hard", &before])?;
    Ok(Merge::CheckFailed(format!(
        "`{}` failed after merging:\n{output}",
        check.trim()
    )))
}

/// Removes a task's worktree and branch; missing ones are ignored.
pub fn remove_task_tree(run: &RunTree, tree: &TaskTree) {
    if tree.path.exists() {
        crate::provider::discard_worktree(&tree.path);
        let _ = git(
            &run.repo,
            &["worktree", "remove", "--force", "--", &tree.path.to_string_lossy()],
        );
    }
    let _ = git(&run.repo, &["worktree", "prune"]);
    if branch_exists(&run.repo, &tree.branch) {
        let _ = git(&run.repo, &["branch", "-D", "--", &tree.branch]);
    }
}
/// Throws a whole run away: every task worktree and branch, then the
/// integration worktree and branch.
pub fn discard(run: &RunTree) -> Result<(), String> {
    let prefix = format!("refs/heads/orch-task/{}/", run.slug);
    for tree in crate::git::worktrees(&run.repo)? {
        let path = PathBuf::from(&tree.path);
        if tree
            .branch
            .as_deref()
            .is_some_and(|b| format!("refs/heads/{b}").starts_with(&prefix))
        {
            crate::provider::discard_worktree(&path);
            let _ = git(&run.repo, &["worktree", "remove", "--force", "--", &tree.path]);
        }
    }
    let branches = git(&run.repo, &["for-each-ref", "--format=%(refname:short)", &prefix])?;
    for branch in branches.lines().filter(|b| !b.is_empty()) {
        let _ = git(&run.repo, &["branch", "-D", "--", branch]);
    }
    if run.path.exists() {
        crate::provider::discard_worktree(&run.path);
        git(
            &run.repo,
            &["worktree", "remove", "--force", "--", &run.path.to_string_lossy()],
        )?;
    }
    let _ = git(&run.repo, &["worktree", "prune"]);
    if branch_exists(&run.repo, &run.branch) {
        git(&run.repo, &["branch", "-D", "--", &run.branch])?;
    }
    Ok(())
}
/// Removes task worktrees once a run settles, keeping the integration branch
/// and its worktree for the user to review and merge.
pub fn finish(run: &RunTree) {
    let prefix = format!("orch-task/{}/", run.slug);
    if let Ok(trees) = crate::git::worktrees(&run.repo) {
        for tree in trees {
            if tree.branch.as_deref().is_some_and(|b| b.starts_with(&prefix)) {
                crate::provider::discard_worktree(Path::new(&tree.path));
                let _ = git(&run.repo, &["worktree", "remove", "--force", "--", &tree.path]);
            }
        }
    }
    let _ = git(&run.repo, &["worktree", "prune"]);
}

/// Environment for a worker in a task worktree. Parallel Rust builds split the
/// machine's cores, and an installed sccache is used unless one is configured.
pub fn worker_env(tree: &Path, parallel: usize) -> HashMap<String, String> {
    let mut env = HashMap::new();
    let rust = tree.join("Cargo.toml").exists()
        || std::fs::read_dir(tree.join("crates")).is_ok_and(|mut d| {
            d.any(|e| e.is_ok_and(|e| e.path().join("Cargo.toml").exists()))
        });
    if !rust {
        return env;
    }
    if std::env::var_os("CARGO_BUILD_JOBS").is_none() {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        env.insert(
            "CARGO_BUILD_JOBS".into(),
            (cores / parallel.max(1)).max(2).to_string(),
        );
    }
    if std::env::var_os("RUSTC_WRAPPER").is_none() && on_path("sccache") {
        env.insert("RUSTC_WRAPPER".into(), "sccache".into());
    }
    env
}
fn on_path(binary: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            dir.join(binary).is_file() || dir.join(format!("{binary}.exe")).is_file()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir(&root).unwrap();
        let git = |args: &[&str]| crate::git::git(&root, args).unwrap();
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        git(&["config", "user.name", "Fixture"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        std::fs::write(root.join("b.txt"), "two\n").unwrap();
        std::fs::write(root.join(".gitignore"), ".env*\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "Initial"]);
        dir
    }
    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn local_setup_wins_and_clearing_it_falls_back_to_the_project_file() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().to_string_lossy().into_owned();
        let store = crate::store::Store::open(Path::new(":memory:")).unwrap();
        assert_eq!(load_setup(&store, &project), ProjectSetup::default());
        std::fs::create_dir(dir.path().join(".placeholder")).unwrap();
        std::fs::write(
            dir.path().join(PROJECT_FILE),
            r#"{"setup":"pnpm install","copy":[".env*"]}"#,
        )
        .unwrap();
        let file = load_setup(&store, &project);
        assert_eq!((file.setup.as_str(), file.check.as_str()), ("pnpm install", ""));
        let mine = ProjectSetup { check: "pnpm typecheck".into(), ..Default::default() };
        save_setup(&store, &project, &mine).unwrap();
        assert_eq!(load_setup(&store, &project), mine);
        save_setup(&store, &project, &ProjectSetup::default()).unwrap();
        assert_eq!(local_setup(&store, &project), None);
        assert_eq!(load_setup(&store, &project), file);
    }
    #[test]
    fn snapshot_carries_uncommitted_work_without_touching_the_index() {
        let dir = repo();
        let root = dir.path().join("app");
        std::fs::write(root.join("a.txt"), "edited\n").unwrap();
        std::fs::write(root.join("new.txt"), "untracked\n").unwrap();
        let run = start(&root, "Rename Monos!").unwrap();
        assert_eq!(run.branch, "orch/rename-monos");
        assert_eq!(read(&run.path.join("a.txt")), "edited\n");
        assert_eq!(read(&run.path.join("new.txt")), "untracked\n");
        let status = crate::git::git(&root, &["status", "--porcelain"]).unwrap();
        assert!(status.contains(" M a.txt") && status.contains("?? new.txt"), "{status}");
        assert_eq!(git(&root, &["branch", "--show-current"]).unwrap(), "main");
        let again = start(&root, "Rename Monos!").unwrap();
        assert_eq!(again.slug, "rename-monos-2");
        discard(&run).unwrap();
        discard(&again).unwrap();
        assert!(!branch_exists(&root, "orch/rename-monos"));
        assert!(!run.path.exists());
    }

    #[test]
    fn approved_tasks_merge_in_order_and_conflicts_leave_integration_unchanged() {
        let dir = repo();
        let root = dir.path().join("app");
        std::fs::write(root.join(".env.local"), "SECRET=1\n").unwrap();
        let run = start(&root, "run").unwrap();
        let setup = ProjectSetup {
            copy: vec![".env*".into()],
            ..Default::default()
        };
        let one = task_tree(&run, "task-1", 0).unwrap();
        let two = task_tree(&run, "task-2", 0).unwrap();
        prepare(&run, &one.path, &setup).unwrap();
        assert_eq!(read(&one.path.join(".env.local")), "SECRET=1\n");

        std::fs::write(one.path.join("a.txt"), "from one\n").unwrap();
        std::fs::write(two.path.join("a.txt"), "from two\n").unwrap();
        assert!(commit_all(&run, &one, "one").unwrap());
        assert!(commit_all(&run, &two, "two").unwrap());
        assert!(diff_stat(&run, &one).contains("a.txt"));

        assert_eq!(merge(&run, &one, "one", "").unwrap(), Merge::Merged);
        assert_eq!(read(&run.path.join("a.txt")), "from one\n");
        let head = git(&run.path, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(
            merge(&run, &two, "two", "").unwrap(),
            Merge::Conflict(vec!["a.txt".into()])
        );
        assert_eq!(git(&run.path, &["rev-parse", "HEAD"]).unwrap(), head);
        assert!(git(&run.path, &["status", "--porcelain"]).unwrap().is_empty());

        // A retry starts from the integration branch, so it sees task 1's work.
        let retry = task_tree(&run, "task-2", 1).unwrap();
        assert_eq!(read(&retry.path.join("a.txt")), "from one\n");
        assert!(!commit_all(&run, &retry, "nothing").unwrap(), "no changes yet");
        remove_task_tree(&run, &retry);
        assert!(!retry.path.exists() && !branch_exists(&root, &retry.branch));
        discard(&run).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn failed_checks_roll_back_the_merge() {
        let dir = repo();
        let root = dir.path().join("app");
        let run = start(&root, "run").unwrap();
        let task = task_tree(&run, "t", 0).unwrap();
        std::fs::write(task.path.join("b.txt"), "broken\n").unwrap();
        commit_all(&run, &task, "t").unwrap();
        let head = git(&run.path, &["rev-parse", "HEAD"]).unwrap();
        let Merge::CheckFailed(output) =
            merge(&run, &task, "t", "grep -q broken b.txt && echo bad && exit 1").unwrap()
        else {
            panic!("expected a failed check")
        };
        assert!(output.contains("bad"));
        assert_eq!(git(&run.path, &["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(
            merge(&run, &task, "t", "true").unwrap(),
            Merge::Merged,
            "a passing check keeps the merge"
        );
        discard(&run).unwrap();
    }

    #[test]
    fn copy_patterns_stay_inside_and_match_the_last_component() {
        assert!(wildcard(".env*", ".env.local"));
        assert!(wildcard("*.toml", "a.toml"));
        assert!(!wildcard("*.toml", "a.json"));
        assert!(wildcard("a?c", "abc"));
        let dir = repo();
        let root = dir.path().join("app");
        assert!(matches(&root, "../secret").is_err());
        assert!(matches(&root, "*/x").is_err());
        assert_eq!(matches(&root, "missing").unwrap(), Vec::<String>::new());
        assert_eq!(matches(&root, "*.txt").unwrap(), vec!["a.txt", "b.txt"]);
    }
}
