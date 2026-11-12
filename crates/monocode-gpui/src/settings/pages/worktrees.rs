use crate::{
    input::{self, Input},
    settings::{
        SettingsEvent, SettingsView,
        management::Target,
        widgets::{note, primary_button, secondary_button},
    },
    theme::*,
};
use gpui::{AnyElement, Context, Div, SharedString, div, prelude::*, px};
use monocode_core::orchestration::worktrees::{PROJECT_FILE, ProjectSetup};

const RUN_SETUP: &str = "run-setup";
const RUN_COPY: &str = "run-copy";
const RUN_CHECK: &str = "run-check";
const RUN_FIELDS: [(&str, &str, &str); 3] = [
    (RUN_SETUP, "Setup command", "Runs in each new task worktree, e.g. pnpm install"),
    (RUN_COPY, "Copy ignored files", "Comma-separated, e.g. .env*, config/local.toml"),
    (RUN_CHECK, "Check command", "Runs after each merge, e.g. cargo check"),
];

fn copy_text(copy: &[String]) -> String {
    copy.join(", ")
}
fn parse_copy(text: &str) -> Vec<String> {
    text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

impl SettingsView {
    fn run_setup_value(&self, key: &'static str, cx: &Context<Self>) -> String {
        self.fields.get(key).map(|i| i.read(cx).content.trim().to_string()).unwrap_or_default()
    }
    fn run_setup_typed(&self, cx: &Context<Self>) -> ProjectSetup {
        ProjectSetup {
            setup: self.run_setup_value(RUN_SETUP, cx),
            copy: parse_copy(&self.run_setup_value(RUN_COPY, cx)),
            check: self.run_setup_value(RUN_CHECK, cx),
        }
    }
    fn save_run_setup(&mut self, cx: &mut Context<Self>) {
        let setup = self.run_setup_typed(cx);
        cx.emit(SettingsEvent::SaveProjectSetup { project: self.selected_project.clone(), setup });
    }
    /// Creates the fields once, refills them when the project changes, and
    /// shows the committed file's values as placeholders.
    fn sync_run_setup_fields(&mut self, cx: &mut Context<Self>) {
        for (key, label, _) in RUN_FIELDS {
            if self.fields.contains_key(key) {
                continue;
            }
            let input = cx.new(|cx| {
                let mut input = Input::field(label, "", content(0.35), cx);
                input.set_font_size(13.);
                input
            });
            self._subscriptions
                .push(cx.subscribe(&input, |_, _, _: &input::Changed, cx| cx.notify()));
            self._subscriptions.push(
                cx.subscribe(&input, |this, _, _: &input::Submitted, cx| this.save_run_setup(cx)),
            );
            self.fields.insert(key, input);
        }
        let file = self.discovery.run_setup_file.clone().unwrap_or_default();
        let fill = self.run_setup_for.as_deref() != Some(self.selected_project.as_str());
        let local = self.ctx.orchestration_setups.get(&self.selected_project).cloned().unwrap_or_default();
        for (key, _, hint) in RUN_FIELDS {
            let (saved, from_file) = match key {
                RUN_SETUP => (local.setup.clone(), file.setup.clone()),
                RUN_COPY => (copy_text(&local.copy), copy_text(&file.copy)),
                _ => (local.check.clone(), file.check.clone()),
            };
            let placeholder = if from_file.is_empty() { hint.to_string() } else { from_file };
            self.fields[key].update(cx, |input, cx| {
                input.set_placeholder(placeholder, cx);
                if fill {
                    input.set_text(saved, cx);
                }
            });
        }
        if fill {
            self.run_setup_for = Some(self.selected_project.clone());
        }
    }
    fn run_setup_field(&self, key: &'static str, label: &'static str) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(content(0.65))
            .child(label)
            .child(
                div()
                    .h(px(36.))
                    .min_w_0()
                    .rounded_lg()
                    .border_1()
                    .border_color(content(0.15))
                    .bg(content(0.03))
                    .px_3()
                    .flex()
                    .items_center()
                    .child(self.fields[key].clone()),
            )
    }
    /// How agent runs prepare and check each task worktree in this project.
    fn run_setup_section(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.sync_run_setup_fields(cx);
        let saved = self.ctx.orchestration_setups.get(&self.selected_project).cloned().unwrap_or_default();
        let typed = self.run_setup_typed(cx);
        let changed = typed != saved;
        let file = self.discovery.run_setup_file.is_some();
        let source = match (saved != ProjectSetup::default(), file) {
            (true, true) => format!("Using this machine's setup; it overrides the project's {PROJECT_FILE}. Clear every field to use the file."),
            (true, false) => "Using this machine's setup.".to_string(),
            (false, true) => format!("Using the project's {PROJECT_FILE}, shown greyed out. Fill in fields to override it on this machine."),
            (false, false) => format!("Nothing set. Agent runs skip setup and merge after review alone. A team can commit {PROJECT_FILE} to share these."),
        };
        div()
            .mt_6()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("Agent run setup"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(content(0.5))
                    .child("Orchestrated runs give each task its own worktree. These steps prepare a new worktree and check the work after each merge."),
            )
            .children(RUN_FIELDS.map(|(key, label, _)| self.run_setup_field(key, label)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_size(px(11.))
                            .text_color(content(0.45))
                            .child(source),
                    )
                    .child(
                        primary_button("save-run-setup", "Save run setup", !changed)
                            .child("Save")
                            .when(changed, |d| {
                                d.on_click(cx.listener(|this, _, _, cx| this.save_run_setup(cx)))
                            }),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn worktrees_page(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let project = self.project_picker(cx);
        let ready = !self.discovery_loading
            && self.discovery.worktrees.error.is_none()
            && !self.discovery.worktrees.main.is_empty();
        let mut page=vec![div().relative().child(self.reveal_probe("project-worktrees")).flex().items_center().gap_1().child(project).child(secondary_button("create-settings-worktree","Create worktree",false,!ready).child(icon("plus",12.)).child("Create worktree").on_click(cx.listener(move|this,_,window,cx|{if ready{this.open_management(Target::Worktree,window,cx);}}))).into_any_element(),div().mt_4().flex().items_center().justify_between().gap_3().child(div().min_w_0().flex_1().text_size(px(12.)).line_height(px(18.)).text_color(content(0.5)).child("Sessions can share a worktree. Deleting one keeps its sessions by default and discards uncommitted changes. Its branch and commits are kept.")).child(self.refresh_button("refresh-settings-worktrees","Refresh",cx)).into_any_element()];
        if self.selected_project.is_empty() {
            page.push(note("Add a project to manage its worktrees."));
        } else if self.discovery_loading {
            page.push(note("Loading worktrees…"));
        } else if let Some(error) = &self.discovery.worktrees.error {
            page.push(
                div()
                    .id("settings-page-error")
                    .mt_4()
                    .role(gpui::Role::Alert)
                    .text_size(px(12.))
                    .text_color(gpui::rgb(0xf87171))
                    .child(error.clone())
                    .into_any_element(),
            );
        } else if self.discovery.worktrees.trees.is_empty() {
            page.push(
                div()
                    .mt_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(content(0.1))
                    .px_4()
                    .py_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(icon("folder-tree", 20.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child("No additional worktrees"),
                    )
                    .child(
                        div().text_size(px(12.)).text_color(content(0.5)).child(
                            "Create a worktree to work on another branch in a separate folder.",
                        ),
                    )
                    .into_any_element(),
            );
        } else {
            let trees = self.discovery.worktrees.trees.clone();
            page.push(
                div()
                    .mt_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(content(0.1))
                    .overflow_hidden()
                    .children(trees.into_iter().enumerate().map(|(ix, tree)| {
                        let t = &tree.tree;
                        let path = std::path::PathBuf::from(&t.path);
                        let count = self
                            .ctx
                            .sessions
                            .iter()
                            .filter(|(_, p, _)| {
                                monocode_core::files::is_equal_or_inside(p, &t.path)
                            })
                            .count();
                        let status = if t.missing {
                            "Missing folder"
                        } else if tree.dirty == Some(true) {
                            "Uncommitted changes"
                        } else if tree.dirty == Some(false) {
                            "Clean"
                        } else {
                            "Status unavailable"
                        };
                        let blocked = tree.locked || t.branch.is_none();
                        div()
                            .p_4()
                            .flex()
                            .items_start()
                            .gap_3()
                            .border_b_1()
                            .border_color(content(0.1))
                            .child(icon("folder-tree", 16.))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .child(
                                                path.file_name()
                                                    .unwrap_or_default()
                                                    .to_string_lossy()
                                                    .into_owned(),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .mt_1()
                                            .text_size(px(11.))
                                            .text_color(content(0.4))
                                            .child(t.path.clone()),
                                    )
                                    .child(
                                        div()
                                            .mt_2()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .text_size(px(11.))
                                            .text_color(content(0.55))
                                            .child(icon("git-branch", 12.))
                                            .child(
                                                t.branch
                                                    .as_ref()
                                                    .map(|b| format!("Current branch: {b}"))
                                                    .unwrap_or_else(|| {
                                                        format!(
                                                            "Detached at {}",
                                                            t.head
                                                                .chars()
                                                                .take(7)
                                                                .collect::<String>()
                                                        )
                                                    }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .mt_2()
                                            .text_size(px(11.))
                                            .text_color(content(0.55))
                                            .child(format!(
                                                "{count} {} in this worktree · {status}{}{}",
                                                if count == 1 { "session" } else { "sessions" },
                                                if tree.unpushed > 0 {
                                                    format!(
                                                        " · {} unpublished commits",
                                                        tree.unpushed
                                                    )
                                                } else {
                                                    String::new()
                                                },
                                                if tree.locked { " · Locked" } else { "" }
                                            )),
                                    ),
                            )
                            .child(
                                secondary_button(
                                    SharedString::from(format!("reveal-tree-{ix}")),
                                    format!("Reveal {}", t.branch.as_deref().unwrap_or("worktree")),
                                    false,
                                    t.missing,
                                )
                                .child(icon("folder-open", 16.))
                                .on_click(move |_, _, cx| cx.reveal_path(&path)),
                            )
                            .child(
                                secondary_button(
                                    SharedString::from(format!("delete-tree-{ix}")),
                                    format!("Delete {}", t.branch.as_deref().unwrap_or("worktree")),
                                    false,
                                    blocked,
                                )
                                .child(icon("trash", 16.))
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        if !blocked {
                                            this.open_management(
                                                Target::DeleteTree(tree.clone()),
                                                window,
                                                cx,
                                            );
                                        }
                                    },
                                )),
                            )
                    }))
                    .into_any_element(),
            );
        }
        if !self.selected_project.is_empty() && !self.discovery_loading {
            page.push(self.run_setup_section(cx));
        }
        if ready {
            page.push(note(format!(
                "New worktrees are created in {}.",
                self.discovery.worktrees.default_root.display()
            )));
        }
        page
    }
}
