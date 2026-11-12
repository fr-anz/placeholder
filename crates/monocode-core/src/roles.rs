//! Worker roles: named presets an orchestration task can name with `role`.
//! A role picks the provider and model the worker runs on (so each kind of work
//! can use a different subscription), its access policy, and instructions that
//! are prepended to every assignment it receives.
use crate::agent::{AccessMode, Settings};
use crate::provider::Kind;
use serde::{Deserialize, Serialize};

/// `native_settings` key holding the roles as JSON.
pub const SETTING_KEY: &str = "fork.worker_roles";
pub const MAX_ROLES: usize = 32;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Role {
    /// Lowercase ASCII, digits, `-` or `_`; at most 40 characters.
    pub id: String,
    pub label: String,
    pub description: String,
    /// Prepended to every assignment this role receives.
    pub instructions: String,
    /// Provider id such as `claude` or `codex`; `None` runs on the lead's provider.
    pub provider: Option<String>,
    /// `None` uses the provider's default model (or the lead's, on the lead's provider).
    pub model: Option<String>,
    pub effort: Option<String>,
    /// `None` inherits the lead's access policy.
    pub access: Option<AccessMode>,
}

pub fn defaults() -> Vec<Role> {
    vec![
        Role {
            id: "scout".into(),
            label: "Scout".into(),
            description: "Explores the codebase and reports findings. Does not edit files.".into(),
            instructions: "You are a scout. Investigate the codebase to answer the task. Do not edit, create, or delete files. Finish with a concise report: relevant files with paths and line numbers, how the code works, risks, and a suggested approach.".into(),
            access: Some(AccessMode::ReadOnly),
            ..Role::default()
        },
        Role {
            id: "implement".into(),
            label: "Worker".into(),
            description: "Makes the change within its assigned files.".into(),
            instructions: "You are an implementer. Make the change the task describes, keep it minimal, and run the relevant tests. Do not commit. Finish with a summary of what changed and how you verified it.".into(),
            ..Role::default()
        },
        Role {
            id: "review".into(),
            label: "Reviewer".into(),
            description: "Reviews a change for bugs and regressions. Does not edit files.".into(),
            instructions: "You are a reviewer. Review the change the task points to for correctness bugs, regressions, and missing tests. Do not edit files. Finish with a verdict on its own line, `VERDICT: approve` or `VERDICT: changes needed`, then a numbered list of concrete issues with file and line.".into(),
            access: Some(AccessMode::ReadOnly),
            ..Role::default()
        },
    ]
}

/// `native_settings` key of the orchestrator agent: what card coordinators
/// and new board orchestrators start on.
pub const ORCHESTRATOR_KEY: &str = "fork.orchestrator_agent";

/// The orchestrator agent; `None` fields fall back to the new-chat defaults.
pub fn load_orchestrator(store: &crate::store::Store) -> Role {
    store
        .setting(ORCHESTRATOR_KEY)
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str::<Role>(&json).ok())
        .filter(|role| role.provider.as_deref().is_none_or(|p| Kind::parse(p).is_some()))
        .unwrap_or_else(|| Role {
            id: "orchestrator".into(),
            label: "Orchestrator".into(),
            ..Role::default()
        })
}

pub fn save_orchestrator(store: &crate::store::Store, role: &Role) -> Result<(), String> {
    if let Some(provider) = &role.provider {
        if Kind::parse(provider).is_none() {
            return Err(format!("Unknown provider \"{provider}\"."));
        }
    }
    store.save_setting(
        ORCHESTRATOR_KEY,
        &serde_json::to_string(role).map_err(|e| e.to_string())?,
    )
}

/// `native_settings` key of the agent that answers orchestration escalations.
pub const DECIDER_KEY: &str = "fork.decider_agent";

/// The escalation decider. Unset fields fall back to the run's lead, so by
/// default it decides on the same model that planned the run.
pub fn load_decider(store: &crate::store::Store) -> Role {
    store
        .setting(DECIDER_KEY)
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str::<Role>(&json).ok())
        .filter(|role| role.provider.as_deref().is_none_or(|p| Kind::parse(p).is_some()))
        .unwrap_or_else(|| Role {
            id: "decider".into(),
            label: "Decider".into(),
            ..Role::default()
        })
}

pub fn save_decider(store: &crate::store::Store, role: &Role) -> Result<(), String> {
    if let Some(provider) = &role.provider {
        if Kind::parse(provider).is_none() {
            return Err(format!("Unknown provider \"{provider}\"."));
        }
    }
    store.save_setting(
        DECIDER_KEY,
        &serde_json::to_string(role).map_err(|e| e.to_string())?,
    )
}

pub fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    id.len() <= 40
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

pub fn validate(roles: &[Role]) -> Result<(), String> {
    if roles.len() > MAX_ROLES {
        return Err(format!("At most {MAX_ROLES} worker roles are allowed."));
    }
    let mut seen = std::collections::HashSet::new();
    for role in roles {
        if !valid_id(&role.id) {
            return Err(format!(
                "Role id \"{}\" must be lowercase letters, digits, - or _ (at most 40).",
                role.id
            ));
        }
        if !seen.insert(role.id.as_str()) {
            return Err(format!("Role id \"{}\" is used twice.", role.id));
        }
        if role.label.trim().is_empty() {
            return Err(format!("Role \"{}\" needs a label.", role.id));
        }
        if let Some(provider) = &role.provider {
            if Kind::parse(provider).is_none() {
                return Err(format!(
                    "Role \"{}\" names unknown provider \"{provider}\".",
                    role.id
                ));
            }
        }
    }
    Ok(())
}

/// Saved roles, or the defaults when none were saved or the saved value is unreadable.
pub fn load(store: &crate::store::Store) -> Vec<Role> {
    store
        .setting(SETTING_KEY)
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str::<Vec<Role>>(&json).ok())
        .filter(|roles| validate(roles).is_ok())
        .unwrap_or_else(defaults)
}

pub fn save(store: &crate::store::Store, roles: &[Role]) -> Result<(), String> {
    validate(roles)?;
    let json = serde_json::to_string(roles).map_err(|e| e.to_string())?;
    store.save_setting(SETTING_KEY, &json)
}

/// A new role id from its label, unique among `roles`.
pub fn new_id(label: &str, roles: &[Role]) -> String {
    let mut base: String = label
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while base.contains("--") {
        base = base.replace("--", "-");
    }
    let mut base = base.trim_matches('-').to_owned();
    base.truncate(32);
    if base.is_empty() {
        base = "role".into();
    }
    let mut id = base.clone();
    let mut n = 2;
    while roles.iter().any(|r| r.id == id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

pub fn find<'a>(roles: &'a [Role], id: Option<&str>) -> Option<&'a Role> {
    id.and_then(|id| roles.iter().find(|role| role.id == id))
}

/// The provider and settings a worker with `role` starts on, derived from the lead's.
/// Switching provider drops the lead's provider-specific choices.
pub fn apply(role: Option<&Role>, lead_provider: &str, lead: &Settings) -> (String, Settings) {
    let mut settings = lead.clone();
    let Some(role) = role else {
        return (lead_provider.to_owned(), settings);
    };
    let provider = role
        .provider
        .clone()
        .unwrap_or_else(|| lead_provider.to_owned());
    if provider != lead_provider {
        settings.account = Default::default();
        settings.model = None;
        settings.effort = None;
        settings.fast = false;
        settings.service_tier = None;
        settings.thinking = None;
        settings.model_settings.clear();
    }
    if role.model.is_some() {
        settings.model = role.model.clone();
        settings.model_settings.clear();
    }
    if role.effort.is_some() {
        settings.effort = role.effort.clone();
    }
    if let Some(access) = role.access {
        settings.access = access;
    }
    (provider, settings)
}

/// The roles section of the orchestration planning instructions.
pub fn planning_brief(roles: &[Role]) -> String {
    if roles.is_empty() {
        return String::new();
    }
    let list = roles
        .iter()
        .map(|role| {
            let runs_on = match (&role.provider, &role.model) {
                (Some(p), Some(m)) => format!(" (runs on {p} {m})"),
                (Some(p), None) => format!(" (runs on {p})"),
                _ => String::new(),
            };
            let about = if role.description.trim().is_empty() {
                role.instructions.lines().next().unwrap_or_default()
            } else {
                role.description.as_str()
            };
            format!("- `{}` {}{runs_on}: {about}", role.id, role.label)
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Worker roles: set each assignment's \"role\" to one of these ids so it runs on that role's provider, model and access policy.\n{list}\nA typical flow is a scout assignment to map the code, then implementer assignments that depend on it. Do not add review assignments: when a `review` role exists, the app has it review every assignment before merging."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_ids_are_checked() {
        validate(&defaults()).unwrap();
        assert!(valid_id("scout"));
        assert!(valid_id("a1_b-c"));
        assert!(!valid_id("Scout"));
        assert!(!valid_id("-lead"));
        assert!(!valid_id(""));
        let mut twice = defaults();
        twice.push(twice[0].clone());
        assert!(validate(&twice).is_err());
        let mut unknown = defaults();
        unknown[0].provider = Some("nope".into());
        assert!(validate(&unknown).is_err());
    }

    #[test]
    fn apply_switches_provider_and_drops_provider_specific_choices() {
        let lead = Settings {
            model: Some("opus".into()),
            effort: Some("high".into()),
            fast: true,
            access: AccessMode::Auto,
            ..Settings::default()
        };
        let (provider, same) = apply(None, "claude", &lead);
        assert_eq!(provider, "claude");
        assert_eq!(same.model.as_deref(), Some("opus"));

        let reviewer = Role {
            id: "review".into(),
            label: "Reviewer".into(),
            provider: Some("codex".into()),
            access: Some(AccessMode::ReadOnly),
            ..Role::default()
        };
        let (provider, settings) = apply(Some(&reviewer), "claude", &lead);
        assert_eq!(provider, "codex");
        assert_eq!(settings.model, None);
        assert_eq!(settings.effort, None);
        assert!(!settings.fast);
        assert_eq!(settings.access, AccessMode::ReadOnly);

        let pinned = Role {
            model: Some("sonnet".into()),
            ..reviewer.clone()
        };
        let pinned = Role {
            provider: None,
            ..pinned
        };
        let (provider, settings) = apply(Some(&pinned), "claude", &lead);
        assert_eq!(provider, "claude");
        assert_eq!(settings.model.as_deref(), Some("sonnet"));
        assert_eq!(settings.effort.as_deref(), Some("high"));
    }

    #[test]
    fn roles_round_trip_through_the_store_and_fall_back_to_defaults() {
        let store = crate::store::Store::open(std::path::Path::new(":memory:")).unwrap();
        assert_eq!(load(&store), defaults());
        let mut roles = defaults();
        roles[2].provider = Some("codex".into());
        roles[2].model = Some("gpt-5.5".into());
        save(&store, &roles).unwrap();
        assert_eq!(load(&store), roles);
        store.save_setting(SETTING_KEY, "not json").unwrap();
        assert_eq!(load(&store), defaults());
    }

    #[test]
    fn the_orchestrator_agent_round_trips_and_defaults_to_new_chat_choices() {
        let store = crate::store::Store::open(std::path::Path::new(":memory:")).unwrap();
        let fresh = load_orchestrator(&store);
        assert_eq!((fresh.id.as_str(), fresh.provider.as_deref()), ("orchestrator", None));
        let picked = Role {
            provider: Some("claude".into()),
            model: Some("claude-opus-5-5".into()),
            effort: Some("high".into()),
            access: Some(AccessMode::Auto),
            ..fresh
        };
        save_orchestrator(&store, &picked).unwrap();
        assert_eq!(load_orchestrator(&store), picked);
        let bad = Role { provider: Some("nope".into()), ..picked };
        assert!(save_orchestrator(&store, &bad).is_err());
    }

    #[test]
    fn new_ids_are_slugs_unique_among_roles() {
        let roles = defaults();
        assert_eq!(new_id("Test Writer!", &roles), "test-writer");
        assert_eq!(new_id("Scout", &roles), "scout-2");
        assert_eq!(new_id("  ", &roles), "role");
        assert!(valid_id(&new_id("Ünïcode reviewer", &roles)));
    }

    #[test]
    fn planning_brief_lists_ids_and_pinned_models() {
        let mut roles = defaults();
        roles[2].provider = Some("codex".into());
        let brief = planning_brief(&roles);
        assert!(brief.contains("`scout` Scout"));
        assert!(brief.contains("`review` Reviewer (runs on codex)"));
        assert!(planning_brief(&[]).is_empty());
    }
}
