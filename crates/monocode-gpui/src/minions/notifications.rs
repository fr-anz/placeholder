use super::*;

/// Forward request-level changes even when the attention boolean is unchanged.
pub(crate) struct NotificationChanged;
impl EventEmitter<NotificationChanged> for Minions {}

/// Capture before dispatch starts another queued turn or a finished run is
/// removed. Consumers need no live entity borrow to deliver this completion.
pub(crate) struct NotificationFinished {
    pub summary: Session,
    pub body: String,
    pub habit: bool,
    pub was_cancelled: bool,
}
impl EventEmitter<NotificationFinished> for Minions {}

/// A Minion wrote to its chat on its own, as a habit's or heartbeat's report.
/// Its main chat session and what it said, for paired phones.
pub(crate) struct NotificationPosted {
    pub session: i64,
    pub body: String,
}
impl EventEmitter<NotificationPosted> for Minions {}

impl Minions {
    pub(crate) fn dismiss_error(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.failed_reports.clear();
        cx.notify();
    }

    pub(crate) fn all_notification_chats(&self, cx: &App) -> Vec<Entity<Conversation>> {
        let mut seen = HashSet::new();
        self.chats
            .values()
            .chain(self.running.values().map(|run| &run.view))
            .filter(|view| seen.insert(view.read(cx).summary.id))
            .cloned()
            .collect()
    }
    /// The Minion a session belongs to: its own chat or one of its habit runs.
    pub(crate) fn notification_minion(&self, id: i64) -> Option<&Minion> {
        self.roster
            .minions
            .iter()
            .find(|m| m.session_id == Some(id) || m.habit_sessions.contains(&id))
    }
    pub(crate) fn active_notification_session(&self, cx: &App) -> Option<i64> {
        if self.settings.is_some() {
            return None;
        }
        self.chat().map(|chat| chat.read(cx).summary.id)
    }
    pub(crate) fn notification_entity(&self, id: i64, cx: &App) -> Option<Entity<Conversation>> {
        self.chats
            .values()
            .chain(self.running.values().map(|run| &run.view))
            .find(|view| view.read(cx).summary.id == id)
            .cloned()
    }
    /// Main Minion chats stay in their workspace. A habit run has no dedicated
    /// Minion-chat surface: return false so the host opens that exact session.
    pub(crate) fn open_notification_session(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // `Preferences.monos` keeps its native name: core's desktop-preferences
        // reconcile table maps this exact field to the upstream key.
        if !self.prefs.monos {
            return false;
        }
        let Some(minion) = self.roster.of_session(id).map(|minion| minion.id.clone()) else {
            return false;
        };
        self.settings = None;
        self.route = None;
        self.open(&minion, window, cx);
        true
    }
}
