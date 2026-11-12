use super::*;
use habits::{Habit, Schedule};
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn identity_projects_order_and_hidden_sessions_survive_restart() {
    let temp = tempfile::tempdir().unwrap();
    let db = Store::open(&temp.path().join("native.sqlite")).unwrap();
    let mut roster = Roster::default();
    let a = roster
        .create(vec!["/one/app".into(), "/two/app".into()])
        .id
        .clone();
    let other = roster.create(vec!["/else".into()]).id.clone();
    let b = roster.create(vec!["/one/app".into()]).id.clone();
    assert!(roster.get(&a).unwrap().project(Some("app")).is_err());
    assert!(roster.get(&a).unwrap().project(Some("/outside")).is_err());
    assert!(roster.get(&a).unwrap().project(None).is_err());
    assert_eq!(roster.get(&b).unwrap().project(None).unwrap(), "/one/app");
    roster.reorder(&[b.clone(), a.clone()]);
    roster.hidden_sessions.push(17);
    roster.save(&db).unwrap();
    let saved = Roster::load(&db).unwrap();
    assert_eq!(
        saved.minions.iter().map(|m| &m.id).collect::<Vec<_>>(),
        vec![&b, &other, &a]
    );
    assert_eq!(saved.hidden_sessions, vec![17]);
    assert_ne!(saved.minions[0].mascot, saved.minions[1].mascot);
}
#[test]
fn memory_supersedes_uniquely_redacts_and_archives_before_prompt_budget() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let id = "memory-test";
    let secret = "sk-1234567890abcdefghijklmnop";
    let entry = memory::entry(&format!("Use Rust. token={secret}"), "2026-10-19", None).unwrap();
    assert!(!entry.contains(secret));
    let first = memory::add("# User notes\n", &entry);
    assert_eq!(memory::add(&first, &entry), first);
    let new = memory::entry("Use GPUI", "2026-10-19", None).unwrap();
    let replaced = memory::replace(&first, "Use Rust", Some(&new), "2026-10-19").unwrap();
    assert!(replaced.contains("~~"));
    assert!(memory::replace(&replaced, "not present", None, "2026-10-19").is_err());
    let mut many = "# User notes\nDo not drop this user-written line.\n".to_string();
    for n in 0..205 {
        many.push_str(&memory::entry(&format!("fact number {n}"), "2026-10-18", None).unwrap());
        many.push('\n');
    }
    memory::edit(root, id, None, |_| Ok(many.clone())).unwrap();
    let file = memory::read(root, id, None).unwrap().text.unwrap();
    assert!(file.lines().count() <= 200);
    assert!(file.contains("Do not drop"));
    assert!(file.contains("fact number 204"));
    assert!(files::read(root, id, None, "memory/archive.md")
        .unwrap()
        .text
        .unwrap()
        .contains("fact number 0"));
    assert_eq!(
        memory::search(root, id, "fact number 204", None)
            .unwrap()
            .len(),
        1
    );
    assert!(memory::topic("../secret").is_err());
}
#[test]
fn habit_updates_retry_conflicts_without_losing_other_edits() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let id = "habit-test";
    let now = 1_792_411_200_000;
    let a = Habit::new(
        "A".into(),
        "Report changes".into(),
        Schedule::default(),
        now,
    )
    .unwrap();
    let b = Habit::new("B".into(), "Check builds".into(), Schedule::default(), now).unwrap();
    let inject = AtomicBool::new(true);
    habits::update(root, id, |all| {
        if inject.swap(false, Ordering::SeqCst) {
            habits::update(root, id, |other| {
                other.push(b.clone());
                Ok(())
            })?;
        }
        all.push(a.clone());
        Ok(())
    })
    .unwrap();
    let all = habits::load(root, id).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].id, b.id);
    assert_eq!(all[1].id, a.id);
    let mut h = a;
    h.enabled = false;
    h.next_run_at = now;
    assert!(!h.due(now));
    h.run_requested = true;
    assert!(h.due(now));
    h.running_since = Some(now);
    assert!(!h.due(now));
    assert!(habits::report("`NOTHING_TO_REPORT`").is_none());
    assert!(habits::report("The build failed.\nNOTHING_TO_REPORT")
        .unwrap()
        .contains("build failed"));
}
#[test]
fn schedules_are_strictly_future_and_reject_invalid_fields() {
    let now = 1_792_411_200_000;
    for kind in ["hourly", "daily", "weekdays", "weekly"] {
        let s = Schedule {
            schedule_kind: kind.into(),
            ..Default::default()
        };
        let next = s.next(now).unwrap();
        assert!(next > now);
        assert!(next - now <= 8 * 86400000);
        assert!(s.next(next).unwrap() > next);
    }
    assert!(Schedule {
        time: "25:00".into(),
        ..Default::default()
    }
    .validate()
    .is_err());
    assert!(Schedule {
        minute: 60,
        ..Default::default()
    }
    .validate()
    .is_err());
    let s: Schedule = serde_json::from_str(r#"{"kind":"hourly","minute":15}"#).unwrap();
    assert_eq!(s.minute, 15);
}
#[test]
fn delivery_receipt_is_saved_with_the_user_message() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("native.sqlite");
    let mut db = Store::open(&path).unwrap();
    let session = db.create("codex", "/project").unwrap();
    let row = Row {
        role: "user".into(),
        text: "hello".into(),
        first: true,
        sent_at: None,
    };
    db.write_rows_with_message(session.id, 0, std::slice::from_ref(&row), Some("message-1"))
        .unwrap();
    drop(db);
    let db = Store::open(&path).unwrap();
    assert_eq!(db.load(session.id).unwrap().rows, vec![row]);
    assert_eq!(
        db.setting(&format!("mono-delivered:{}:message-1", session.id))
            .unwrap()
            .as_deref(),
        Some("1")
    );
}

#[test]
fn context_shows_how_to_delegate_without_running_help() {
    let minion = Minion {
        projects: vec!["/repo".into()],
        ..Default::default()
    };
    let files = files::AgentFiles {
        id: minion.id.clone(),
        dir: String::new(),
        soul: Some(DEFAULT_SOUL.into()),
        soul_hash: String::new(),
        memory: String::new(),
        memory_hash: String::new(),
        memory_path: String::new(),
        topics: Vec::new(),
    };
    let executable = std::path::Path::new("/Applications/Monocode.app/monocode");
    let start = r#"/Applications/Monocode.app/monocode app sessions.start --json '{"project":"<project path>","title":"<short title>","prompt":"<complete instructions>","notifyOnComplete":true}'"#;
    let rules = instructions(&minion, executable);
    assert!(rules.contains(start));
    assert!(rules.contains("## Be quick"));
    assert!(rules.contains("- /repo"));
    // Without system instructions, the rules ride along in the message.
    let inline = context(&minion, &files, executable, false);
    assert!(inline.contains(&rules));
    assert!(inline.contains("<soul>"));
    // With them, the message carries only what changes between turns.
    let system = context(&minion, &files, executable, true);
    assert!(!system.contains(start));
    assert!(system.contains("<soul>") && system.contains("<memory>"));
    assert!(system_instructions_supported(crate::provider::Kind::Claude));
    assert!(system_instructions_supported(crate::provider::Kind::Codex));
    assert!(!system_instructions_supported(crate::provider::Kind::Devin));
}

#[test]
fn a_resumed_codex_thread_gets_rules_it_has_not_seen_in_the_message() {
    use crate::provider::Kind;
    // A fresh thread takes them as developer instructions.
    assert!(rules_in_system(Kind::Codex, false, None, "new"));
    // Codex keeps a thread's first instructions, so a moved app CLI or
    // rules from before they lived there must come with the message.
    assert!(!rules_in_system(Kind::Codex, true, None, "new"));
    assert!(!rules_in_system(Kind::Codex, true, Some("old"), "new"));
    assert!(rules_in_system(Kind::Codex, true, Some("new"), "new"));
    // Claude's system prompt is sent with every process.
    assert!(rules_in_system(Kind::Claude, true, None, "new"));
    assert!(!rules_in_system(Kind::Devin, false, None, "new"));
}

#[test]
fn subsessions_fill_only_a_start_that_names_no_model() {
    let set = Subsessions {
        harness: Some("claude".into()),
        model: Some("opus".into()),
        effort: Some("high".into()),
    };
    let mut bare = serde_json::json!({"prompt": "x"});
    set.fill(&mut bare);
    assert_eq!(
        bare,
        serde_json::json!({"prompt": "x", "harness": "claude", "model": "opus", "effort": "high"})
    );
    for chosen in ["harness", "model", "modelSettings", "effort"] {
        let mut named = serde_json::json!({"prompt": "x", chosen: "codex"});
        let before = named.clone();
        set.fill(&mut named);
        assert_eq!(named, before, "{chosen} is the agent's own choice");
    }
    let effort_only = Subsessions {
        effort: Some("low".into()),
        ..Default::default()
    };
    let mut input = serde_json::json!({"prompt": "x"});
    effort_only.fill(&mut input);
    assert_eq!(input, serde_json::json!({"prompt": "x", "effort": "low"}));
}

#[test]
fn subsessions_normalize_away_when_everything_is_inherited() {
    assert_eq!(Subsessions::default().normalized(), None);
    let blank = Subsessions {
        harness: Some(" ".into()),
        ..Default::default()
    };
    assert_eq!(blank.normalized(), None);
    // A model without its provider can't be named.
    let orphan = Subsessions {
        model: Some("opus".into()),
        ..Default::default()
    };
    assert_eq!(orphan.normalized(), None);
}

#[test]
fn legacy_mono_roster_migrates_to_minions_and_names_the_unnamed() {
    use serde_json::json;
    let store = Store::open(Path::new(":memory:")).unwrap();
    // A roster saved before the rename: legacy setting key, the legacy
    // "monos" field name, one unnamed Minion and one with a custom name.
    let legacy = json!({
        "monos": [
            {
                "id": "legacy-unnamed",
                "name": "",
                "mascot": "ghost",
                "color": 2236608,
                "projects": ["/one/app"],
                "launches": [],
                "documents": [],
                "outbox": [],
                "pendingReports": [],
                "habitSessions": [7],
                "seenRows": 12,
                "heartbeatMinutes": 45
            },
            {
                "id": "legacy-custom",
                "name": "Big Boss",
                "mascot": "rocket",
                "color": 2236609,
                "projects": [],
                "launches": [{"sessionId": 3, "cwd": "/repo", "title": "Review",
                    "harness": "codex", "model": "gpt-6", "originTurn": 1,
                    "notifyOnComplete": false}],
                "documents": [{"id": "doc", "kind": "document", "title": "Report",
                    "summary": "", "body": "", "createdAt": 5, "updatedAt": 6}],
                "outbox": [],
                "pendingReports": [],
                "habitSessions": [],
                "seenRows": 0,
                "heartbeatMinutes": 30
            }
        ],
        "hidden_sessions": [17]
    });
    store.save_setting("mono-roster", &legacy.to_string()).unwrap();
    let roster = Roster::load(&store).unwrap();
    assert_eq!(roster.minions.len(), 2);
    // The unnamed Minion takes the first list name; the custom name stays.
    assert_eq!(roster.minions[0].id, "legacy-unnamed");
    assert_eq!(roster.minions[0].name, "Franz");
    assert_eq!(roster.minions[0].habit_sessions, vec![7]);
    assert_eq!(roster.minions[0].seen_rows, 12);
    assert_eq!(roster.minions[1].id, "legacy-custom");
    assert_eq!(roster.minions[1].name, "Big Boss");
    assert_eq!(roster.minions[1].launches.len(), 1);
    assert_eq!(roster.minions[1].documents.len(), 1);
    assert_eq!(roster.hidden_sessions, vec![17]);
    // The migrated roster is saved under the new key and reloads from it.
    assert!(store.setting("minion-roster").unwrap().is_some());
    let reloaded = Roster::load(&store).unwrap();
    assert_eq!(reloaded.minions[0].name, "Franz");
    assert_eq!(reloaded.minions[1].name, "Big Boss");
}

#[test]
fn new_minions_take_the_first_unused_list_name() {
    let mut roster = Roster::default();
    assert_eq!(roster.create(Vec::new()).name, "Franz");
    assert_eq!(roster.create(Vec::new()).name, "Vincent");
    // A custom name wins, and list names stay unique case-insensitively.
    let first = roster.minions[0].id.clone();
    roster.get_mut(&first).unwrap().name = "  FRANZ ".into();
    assert_eq!(roster.create(Vec::new()).name, "Ada");
    // Exhausting the list continues with numbered names, deterministically.
    let mut roster = Roster::default();
    for _ in 0..MINION_NAMES.len() {
        roster.create(Vec::new());
    }
    assert_eq!(roster.create(Vec::new()).name, "Franz 2");
    assert_eq!(roster.create(Vec::new()).name, "Vincent 2");
    let mut names: Vec<String> = roster.minions.iter().map(|m| m.name.clone()).collect();
    let total = names.len();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), total);
    // No label ever falls back to Minion-era text.
    assert!(!roster.minions.iter().any(|m| m.label().contains("Minion")));
}
