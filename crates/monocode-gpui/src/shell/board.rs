//! The project board in the shell: opening it, reporting each started card's
//! coordinator state, and starting a card as an orchestrator session.
use super::app_views::AppView;
use super::*;
use crate::board;
use monocode_core::board::{Card, CardMode, Digest, Lane, LeadState};
use monocode_core::provider::Kind;
use serde_json::{Value, json};
use std::collections::HashMap;

/// What the shell last saw of every board, so it saves digests only when they
/// change. Orchestrators are never woken: each turn the user sends carries the
/// board's changes since the last one, so an unattended board costs no model
/// turns.
#[derive(Default)]
pub(super) struct Watch {
    digests: HashMap<String, Digest>,
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
                    board::Event::Changed => this.board_changed(cx),
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
        // Load the orchestrator's chat: the board shows its latest reply and
        // docks its composer.
        if let Some(view) = self.ensure_board_orchestrator(&self.workspace.project.clone(), cx) {
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

    /// Any session or card change: keep every board's digest current and
    /// redraw the board.
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
        self.sync_board(cx);
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

    /// What a new orchestrator chat (a card's coordinator or the board's
    /// orchestrator) starts on: the Orchestrator agent where set, else the
    /// defaults for a new chat.
    fn orchestrator_start(
        &self,
        project: &str,
        cx: &App,
    ) -> (Kind, monocode_core::agent::Settings) {
        let agent = locked(&self.store)
            .map(|store| monocode_core::roles::load_orchestrator(&store))
            .unwrap_or_default();
        let choice = self.new_chat_choice(project);
        let kind = agent.provider.as_deref().and_then(Kind::parse).unwrap_or(choice.kind);
        let model = if agent.provider.is_some() { agent.model.clone() } else { choice.model };
        let settings = monocode_core::agent::Settings {
            model,
            effort: agent
                .effort
                .clone()
                .or_else(|| crate::conversation::preferred_effort(kind.id(), cx)),
            access: agent.access.unwrap_or_else(|| {
                self.workspace
                    .preferences
                    .default_access
                    .as_deref()
                    .map(monocode_core::agent::AccessMode::from_id)
                    .unwrap_or_default()
            }),
            account: crate::accounts::get(cx)
                .map(|accounts| accounts.read(cx).selected(kind, project))
                .unwrap_or_default(),
            ..Default::default()
        };
        (kind, settings)
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
            view.update(cx, |view, cx| view.set_board_orchestrator(project, cx));
            return Some(view);
        }
        let (kind, mut settings) = self.orchestrator_start(project, cx);
        settings.operator = true;
        settings.operator_enabled = true;
        settings.board_orchestrator = Some(project.to_owned());
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
        view.update(cx, |view, cx| {
            view.install_transfer(settings, None, cx);
            view.set_board_orchestrator(project, cx);
        });
        self.persist(cx);
        Some(view)
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
        // The orchestrator's own composer, once its chat exists; a new one is
        // made when the project changes.
        let project = self.workspace.project.clone();
        let chat = orchestrator.as_ref().and_then(|o| self.sessions.get(&o.session)).cloned();
        let dock = match (chat, &self.board_dock) {
            (Some(chat), Some((p, dock))) if *p == project && dock.read(cx).conversation() == &chat => {
                Some(dock.clone())
            }
            (Some(chat), _) => {
                let dock = cx.new(|cx| crate::conversation::ComposerDock::new(chat, cx));
                self.board_dock = Some((project, dock.clone()));
                Some(dock)
            }
            (None, _) => {
                self.board_dock = None;
                None
            }
        };
        view.update(cx, |view, cx| {
            view.set_states(states, infos, cx);
            view.set_orchestrator(orchestrator, cx);
            view.set_dock(dock.map(Into::into), cx);
        });
    }

    /// A solo card's one-assignment run: the card on the implementer role
    /// when it exists, else on the lead's own settings.
    fn solo_plan(&self, key: &str, card: &Card) -> monocode_core::orchestration::Plan {
        let roles = locked(&self.store)
            .map(|store| monocode_core::roles::load(&store))
            .unwrap_or_default();
        let role = monocode_core::roles::find(&roles, Some("implement")).map(|r| r.id.clone());
        monocode_core::orchestration::Plan {
            title: card.title.clone(),
            summary: String::new(),
            tasks: vec![monocode_core::board::solo_task(key, card, role)],
        }
    }

    /// Opens the card's coordinator: an orchestrator session in the card's
    /// project whose first message is the card brief.
    pub(super) fn start_board_card(&mut self, card: Card, cx: &mut Context<Self>) {
        let cwd = card.project.clone();
        let key = monocode_core::board::card_key(
            &monocode_core::board::prefix(&self.project_label(&cwd)),
            card.number,
        );
        let (kind, mut settings) = self.orchestrator_start(&cwd, cx);
        let solo = card.mode == monocode_core::board::CardMode::Solo;
        settings.orchestrator = !solo;
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
                    if solo {
                        // No planning turn: the card goes straight to one
                        // implementer through the review and merge pipeline,
                        // and this session shows the run.
                        this.sessions[&id].update(cx, |view, cx| view.install_transfer(settings, None, cx));
                        let plan = this.solo_plan(&key, &card);
                        this.start_orchestration(id, plan, cx);
                    } else {
                        this.sessions[&id].update(cx, |view, cx| {
                            view.install_transfer(settings, None, cx);
                            view.send_with_context(visible, prompt, Some("orchestrator".into()), cx);
                        });
                    }
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
                "mode": card.mode.id(),
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
                // The user's card mode wins; on Auto the orchestrator's pick applies.
                let preference = locked(&self.store)
                    .map(|store| monocode_core::board::load_cards_preference(&store, project))
                    .unwrap_or_default();
                let mode = match preference {
                    monocode_core::board::CardsPreference::Solo => CardMode::Solo,
                    monocode_core::board::CardsPreference::Orchestrate => CardMode::Orchestrate,
                    monocode_core::board::CardsPreference::Auto => input["mode"]
                        .as_str()
                        .and_then(CardMode::parse)
                        .unwrap_or_default(),
                };
                let card = locked(&self.store).and_then(|store| {
                    monocode_core::board::add(&store, project, title, details, mode)
                })?;
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
                    monocode_core::board::add(&store, "/work/monocode", "Todo", "", CardMode::default()).unwrap();
                    let started =
                        monocode_core::board::add(&store, "/work/monocode", "Started", "Spec", CardMode::default())
                            .unwrap();
                    monocode_core::board::add(&store, "/elsewhere", "Other project", "", CardMode::default()).unwrap();
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
    fn board_changes_keep_digests_without_waking_the_orchestrator(cx: &mut TestAppContext) {
        let (window, _, lead) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, _, cx| {
                let project = "/work/monocode";
                {
                    let store = locked(&shell.store).unwrap();
                    let card = monocode_core::board::add(&store, project, "Ship it", "", CardMode::default()).unwrap();
                    monocode_core::board::update(&store, &card.id, |c| c.lead = Some(lead)).unwrap();
                    monocode_core::board::set_orchestrator_session(&store, project, lead).unwrap();
                }
                shell.board_changed(cx);
                let digest = &shell.board_watch.digests[project];
                assert_eq!(digest.key, "MON");
                assert!(digest.text.contains("- MON-001 Ship it (Finished)"));
                assert!(
                    !shell.sessions[&lead].read(cx).busy,
                    "a finished card does not wake the orchestrator"
                );
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
    fn project_shortcuts_pass_through_the_empty_docked_composer(cx: &mut TestAppContext) {
        let (window, _, _) = crate::shell::tests::window(cx);
        cx.update(|cx| {
            crate::input::bind(cx);
            crate::shell::bind_keys(cx);
        });
        window
            .update(cx, |shell, w, cx| {
                shell.open_board(w, cx);
                // Typing goes to the board orchestrator's docked chat composer.
                let (_, dock) = shell.board_dock.clone().expect("the board docks its composer");
                let chat = dock.read(cx).conversation().clone();
                assert!(chat.read(cx).settings.board_orchestrator.is_some());
                let focus = chat.read(cx).input.focus_handle(cx);
                w.focus(&focus, cx);
            })
            .unwrap();
        let before = window.update(cx, |shell, _, _| shell.workspace.project.clone()).unwrap();
        // On Linux and Windows this is also the field's select-word key.
        let modifier = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
        cx.simulate_keystrokes(window.into(), &format!("{modifier}-shift-right"));
        cx.run_until_parked();
        let after = window.update(cx, |shell, _, _| shell.workspace.project.clone()).unwrap();
        assert_ne!(before, after, "the shortcut switched projects from the board");
    }

    #[gpui::test]
    fn the_start_menu_picks_a_mode_and_starts_the_card(cx: &mut TestAppContext) {
        let (window, _, _) = crate::shell::tests::window(cx);
        let card_id = window
            .update(cx, |shell, w, cx| {
                let project = shell.workspace.project.clone();
                let card = locked(&shell.store)
                    .and_then(|s| monocode_core::board::add(&s, &project, "Fix typo", "", CardMode::Orchestrate))
                    .unwrap();
                shell.open_board(w, cx);
                let board = shell.board.clone().unwrap();
                board.update(cx, |b, cx| b.open_start_menu_for_test(&card.id, w, cx));
                let menu = board.read(cx).start_menu_for_test().expect("the arrow opens a menu");
                menu.update(cx, |_, cx| {
                    cx.emit(crate::popover::MenuEvent::Picked("solo".into(), None))
                });
                card.id.clone()
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                let board = shell.board.clone().unwrap();
                assert!(board.read(cx).start_menu_for_test().is_none(), "picking closes it");
                let saved = monocode_core::board::load(&locked(&shell.store).unwrap())
                    .into_iter()
                    .find(|c| c.id == card_id)
                    .unwrap();
                assert_eq!(saved.mode, CardMode::Solo, "the pick is the card's mode");
                // The fixture's project folder does not exist, so the start
                // reports that instead of launching an agent.
                assert!(shell.error.as_deref().is_some_and(|e| e.contains("project folder")));
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_board_composer_sets_card_modes_with_solo_and_orchestrator_tags(cx: &mut TestAppContext) {
        use monocode_core::board::{CardsPreference, load_cards_preference};
        let (window, _, _) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, w, cx| {
                shell.open_board(w, cx);
                let project = shell.workspace.project.clone();
                let chat = shell.board_dock.clone().unwrap().1.read(cx).conversation().clone();
                let saved = |shell: &Shell| {
                    load_cards_preference(&locked(&shell.store).unwrap(), &project)
                };
                assert_eq!(saved(shell), CardsPreference::Auto);
                chat.update(cx, |c, cx| c.toggle_mode("solo", cx));
                assert_eq!(saved(shell), CardsPreference::Solo);
                assert!(chat.read(cx).input.read(cx).content.starts_with("/solo"));
                chat.update(cx, |c, cx| c.toggle_mode("orchestrator", cx));
                assert_eq!(saved(shell), CardsPreference::Orchestrate);
                assert!(
                    !chat.read(cx).settings.orchestrator,
                    "on the board, /orchestrator is a card mode, not the chat's planning mode"
                );
                assert!(chat.read(cx).input.read(cx).content.starts_with("/orchestrator"));
                chat.update(cx, |c, cx| c.toggle_mode("orchestrator", cx));
                assert_eq!(saved(shell), CardsPreference::Auto, "picking it again returns to Auto");
                chat.update(cx, |c, cx| c.toggle_mode("solo", cx));
                chat.update(cx, |c, cx| c.clear_mode("solo", cx));
                assert_eq!(saved(shell), CardsPreference::Auto, "removing the tag returns to Auto");
                // Chat-only modes stay off the board's slash menu.
                assert!(chat.read(cx).completion_is_board(cx));
            })
            .unwrap();
    }

    #[gpui::test]
    fn card_modes_follow_the_composer_choice_and_solo_cards_run_the_implementer(cx: &mut TestAppContext) {
        use monocode_core::board::{CardsPreference, save_cards_preference};
        let dir = std::env::temp_dir().join(format!(
            "placeholder-solo-card-{}-{}",
            std::process::id(),
            monocode_core::time::now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let project = dir.to_string_lossy().into_owned();
        let (window, _, _) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, _, cx| {
                let mode_of = |shell: &mut Shell, input: Value, cx: &mut Context<Shell>| {
                    let added = shell.board_action("board.add", &input, &project, cx).unwrap();
                    let number = added["added"]["number"].as_u64().unwrap() as u32;
                    let store = locked(&shell.store).unwrap();
                    monocode_core::board::load(&store)
                        .into_iter()
                        .find(|c| c.project == project && c.number == number)
                        .unwrap()
                        .mode
                };
                // Auto: the orchestrator's pick, defaulting to Orchestrate.
                assert_eq!(mode_of(shell, json!({"title": "Typo", "mode": "solo"}), cx), CardMode::Solo);
                assert_eq!(mode_of(shell, json!({"title": "Big"}), cx), CardMode::Orchestrate);
                // A fixed choice wins over the orchestrator's pick.
                locked(&shell.store)
                    .and_then(|s| save_cards_preference(&s, &project, CardsPreference::Orchestrate))
                    .unwrap();
                assert_eq!(
                    mode_of(shell, json!({"title": "Also big", "mode": "solo"}), cx),
                    CardMode::Orchestrate
                );
                let listed = shell.board_action("board.list", &json!({}), &project, cx).unwrap();
                assert!(listed["cards"].as_array().unwrap().iter().any(|c| c["mode"] == "solo"));
                assert!(shell.board_snapshot(&project, cx).contains("Typo (Not started) [solo]"));
                // Starting it is not exercised here: that would launch a real
                // agent. The run itself is covered by the orchestration tests.
                let card = {
                    let store = locked(&shell.store).unwrap();
                    monocode_core::board::load(&store)
                        .into_iter()
                        .find(|c| c.project == project && c.number == 1)
                        .unwrap()
                };
                let plan = shell.solo_plan("MON-001", &card);
                assert_eq!(plan.tasks.len(), 1);
                assert_eq!(plan.tasks[0].role.as_deref(), Some("implement"));
                assert!(monocode_core::orchestration::validate(&plan).is_ok());
            })
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
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
