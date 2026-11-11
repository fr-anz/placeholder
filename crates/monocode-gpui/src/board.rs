//! The project board: Backlog, In progress and Needs review lanes derived from
//! each card's coordinator session. Starting a card hands it to the shell,
//! which opens an orchestrator session that plans role-assigned workers.
use crate::{
    input::{self, Input},
    runtime::{Database, locked},
    theme::*,
};
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, Role,
    SharedString, Subscription, Window, div, prelude::*, px,
};
use monocode_core::board::{self, Card, Lane, LeadState};
use std::collections::HashMap;

pub(crate) enum Event {
    Close,
    OpenSession(i64),
    /// Start a backlog card: the shell creates its coordinator session.
    Start(Card),
    /// Ask the board orchestrator: the shell opens a fresh session for it.
    Ask(String),
}

pub(crate) struct Board {
    store: Database,
    cwd: String,
    project_label: String,
    cards: Vec<Card>,
    states: HashMap<i64, LeadState>,
    title: Entity<Input>,
    details: Entity<Input>,
    ask: Entity<Input>,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Event> for Board {}
impl Focusable for Board {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.title.focus_handle(cx)
    }
}

impl Board {
    pub(crate) fn new(store: Database, cx: &mut Context<Self>) -> Self {
        let title = cx.new(|cx| {
            let mut input = Input::field("Card title", "New card title…", content(0.35), cx);
            input.set_max_chars(board::MAX_TITLE);
            input
        });
        let details = cx.new(|cx| {
            let mut input = Input::multiline_field(
                "Card spec",
                "Spec: goal, relevant files, acceptance criteria (optional)",
                cx,
            );
            input.set_max_chars(board::MAX_DETAILS);
            input.set_min_height(64.);
            input.set_max_height(160.);
            input
        });
        let ask = cx.new(|cx| {
            Input::field(
                "Ask the board orchestrator",
                "Ask the orchestrator to plan, split or start cards…",
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

    pub(crate) fn set_states(&mut self, states: HashMap<i64, LeadState>, cx: &mut Context<Self>) {
        if self.states != states {
            self.states = states;
            cx.notify();
        }
    }

    /// Leads of this project's started cards, for the shell to report on.
    pub(crate) fn leads(&self) -> Vec<i64> {
        self.cards.iter().filter_map(|c| c.lead).collect()
    }

    pub(crate) fn key(&self) -> String {
        board::prefix(&self.project_label)
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        match locked(&self.store) {
            Ok(store) => {
                self.cards = board::load(&store)
                    .into_iter()
                    .filter(|c| c.project == self.cwd && !c.archived)
                    .collect();
                self.cards.sort_by_key(|c| c.number);
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn add(&mut self, cx: &mut Context<Self>) {
        let title = self.title.read(cx).content.trim().to_owned();
        if title.is_empty() || self.cwd.is_empty() {
            return;
        }
        let details = self.details.read(cx).content.to_string();
        let result = locked(&self.store).and_then(|s| board::add(&s, &self.cwd, &title, &details));
        match result {
            Ok(_) => {
                self.error = None;
                self.title.update(cx, |i, cx| i.set_text(String::new(), cx));
                self.details.update(cx, |i, cx| i.set_text(String::new(), cx));
            }
            Err(error) => self.error = Some(error),
        }
        self.refresh(cx);
    }

    fn change(&mut self, id: &str, change: impl FnOnce(&mut Card), cx: &mut Context<Self>) {
        if let Err(error) = locked(&self.store).and_then(|s| board::update(&s, id, change)) {
            self.error = Some(error);
        }
        self.refresh(cx);
    }

    fn delete(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Err(error) = locked(&self.store).and_then(|s| board::remove(&s, id)) {
            self.error = Some(error);
        }
        self.refresh(cx);
    }

    fn ask(&mut self, cx: &mut Context<Self>) {
        let message = self.ask.read(cx).content.trim().to_owned();
        if message.is_empty() || self.cwd.is_empty() {
            return;
        }
        self.ask.update(cx, |i, cx| i.set_text(String::new(), cx));
        cx.emit(Event::Ask(message));
    }

    fn state(&self, card: &Card) -> Option<LeadState> {
        card.lead
            .map(|lead| self.states.get(&lead).copied().unwrap_or_default())
    }

    fn card(&self, card: &Card, lane: Lane, cx: &mut Context<Self>) -> AnyElement {
        let key = format!("{}-{}", self.key(), card.number);
        let status = board::status_label(card, self.state(card));
        let preview: String = card.details.lines().next().unwrap_or_default().chars().take(140).collect();
        let id = card.id.clone();
        let mut actions = div().flex().items_center().gap_1().mt_2();
        match lane {
            Lane::Backlog => {
                let start = card.clone();
                actions = actions
                    .child(
                        crate::modal::primary_button(
                            SharedString::from(format!("board-start-{id}")),
                            "Start",
                        )
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(Event::Start(start.clone())))),
                    )
                    .child(
                        crate::modal::danger_ghost_button(
                            SharedString::from(format!("board-delete-{id}")),
                            "Delete",
                        )
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |this, _, _, cx| this.delete(&id, cx)
                        })),
                    );
            }
            Lane::InProgress | Lane::NeedsReview => {
                if let Some(lead) = card.lead {
                    actions = actions.child(
                        crate::modal::ghost_button(
                            SharedString::from(format!("board-open-{id}")),
                            "Open",
                        )
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(Event::OpenSession(lead)))),
                    );
                }
                if lane == Lane::NeedsReview {
                    actions = actions.child(
                        crate::modal::ghost_button(
                            SharedString::from(format!("board-archive-{id}")),
                            "Archive",
                        )
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |this, _, _, cx| this.change(&id, |c| c.archived = true, cx)
                        })),
                    );
                }
            }
        }
        div()
            .id(SharedString::from(format!("board-card-{id}")))
            .debug_selector(|| "board-card".into())
            .role(Role::Group)
            .aria_label(format!("{key} {}", card.title))
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(content(0.08))
            .bg(content(0.03))
            .hover(|s| s.bg(content(0.05)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .text_size(px(11.))
                    .text_color(content(0.45))
                    .child(div().font_family(crate::fonts::mono()).child(key))
                    .child(
                        div()
                            .px_1p5()
                            .py(px(1.))
                            .rounded_sm()
                            .bg(content(match status {
                                "Waiting on you" | "Plan ready" => 0.14,
                                _ => 0.07,
                            }))
                            .child(status),
                    ),
            )
            .child(
                div()
                    .mt_1()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(card.title.clone()),
            )
            .when(!preview.is_empty(), |d| {
                d.child(
                    div()
                        .mt_1()
                        .text_size(px(12.))
                        .text_color(content(0.5))
                        .child(preview),
                )
            })
            .child(actions)
            .into_any_element()
    }
}

impl gpui::Render for Board {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut lanes: HashMap<Lane, Vec<Card>> = HashMap::new();
        for card in &self.cards {
            lanes
                .entry(board::lane(card, self.state(card)))
                .or_default()
                .push(card.clone());
        }
        let can_add = !self.title.read(cx).content.trim().is_empty() && !self.cwd.is_empty();
        let can_ask = !self.ask.read(cx).content.trim().is_empty() && !self.cwd.is_empty();
        let orchestrator = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .h(px(32.))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_md()
                    .border_1()
                    .border_color(content(0.1))
                    .bg(content(0.04))
                    .child(div().flex_1().min_w_0().child(self.ask.clone())),
            )
            .child(
                crate::modal::ghost_button("board-ask", "Ask orchestrator")
                    .when(!can_ask, |d| d.opacity(0.4))
                    .on_click(cx.listener(|this, _, _, cx| this.ask(cx))),
            );
        let composer = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(content(0.08))
            .child(
                div()
                    .h(px(30.))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_md()
                    .bg(content(0.04))
                    .child(div().flex_1().min_w_0().child(self.title.clone())),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(content(0.04))
                    .child(self.details.clone()),
            )
            .child(
                div().flex().justify_end().child(
                    crate::modal::primary_button("board-add-card", "Add to backlog")
                        .when(!can_add, |d| d.opacity(0.4))
                        .on_click(cx.listener(|this, _, _, cx| this.add(cx))),
                ),
            );
        let columns = Lane::ALL.map(|lane| {
            let cards = lanes.remove(&lane).unwrap_or_default();
            let count = cards.len();
            div()
                .id(SharedString::from(format!("board-lane-{}", lane.label())))
                .role(Role::List)
                .aria_label(lane.label())
                .flex_1()
                .min_w(px(220.))
                .h_full()
                .min_h_0()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(content(0.7))
                        .child(lane.label())
                        .child(div().text_color(content(0.35)).child(count.to_string())),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("board-lane-scroll-{}", lane.label())))
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .children(cards.iter().map(|card| self.card(card, lane, cx)))
                        .when(count == 0, |d| {
                            d.child(
                                div()
                                    .py_4()
                                    .text_size(px(12.))
                                    .text_color(content(0.35))
                                    .child(match lane {
                                        Lane::Backlog => "Add a card above.",
                                        Lane::InProgress => "Started cards appear here while agents work.",
                                        Lane::NeedsReview => "Finished cards and plans waiting on you.",
                                    }),
                            )
                        }),
                )
        });
        div()
            .id("board-view")
            .role(Role::Region)
            .aria_label("Board")
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .text_color(content(1.))
            .on_key_down(cx.listener(|_, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.emit(Event::Close);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child("Board"))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.45))
                            .child(if self.cwd.is_empty() {
                                "Open a project to use its board".to_owned()
                            } else {
                                format!("{} · {}", self.project_label, self.key())
                            }),
                    ),
            )
            .when_some(self.error.clone(), |d, error| {
                d.child(div().text_size(px(12.)).text_color(gpui::red()).child(error))
            })
            .child(orchestrator)
            .child(composer)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap_4()
                    .children(columns),
            )
    }
}
