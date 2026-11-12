//! The project board in the shell: opening it, reporting each started card's
//! coordinator state, and starting a card as an orchestrator session.
use super::app_views::AppView;
use super::*;
use crate::board;
use monocode_core::board::{Card, Digest, Lane, LeadState};
use monocode_core::provider::Kind;
use serde_json::{Value, json};
use std::collections::HashMap;

/// What the shell last saw of every board, so it saves digests only when they
/// change and tells orchestrators only about real transitions.
#[derive(Default)]
pub(super) struct Watch {
    digests: HashMap<String, Digest>,
    statuses: HashMap<String, &'static str>,
    pending: HashMap<String, Vec<String>>,
}

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
                    board::Event::Ask(message) => this.message_board_orchestrator(message.clone(), cx),
                    board::Event::Changed => this.board_changed(cx),
                    board::Event::OpenModelMenu => this.open_board_model_menu(window, cx),
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
        // Load the orchestrator's chat so the board can show its latest reply.
        if let Some(id) = self.board_orchestrator(&self.workspace.project.clone())
            && let Some(view) = self.ensure_session(id, cx)
        {
            view.update(cx, |view, cx| view.load(cx));
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

    /// Any session or card change: keep every board's digest current, wake
    /// orchestrators about tasks that now need attention, and redraw the board.
    pub(super) fn board_changed(&mut self, cx: &mut Context<Self>) {
        let cards = match locked(&self.store) {
            Ok(store) => monocode_core::board::load(&store),
            Err(_) => return,
        };
        let mut projects: HashMap<String, Vec<Card>> = HashMap::new();
        for card in cards.into_iter().filter(|c| !c.archived) {
            projects.entry(card.project.clone()).or_default().push(card);
        }
        let mut saves = Vec::new();
        for (project, cards) in &projects {
            let label = self.project_label(project);
            let key = monocode_core::board::prefix(&label);
            let text = monocode_core::board::snapshot(cards, &key, |card| {
                card.lead.map(|lead| self.lead_state(lead, cx))
            });
            for card in cards.iter().filter(|c| c.lead.is_some()) {
                let state = card.lead.map(|lead| self.lead_state(lead, cx));
                let status = monocode_core::board::status_label(card, state);
                let previous = self.board_watch.statuses.insert(card.id.clone(), status);
                // A first sighting (such as at launch) is not news.
                if previous.is_some_and(|p| p != status) && monocode_core::board::notable(status) {
                    self.board_watch.pending.entry(project.clone()).or_default().push(format!(
                        "- {} {}: {}",
                        monocode_core::board::card_key(&key, card.number),
                        card.title,
                        status.to_lowercase()
                    ));
                }
            }
            let digest = Digest { label, key, text };
            if self.board_watch.digests.get(project) != Some(&digest) {
                self.board_watch.digests.insert(project.clone(), digest.clone());
                saves.push((project.clone(), digest));
            }
        }
        if !saves.is_empty() {
            let store = self.store.clone();
            cx.background_executor()
                .spawn(async move {
                    if let Ok(store) = locked(&store) {
                        for (project, digest) in saves {
                            let _ = monocode_core::board::save_digest(&store, &project, &digest);
                        }
                    }
                })
                .detach();
        }
        self.flush_board_events(cx);
        self.sync_board(cx);
    }

    /// Delivers waiting board events to each project's orchestrator once it is idle.
    fn flush_board_events(&mut self, cx: &mut Context<Self>) {
        let projects: Vec<String> = self.board_watch.pending.keys().cloned().collect();
        for project in projects {
            let orchestrator = locked(&self.store)
                .ok()
                .and_then(|store| monocode_core::board::orchestrator_session(&store, &project))
                .filter(|id| self.session_summaries.contains_key(id) || self.sessions.contains_key(id));
            let Some(id) = orchestrator else {
                // No orchestrator yet: the board itself shows the change.
                self.board_watch.pending.remove(&project);
                continue;
            };
            if self.session_metadata(id, cx).busy {
                continue;
            }
            let Some(view) = self.ensure_session(id, cx) else {
                continue;
            };
            let changes = self.board_watch.pending.remove(&project).unwrap_or_default();
            let note = monocode_core::board::event_note(&changes);
            view.update(cx, |view, cx| {
                view.settings.board_orchestrator = Some(project.clone());
                view.send_coordinator_report(note, cx);
            });
        }
    }

    /// The project's orchestrator session, if it still exists.
    fn board_orchestrator(&self, project: &str) -> Option<i64> {
        locked(&self.store)
            .ok()
            .and_then(|store| monocode_core::board::orchestrator_session(&store, project))
            .filter(|id| {
                self.session_summaries.get(id).is_some_and(|s| !s.archived)
                    || self.sessions.contains_key(id)
            })
    }

    /// The project's orchestrator chat, created (empty) the first time it is
    /// needed: by a board message or by its model menu.
    fn ensure_board_orchestrator(
        &mut self,
        project: &str,
        cx: &mut Context<Self>,
    ) -> Option<Entity<crate::conversation::Conversation>> {
        if project.is_empty() {
            return None;
        }
        if let Some(id) = self.board_orchestrator(project) {
            let view = self.ensure_session(id, cx)?;
            view.update(cx, |view, _| {
                view.settings.board_orchestrator = Some(project.to_owned())
            });
            return Some(view);
        }
        let choice = self.new_chat_choice(project);
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
                .map(|accounts| accounts.read(cx).selected(kind, project))
                .unwrap_or_default(),
            operator: true,
            operator_enabled: true,
            board_orchestrator: Some(project.to_owned()),
            ..Default::default()
        };
        let title = format!("Orchestrator · {}", self.project_label(project));
        let created = locked(&self.store).and_then(|store| {
            let mut summary = store.create(kind.id(), project)?;
            summary.title = title;
            store.title(summary.id, &summary.title)?;
            store.save_agent_settings(summary.id, &settings)?;
            monocode_core::board::set_orchestrator_session(&store, project, summary.id)?;
            Ok(summary)
        });
        let summary = match created {
            Ok(summary) => summary,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return None;
            }
        };
        let id = summary.id;
        self.add(summary, cx);
        let view = self.sessions.get(&id)?.clone();
        view.update(cx, |view, cx| view.install_transfer(settings, None, cx));
        self.persist(cx);
        Some(view)
    }

    /// The composer's own "Model and settings" menu, over the board's chip.
    fn open_board_model_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let project = self.workspace.project.clone();
        let Some(view) = self.ensure_board_orchestrator(&project, cx) else {
            return;
        };
        let (menu, events) = view.update(cx, |view, cx| view.model_settings_menu(window, cx));
        if let Some(board) = &self.board {
            board.update(cx, |board, cx| board.show_menu(menu, events, cx));
        }
        self.sync_board(cx);
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
        let orchestrator = locked(&self.store)
            .ok()
            .and_then(|store| monocode_core::board::orchestrator_session(&store, &self.workspace.project))
            .filter(|id| self.session_summaries.contains_key(id) || self.sessions.contains_key(id))
            .map(|id| board::OrchestratorStatus {
                session: id,
                busy: self.session_metadata(id, cx).busy,
                reply: self
                    .sessions
                    .get(&id)
                    .map(|v| v.read(cx).latest_reply(cx))
                    .unwrap_or_default(),
            });
        // The chip reads the orchestrator's composer, or the new-chat default.
        let (kind, model, effort) = match orchestrator
            .as_ref()
            .and_then(|o| self.sessions.get(&o.session))
        {
            Some(chat) => {
                let chat = chat.read(cx);
                let (model, effort) = chat.model_chip();
                (Kind::from_id(&chat.summary.provider), model, effort)
            }
            None => {
                let choice = self.new_chat_choice(&self.workspace.project);
                (
                    choice.kind,
                    crate::pickers::cached_model_name(choice.kind, choice.model.as_deref()),
                    None,
                )
            }
        };
        view.update(cx, |view, cx| {
            view.set_states(states, infos, cx);
            view.set_orchestrator(orchestrator, cx);
            view.set_chip(kind, model, effort, cx);
        });
    }

    /// Opens the card's coordinator: an orchestrator session in the card's
    /// project whose first message is the card brief.
    pub(super) fn start_board_card(&mut self, card: Card, cx: &mut Context<Self>) {
        let cwd = card.project.clone();
        let key = monocode_core::board::card_key(
            &monocode_core::board::prefix(&self.project_label(&cwd)),
            card.number,
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
        let visible = monocode_core::board::coordinator_message(&key, &card);
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
                    this.sessions[&id].update(cx, |view, cx| {
                        view.install_transfer(settings, None, cx);
                        view.send_with_context(visible, prompt, Some("orchestrator".into()), cx);
                    });
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
                    this.board_changed(cx);
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
                "card": monocode_core::board::card_key(&key, card.number),
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
                    .ok_or_else(|| format!("No task {} on this board", monocode_core::board::card_key(&key, number)))?;
                if card.lead.is_some() {
                    return Err(format!("{} has already started", monocode_core::board::card_key(&key, number)));
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
        self.board_changed(cx);
        result
    }

    /// Sends a board message to the project's long-lived orchestrator, creating
    /// it on the first message. The runtime adds its role and the board.
    fn message_board_orchestrator(&mut self, message: String, cx: &mut Context<Self>) {
        let cwd = self.workspace.project.clone();
        if cwd.is_empty() {
            return;
        }
        // Make sure the digest it reads is current.
        self.board_changed(cx);
        if let Some(view) = self.ensure_board_orchestrator(&cwd, cx) {
            view.update(cx, |view, cx| view.send_text(message, cx));
        }
        self.sync_board(cx);
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
    fn board_changes_keep_digests_and_queue_only_real_transitions(cx: &mut TestAppContext) {
        let (window, _, lead) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, _, cx| {
                let project = "/work/monocode";
                let card = {
                    let store = locked(&shell.store).unwrap();
                    let card = monocode_core::board::add(&store, project, "Ship it", "").unwrap();
                    monocode_core::board::update(&store, &card.id, |c| c.lead = Some(lead)).unwrap()
                };
                shell.board_changed(cx);
                // First sighting: recorded, not news.
                assert_eq!(shell.board_watch.statuses.get(&card.id), Some(&"Finished"));
                assert!(shell.board_watch.pending.is_empty());
                let digest = &shell.board_watch.digests[project];
                assert_eq!(digest.key, "MON");
                assert!(digest.text.contains("- MON-001 Ship it (Finished)"));
                // A real transition with an orchestrator queues an event until it is idle.
                shell.board_watch.statuses.insert(card.id.clone(), "Planning");
                locked(&shell.store)
                    .and_then(|s| monocode_core::board::set_orchestrator_session(&s, project, lead))
                    .unwrap();
                shell.board_watch.pending.clear();
                let before = shell.board_watch.statuses.clone();
                assert_eq!(before.get(&card.id), Some(&"Planning"));
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                let store = locked(&shell.store).unwrap();
                let saved = monocode_core::board::load_digest(&store, "/work/monocode");
                assert!(saved.text.contains("MON-001 Ship it"), "digest saved for the runtime");
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
                assert_eq!(added["added"]["card"], "MON-001");
                assert_eq!(added["started"], false);
                assert!(shell.board_action("board.add", &json!({}), project, cx).is_err());
                let listed = shell.board_action("board.list", &json!({}), project, cx).unwrap();
                assert_eq!(listed["cards"][0]["lane"], "backlog");
                assert_eq!(listed["cards"][0]["status"], "Not started");
                let other = shell.board_action("board.list", &json!({}), "/elsewhere", cx).unwrap();
                assert_eq!(other["cards"].as_array().unwrap().len(), 0);
                let missing = shell
                    .board_action("board.start", &json!({"card": "MON-009"}), project, cx)
                    .unwrap_err();
                assert!(missing.contains("No task MON-009"));
                assert!(shell.board_snapshot(project, cx).contains("- MON-001 Split auth (Not started)"));
            })
            .unwrap();
    }
}
