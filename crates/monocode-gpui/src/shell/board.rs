//! The project board in the shell: opening it, reporting each started card's
//! coordinator state, and starting a card as an orchestrator session.
use super::app_views::AppView;
use super::*;
use crate::board;
use monocode_core::board::{Card, LeadState};

impl Shell {
    pub(super) fn open_board(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prepare_app_view(window, cx);
        if self.board.is_none() {
            let view = cx.new(|cx| board::Board::new(self.store.clone(), cx));
            self.app_view_events.push(cx.subscribe_in(
                &view,
                window,
                |this, _, event: &board::Event, window, cx| match event {
                    board::Event::Close => this.close_app_view(window, cx),
                    board::Event::OpenSession(id) => this.open(*id, window, cx),
                    board::Event::Start(card) => this.start_board_card(card.clone(), cx),
                },
            ));
            self.board = Some(view);
        }
        if let Some(view) = &self.board {
            let cwd = self.workspace.project.clone();
            let label = self.project_label(&cwd);
            view.update(cx, |view, cx| {
                view.set_context(cwd, label, cx);
                view.refresh(cx);
            });
        }
        self.sync_board(cx);
        self.show_app_view(AppView::Board, window, cx);
    }

    /// What the board shows for a coordinator session right now.
    fn lead_state(&self, lead: i64, cx: &App) -> LeadState {
        if !self.session_summaries.contains_key(&lead) && !self.sessions.contains_key(&lead) {
            return LeadState::default();
        }
        let view = self.sessions.get(&lead).map(|v| v.read(cx));
        let run = self.orchestrations.contains_key(&lead);
        let workers = self
            .orchestration_summaries
            .get(&lead)
            .map(|s| s.tasks.as_slice())
            .unwrap_or_default();
        let worker_needs_input = workers.iter().any(|task| {
            task.session.is_some_and(|s| {
                self.sessions
                    .get(&s)
                    .is_some_and(|v| v.read(cx).needs_approval())
            })
        });
        LeadState {
            exists: true,
            busy: self.session_metadata(lead, cx).busy,
            workers_live: run
                || self
                    .orchestration_summaries
                    .get(&lead)
                    .is_some_and(|s| s.live),
            plan_ready: !run && view.is_some_and(|v| v.orchestration_plan.is_some()),
            needs_input: view.is_some_and(|v| v.needs_approval()) || worker_needs_input,
        }
    }

    /// Pushes the started cards' coordinator states to an open board.
    pub(super) fn sync_board(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.board.clone() else {
            return;
        };
        let states = view
            .read(cx)
            .leads()
            .into_iter()
            .map(|lead| (lead, self.lead_state(lead, cx)))
            .collect();
        view.update(cx, |view, cx| view.set_states(states, cx));
    }

    /// Opens the card's coordinator: an orchestrator session in the card's
    /// project whose first message is the card brief.
    fn start_board_card(&mut self, card: Card, cx: &mut Context<Self>) {
        let cwd = card.project.clone();
        let key = format!(
            "{}-{}",
            monocode_core::board::prefix(&self.project_label(&cwd)),
            card.number
        );
        let choice = self.new_chat_choice(&cwd);
        let kind = choice.kind;
        let settings = monocode_core::agent::Settings {
            model: choice.model,
            effort: crate::conversation::preferred_effort(kind.id(), cx),
            access: self
                .workspace
                .preferences
                .default_access
                .as_deref()
                .map(monocode_core::agent::AccessMode::from_id)
                .unwrap_or_default(),
            account: crate::accounts::get(cx)
                .map(|accounts| accounts.read(cx).selected(kind, &cwd))
                .unwrap_or_default(),
            orchestrator: true,
            ..Default::default()
        };
        let prompt = monocode_core::board::coordinator_brief(&key, &card);
        let title = format!("{key} · {}", card.title);
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            if !std::path::Path::new(&cwd).is_dir() {
                return Err("The card's project folder no longer exists".into());
            }
            let store = locked(&store)?;
            let mut summary = store.create(kind.id(), &cwd)?;
            summary.title = title;
            store.title(summary.id, &summary.title)?;
            store.save_agent_settings(summary.id, &settings)?;
            Ok::<_, String>((summary, settings, prompt))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok((summary, settings, prompt)) => {
                    let id = summary.id;
                    this.add(summary, cx);
                    this.sessions[&id].update(cx, |view, cx| view.quick_submit(prompt, settings, cx));
                    if let Some(board) = &this.board {
                        board.update(cx, |board, cx| board.started(&card.id, id, cx));
                    }
                    this.sync_board(cx);
                    this.persist(cx);
                }
                Err(error) => {
                    this.error = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use monocode_core::board::Lane;

    #[gpui::test]
    fn the_board_lists_project_cards_and_follows_their_coordinators(cx: &mut TestAppContext) {
        let (window, _, lead) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, window, cx| {
                {
                    let store = locked(&shell.store).unwrap();
                    monocode_core::board::add(&store, "/work/monocode", "Todo", "").unwrap();
                    let started =
                        monocode_core::board::add(&store, "/work/monocode", "Started", "Spec")
                            .unwrap();
                    monocode_core::board::add(&store, "/elsewhere", "Other project", "").unwrap();
                    monocode_core::board::update(&store, &started.id, |c| c.lead = Some(lead))
                        .unwrap();
                }
                shell.open_board(window, cx);
                assert_eq!(shell.active_app_view, Some(AppView::Board));
                let board = shell.board.clone().unwrap();
                let read = board.read(cx);
                assert_eq!(read.key(), "MON");
                assert_eq!(read.leads(), vec![lead]);
                let state = shell.lead_state(lead, cx);
                assert!(state.exists && !state.busy && !state.workers_live);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                let store = locked(&shell.store).unwrap();
                let cards = monocode_core::board::load(&store);
                let lane_of = |title: &str| {
                    let card = cards.iter().find(|c| c.title == title).unwrap();
                    monocode_core::board::lane(
                        card,
                        card.lead.map(|l| shell.lead_state(l, cx)),
                    )
                };
                assert_eq!(lane_of("Todo"), Lane::Backlog);
                assert_eq!(lane_of("Started"), Lane::NeedsReview, "an idle coordinator is done");
            })
            .unwrap();
    }
}
