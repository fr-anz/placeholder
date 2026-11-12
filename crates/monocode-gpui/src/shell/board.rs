//! The project board in the shell: opening it, reporting each started card's
//! coordinator state, and starting a card as an orchestrator session.
use super::app_views::AppView;
use super::*;
use crate::board;
use monocode_core::board::{Card, Lane, LeadState};
use serde_json::{Value, json};

impl Shell {
    pub(crate) fn open_board(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                    board::Event::Ask(message) => {
                        this.ask_board_orchestrator(message.clone(), window, cx)
                    }
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

    /// The coordinator's provider and model, and each worker's status.
    fn lead_info(&self, lead: i64, cx: &App) -> board::LeadInfo {
        let metadata = self.session_metadata(lead, cx);
        board::LeadInfo {
            provider: metadata.summary.provider.clone(),
            model: self
                .sessions
                .get(&lead)
                .and_then(|v| v.read(cx).settings.model.clone())
                .or_else(|| self.session_details.get(&lead).and_then(|d| d.model.clone())),
            workers: self
                .orchestration_summaries
                .get(&lead)
                .map(|s| s.tasks.iter().map(|t| t.status.clone()).collect())
                .unwrap_or_default(),
        }
    }

    /// Pushes the started cards' coordinator states to an open board.
    pub(super) fn sync_board(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.board.clone() else {
            return;
        };
        let leads = view.read(cx).leads();
        let states = leads
            .iter()
            .map(|&lead| (lead, self.lead_state(lead, cx)))
            .collect();
        let infos = leads
            .iter()
            .filter(|&&lead| self.session_summaries.contains_key(&lead) || self.sessions.contains_key(&lead))
            .map(|&lead| (lead, self.lead_info(lead, cx)))
            .collect();
        view.update(cx, |view, cx| view.set_states(states, infos, cx));
    }

    /// Opens the card's coordinator: an orchestrator session in the card's
    /// project whose first message is the card brief.
    pub(super) fn start_board_card(&mut self, card: Card, cx: &mut Context<Self>) {
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
                    // Saved here, not by the view: agents start cards with the board closed.
                    let saved = locked(&this.store).and_then(|store| {
                        monocode_core::board::update(&store, &card.id, |c| c.lead = Some(id))
                    });
                    if let Err(error) = saved {
                        this.error = Some(error);
                    }
                    if let Some(board) = &this.board {
                        board.update(cx, |board, cx| board.refresh(cx));
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

impl Shell {
    fn board_key(&self, project: &str) -> String {
        monocode_core::board::prefix(&self.project_label(project))
    }

    fn board_snapshot(&self, project: &str, cx: &App) -> String {
        let cards: Vec<_> = locked(&self.store)
            .map(|store| monocode_core::board::load(&store))
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.project == project)
            .collect();
        monocode_core::board::snapshot(&cards, &self.board_key(project), |card| {
            card.lead.map(|lead| self.lead_state(lead, cx))
        })
    }

    /// `board.list`, `board.add` and `board.start` for app-access sessions in `project`.
    pub(super) fn board_action(
        &mut self,
        action: &str,
        input: &Value,
        project: &str,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let key = self.board_key(project);
        let card_json = |this: &Self, card: &Card, cx: &App| {
            let state = card.lead.map(|lead| this.lead_state(lead, cx));
            let lane = match monocode_core::board::lane(card, state) {
                Lane::Backlog => "backlog",
                Lane::InProgress => "in_progress",
                Lane::NeedsReview => "needs_review",
            };
            json!({
                "card": format!("{key}-{}", card.number),
                "number": card.number,
                "title": card.title,
                "details": card.details.chars().take(500).collect::<String>(),
                "lane": lane,
                "status": monocode_core::board::status_label(card, state),
                "sessionId": card.lead,
            })
        };
        let result = match action {
            "board.list" => {
                let cards: Vec<_> = locked(&self.store)
                    .map(|store| monocode_core::board::load(&store))?
                    .into_iter()
                    .filter(|c| c.project == project && !c.archived)
                    .collect();
                Ok(json!({"cards": cards.iter().map(|c| card_json(self, c, cx)).collect::<Vec<_>>()}))
            }
            "board.add" => {
                let title = input["title"].as_str().ok_or("board.add needs a title")?;
                let details = input["details"].as_str().unwrap_or_default();
                let card = locked(&self.store)
                    .and_then(|store| monocode_core::board::add(&store, project, title, details))?;
                let value = card_json(self, &card, cx);
                if input["start"] == true {
                    self.start_board_card(card, cx);
                }
                Ok(json!({"added": value, "started": input["start"] == true}))
            }
            "board.start" => {
                let raw = match &input["card"] {
                    Value::Number(n) => n.to_string(),
                    Value::String(s) => s.clone(),
                    _ => return Err("board.start needs card, such as 3 or PRJ-3".into()),
                };
                let number = monocode_core::board::parse_card_number(&raw)
                    .ok_or("board.start needs card, such as 3 or PRJ-3")?;
                let card = locked(&self.store)
                    .map(|store| monocode_core::board::load(&store))?
                    .into_iter()
                    .find(|c| c.project == project && c.number == number && !c.archived)
                    .ok_or_else(|| format!("No card {key}-{number} on this board"))?;
                if card.lead.is_some() {
                    return Err(format!("{key}-{number} has already started"));
                }
                let value = card_json(self, &card, cx);
                self.start_board_card(card, cx);
                Ok(json!({"started": value}))
            }
            _ => Err("Unknown board action".into()),
        };
        if let Some(board) = &self.board {
            board.update(cx, |board, cx| board.refresh(cx));
        }
        result
    }

    /// Each question from the board opens a fresh orchestrator session with app
    /// access and a snapshot of the board.
    fn ask_board_orchestrator(
        &mut self,
        message: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cwd = self.workspace.project.clone();
        if cwd.is_empty() {
            return;
        }
        let label = self.project_label(&cwd);
        let key = self.board_key(&cwd);
        let prompt = monocode_core::board::orchestrator_brief(
            &label,
            &key,
            &self.board_snapshot(&cwd, cx),
            &message,
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
            operator: true,
            operator_enabled: true,
            ..Default::default()
        };
        let title = format!("{key} board · {}", message.lines().next().unwrap_or_default());
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            let store = locked(&store)?;
            let mut summary = store.create(kind.id(), &cwd)?;
            summary.title = title.chars().take(120).collect();
            store.title(summary.id, &summary.title)?;
            store.save_agent_settings(summary.id, &settings)?;
            Ok::<_, String>((summary, settings, prompt))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok((summary, settings, prompt)) => {
                    let id = summary.id;
                    this.add(summary, cx);
                    this.sessions[&id].update(cx, |view, cx| view.quick_submit(prompt, settings, cx));
                    this.activate_session(id, window, cx);
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

    #[gpui::test]
    fn board_actions_add_list_and_guard_starts(cx: &mut TestAppContext) {
        let (window, _, _) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, _, cx| {
                let project = "/work/monocode";
                let added = shell
                    .board_action(
                        "board.add",
                        &json!({"title": "Split auth", "details": "Goal: ..."}),
                        project,
                        cx,
                    )
                    .unwrap();
                assert_eq!(added["added"]["card"], "MON-1");
                assert_eq!(added["started"], false);
                assert!(shell.board_action("board.add", &json!({}), project, cx).is_err());
                let listed = shell.board_action("board.list", &json!({}), project, cx).unwrap();
                assert_eq!(listed["cards"][0]["lane"], "backlog");
                assert_eq!(listed["cards"][0]["status"], "Not started");
                let other = shell.board_action("board.list", &json!({}), "/elsewhere", cx).unwrap();
                assert_eq!(other["cards"].as_array().unwrap().len(), 0);
                let missing = shell
                    .board_action("board.start", &json!({"card": "MON-9"}), project, cx)
                    .unwrap_err();
                assert!(missing.contains("No card MON-9"));
                assert!(shell.board_snapshot(project, cx).contains("- MON-1 Split auth (Not started)"));
            })
            .unwrap();
    }
}
