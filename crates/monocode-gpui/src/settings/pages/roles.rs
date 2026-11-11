//! Settings → Worker roles: the presets orchestration assignments run on.
use crate::{
    settings::{
        SettingsEvent, SettingsView,
        management::Target,
        widgets::{icon_action, note, secondary_button},
    },
    theme::*,
};
use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
use monocode_core::{agent::AccessMode, provider::Kind, roles::Role};

/// `select` keys its open state by `&'static str`; role ids are few, so each
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

/// "provider:model", "provider:" for its default model, or "" for the lead's.
fn runs_on_value(role: &Role) -> String {
    match (&role.provider, &role.model) {
        (Some(p), m) => format!("{p}:{}", m.as_deref().unwrap_or_default()),
        (None, Some(m)) => format!(":{m}"),
        (None, None) => String::new(),
    }
}

impl SettingsView {
    pub(super) fn roles_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let roles = self.ctx.roles.clone();
        // (value, label, provider icon); elements are built per select.
        let mut runs_on: Vec<(String, SharedString, Option<&'static str>)> = vec![(
            String::new(),
            SharedString::from("Lead's provider and model"),
            None,
        )];
        for kind in Kind::ALL {
            runs_on.push((
                format!("{}:", kind.id()),
                SharedString::from(format!("{} · Default model", kind.label())),
                Some(kind.id()),
            ));
            if let Some(models) = self.ctx.catalogs.get(kind.id()) {
                runs_on.extend(models.iter().map(|(id, name)| {
                    (
                        format!("{}:{id}", kind.id()),
                        SharedString::from(format!("{} · {name}", kind.label())),
                        Some(kind.id()),
                    )
                }));
            }
        }
        let mut access: Vec<(String, SharedString)> =
            vec![(String::new(), SharedString::from("Lead's access"))];
        access.extend(
            [
                AccessMode::ReadOnly,
                AccessMode::Supervised,
                AccessMode::AutoAcceptEdits,
                AccessMode::Auto,
                AccessMode::FullAccess,
            ]
            .map(|mode| (mode.id().to_owned(), SharedString::from(mode.label()))),
        );

        let mut rows = Vec::new();
        for (ix, role) in roles.iter().enumerate() {
            let current = runs_on_value(role);
            let mut options: Vec<_> = runs_on
                .iter()
                .map(|(value, label, icon)| {
                    (
                        value.clone(),
                        label.clone(),
                        icon.map(|p| provider_icon(p, 14.).into_any_element()),
                    )
                })
                .collect();
            if !options.iter().any(|(v, _, _)| *v == current) {
                // A model no session has listed yet still shows as chosen.
                options.push((current.clone(), SharedString::from(current.clone()), None));
            }
            let id = role.id.clone();
            let model_select = self.select(
                interned(format!("role-runs-on-{id}")),
                "Runs on",
                &current,
                options,
                Some(220.),
                cx,
                {
                    let id = id.clone();
                    move |this, value, cx| {
                        let mut roles = this.ctx.roles.clone();
                        if let Some(role) = roles.iter_mut().find(|r| r.id == id) {
                            let (provider, model) = value.split_once(':').unwrap_or(("", ""));
                            role.provider = (!provider.is_empty()).then(|| provider.to_owned());
                            role.model = (!model.is_empty()).then(|| model.to_owned());
                        }
                        cx.emit(SettingsEvent::SaveRoles(roles));
                    }
                },
            );
            let access_select = self.select(
                interned(format!("role-access-{id}")),
                "Access",
                role.access.map(AccessMode::id).unwrap_or_default(),
                access
                    .iter()
                    .map(|(value, label)| (value.clone(), label.clone(), None))
                    .collect(),
                Some(150.),
                cx,
                {
                    let id = id.clone();
                    move |this, value, cx| {
                        let mut roles = this.ctx.roles.clone();
                        if let Some(role) = roles.iter_mut().find(|r| r.id == id) {
                            role.access = (!value.is_empty()).then(|| AccessMode::from_id(&value));
                        }
                        cx.emit(SettingsEvent::SaveRoles(roles));
                    }
                },
            );
            let edit = icon_action(
                SharedString::from(format!("edit-role-{id}")),
                format!("Edit {}", role.label),
                "sliders",
                28.,
                14.,
                0.4,
                false,
            )
            .on_click(cx.listener({
                let id = id.clone();
                move |this, _, window, cx| {
                    this.open_management(Target::Role(Some(id.clone())), window, cx)
                }
            }))
            .into_any_element();
            let remove = icon_action(
                SharedString::from(format!("delete-role-{id}")),
                format!("Delete {}", role.label),
                "trash",
                28.,
                14.,
                0.4,
                roles.len() == 1,
            )
            .on_click(cx.listener({
                let id = id.clone();
                move |this, _, _, cx| {
                    let roles: Vec<_> = this
                        .ctx
                        .roles
                        .iter()
                        .filter(|r| r.id != id)
                        .cloned()
                        .collect();
                    if !roles.is_empty() {
                        cx.emit(SettingsEvent::SaveRoles(roles));
                    }
                }
            }))
            .into_any_element();
            let about = if role.description.trim().is_empty() {
                role.instructions.lines().next().unwrap_or_default().to_owned()
            } else {
                role.description.clone()
            };
            let label = div()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .child(div().truncate().child(role.label.clone()))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(content(0.4))
                        .font_family(crate::fonts::mono())
                        .child(role.id.clone()),
                );
            rows.push(
                div()
                    .id(SharedString::from(format!("settings-role-{id}")))
                    .child(self.row(
                        None,
                        ix == 0,
                        label,
                        Some(about.into()),
                        vec![model_select, access_select, edit, remove],
                    ))
                    .into_any_element(),
            );
        }
        if rows.is_empty() {
            rows.push(note("No roles yet."));
        }
        let actions = div()
            .flex()
            .gap_2()
            .child(
                secondary_button("add-role", "Add role", false, false)
                    .child("Add role")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_management(Target::Role(None), window, cx)
                    })),
            )
            .child(
                secondary_button("restore-roles", "Restore default roles", false, false)
                    .child("Restore defaults")
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(SettingsEvent::SaveRoles(Vec::new()))
                    })),
            )
            .into_any_element();
        vec![self.group(
            Some("worker-roles"),
            true,
            "Worker roles",
            Some("An orchestration plan names a role on each assignment. The worker then starts on that role's provider and model, so scouting, implementation and review can each use a different subscription. Instructions are prepended to every assignment the role receives.".into()),
            Some(actions),
            rows,
        )]
    }
}
