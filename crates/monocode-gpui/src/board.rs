//! The project board: Backlog, In progress and Needs review lanes derived from
//! each card's lead session. Starting a card hands it to the shell, which
//! opens a coordinator for an orchestrated card or one implementer for a solo
//! card. The message bar is the board orchestrator's own chat composer.
use crate::{
    input::{self, Input},
    popover::{Anchor, AnchorSlot, Menu, MenuAction, MenuEvent, Side, menu_popover},
    runtime::{Database, locked},
    theme::*,
};
use gpui::{
    AnyElement, AnyView, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    Hsla, Role, SharedString, Stateful, Subscription, Window, div, prelude::*, px,
};
use monocode_core::board::{self, Card, CardMode, Lane, LeadState};
use std::collections::HashMap;

pub(crate) enum Event {
    Close,
    OpenSession(i64),
    /// Start a backlog card: the shell creates its lead session.
    Start(Card),
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

/// `native_settings` key of the orchestrator reply last seen on a board.
const REPLY_SEEN_PREFIX: &str = "fork.board_reply_seen:";

/// The plain-text excerpt the reply card shows, which also identifies a reply.
fn reply_excerpt(markdown: &str) -> String {
    let collapsed = plain_excerpt(markdown);
    let mut reply: String = collapsed.chars().take(240).collect();
    if collapsed.chars().count() > 240 {
        reply.push('…');
    }
    reply
}

/// The reply card is hidden unless the orchestrator is working or has a
/// reply the user has not dismissed or opened.
fn shows_reply(busy: bool, reply: &str, seen: &str) -> bool {
    busy || (!reply.is_empty() && reply != seen)
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
    /// The board orchestrator's chat composer, docked under the lanes.
    dock: Option<AnyView>,
    focus: FocusHandle,
    tab: Tab,
    composing: bool,
    /// The mode of the card being composed.
    compose_mode: CardMode,
    /// The excerpt of the orchestrator reply last dismissed or opened on this
    /// project's board, loaded with the cards.
    reply_seen: String,
    /// Each backlog card's Start arrow, where its menu opens.
    start_anchors: HashMap<String, AnchorSlot>,
    /// The open "Start solo / Start with orchestrator" menu and its card.
    start_menu: Option<(String, Entity<Menu>)>,
    _start_menu_events: Option<Subscription>,
    title: Entity<Input>,
    details: Entity<Input>,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Event> for Board {}
impl Focusable for Board {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
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
        "running" | "reviewing" | "merging" | "escalated" => amber(1.),
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
            input.set_yields_when_empty(true);
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
        let subscriptions = vec![
            cx.subscribe(&title, |this, _, _: &input::Submitted, cx| this.add(cx)),
            cx.subscribe(&title, |_, _, _: &input::Changed, cx| cx.notify()),
        ];
        Self {
            store,
            cwd: String::new(),
            project_label: String::new(),
            cards: Vec::new(),
            states: HashMap::new(),
            infos: HashMap::new(),
            orchestrator: None,
            dock: None,
            focus: cx.focus_handle(),
            tab: Tab::Active,
            composing: false,
            compose_mode: CardMode::default(),
            reply_seen: String::new(),
            start_anchors: HashMap::new(),
            start_menu: None,
            _start_menu_events: None,
            title,
            details,
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

    /// The board orchestrator's composer view; `None` outside a project.
    pub(crate) fn set_dock(&mut self, dock: Option<AnyView>, cx: &mut Context<Self>) {
        if self.dock.as_ref().map(|d| d.entity_id()) != dock.as_ref().map(|d| d.entity_id()) {
            self.dock = dock;
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
                self.reply_seen = store
                    .setting(&format!("{REPLY_SEEN_PREFIX}{}", self.cwd))
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                let cards = &self.cards;
                self.start_anchors.retain(|id, _| cards.iter().any(|c| c.id == *id));
                for card in cards.iter().filter(|c| c.lead.is_none()) {
                    self.start_anchors.entry(card.id.clone()).or_default();
                }
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
        let mode = self.compose_mode;
        match locked(&self.store).and_then(|s| board::add(&s, &self.cwd, &title, &details, mode)) {
            Ok(_) => {
                self.error = None;
                self.cancel_compose(cx);
            }
            Err(error) => self.error = Some(error),
        }
        self.refresh(cx);
        cx.emit(Event::Changed);
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
                    .items_center()
                    .justify_end()
                    .gap_1p5()
                    .child(self.compose_mode_switch(cx))
                    .child(div().flex_1())
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
                    .child(self.start_button(&start, cx));
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

        let card_icon = match &info {
            Some(info) => provider_icon(&info.provider, 13.).into_any_element(),
            None => icon("dashboard", 12.).into_any_element(),
        };
        let meta = info.filter(|info| !info.workers.is_empty()).map(|info| {
            let done = info.workers.iter().filter(|s| *s == "completed").count();
            div()
                .mt_2()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(11.))
                .text_color(content(0.5))
                .child(div().child("Agents"))
                .map(|d| {
                    d.child(
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
                            .child(card_icon)
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

    /// The orchestrator's latest reply, above the composer. Clicking it opens
    /// the chat; its chevron folds it to one line so it stops covering the
    /// lanes.
    fn reply(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let status = self.orchestrator.clone()?;
        // A short plain-text excerpt: replies are Markdown.
        let reply = reply_excerpt(&status.reply);
        let unseen = !reply.is_empty() && reply != self.reply_seen;
        if !shows_reply(status.busy, &reply, &self.reply_seen) {
            return None;
        }
        let session = status.session;
        let open = unseen;
        let dismiss = div()
            .id("board-orchestrator-reply-dismiss")
            .role(Role::Button)
            .aria_label("Dismiss the orchestrator's reply")
            .cursor_pointer()
            .flex_shrink_0()
            .size(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .hover(|s| s.bg(content(0.08)))
            .child(icon("close", 12.).text_color(content(0.45)))
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.mark_reply_seen(cx);
            }));
        let header = div()
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
            });
        Some(
            div()
                .id("board-orchestrator-reply")
                .role(Role::Button)
                .aria_label("Open the orchestrator chat")
                .cursor_pointer()
                .px_3()
                .py(px(7.))
                .flex()
                .items_start()
                .gap_2p5()
                .hover(|s| s.bg(content(0.035)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.mark_reply_seen(cx);
                    cx.emit(Event::OpenSession(session))
                }))
                .child(
                    div()
                        .mt(px(1.))
                        .size(px(20.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(accent_color().opacity(0.16))
                        .child(icon("git-merge", 11.).text_color(accent_color())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(div().h(px(22.)).flex().items_center().child(header))
                        .when(open && !reply.is_empty(), |d| {
                            d.child(
                                div()
                                    .text_size(px(12.5))
                                    .line_height(px(18.))
                                    .text_color(content(0.72))
                                    .max_h(px(36.))
                                    .overflow_hidden()
                                    .child(reply),
                            )
                        }),
                )
                .when(unseen, |d| d.child(dismiss))
                .into_any_element(),
        )
    }

    /// Solo or Orchestrate for the card being composed.
    fn compose_mode_switch(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.compose_mode;
        div()
            .flex()
            .items_center()
            .p(px(2.))
            .gap(px(2.))
            .rounded_md()
            .bg(content(0.05))
            .children([CardMode::Solo, CardMode::Orchestrate].map(|mode| {
                let on = mode == current;
                div()
                    .id(SharedString::from(format!("board-compose-mode-{}", mode.id())))
                    .role(Role::Button)
                    .aria_label(format!("{} card", mode.label()))
                    .aria_selected(on)
                    .cursor_pointer()
                    .h(px(22.))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded(px(5.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(content(if on { 0.95 } else { 0.5 }))
                    .when(on, |d| d.bg(content(0.1)))
                    .hover(|s| s.text_color(content(1.)))
                    .child(mode.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.compose_mode = mode;
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }

    /// Hides the current reply until the orchestrator writes a new one.
    fn mark_reply_seen(&mut self, cx: &mut Context<Self>) {
        let Some(status) = &self.orchestrator else {
            return;
        };
        let reply = reply_excerpt(&status.reply);
        if reply.is_empty() || reply == self.reply_seen {
            return;
        }
        let key = format!("{REPLY_SEEN_PREFIX}{}", self.cwd);
        if let Err(error) = locked(&self.store).and_then(|s| s.save_setting(&key, &reply)) {
            self.error = Some(error);
        }
        self.reply_seen = reply;
        cx.notify();
    }

    /// Start, in the card's mode, with an arrow for "Start solo" and
    /// "Start with orchestrator".
    fn start_button(&self, card: &Card, cx: &mut Context<Self>) -> AnyElement {
        let id = card.id.clone();
        let anchor = self.start_anchors.get(&id).cloned().unwrap_or_default();
        let open = self.start_menu.as_ref().is_some_and(|(c, _)| *c == id);
        let glyph = match card.mode {
            CardMode::Solo => "user",
            CardMode::Orchestrate => "share",
        };
        let start = card.clone();
        let half = |el: Stateful<gpui::Div>| {
            el.h(px(26.))
                .flex()
                .items_center()
                .bg(accent_color())
                .text_color(gpui::white())
                .cursor_pointer()
                .hover(|s| s.opacity(0.9))
        };
        div()
            .flex()
            .items_center()
            .gap(px(1.))
            .child(
                half(div().id(SharedString::from(format!("board-start-{id}"))))
                    .role(Role::Button)
                    .aria_label(format!(
                        "Start {}",
                        match card.mode {
                            CardMode::Solo => "solo",
                            CardMode::Orchestrate => "with orchestrator",
                        }
                    ))
                    .pl(px(9.))
                    .pr(px(8.))
                    .gap_1()
                    .rounded_l_md()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(icon(glyph, 11.).text_color(gpui::white()))
                    .child("Start")
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.stop_propagation();
                        cx.emit(Event::Start(start.clone()))
                    })),
            )
            .child(
                half(div().id(SharedString::from(format!("board-start-menu-{id}"))))
                    .role(Role::Button)
                    .aria_label("More ways to start")
                    .aria_expanded(open)
                    .px(px(5.))
                    .rounded_r_md()
                    .child(anchor.probe())
                    .child(icon("chevron-down", 12.).text_color(gpui::white()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.open_start_menu(&id, window, cx)
                    })),
            )
            .into_any_element()
    }

    #[cfg(test)]
    pub(crate) fn open_start_menu_for_test(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_start_menu(id, window, cx)
    }
    #[cfg(test)]
    pub(crate) fn start_menu_for_test(&self) -> Option<Entity<Menu>> {
        self.start_menu.as_ref().map(|(_, menu)| menu.clone())
    }
    fn open_start_menu(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.start_menu.take().is_some_and(|(c, _)| c == id) {
            self._start_menu_events = None;
            cx.notify();
            return;
        }
        let Some(card) = self.cards.iter().find(|c| c.id == id).cloned() else {
            return;
        };
        let items = vec![
            MenuAction::new("solo", "Start solo")
                .icon("user")
                .description("One implementer works it, with no planning turn")
                .checked(card.mode == CardMode::Solo)
                .into(),
            MenuAction::new("orchestrate", "Start with orchestrator")
                .icon("share")
                .description("A coordinator plans several workers")
                .checked(card.mode == CardMode::Orchestrate)
                .into(),
        ];
        let menu = cx.new(|cx| Menu::new("Start", items, window, cx));
        let card_id = card.id.clone();
        self._start_menu_events = Some(cx.subscribe(&menu, move |this, _, event: &MenuEvent, cx| {
            this.start_menu = None;
            this._start_menu_events = None;
            if let MenuEvent::Picked(pick, _) = event {
                let mode = if pick.as_ref() == "solo" { CardMode::Solo } else { CardMode::Orchestrate };
                let started = locked(&this.store)
                    .and_then(|s| board::update(&s, &card_id, |c| c.mode = mode));
                match started {
                    Ok(card) => cx.emit(Event::Start(card)),
                    Err(error) => this.error = Some(error),
                }
                this.refresh(cx);
            }
            cx.notify();
        }));
        self.start_menu = Some((card.id, menu));
        cx.notify();
    }

    /// The orchestrator's latest reply over its real chat composer, so the
    /// board gets attachments, images, the model menu and slash commands.
    fn composer(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_shrink_0()
            .px_2()
            .pb_2()
            .pt_1()
            .flex()
            .flex_col()
            .items_center()
            // The same column and inset as the docked composer below it.
            .children(self.reply(cx).map(|reply| {
                div()
                    .w_full()
                    .max_w(px(crate::conversation::COMPOSER_COLUMN))
                    .px(px(10.))
                    .mb_1()
                    .child(
                        div()
                            .rounded(px(14.))
                            .border_1()
                            .border_color(content(0.08))
                            .bg(content(0.03))
                            .overflow_hidden()
                            .child(reply),
                    )
            }))
            .children(self.dock.clone().map(|dock| div().w_full().flex().justify_center().child(dock)))
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
                    if this.start_menu.take().is_some() {
                        this._start_menu_events = None;
                        cx.notify();
                    } else if this.composing {
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
            .track_focus(&self.focus)
            .when(!self.cwd.is_empty(), |d| d.child(self.composer(cx)))
            .children(self.start_menu.as_ref().and_then(|(id, menu)| {
                let anchor = self.start_anchors.get(id)?.clone();
                Some(menu_popover(menu, Anchor::Slot(anchor), cx).side(Side::Bottom))
            }))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_reply_card_shows_only_while_working_or_until_seen() {
        use super::shows_reply;
        assert!(!shows_reply(false, "", ""), "no reply, nothing to show");
        assert!(shows_reply(false, "Done.", ""), "a new reply shows");
        assert!(!shows_reply(false, "Done.", "Done."), "a seen reply stays hidden");
        assert!(shows_reply(true, "Done.", "Done."), "working shows the card again");
        assert!(shows_reply(false, "Next step.", "Done."), "the next reply shows");
    }

    #[test]
    fn previews_read_markdown_as_prose() {
        let reply = "## Step 1: tighten the loop\n\nKeep **rows** `retained`.\n\n- one\n- two\n\n```rust\nfn x() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        assert_eq!(
            super::plain_excerpt(reply),
            "Step 1: tighten the loop. Keep rows retained. one two"
        );
    }
}
