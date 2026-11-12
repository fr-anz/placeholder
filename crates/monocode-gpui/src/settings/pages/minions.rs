use crate::settings::SettingsView;
use crate::{
    settings::{SettingsEvent, management::Target, widgets::icon_action},
    theme::*,
};
use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};
impl SettingsView {
    pub(super) fn minions_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = self.prefs.clone();
        let mut rows=vec![
            // `Preferences.monos`, `rail_monos_pinned` and `mono_menu_bar_icon`
            // keep their native names: core's desktop-preferences reconcile
            // table maps these exact fields to the upstream keys.
            self.row(Some("minions-enabled"),true,"Show Minions",Some("Agents of your own on the project rail. Each works on the projects you give it, remembers what matters and picks up habits it runs on its own. Turn this off to hide them.".into()),vec![self.toggle("minions-enabled","Show Minions",p.monos,false,cx,|this,on,cx|this.update(cx,|p|p.monos=on))]),
            self.row(Some("rail-minions-pinned"),false,"Pin Minions to the icon rail",Some("When the project rail is collapsed to icons, show each Minion at the top of the rail, above a divider, instead of inside the project picker.".into()),vec![self.toggle("rail-minions-pinned","Pin Minions to the icon rail",p.rail_monos_pinned,!p.monos,cx,|this,on,cx|this.update(cx,|p|p.rail_monos_pinned=on))])];
        if cfg!(target_os = "macos") {
            rows.push(self.row(Some("minion-menu-bar-icon"),false,"Menu bar icon",Some("Chat with a Minion or open the quick composer from the macOS menu bar. Turn this off to hide the icon.".into()),vec![self.toggle("minion-menu-bar-icon","Menu bar icon",p.mono_menu_bar_icon,false,cx,|this,on,cx|this.update(cx,|p|p.mono_menu_bar_icon=on))]));
        }
        let mut roster = Vec::new();
        for (ix, minion) in self.ctx.minions.clone().into_iter().enumerate() {
            let show_id = minion.id.clone();
            let show = minion.show;
            let folders = minion.folders;
            let description = match minion.projects.split_last() {
                None => "No projects yet".to_owned(),
                Some((last, [])) => format!("Works on {last}"),
                Some((last, others)) => format!("Works on {} and {last}", others.join(", ")),
            };
            let label = div()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .debug_selector(|| "settings-minion-mascot".into())
                        .size(px(16.))
                        .flex_shrink_0()
                        .child(crate::minions::avatar::avatar(
                            &minion.mascot,
                            minion.color,
                            "idle",
                            16.,
                            true,
                        )),
                )
                .child(
                    div()
                        .debug_selector(|| "settings-minion-name".into())
                        .min_w_0()
                        .truncate()
                        .child(minion.name.clone()),
                );
            let reset = minion.clone();
            let controls = vec![
                div()
                    .text_size(px(12.))
                    .line_height(px(20.))
                    .text_color(content(0.5))
                    .child("Show Minion spawned session on the sidebar")
                    .into_any_element(),
                div()
                    .debug_selector(|| "settings-minion-sidebar-switch".into())
                    .flex_shrink_0()
                    .child(self.toggle(
                        "minion-sidebar-show",
                        format!("Show sessions started by {} in sidebar", minion.name),
                        show,
                        false,
                        cx,
                        move |_, on, cx| {
                            cx.emit(SettingsEvent::MinionSidebar(show_id.clone(), on, folders))
                        },
                    ))
                    .into_any_element(),
                icon_action(
                    "reset-minion",
                    format!("Reset {} to defaults", minion.name),
                    "rotate-ccw",
                    28.,
                    14.,
                    0.4,
                    false,
                )
                .debug_selector(|| "settings-minion-reset".into())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_management(Target::ResetMinion(reset.clone()), window, cx)
                }))
                .into_any_element(),
            ];
            roster.push(
                div()
                    .id(SharedString::from(format!("settings-minion-{}", minion.id)))
                    .child(self.row(None, ix == 0, label, Some(description.into()), controls))
                    .into_any_element(),
            );
        }
        if roster.is_empty() {
            roster.push(super::super::widgets::note("No minions yet."));
        }
        vec![self.group(None,true,"Minions",None,None,rows),self.group(Some("minion-list"),false,"Your Minions",Some("Show or hide sessions started by each Minion. Hidden sessions remain available from the Minion's chat.".into()),None,roster)]
    }
}
