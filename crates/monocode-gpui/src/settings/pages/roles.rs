//! Settings → Agents: the orchestrator and each worker role, each with the
//! agent it runs (provider, then model and thinking level) and its approval.
use crate::{
    pickers::{ModelList, ModelPicked, cached_model_name},
    popover::{Anchor, AnchorSlot, Flyout, Menu, MenuAction, MenuEvent, MenuStyle, menu_popover},
    settings::{
        SettingsEvent, SettingsView,
        management::Target,
        widgets::{icon_action, note, secondary_button},
    },
    theme::*,
};
use gpui::{AnyElement, Context, Entity, SharedString, Subscription, Window, div, prelude::*, px};
use monocode_core::{agent::AccessMode, provider::Kind, roles::Role};
use std::collections::HashMap;

/// The orchestrator's key; roles use `role:<id>`.
const ORCHESTRATOR: &str = "orchestrator";

#[derive(Default)]
pub(in crate::settings) struct AgentsState {
    /// The open menu, its anchor and the agent it belongs to.
    menu: Option<(Entity<Menu>, AnchorSlot, String)>,
    _events: Vec<Subscription>,
    anchors: HashMap<String, AnchorSlot>,
}

/// `select` keys its open state by `&'static str`; agents are few, so each
/// distinct id is interned once.
fn interned(id: String) -> &'static str {
    use std::{
        collections::HashSet,
        sync::{Mutex, OnceLock},
    };
    static IDS: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut ids = IDS.get_or_init(Default::default).lock().unwrap();
    if let Some(found) = ids.get(id.as_str()) {
        return found;
    }
    let leaked: &'static str = Box::leak(id.into_boxed_str());
    ids.insert(leaked);
    leaked
}

const ACCESS: [AccessMode; 5] = [
    AccessMode::ReadOnly,
    AccessMode::Supervised,
    AccessMode::AutoAcceptEdits,
    AccessMode::Auto,
    AccessMode::FullAccess,
];

impl SettingsView {
    fn agent(&self, key: &str) -> Option<Role> {
        if key == ORCHESTRATOR {
            return Some(self.ctx.orchestrator.clone());
        }
        let id = key.strip_prefix("role:")?;
        self.ctx.roles.iter().find(|r| r.id == id).cloned()
    }

    fn update_agent(&mut self, key: &str, change: impl FnOnce(&mut Role), cx: &mut Context<Self>) {
        if key == ORCHESTRATOR {
            let mut role = self.ctx.orchestrator.clone();
            change(&mut role);
            self.ctx.orchestrator = role.clone();
            cx.emit(SettingsEvent::SaveOrchestrator(role));
        } else if let Some(id) = key.strip_prefix("role:") {
            let mut roles = self.ctx.roles.clone();
            if let Some(role) = roles.iter_mut().find(|r| r.id == id) {
                change(role);
            }
            self.ctx.roles = roles.clone();
            cx.emit(SettingsEvent::SaveRoles(roles));
        }
        cx.notify();
    }

    /// The agent's catalog entry, from the cached catalog of its provider.
    fn agent_model_entry(&self, kind: Kind, model: Option<&str>, cx: &gpui::App) -> Option<monocode_core::catalog::Model> {
        let profile = crate::accounts::get(cx)
            .map(|accounts| accounts.read(cx).selected(kind, &self.ctx.cwd))
            .unwrap_or_default();
        let models = monocode_core::catalog::cached_models_for(kind, &profile)?;
        match model {
            Some(id) => models.into_iter().find(|m| m.id == id),
            None => models.into_iter().find(|m| m.default),
        }
    }

    /// The thinking level in effect: the agent's pick if the model offers it,
    /// else the model's default.
    fn agent_effort(&self, role: &Role, kind: Kind, cx: &gpui::App) -> Option<(Vec<String>, String)> {
        let entry = self.agent_model_entry(kind, role.model.as_deref(), cx)?;
        if entry.efforts.is_empty() {
            return None;
        }
        let selected = role
            .effort
            .clone()
            .filter(|e| entry.efforts.contains(e))
            .or(entry.default_effort.clone())
            .unwrap_or_else(|| entry.efforts[0].clone());
        Some((entry.efforts, selected))
    }

    /// The composer's model menu for one agent: Reasoning, then Model with
    /// the model picker as its flyout.
    fn open_agent_menu(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.agents.menu.take().is_some() {
            self.agents._events.clear();
            cx.notify();
            return;
        }
        let Some(role) = self.agent(&key) else { return };
        let inherit = if key == ORCHESTRATOR { "Default" } else { "Inherit" };
        // Unset agents open the picker on Claude Code; the rail switches providers.
        let set = role.provider.as_deref().and_then(Kind::parse);
        let kind = set.unwrap_or(Kind::Claude);
        let anchor = self.agents.anchors.entry(key.clone()).or_default().clone();
        let profile = crate::accounts::get(cx)
            .map(|accounts| accounts.read(cx).selected(kind, &self.ctx.cwd))
            .unwrap_or_default();
        let cwd = self.ctx.cwd.clone();
        let list = cx.new(|cx| ModelList::new_for(kind, role.model.clone(), &cwd, profile, cx));
        let focus = list.read(cx).search_focus(cx);
        let mut items = Vec::new();
        if let Some((efforts, selected)) = set.and_then(|kind| self.agent_effort(&role, kind, cx)) {
            let label = if kind == Kind::Codex { "Reasoning" } else { "Effort" };
            items.push(
                MenuAction::new("reasoning", label)
                    .value(crate::conversation::effort_name(&selected))
                    .submenu(
                        efforts
                            .iter()
                            .map(|e| {
                                MenuAction::new(
                                    format!("effort:{e}"),
                                    crate::conversation::effort_name(e),
                                )
                                .checked(*e == selected)
                            })
                            .collect(),
                    )
                    .into(),
            );
        }
        items.push(
            {
                let row = MenuAction::new("model", "Model");
                match set {
                    Some(kind) => row
                        .value_image(kind.id())
                        .value(cached_model_name(kind, role.model.as_deref())),
                    None => row.value(inherit),
                }
            }
                .flyout(Flyout {
                    view: list.clone().into(),
                    width: 310.,
                    padding: 0.,
                    height: Some(442.),
                    max_height: None,
                    focus: Some(focus),
                    surface: None,
                })
                .into(),
        );
        items.push(crate::popover::MenuItem::Separator);
        items.push(MenuAction::new("inherit", inherit).checked(set.is_none()).into());
        let menu = cx.new(|cx| {
            Menu::new("Agent model", items, window, cx)
                .width(250.)
                .style(MenuStyle::Settings)
        });
        let picked_key = key.clone();
        let menu_owner = key.clone();
        let picked = cx.subscribe(&list, move |this, _, picked: &ModelPicked, cx| {
            this.update_agent(
                &picked_key,
                |role| {
                    if role.provider.as_deref() != Some(picked.0.id()) {
                        role.effort = None;
                    }
                    role.provider = Some(picked.0.id().into());
                    role.model = picked.1.clone();
                },
                cx,
            );
            this.agents.menu = None;
            this.agents._events.clear();
            cx.notify();
        });
        let events = cx.subscribe(&menu, move |this, _, event: &MenuEvent, cx| {
            match event {
                MenuEvent::Picked(id, _) if id.as_ref() == "inherit" => {
                    this.update_agent(
                        &key,
                        |role| {
                            role.provider = None;
                            role.model = None;
                            role.effort = None;
                        },
                        cx,
                    );
                }
                MenuEvent::Picked(id, _) => {
                    // Rows that open a submenu or the model picker stay open.
                    let Some(effort) = id.strip_prefix("effort:") else {
                        return;
                    };
                    let effort = effort.to_owned();
                    this.update_agent(&key, |role| role.effort = Some(effort), cx);
                }
                MenuEvent::Dismissed => {}
            }
            this.agents.menu = None;
            this.agents._events.clear();
            cx.notify();
        });
        self.agents.menu = Some((menu, anchor, menu_owner));
        self.agents._events = vec![picked, events];
        cx.notify();
    }

    pub(in crate::settings) fn agents_overlay(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.agents
            .menu
            .as_ref()
            .map(|(menu, anchor, _)| {
                menu_popover(menu, Anchor::Slot(anchor.clone()), cx)
                    .gap(4.)
                    .into_any_element()
            })
            .into_iter()
            .collect()
    }

    /// One agent: its Agent row (provider, then model and thinking level) and
    /// its Approval row.
    fn agent_rows(&mut self, key: &str, role: &Role, inherit: &'static str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let kind = role.provider.as_deref().and_then(Kind::parse);
        let anchor = self.agents.anchors.entry(key.to_owned()).or_default().clone();
        let open = self.agents.menu.as_ref().is_some_and(|(_, _, k)| k == key);
        let (label, effort) = match kind {
            Some(kind) => (
                cached_model_name(kind, role.model.as_deref()),
                self.agent_effort(role, kind, cx)
                    .map(|(_, e)| crate::conversation::effort_name(&e)),
            ),
            None => (SharedString::from(inherit), None),
        };
        let menu_key = key.to_owned();
        let model = div()
            .id(SharedString::from(format!("agent-model-{key}")))
            .relative()
            .h(px(28.))
            .max_w(px(300.))
            .px_2()
            .flex()
            .items_center()
            .gap_1p5()
            .rounded_md()
            .bg(content(0.05))
            .text_size(px(13.))
            .text_color(content(if kind.is_some() { 0.9 } else { 0.5 }))
            .when(open, |d| d.bg(content(0.1)))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.1)))
            .child(anchor.probe())
            .when_some(kind, |d, kind| d.child(provider_icon(kind.id(), 14.)))
            .child(div().truncate().child(label))
            .when_some(effort, |d, effort| {
                d.child(div().flex_shrink_0().text_color(content(0.45)).child(format!("· {effort}")))
            })
            .child(icon("chevron-down", 12.).text_color(content(0.4)))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_agent_menu(menu_key.clone(), window, cx)
            }))
            .into_any_element();
        let mut access = vec![(String::new(), SharedString::from(inherit), None::<AnyElement>)];
        access.extend(
            ACCESS.map(|mode| (mode.id().to_owned(), SharedString::from(mode.label()), None)),
        );
        let access_key = key.to_owned();
        let approval = self.select(
            interned(format!("agent-access-{key}")),
            "Approval",
            role.access.map(AccessMode::id).unwrap_or_default(),
            access,
            Some(150.),
            cx,
            move |this, value, cx| {
                this.update_agent(
                    &access_key,
                    |role| role.access = (!value.is_empty()).then(|| AccessMode::from_id(&value)),
                    cx,
                )
            },
        );
        vec![
            self.row(None, true, "Agent", None, vec![model]),
            self.row(None, false, "Approval", None, vec![approval]),
        ]
    }

    pub(super) fn roles_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut groups = vec![
            div()
                .mb_4()
                .flex()
                .gap_2()
                .child(
                    secondary_button("add-role", "Add agent", false, false)
                        .child("Add agent")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_management(Target::Role(None), window, cx)
                        })),
                )
                .child(
                    secondary_button("restore-roles", "Restore default agents", false, false)
                        .child("Restore defaults")
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(SettingsEvent::SaveRoles(Vec::new()))
                        })),
                )
                .into_any_element(),
        ];
        let orchestrator = self.ctx.orchestrator.clone();
        let rows = self.agent_rows(ORCHESTRATOR, &orchestrator, "Default", cx);
        groups.push(self.group(Some("worker-roles"), true, "Orchestrator", None, None, rows));
        let roles = self.ctx.roles.clone();
        let only = roles.len() == 1;
        for role in &roles {
            let key = format!("role:{}", role.id);
            let rows = self.agent_rows(&key, role, "Inherit", cx);
            let id = role.id.clone();
            let actions = div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    icon_action(
                        SharedString::from(format!("edit-role-{id}")),
                        format!("Edit {}", role.label),
                        "sliders",
                        26.,
                        13.,
                        0.4,
                        false,
                    )
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, window, cx| {
                            this.open_management(Target::Role(Some(id.clone())), window, cx)
                        }
                    })),
                )
                .child(
                    icon_action(
                        SharedString::from(format!("delete-role-{id}")),
                        format!("Delete {}", role.label),
                        "trash",
                        26.,
                        13.,
                        0.4,
                        only,
                    )
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, _, cx| {
                            let roles: Vec<_> =
                                this.ctx.roles.iter().filter(|r| r.id != id).cloned().collect();
                            if !roles.is_empty() {
                                cx.emit(SettingsEvent::SaveRoles(roles));
                            }
                        }
                    })),
                )
                .into_any_element();
            groups.push(self.group(None, false, role.label.clone(), None, Some(actions), rows));
        }
        if roles.is_empty() {
            groups.push(note("No agents yet."));
        }
        groups
    }

}
