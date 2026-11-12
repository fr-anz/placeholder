//! An indexed Minion is read like the desktop app pages it: the newest turns
//! first, older turns only on request. Row indices count from a sticky window
//! start, so saves, turn receipts and edits never read or rewrite older blocks.
use super::*;

/// Turns in the first window and in each older page, as desktop pages them.
pub const PAGE_TURNS: usize = 10;

/// Rows strictly before a window or an earlier page, oldest first.
#[derive(Debug, Default, Clone)]
pub struct HistoryPage {
    pub rows: Vec<Row>,
    /// Turn receipts keyed by row offset within `rows`.
    pub turns: HashMap<usize, crate::agent::TurnInfo>,
    /// The cursor for the page before this one, if any older blocks remain.
    pub before: Option<i64>,
}

impl Store {
    pub(super) fn shared_indexed(&self, original: &str) -> Result<bool, String> {
        Ok(has_table(&self.conn, crate::store::LEGACY_TRANSCRIPTS_TABLE)?
            && sql(self.conn.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE session_id=?1)",
                    table = crate::store::LEGACY_TRANSCRIPTS_TABLE),
                [original],
                |row| row.get::<_, bool>(0),
            ))?)
    }
    /// The first block sequence of an indexed Minion's window, chosen once per
    /// store so that every row index handed out for it shares one origin.
    /// None for a history stored as one blob, which is always read whole.
    pub(super) fn shared_window(&self, id: i64, original: &str) -> Result<Option<i64>, String> {
        if !self.shared_indexed(original)? {
            return Ok(None);
        }
        let windows = &self.shared.as_ref().unwrap().windows;
        if let Some(from) = windows.borrow().get(&id) {
            return Ok(Some(*from));
        }
        let from = if has_table(&self.conn, crate::store::LEGACY_TURNS_TABLE)? {
            sql(self
                .conn
                .query_row(
                    &format!("SELECT seq FROM {table} WHERE session_id=?1 ORDER BY seq DESC LIMIT 1 OFFSET ?2",
                        table = crate::store::LEGACY_TURNS_TABLE),
                    params![original, PAGE_TURNS as i64 - 1],
                    |row| row.get::<_, i64>(0),
                )
                .optional())?
            .unwrap_or(0)
        } else {
            0
        };
        windows.borrow_mut().insert(id, from);
        Ok(Some(from))
    }
    pub(super) fn shared_indexed_blocks(
        &self,
        original: &str,
        from: i64,
        before: i64,
    ) -> Result<Vec<Value>, String> {
        let mut query = sql(self.conn.prepare(&format!(
            "SELECT block_json FROM {blocks} WHERE session_id=?1 AND seq>=?2 AND seq<?3 ORDER BY seq",
            blocks = crate::store::LEGACY_BLOCKS_TABLE,
        )))?;
        let mut records = sql(query.query(params![original, from, before]))?;
        let mut result = Vec::new();
        while let Some(record) = sql(records.next())? {
            result.push(parse(&sql(record.get::<_, String>(0))?)?);
        }
        Ok(result)
    }
    /// Every block, numbered from the first. Only whole-history readers such as
    /// search use this; row indices it implies are not the window's.
    pub(super) fn shared_all_blocks(&self, id: i64) -> Result<Vec<Value>, String> {
        let original = self.persisted_session_id(id)?;
        if self.shared_indexed(&original)? {
            self.shared_indexed_blocks(&original, 0, i64::MAX)
        } else {
            self.shared_blocks(id)
        }
    }
    /// The cursor for the page before the window, if anything precedes it.
    pub(crate) fn shared_history_before(&self, id: i64) -> Result<Option<i64>, String> {
        let original = self.persisted_session_id(id)?;
        let Some(from) = self.shared_window(id, &original)? else {
            return Ok(None);
        };
        let older = sql(self.conn.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {blocks} WHERE session_id=?1 AND seq<?2)",
                blocks = crate::store::LEGACY_BLOCKS_TABLE),
            params![original, from],
            |row| row.get::<_, bool>(0),
        ))?;
        Ok(older.then_some(from))
    }
    /// The turns before `before`: one page, or with `all` everything older.
    /// Pages are read-only. Their rows precede the window and have no index in
    /// it, so nothing here records a revision or moves the window.
    pub(crate) fn shared_load_before(
        &self,
        id: i64,
        before: i64,
        all: bool,
    ) -> Result<HistoryPage, String> {
        let original = self.persisted_session_id(id)?;
        if !self.shared_indexed(&original)? {
            return Ok(HistoryPage::default());
        }
        let mut start = 0;
        let mut more = false;
        if !all && has_table(&self.conn, crate::store::LEGACY_TURNS_TABLE)? {
            let mut query = sql(self.conn.prepare(&format!(
                "SELECT seq FROM {turns} WHERE session_id=?1 AND seq<?2 ORDER BY seq DESC LIMIT ?3",
                turns = crate::store::LEGACY_TURNS_TABLE,
            )))?;
            let starts = sql(sql(query
                .query_map(params![original, before, PAGE_TURNS as i64 + 1], |row| {
                    row.get::<_, i64>(0)
                }))?
            .collect::<rusqlite::Result<Vec<_>>>())?;
            if starts.len() > PAGE_TURNS {
                start = starts[PAGE_TURNS - 1];
                more = true;
            }
        }
        let blocks = self.shared_indexed_blocks(&original, start, before)?;
        Ok(HistoryPage {
            rows: blocks::project(&blocks)?,
            turns: project_turns(&blocks)?,
            before: more.then_some(start),
        })
    }
    /// Whether any block of the whole history, not only the window, records
    /// this accepted message.
    pub(super) fn shared_delivered(&self, id: i64, message: &str) -> Result<bool, String> {
        let original = self.persisted_session_id(id)?;
        if !self.shared_indexed(&original)? {
            return Ok(self
                .shared_blocks(id)?
                .iter()
                .any(|b| b["appRequestId"].as_str() == Some(message)));
        }
        sql(self.conn.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {blocks} WHERE session_id=?1 AND is_user=1
             AND json_extract(block_json,'$.appRequestId')=?2)",
                blocks = crate::store::LEGACY_BLOCKS_TABLE),
            params![original, message],
            |row| row.get::<_, bool>(0),
        ))
    }
}
