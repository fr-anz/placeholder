//! The project board: Backlog, In progress and Needs review lanes derived from
//! each card's coordinator session. Starting a card hands it to the shell,
//! which opens an orchestrator session that plans role-assigned workers.
use crate::{
    input::{self, Input},
    runtime::{Database, locked},
    theme::*,
};
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, Hsla,
    Role, SharedString, Stateful, Subscription, Window, div, prelude::*, px,
};
use monocode_core::board::{self, Card, Lane, LeadState};
use std::collections::HashMap;

pub(crate) enum Event {
    Close,
    OpenSession(i64),
    /// Start a backlog card: the shell creates its coordinator session.
    Start(Card),
    /// Message the project's orchestrator.
    Ask(String),
    /// Cards were added, deleted, archived or restored.
    Changed,
}

/// What a started card's coordinator runs on and how its workers are doing.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LeadInfo {
    pub provider: String,
    pub model: Option<String>,
    /// Each worker's status: queued, running, completed, failed or blocked.
    pub workers: Vec<String>,
}

/// The project's board orchestrator, as the message bar shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct OrchestratorStatus {
    pub session: i64,
    pub busy: bool,
    /// Prose of its latest reply.
    pub reply: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Active,
    Archived,
}

pub(crate) struct Board {
    store: Database,
    cwd: String,
    project_label: String,
    /// Every card of this project, archived ones included.
    cards: Vec<Card>,
    states: HashMap<i64, LeadState>,
    infos: HashMap<i64, LeadInfo>,
    orchestrator: Option<OrchestratorStatus>,
    tab: Tab,
    composing: bool,
    title: Entity<Input>,
    details: Entity<Input>,
    ask: Entity<Input>,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Event> for Board {}
impl Focusable for Board {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.ask.focus_handle(cx)
    }
}

fn danger(alpha: f32) -> Hsla {
    gpui::hsla(0., 0.66, if is_light() { 0.5 } else { 0.47 }, alpha)
}
fn amber(alpha: f32) -> Hsla {
    gpui::hsla(38. / 360., 0.92, if is_light() { 0.45 } else { 0.55 }, alpha)
}
fn green(alpha: f32) -> Hsla {
    gpui::hsla(142. / 360., 0.6, if is_light() { 0.38 } else { 0.5 }, alpha)
}

fn lane_color(lane: Lane) -> Hsla {
    match lane {
        Lane::Backlog => content(0.55),
        Lane::InProgress => amber(1.),
        Lane::NeedsReview => green(1.),
    }
}

fn worker_color(status: &str) -> Hsla {
    match status {
        "completed" => green(1.),
        "running" => amber(1.),
        "failed" | "blocked" => danger(1.),
        _ => content(0.18),
    }
}

fn ago(seconds: u64) -> String {
    let now = monocode_core::time::now_ms() / 1000;
    let s = now.saturating_sub(seconds);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", s / 60),
        3600..86400 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

/// Markdown as plain prose for a preview: text and inline code only; code
/// blocks and tables are skipped, block ends become spaces.
fn plain_excerpt(markdown: &str) -> String {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    let mut out = String::new();
    let mut skip = 0usize;
    for event in Parser::new_ext(markdown, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH) {
        match event {
            Event::Start(Tag::CodeBlock(_) | Tag::Table(_) | Tag::HtmlBlock) => skip += 1,
            Event::End(TagEnd::CodeBlock | TagEnd::Table | TagEnd::HtmlBlock) => {
                skip = skip.saturating_sub(1)
            }
            Event::Text(text) | Event::Code(text) if skip == 0 => out.push_str(&text),
            Event::SoftBreak | Event::HardBreak if skip == 0 => out.push(' '),
            Event::End(TagEnd::Heading(_)) if skip == 0 => out.push_str(". "),
            Event::End(TagEnd::Paragraph | TagEnd::Item) if skip == 0 => out.push(' '),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Copy)]
enum Tone {
    Danger,
    Accent,
    Plain,
}

/// The small footer buttons of a card: Delete, Start, Open, Archive.
fn card_button(id: SharedString, label: &'static str, tone: Tone) -> Stateful<gpui::Div> {
    let base = div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .cursor_pointer()
        .h(px(26.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded_md()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .child(label);
    match tone {
        Tone::Danger => base
            .bg(danger(0.9))
            .text_color(gpui::white())
            .hover(|s| s.bg(danger(1.))),
        Tone::Accent => base
            .bg(accent_color())
            .text_color(gpui::white())
            .hover(|s| s.opacity(0.9)),
        Tone::Plain => base
            .border_1()
            .border_color(content(0.12))
            .text_color(content(0.85))
            .hover(|s| s.bg(content(0.06))),
    }
}

impl Board {
    pub(crate) fn new(store: Database, cx: &mut Context<Self>) -> Self {
        let title = cx.new(|cx| {
            let mut input = Input::field("Task title", "Task title", content(0.35), cx);
            input.set_max_chars(board::MAX_TITLE);
            input
        });
        let details = cx.new(|cx| {
            let mut input = Input::multiline_field(
                "Task spec",
                "Goal, relevant files, acceptance criteria (optional)",
                cx,
            );
            input.set_max_chars(board::MAX_DETAILS);
            input.set_min_height(56.);
            input.set_max_height(160.);
            input
        });
        let ask = cx.new(|cx| {
            Input::field(
                "Message the board orchestrator",
                "Message the orchestrator to plan or start tasks…",
                content(0.35),
                cx,
            )
        });
        let subscriptions = vec![
            cx.subscribe(&title, |this, _, _: &input::Submitted, cx| this.add(cx)),
            cx.subscribe(&title, |_, _, _: &input::Changed, cx| cx.notify()),
            cx.subscribe(&ask, |this, _, _: &input::Submitted, cx| this.ask(cx)),
            cx.subscribe(&ask, |_, _, _: &input::Changed, cx| cx.notify()),
        ];
        Self {
            store,
            cwd: String::new(),
            project_label: String::new(),
            cards: Vec::new(),
            states: HashMap::new(),
            infos: HashMap::new(),
            orchestrator: None,
            tab: Tab::Active,
            composing: false,
            title,
            details,
            ask,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    pub(crate) fn set_context(&mut self, cwd: String, label: String, cx: &mut Context<Self>) {
        if self.cwd != cwd || self.project_label != label {
            self.cwd = cwd;
            self.project_label = label;
            self.refresh(cx);
        }
    }

    pub(crate) fn set_states(
        &mut self,
        states: HashMap<i64, LeadState>,
        infos: HashMap<i64, LeadInfo>,
        cx: &mut Context<Self>,
    ) {
        if self.states != states || self.infos != infos {
            self.states = states;
            self.infos = infos;
            cx.notify();
        }
    }

    pub(crate) fn set_orchestrator(
        &mut self,
        status: Option<OrchestratorStatus>,
        cx: &mut Context<Self>,
    ) {
        if self.orchestrator != status {
            self.orchestrator = status;
            cx.notify();
        }
    }

    /// Leads of this project's started cards, for the shell to report on.
    pub(crate) fn leads(&self) -> Vec<i64> {
        self.cards
            .iter()
            .filter(|c| !c.archived)
            .filter_map(|c| c.lead)
            .collect()
    }

    pub(crate) fn key(&self) -> String {
        board::prefix(&self.project_label)
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        match locked(&self.store) {
            Ok(store) => {
                self.cards = board::load(&store)
                    .into_iter()
                    .filter(|c| c.project == self.cwd)
                    .collect();
                // Newest first, like the references.
                self.cards.sort_by(|a, b| b.number.cmp(&a.number));
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = Tab::Active;
        self.composing = true;
        window.focus(&self.title.focus_handle(cx), cx);
        cx.notify();
    }

    fn cancel_compose(&mut self, cx: &mut Context<Self>) {
        self.composing = false;
        self.title.update(cx, |i, cx| i.set_text(String::new(), cx));
        self.details.update(cx, |i, cx| i.set_text(String::new(), cx));
        cx.notify();
    }

    fn add(&mut self, cx: &mut Context<Self>) {
        let title = self.title.read(cx).content.trim().to_owned();
        if title.is_empty() || self.cwd.is_empty() {
            return;
        }
        let details = self.details.read(cx).content.to_string();
        match locked(&self.store).and_then(|s| board::add(&s, &self.cwd, &title, &details)) {
            Ok(_) => {
                self.error = None;
                self.cancel_compose(cx);
            }
            Err(error) => self.error = Some(error),
        }
        self.refresh(cx);
        cx.emit(Event::Changed);
    }

    fn ask(&mut self, cx: &mut Context<Self>) {
        let message = self.ask.read(cx).content.trim().to_owned();
        if message.is_empty() || self.cwd.is_empty() {
            return;
        }
        self.ask.update(cx, |i, cx| i.set_text(String::new(), cx));
        cx.emit(Event::Ask(message));
    }

    fn change(&mut self, id: &str, change: impl FnOnce(&mut Card), cx: &mut Context<Self>) {
        if let Err(error) = locked(&self.store).and_then(|s| board::update(&s, id, change)) {
            self.error = Some(error);
        }
        self.refresh(cx);
        cx.emit(Event::Changed);
    }

    fn delete(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Err(error) = locked(&self.store).and_then(|s| board::remove(&s, id)) {
            self.error = Some(error);
        }
        self.refresh(cx);
        cx.emit(Event::Changed);
    }

    fn state(&self, card: &Card) -> Option<LeadState> {
        card.lead
            .map(|lead| self.states.get(&lead).copied().unwrap_or_default())
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let active = self.cards.iter().filter(|c| !c.archived).count();
        let archived = self.cards.len() - active;
        let tab = |id: &'static str, label: &'static str, count: usize, which: Tab, cx: &mut Context<Self>| {
            let selected = self.tab == which;
            div()
                .id(id)
                .role(Role::Tab)
                .aria_selected(selected)
                .aria_label(label)
                .cursor_pointer()
                .h_full()
                .px_1()
                .flex()
                .items_center()
                .gap_1p5()
                .border_b_2()
                .border_color(if selected { content(0.9) } else { gpui::transparent_black() })
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(content(if selected { 1. } else { 0.5 }))
                .hover(|s| s.text_color(content(1.)))
                .child(label)
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(content(0.4))
                        .child(count.to_string()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = which;
                    cx.notify();
                }))
        };
        div()
            .h(px(48.))
            .flex_shrink_0()
            .px_4()
            .flex()
            .items_center()
            .gap_5()
            .border_b_1()
            .border_color(content(0.08))
            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child("Board"))
            .child(
                div()
                    .h_full()
                    .flex()
                    .gap_4()
                    .child(tab("board-tab-active", "Active", active, Tab::Active, cx))
                    .child(tab("board-tab-archived", "Archived", archived, Tab::Archived, cx)),
            )
            .into_any_element()
    }

    fn composer_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let can_add = !self.title.read(cx).content.trim().is_empty() && !self.cwd.is_empty();
        div()
            .flex_shrink_0()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .rounded_lg()
            .border_1()
            .border_color(accent_color().opacity(0.5))
            .bg(content(0.03))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(content(0.45))
                    .font_family(crate::fonts::mono())
                    .child(format!(
                        "{}-{:03}",
                        self.key(),
                        board::next_number(&self.cards, &self.cwd)
                    )),
            )
            .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(self.title.clone()))
            .child(self.details.clone())
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_1p5()
                    .child(
                        card_button("board-compose-cancel".into(), "Cancel", Tone::Plain)
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_compose(cx))),
                    )
                    .child(
                        card_button("board-compose-add".into(), "Add to backlog", Tone::Accent)
                            .when(!can_add, |d| d.opacity(0.4))
                            .on_click(cx.listener(|this, _, _, cx| this.add(cx))),
                    ),
            )
            .into_any_element()
    }

    fn card(&self, card: &Card, lane: Option<Lane>, cx: &mut Context<Self>) -> AnyElement {
        let key = board::card_key(&self.key(), card.number);
        let state = self.state(card);
        let status = board::status_label(card, state);
        let info = card.lead.and_then(|lead| self.infos.get(&lead)).cloned();
        let id = card.id.clone();

        let mut actions = div().flex().items_center().gap_1p5();
        match lane {
            Some(Lane::Backlog) => {
                let start = card.clone();
                actions = actions
                    .child(
                        card_button(format!("board-delete-{id}").into(), "Delete", Tone::Danger)
                            .on_click(cx.listener({
                                let id = id.clone();
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.delete(&id, cx)
                                }
                            })),
                    )
                    .child(
                        card_button(format!("board-start-{id}").into(), "Start", Tone::Accent)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.stop_propagation();
                                cx.emit(Event::Start(start.clone()))
                            })),
                    );
            }
            Some(Lane::InProgress) => {}
            Some(Lane::NeedsReview) => {
                if let Some(lead) = card.lead.filter(|_| state.is_some_and(|s| s.exists)) {
                    actions = actions.child(
                        card_button(format!("board-open-{id}").into(), "Open", Tone::Plain)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.stop_propagation();
                                cx.emit(Event::OpenSession(lead))
                            })),
                    );
                }
                actions = actions.child(
                    card_button(format!("board-archive-{id}").into(), "Archive", Tone::Accent)
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.change(&id, |c| c.archived = true, cx)
                            }
                        })),
                );
            }
            None => {
                actions = actions
                    .child(
                        card_button(format!("board-delete-{id}").into(), "Delete", Tone::Danger)
                            .on_click(cx.listener({
                                let id = id.clone();
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.delete(&id, cx)
                                }
                            })),
                    )
                    .child(
                        card_button(format!("board-restore-{id}").into(), "Restore", Tone::Plain)
                            .on_click(cx.listener({
                                let id = id.clone();
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.change(&id, |c| c.archived = false, cx)
                                }
                            })),
                    );
            }
        }

        let (status_icon, status_color) = match status {
            "Waiting on you" | "Plan ready" => ("circle-alert", accent_color()),
            "Planning" | "Workers running" => ("loader", amber(1.)),
            "Finished" => ("check-hi", green(1.)),
            "Session removed" => ("x", danger(1.)),
            _ => ("clock", content(0.45)),
        };
        let footer_status = div()
            .flex()
            .items_center()
            .gap_1p5()
            .min_w_0()
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(status_color)
            .child(icon(status_icon, 13.).text_color(status_color))
            .child(div().truncate().child(status));

        let meta = info.map(|info| {
            let done = info.workers.iter().filter(|s| *s == "completed").count();
            div()
                .mt_2()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(11.))
                .text_color(content(0.5))
                .child(provider_icon(&info.provider, 13.))
                .child(
                    div()
                        .truncate()
                        .font_family(crate::fonts::mono())
                        .child(info.model.clone().unwrap_or_else(|| info.provider.clone())),
                )
                .when(!info.workers.is_empty(), |d| {
                    d.child(div().text_color(content(0.3)).child("•"))
                        .child(
                            div().flex().gap(px(2.)).children(
                                info.workers
                                    .iter()
                                    .take(12)
                                    .map(|s| div().size(px(8.)).rounded(px(2.)).bg(worker_color(s))),
                            ),
                        )
                        .child(format!("{done}/{}", info.workers.len()))
                })
        });

        let lead = card.lead;
        div()
            .id(SharedString::from(format!("board-card-{id}")))
            .debug_selector(|| "board-card".into())
            .role(Role::Group)
            .aria_label(format!("{key} {}", card.title))
            .flex_shrink_0()
            .rounded_lg()
            .border_1()
            .border_color(content(0.09))
            .bg(pane_fill())
            .overflow_hidden()
            .when(lead.is_some(), |d| {
                d.cursor_pointer()
                    .hover(|s| s.border_color(content(0.18)))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        if let Some(lead) = lead {
                            cx.emit(Event::OpenSession(lead))
                        }
                    }))
            })
            .child(
                div()
                    .p_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_size(px(11.))
                            .text_color(content(0.45))
                            .child(icon("dashboard", 12.))
                            .child(div().font_family(crate::fonts::mono()).child(key))
                            .child(div().flex_1())
                            .child(ago(card.created_at)),
                    )
                    .child(
                        div()
                            .mt_1p5()
                            .text_size(px(14.))
                            .line_height(px(19.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(card.title.clone()),
                    )
                    .when(!card.details.trim().is_empty(), |d| {
                        d.child(
                            div()
                                .mt_1()
                                .text_size(px(12.))
                                .line_height(px(17.))
                                .text_color(content(0.55))
                                .line_clamp(2)
                                .child(card.details.trim().to_owned()),
                        )
                    })
                    .children(meta),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .border_t_1()
                    .border_color(content(0.07))
                    .bg(content(0.02))
                    .child(footer_status)
                    .child(actions),
            )
            .into_any_element()
    }

    fn lanes(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut by_lane: HashMap<Lane, Vec<Card>> = HashMap::new();
        for card in self.cards.iter().filter(|c| !c.archived) {
            by_lane
                .entry(board::lane(card, self.state(card)))
                .or_default()
                .push(card.clone());
        }
        let last = Lane::ALL.len() - 1;
        let columns: Vec<AnyElement> = Lane::ALL
            .into_iter()
            .enumerate()
            .map(|(ix, lane)| {
                let cards = by_lane.remove(&lane).unwrap_or_default();
                let count = cards.len();
                let color = lane_color(lane);
                let composer = (lane == Lane::Backlog && self.composing).then(|| self.composer_card(cx));
                let cards: Vec<_> = cards.iter().map(|c| self.card(c, Some(lane), cx)).collect();
                div()
                    .id(SharedString::from(format!("board-lane-{}", lane.label())))
                    .role(Role::List)
                    .aria_label(lane.label())
                    .flex_1()
                    .min_w(px(260.))
                    .h_full()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .when(ix < last, |d| d.border_r_1().border_color(content(0.07)))
                    .child(
                        div()
                            .h(px(40.))
                            .flex_shrink_0()
                            .px_4()
                            .flex()
                            .items_center()
                            .gap_2()
                            .border_b_1()
                            .border_color(content(0.07))
                            .child(div().size(px(8.)).rounded_full().bg(color))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(color)
                                    .child(lane.label()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(content(0.4))
                                    .child(count.to_string()),
                            )
                            .child(div().flex_1())
                            .when(lane == Lane::Backlog, |d| {
                                d.child(
                                    div()
                                        .id("board-add-task")
                                        .role(Role::Button)
                                        .aria_label("Add a task")
                                        .cursor_pointer()
                                        .size(px(22.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_md()
                                        .hover(|s| s.bg(content(0.08)))
                                        .child(icon("plus", 14.))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.compose(window, cx)
                                        })),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("board-lane-scroll-{}", lane.label())))
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_3()
                            .flex()
                            .flex_col()
                            .gap_2p5()
                            .children(composer)
                            .children(cards)
                            .when(count == 0 && !(lane == Lane::Backlog && self.composing), |d| {
                                d.child(
                                    div()
                                        .pt_2()
                                        .text_size(px(12.))
                                        .text_color(content(0.35))
                                        .child(match lane {
                                            Lane::Backlog => "No tasks yet. Ask the orchestrator below, or press +.",
                                            Lane::InProgress => "Started tasks appear here while agents work.",
                                            Lane::NeedsReview => "Finished tasks and plans waiting on you.",
                                        }),
                                )
                            }),
                    )
                    .into_any_element()
            })
            .collect();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .children(columns)
            .into_any_element()
    }

    fn archived(&self, cx: &mut Context<Self>) -> AnyElement {
        let cards: Vec<_> = self
            .cards
            .iter()
            .filter(|c| c.archived)
            .map(|c| div().w(px(320.)).child(self.card(c, None, cx)).into_any_element())
            .collect();
        let empty = cards.is_empty();
        div()
            .id("board-archived")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_4()
            .flex()
            .flex_wrap()
            .content_start()
            .gap_3()
            .children(cards)
            .when(empty, |d| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .text_color(content(0.35))
                        .child("Archived tasks appear here."),
                )
            })
            .into_any_element()
    }

    fn reply_strip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let status = self.orchestrator.clone()?;
        // A short plain-text excerpt: replies are Markdown.
        let collapsed = plain_excerpt(&status.reply);
        let mut reply: String = collapsed.chars().take(280).collect();
        if collapsed.chars().count() > 280 {
            reply.push('…');
        }
        if reply.is_empty() && !status.busy {
            return None;
        }
        let session = status.session;
        Some(
            div()
                .w_full()
                .max_w(px(720.))
                .px_3()
                .py_2p5()
                .flex()
                .gap_2p5()
                .rounded_xl()
                .border_1()
                .border_color(content(0.08))
                .bg(pane_fill())
                .child(
                    div()
                        .size(px(22.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .bg(content(0.08))
                        .child(icon("git-merge", 13.)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_size(px(12.))
                                .child(div().font_weight(FontWeight::SEMIBOLD).child("Orchestrator"))
                                .when(status.busy, |d| {
                                    d.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .text_color(amber(1.))
                                            .child(icon("loader", 12.).text_color(amber(1.)))
                                            .child("Working…"),
                                    )
                                })
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .id("board-open-orchestrator")
                                        .role(Role::Button)
                                        .aria_label("Open the orchestrator chat")
                                        .cursor_pointer()
                                        .text_color(content(0.5))
                                        .hover(|s| s.text_color(content(1.)))
                                        .child("Open chat")
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.emit(Event::OpenSession(session))
                                        })),
                                ),
                        )
                        .when(!reply.is_empty(), |d| {
                            d.child(
                                div()
                                    .text_size(px(12.))
                                    .line_height(px(17.))
                                    .text_color(content(0.7))
                                    .max_h(px(51.))
                                    .overflow_hidden()
                                    .child(reply),
                            )
                        }),
                )
                .into_any_element(),
        )
    }

    fn message_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let can_ask = !self.ask.read(cx).content.trim().is_empty() && !self.cwd.is_empty();
        div()
            .flex_shrink_0()
            .px_4()
            .pb_4()
            .pt_2()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .children(self.reply_strip(cx))
            .child(
                div()
                    .w_full()
                    .max_w(px(720.))
                    .h(px(44.))
                    .pl_4()
                    .pr_1p5()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_xl()
                    .border_1()
                    .border_color(composer_border(false))
                    .bg(composer_fill())
                    .shadow(composer_shadow())
                    .child(div().flex_1().min_w_0().child(self.ask.clone()))
                    .child(
                        div()
                            .id("board-ask")
                            .role(Role::Button)
                            .aria_label("Send to the orchestrator")
                            .cursor_pointer()
                            .size(px(32.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_lg()
                            .bg(content(if can_ask { 0.92 } else { 0.12 }))
                            .child(icon("arrow-up", 16.).text_color(if can_ask {
                                floating_fill().opacity(1.)
                            } else {
                                content(0.4)
                            }))
                            .on_click(cx.listener(|this, _, _, cx| this.ask(cx))),
                    ),
            )
            .into_any_element()
    }
}

impl gpui::Render for Board {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.cwd.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(13.))
                .text_color(content(0.45))
                .child("Open a project to use its board.")
                .into_any_element()
        } else if self.tab == Tab::Archived {
            self.archived(cx)
        } else {
            self.lanes(cx)
        };
        div()
            .id("board-view")
            .role(Role::Region)
            .aria_label("Board")
            .size_full()
            .flex()
            .flex_col()
            .text_color(content(1.))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    if this.composing {
                        this.cancel_compose(cx);
                    } else {
                        cx.emit(Event::Close);
                    }
                    cx.stop_propagation();
                }
            }))
            .child(self.header(cx))
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .px_4()
                        .py_1p5()
                        .text_size(px(12.))
                        .text_color(danger(1.))
                        .child(error),
                )
            })
            .child(body)
            .when(!self.cwd.is_empty(), |d| d.child(self.message_bar(cx)))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn previews_read_markdown_as_prose() {
        let reply = "## Step 1: tighten the loop\n\nKeep **rows** `retained`.\n\n- one\n- two\n\n```rust\nfn x() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        assert_eq!(
            super::plain_excerpt(reply),
            "Step 1: tighten the loop. Keep rows retained. one two"
        );
    }
}
