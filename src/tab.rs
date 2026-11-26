//! What a tab can ask of the workspace around it.
use crate::db::Value;
use crate::menu;
use gpui::*;

pub enum TabEvent {
    /// Title, dirty state or status-bar details changed.
    Changed,
    Toast(String),
    Error(String),
    /// Show a popup menu at a point.
    Menu { position: Point<Pixels>, build: menu::Builder, width: f32 },
    /// Open a table, optionally filtered to one value of a column.
    OpenTable { table: String, filter: Option<(String, Value)> },
    /// The tab has pending changes to commit; the workspace confirms where needed.
    RequestCommit,
    /// Show the pending changes as SQL without committing yet.
    RequestReview,
    /// Statements that write to a production database, waiting for a yes.
    ConfirmRun(String),
    /// A statement ran, for the history.
    Ran(String),
    HistoryMenu(Point<Pixels>),
    /// Keep this SQL as a saved query.
    Save(String),
}
