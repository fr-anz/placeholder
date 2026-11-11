//! Project board: cards with a stable number and a spec. A backlog card has no
//! lead; starting it creates an orchestrator session (the card's coordinator)
//! whose live state decides the card's lane.
use serde::{Deserialize, Serialize};

/// `native_settings` key holding every project's cards as JSON.
pub const SETTING_KEY: &str = "fork.board_cards";
pub const MAX_CARDS: usize = 2_000;
pub const MAX_TITLE: usize = 200;
pub const MAX_DETAILS: usize = 20_000;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Card {
    pub id: String,
    /// Project path the card belongs to.
    pub project: String,
    /// Never reused within a project, even after cards are deleted.
    pub number: u32,
    pub title: String,
    pub details: String,
    /// The coordinator session once the card is started.
    pub lead: Option<i64>,
    pub archived: bool,
    /// Unix seconds.
    pub created_at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lane {
    Backlog,
    InProgress,
    NeedsReview,
}
impl Lane {
    pub const ALL: [Lane; 3] = [Lane::Backlog, Lane::InProgress, Lane::NeedsReview];
    pub fn label(self) -> &'static str {
        match self {
            Lane::Backlog => "Backlog",
            Lane::InProgress => "In progress",
            Lane::NeedsReview => "Needs review",
        }
    }
}

/// What the app knows about a started card's coordinator right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LeadState {
    /// The session still exists.
    pub exists: bool,
    /// The coordinator is running a turn.
    pub busy: bool,
    /// Its orchestration workers are still dispatching or running.
    pub workers_live: bool,
    /// A plan is waiting for the user to review and start it.
    pub plan_ready: bool,
    /// The coordinator or one of its workers waits on an approval.
    pub needs_input: bool,
}

/// Lanes are derived, never stored: backlog until started, in progress while
/// agents run, needs review when finished or waiting on the user.
pub fn lane(card: &Card, state: Option<LeadState>) -> Lane {
    let Some(_) = card.lead else {
        return Lane::Backlog;
    };
    match state {
        Some(s) if s.exists && !s.needs_input && !s.plan_ready && (s.busy || s.workers_live) => {
            Lane::InProgress
        }
        _ => Lane::NeedsReview,
    }
}

/// Why a card sits where it does, for its badge.
pub fn status_label(card: &Card, state: Option<LeadState>) -> &'static str {
    match (card.lead, state) {
        (None, _) => "Not started",
        (Some(_), Some(s)) if !s.exists => "Session removed",
        (Some(_), Some(s)) if s.needs_input => "Waiting on you",
        (Some(_), Some(s)) if s.plan_ready => "Plan ready",
        (Some(_), Some(s)) if s.workers_live => "Workers running",
        (Some(_), Some(s)) if s.busy => "Planning",
        _ => "Finished",
    }
}

/// Short project key for card ids, such as `CAP` in `CAP-12`.
pub fn prefix(project_label: &str) -> String {
    let key: String = project_label
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(3)
        .collect();
    if key.is_empty() {
        "PRJ".into()
    } else {
        key.to_ascii_uppercase()
    }
}

pub fn next_number(cards: &[Card], project: &str) -> u32 {
    cards
        .iter()
        .filter(|c| c.project == project)
        .map(|c| c.number)
        .max()
        .unwrap_or(0)
        + 1
}

pub fn load(store: &crate::store::Store) -> Vec<Card> {
    store
        .setting(SETTING_KEY)
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

pub fn save(store: &crate::store::Store, cards: &[Card]) -> Result<(), String> {
    if cards.len() > MAX_CARDS {
        return Err(format!("A board holds at most {MAX_CARDS} cards."));
    }
    let json = serde_json::to_string(cards).map_err(|e| e.to_string())?;
    store.save_setting(SETTING_KEY, &json)
}

/// Adds a backlog card to `project` and returns it.
pub fn add(
    store: &crate::store::Store,
    project: &str,
    title: &str,
    details: &str,
) -> Result<Card, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("A card needs a title.".into());
    }
    let mut cards = load(store);
    let number = next_number(&cards, project);
    let card = Card {
        id: uuid::Uuid::new_v4().to_string(),
        project: project.to_owned(),
        number,
        title: title.chars().take(MAX_TITLE).collect(),
        details: details.trim().chars().take(MAX_DETAILS).collect(),
        lead: None,
        archived: false,
        created_at: crate::time::now_ms() / 1000,
    };
    cards.push(card.clone());
    save(store, &cards)?;
    Ok(card)
}

/// Applies `change` to the card with `id` and saves the board.
pub fn update(
    store: &crate::store::Store,
    id: &str,
    change: impl FnOnce(&mut Card),
) -> Result<Card, String> {
    let mut cards = load(store);
    let card = cards
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or("That card no longer exists.")?;
    change(card);
    let card = card.clone();
    save(store, &cards)?;
    Ok(card)
}

pub fn remove(store: &crate::store::Store, id: &str) -> Result<(), String> {
    let mut cards = load(store);
    cards.retain(|c| c.id != id);
    save(store, &cards)
}

/// The first message of a started card's coordinator. Its session runs in
/// orchestrator mode, so it answers with a reviewable plan that assigns roles.
pub fn coordinator_brief(key: &str, card: &Card) -> String {
    let mut brief = format!(
        "You coordinate board card {key}. Do not do the work yourself: plan assignments for workers and give each one a role so it runs on that role's provider and model. A typical plan maps the code with a scout, makes the change with implementers that depend on the scout, and ends with a reviewer that depends on the implementers. Give each assignment a self-contained prompt: workers do not see this conversation.\n\n## Card {key}: {}",
        card.title
    );
    if !card.details.is_empty() {
        brief.push_str("\n\n");
        brief.push_str(&card.details);
    }
    brief
}

/// Opens each board orchestrator conversation: a fresh session with a snapshot
/// of the board, so a long history never fills the model's context.
pub fn orchestrator_brief(project_label: &str, key: &str, snapshot: &str, message: &str) -> String {
    format!(
        "You are the orchestrator for the \"{project_label}\" board. The user talks to you here to plan and steer work; you do not write code yourself.\n\n- Turn requests into well-scoped cards with `board.add`: a short title and a self-contained spec in `details` (goal, relevant files if known, acceptance criteria). Split large requests into several cards.\n- Cards land in the backlog. Pass `start: true` only when the user asks to begin; otherwise let them review the specs and start cards themselves.\n- `board.start` starts a backlog card by number ({key}-3 or 3). Its coordinator plans scout, implementer and reviewer assignments.\n- `board.list` lists the board with each card's coordinator session; read one with `sessions.read`.\n- Lanes move on their own: Backlog until started, In progress while agents run, Needs review when finished or waiting on the user.\n\n## Board now\n\n{}\n\n## User\n\n{message}",
        if snapshot.trim().is_empty() { "No cards yet." } else { snapshot.trim_end() }
    )
}

/// Resolves `3` or `PRJ-3` to a card number.
pub fn parse_card_number(value: &str) -> Option<u32> {
    value.rsplit('-').next()?.trim().parse().ok()
}

/// A plain-text board for an orchestrator conversation.
pub fn snapshot(cards: &[Card], key: &str, state: impl Fn(&Card) -> Option<LeadState>) -> String {
    let mut out = String::new();
    for lane_kind in Lane::ALL {
        let in_lane: Vec<_> = cards
            .iter()
            .filter(|c| !c.archived && lane(c, state(c)) == lane_kind)
            .collect();
        if in_lane.is_empty() {
            continue;
        }
        out.push_str(&format!("### {}\n", lane_kind.label()));
        for card in in_lane {
            out.push_str(&format!(
                "- {key}-{} {} ({})\n",
                card.number,
                card.title,
                status_label(card, state(card))
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(state: LeadState) -> (Card, Option<LeadState>) {
        (
            Card {
                lead: Some(7),
                ..Card::default()
            },
            Some(state),
        )
    }

    #[test]
    fn lanes_follow_the_coordinator() {
        assert_eq!(lane(&Card::default(), None), Lane::Backlog);
        let live = LeadState {
            exists: true,
            busy: true,
            ..LeadState::default()
        };
        let (card, state) = started(live);
        assert_eq!(lane(&card, state), Lane::InProgress);
        assert_eq!(status_label(&card, state), "Planning");
        let (card, state) = started(LeadState {
            plan_ready: true,
            ..live
        });
        assert_eq!(lane(&card, state), Lane::NeedsReview);
        assert_eq!(status_label(&card, state), "Plan ready");
        let (card, state) = started(LeadState {
            busy: false,
            workers_live: true,
            ..live
        });
        assert_eq!(lane(&card, state), Lane::InProgress);
        let (card, state) = started(LeadState {
            needs_input: true,
            workers_live: true,
            ..live
        });
        assert_eq!(lane(&card, state), Lane::NeedsReview);
        let (card, state) = started(LeadState {
            exists: true,
            ..LeadState::default()
        });
        assert_eq!(lane(&card, state), Lane::NeedsReview);
        assert_eq!(status_label(&card, state), "Finished");
        let (card, state) = started(LeadState::default());
        assert_eq!(status_label(&card, state), "Session removed");
    }

    #[test]
    fn numbers_and_prefixes() {
        assert_eq!(prefix("capy-app"), "CAP");
        assert_eq!(prefix("--"), "PRJ");
        let cards = vec![
            Card {
                project: "/a".into(),
                number: 4,
                ..Card::default()
            },
            Card {
                project: "/b".into(),
                number: 9,
                ..Card::default()
            },
        ];
        assert_eq!(next_number(&cards, "/a"), 5);
        assert_eq!(next_number(&cards, "/c"), 1);
    }

    #[test]
    fn cards_persist_and_numbers_are_not_reused() {
        let store = crate::store::Store::open(std::path::Path::new(":memory:")).unwrap();
        assert!(add(&store, "/p", "  ", "").is_err());
        let first = add(&store, "/p", "Fix login", "Users get logged out").unwrap();
        let second = add(&store, "/p", "Add dark mode", "").unwrap();
        assert_eq!((first.number, second.number), (1, 2));
        remove(&store, &second.id).unwrap();
        assert_eq!(add(&store, "/p", "Third", "").unwrap().number, 2);
        let updated = update(&store, &first.id, |c| c.lead = Some(3)).unwrap();
        assert_eq!(updated.lead, Some(3));
        assert_eq!(load(&store).len(), 2);
        let brief = coordinator_brief("PRJ-1", &updated);
        assert!(brief.contains("## Card PRJ-1: Fix login\n\nUsers get logged out"));
    }

    #[test]
    fn card_numbers_parse_from_keys_and_briefs_carry_the_board() {
        assert_eq!(parse_card_number("CAP-12"), Some(12));
        assert_eq!(parse_card_number("7"), Some(7));
        assert_eq!(parse_card_number("CAP-"), None);
        let brief = orchestrator_brief("capy", "CAP", "", "Plan auth");
        assert!(brief.contains("No cards yet."));
        assert!(brief.ends_with("## User\n\nPlan auth"));
    }

    #[test]
    fn snapshot_groups_cards_by_lane() {
        let cards = vec![
            Card {
                number: 1,
                title: "Todo".into(),
                ..Card::default()
            },
            Card {
                number: 2,
                title: "Done".into(),
                lead: Some(1),
                ..Card::default()
            },
            Card {
                number: 3,
                title: "Hidden".into(),
                archived: true,
                ..Card::default()
            },
        ];
        let text = snapshot(&cards, "PRJ", |_| {
            Some(LeadState {
                exists: true,
                ..LeadState::default()
            })
        });
        assert!(text.contains("### Backlog\n- PRJ-1 Todo (Not started)"));
        assert!(text.contains("### Needs review\n- PRJ-2 Done (Finished)"));
        assert!(!text.contains("Hidden"));
    }
}
