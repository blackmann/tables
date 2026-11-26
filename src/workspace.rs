//! The window: title bar, sidebar, tabs, the active tab, the row detail panel
//! and status bar, plus menus, modals and toasts drawn over them.
mod chrome;
mod connect;
mod detail;
mod overlays;
mod preferences;
mod sidebar;

use crate::db::config::{self, ConnectionConfig, Env, Store};
use crate::db::{Engine, Value};
use crate::menu::{self, Menu};
use crate::session::{self, Session, SessionEvent};
use crate::sql_view::SqlView;
use crate::tab::TabEvent;
use crate::table_view::{Mode, TableView};
use crate::theme::palette;
use crate::ui::*;
pub use connect::{ConnectForm, FormEvent};
use detail::DetailState;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::{InputEvent, InputState};
use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

actions!(
    workspace,
    [
        NewQuery, CloseTab, ToggleSidebar, ToggleDetail, OpenPalette, Save, Reload, FocusFilter, NextTab, PrevTab, NewConnection, Disconnect, OpenSettings,
        Quit, ConfirmModal, CancelModal, Tab1, Tab2, Tab3, Tab4, Tab5, Tab6, Tab7, Tab8, Tab9
    ]
);

pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("cmd-t", NewQuery, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("cmd-.", ToggleDetail, None),
        KeyBinding::new("cmd-k", OpenPalette, None),
        KeyBinding::new("cmd-p", OpenPalette, None),
        KeyBinding::new("cmd-s", Save, None),
        KeyBinding::new("cmd-r", Reload, None),
        KeyBinding::new("cmd-f", FocusFilter, None),
        KeyBinding::new("cmd-shift-]", NextTab, None),
        KeyBinding::new("cmd-shift-[", PrevTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PrevTab, None),
        KeyBinding::new("cmd-n", NewConnection, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-enter", ConfirmModal, Some("Modal")),
        KeyBinding::new("escape", CancelModal, Some("Modal")),
        KeyBinding::new("cmd-1", Tab1, None),
        KeyBinding::new("cmd-2", Tab2, None),
        KeyBinding::new("cmd-3", Tab3, None),
        KeyBinding::new("cmd-4", Tab4, None),
        KeyBinding::new("cmd-5", Tab5, None),
        KeyBinding::new("cmd-6", Tab6, None),
        KeyBinding::new("cmd-7", Tab7, None),
        KeyBinding::new("cmd-8", Tab8, None),
        KeyBinding::new("cmd-9", Tab9, None),
    ]
}

#[derive(Clone)]
pub enum Tab {
    Table(Entity<TableView>),
    Sql(Entity<SqlView>),
}

pub enum Modal {
    /// The SQL pending in a table tab; `commit` when it was opened to commit.
    Review { tab: Entity<TableView>, sql: Entity<InputState>, commit: bool },
    /// Statements that write to production, waiting for a yes.
    ConfirmRun { tab: Entity<SqlView>, sql: String, view: Entity<InputState> },
    Connection(Entity<ConnectForm>),
    Message { title: String, body: String },
    /// Closing a tab with uncommitted edits.
    CloseDirty(usize),
    Settings,
    /// Picking the columns of a table export: `(name, type, included)`.
    Export { table: String, format: sidebar::ExportFormat, columns: Vec<(String, String, bool)> },
}

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Left,
    Right,
}

struct Toast {
    id: u64,
    text: String,
    error: bool,
}

pub struct Workspace {
    pub store: Store,
    focus: FocusHandle,
    pub session: Option<Entity<Session>>,
    connecting: Option<(String, Task<()>)>,
    pub tabs: Vec<Tab>,
    pub active: Option<usize>,
    sidebar_open: bool,
    detail_open: bool,
    sidebar_width: f32,
    detail_width: f32,
    /// The panel edge being dragged: which side, where the drag began and the width then.
    resizing: Option<(Side, f32, f32)>,
    /// Waiting on Touch ID before a production write.
    authenticating: bool,
    sidebar_filter: Entity<InputState>,
    closed_sections: HashSet<&'static str>,
    popup: Option<(Entity<Menu>, Point<Pixels>)>,
    modal: Option<Modal>,
    modal_focus: FocusHandle,
    toasts: Vec<Toast>,
    next_toast: u64,
    detail: DetailState,
    prefs: preferences::Prefs,
    query_count: usize,
    self_weak: WeakEntity<Workspace>,
    _subs: Vec<Subscription>,
    _session_sub: Option<Subscription>,
    /// Where the title bar's connection and schema buttons were last drawn, so their menus open under them.
    connection_button: Rc<Cell<Option<Bounds<Pixels>>>>,
    schema_button: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl Workspace {
    /// A workspace for a connection the welcome window has already opened.
    pub fn new(config: ConnectionConfig, opened: session::Opened, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::load();
        let sidebar_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter tables"));
        let subs = vec![cx.subscribe(&sidebar_filter, |_, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                cx.notify();
            }
        })];
        let mut w = Workspace {
            store,
            focus: cx.focus_handle(),
            session: None,
            connecting: None,
            tabs: vec![],
            active: None,
            sidebar_open: true,
            detail_open: true,
            sidebar_width: 252.,
            detail_width: 330.,
            resizing: None,
            authenticating: false,
            sidebar_filter,
            closed_sections: HashSet::new(),
            popup: None,
            modal: None,
            modal_focus: cx.focus_handle(),
            toasts: vec![],
            next_toast: 0,
            detail: DetailState::default(),
            prefs: preferences::Prefs::new(window, cx),
            query_count: 0,
            self_weak: cx.entity().downgrade(),
            _subs: subs,
            _session_sub: None,
            connection_button: Rc::default(),
            schema_button: Rc::default(),
        };
        let schema = w.store.state(&config.id).schema.clone();
        let session = cx.new(|cx| Session::new(config, opened, schema, cx));
        w.set_session(session, window, cx);
        w
    }

    pub fn save_store(&self) {
        if let Err(e) = self.store.save() {
            eprintln!("Couldn't save settings: {e}");
        }
    }

    /// Switch this window to another connection. The current one stays open
    /// until the new one connects.
    pub fn connect(&mut self, config: ConnectionConfig, _window: &mut Window, cx: &mut Context<Self>) {
        let id = config.id.clone();
        self.toast(format!("Connecting to {}…", config.name), cx);
        let password = if config.engine == Engine::Postgres { config::password(&config.id) } else { None };
        let c2 = config.clone();
        let task = cx.background_spawn(async move { session::open(c2, password) });
        let t = cx.spawn_in(_window, async move |this, cx| {
            let res = task.await;
            let _ = this.update_in(cx, |w, window, cx| {
                w.connecting = None;
                match res {
                    Ok(opened) => {
                        let schema = w.store.state(&config.id).schema.clone();
                        let session = cx.new(|cx| Session::new(config.clone(), opened, schema, cx));
                        w.set_session(session, window, cx);
                        w.store.last = Some(config.id.clone());
                        w.store.state(&config.id).last_used = Some(chrono::Utc::now().timestamp());
                        w.save_store();
                    }
                    Err(e) => w.toast_error(format!("Couldn't connect to {}: {e}", config.name), cx),
                }
                cx.notify();
            });
        });
        self.connecting = Some((id, t));
        cx.notify();
    }

    fn set_session(&mut self, session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.clear();
        self.active = None;
        self.detail = DetailState::default();
        self._session_sub = Some(cx.subscribe_in(&session, window, |w, _, e: &SessionEvent, _, cx| match e {
            SessionEvent::Error(m) => w.toast_error(m.clone(), cx),
            SessionEvent::Reconnected => w.toast("Reconnected".into(), cx),
            _ => cx.notify(),
        }));
        self.session = Some(session);
        cx.notify();
    }

    /// Close this window and go back to the welcome window.
    pub fn disconnect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.iter().any(|t| matches!(t, Tab::Table(t) if t.read(cx).has_pending(cx))) {
            self.toast_error("Commit or discard pending changes before disconnecting".into(), cx);
            return;
        }
        self.save_store();
        crate::welcome::open(None, cx);
        window.remove_window();
    }

    pub fn conn_id(&self, cx: &App) -> Option<String> {
        self.session.as_ref().map(|s| s.read(cx).config.id.clone())
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|i| self.tabs.get(i))
    }

    fn subscribe_tab(&mut self, tab: &Tab, window: &mut Window, cx: &mut Context<Self>) {
        let sub = match tab {
            Tab::Table(t) => {
                let t2 = t.clone();
                cx.subscribe_in(t, window, move |w, _, e: &TabEvent, window, cx| w.on_tab_event(Tab::Table(t2.clone()), e, window, cx))
            }
            Tab::Sql(s) => {
                let s2 = s.clone();
                cx.subscribe_in(s, window, move |w, _, e: &TabEvent, window, cx| w.on_tab_event(Tab::Sql(s2.clone()), e, window, cx))
            }
        };
        self._subs.push(sub);
    }

    fn on_tab_event(&mut self, tab: Tab, e: &TabEvent, window: &mut Window, cx: &mut Context<Self>) {
        match e {
            TabEvent::Changed => cx.notify(),
            TabEvent::Toast(m) => self.toast(m.clone(), cx),
            TabEvent::Error(m) => {
                self.modal = Some(Modal::Message { title: "Something went wrong".into(), body: m.clone() });
                self.modal_focus.focus(window);
                cx.notify();
            }
            TabEvent::Menu { position, build, width } => self.open_menu(build.clone(), *position, *width, None, window, cx),
            TabEvent::OpenTable { table, filter } => self.open_table(table.clone(), false, filter.clone(), window, cx),
            TabEvent::RequestCommit => {
                if let Tab::Table(t) = tab {
                    self.request_commit(t, window, cx);
                }
            }
            TabEvent::RequestReview => {
                if let Tab::Table(t) = tab {
                    self.review(t, false, window, cx);
                }
            }
            TabEvent::ConfirmRun(sql) => {
                if let Tab::Sql(s) = tab {
                    let view = read_only_sql(sql, window, cx);
                    self.modal = Some(Modal::ConfirmRun { tab: s, sql: sql.clone(), view });
                    self.modal_focus.focus(window);
                    cx.notify();
                }
            }
            TabEvent::Ran(sql) => {
                self.query_count += 1;
                if let Some(id) = self.conn_id(cx) {
                    self.store.remember(&id, sql);
                    self.save_store();
                }
            }
            TabEvent::HistoryMenu(position) => {
                if let Tab::Sql(s) = tab {
                    let build = self.history_menu(s, cx);
                    self.open_menu(build, *position, 420., None, window, cx);
                }
            }
            TabEvent::Save(sql) => {
                if let Tab::Sql(s) = tab {
                    self.save_query(s, sql.clone(), cx);
                }
            }
        }
    }

    fn history_menu(&self, tab: Entity<SqlView>, cx: &App) -> menu::Builder {
        let history = self.conn_id(cx).and_then(|id| self.store.state.get(&id)).map(|s| s.history.clone()).unwrap_or_default();
        let weak = tab.downgrade();
        std::rc::Rc::new(move |_| {
            if history.is_empty() {
                return vec![menu::Entry::Item(menu::Item::new("Nothing run yet on this connection").disabled(true))];
            }
            let mut v = vec![menu::Entry::Label("Recent".into())];
            for sql in history.iter().take(25) {
                let (weak, sql2) = (weak.clone(), sql.clone());
                let one = sql.split_whitespace().collect::<Vec<_>>().join(" ");
                v.push(menu::Entry::Item(
                    menu::Item::new(crate::grid::ellipsize(&one, 70)).font(crate::theme::mono()).on_click(move |window, cx| {
                        let _ = weak.update(cx, |t, cx| t.insert(&sql2, window, cx));
                    }),
                ));
            }
            v
        })
    }

    fn save_query(&mut self, tab: Entity<SqlView>, sql: String, cx: &mut Context<Self>) {
        let Some(id) = self.conn_id(cx) else { return };
        let title = tab.read(cx).title.clone();
        // A leading comment names the query; otherwise it keeps the tab's title.
        let name = sql
            .lines()
            .next()
            .and_then(|l| l.trim().strip_prefix("--"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or(title);
        let state = self.store.state(&id);
        match state.saved.iter_mut().find(|q| q.name == name) {
            Some(q) => q.sql = sql,
            None => state.saved.push(config::SavedQuery { name: name.clone(), sql }),
        }
        self.save_store();
        tab.update(cx, |t, cx| {
            t.title = name.clone();
            t.mark_saved(cx);
        });
        self.toast(format!("Saved “{name}”"), cx);
    }

    pub fn open_table(&mut self, table: String, preview: bool, filter: Option<(String, Value)>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else { return };
        if let Some(i) = self.tabs.iter().position(|t| matches!(t, Tab::Table(v) if v.read(cx).table == table)) {
            if let Tab::Table(v) = &self.tabs[i] {
                v.update(cx, |v, cx| {
                    if !preview {
                        v.preview = false;
                    }
                    if let Some((col, val)) = filter {
                        v.mode = Mode::Data;
                        v.filter_to(col, val, window, cx);
                    } else if v.error.is_some() {
                        // Coming back to a table that failed (say, the connection
                        // dropped) tries again.
                        v.reload(cx);
                    }
                });
            }
            self.active = Some(i);
            cx.notify();
            return;
        }
        let view = cx.new(|cx| TableView::new(session, table, preview, filter, window, cx));
        let tab = Tab::Table(view);
        self.subscribe_tab(&tab, window, cx);
        // One preview tab at a time: a new preview takes the old one's place.
        let replace = preview.then(|| self.tabs.iter().position(|t| matches!(t, Tab::Table(v) if v.read(cx).preview && !v.read(cx).has_pending(cx)))).flatten();
        match replace {
            Some(i) => {
                self.tabs[i] = tab;
                self.active = Some(i);
            }
            None => {
                let at = self.active.map_or(self.tabs.len(), |a| a + 1).min(self.tabs.len());
                self.tabs.insert(at, tab);
                self.active = Some(at);
            }
        }
        cx.notify();
    }

    pub fn new_query(&mut self, title: Option<String>, text: String, run: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else { return };
        let n = self.tabs.iter().filter(|t| matches!(t, Tab::Sql(_))).count() + 1;
        let title = title.unwrap_or_else(|| format!("Query {n}"));
        let view = cx.new(|cx| SqlView::new(session, title, text, window, cx));
        let tab = Tab::Sql(view.clone());
        self.subscribe_tab(&tab, window, cx);
        let at = self.active.map_or(self.tabs.len(), |a| a + 1).min(self.tabs.len());
        self.tabs.insert(at, tab);
        self.active = Some(at);
        view.update(cx, |v, cx| {
            v.focus_editor(window, cx);
            if run {
                v.run(true, window, cx);
            }
        });
        cx.notify();
    }

    pub fn open_saved(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.tabs.iter().position(|t| matches!(t, Tab::Sql(s) if s.read(cx).title == name)) {
            self.active = Some(i);
            cx.notify();
            return;
        }
        let Some(id) = self.conn_id(cx) else { return };
        let Some(q) = self.store.state(&id).saved.iter().find(|q| q.name == name).cloned() else { return };
        self.new_query(Some(q.name), q.sql, false, window, cx);
    }

    pub fn close_tab(&mut self, i: usize, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(i) else { return };
        if !force && let Tab::Table(t) = tab
            && t.read(cx).has_pending(cx)
        {
            self.modal = Some(Modal::CloseDirty(i));
            self.modal_focus.focus(window);
            cx.notify();
            return;
        }
        self.tabs.remove(i);
        self.active = match self.active {
            _ if self.tabs.is_empty() => None,
            Some(a) if a > i => Some(a - 1),
            Some(a) if a == i => Some(i.min(self.tabs.len() - 1)),
            other => other,
        };
        cx.notify();
    }

    fn request_commit(&mut self, t: Entity<TableView>, window: &mut Window, cx: &mut Context<Self>) {
        let production = self.session.as_ref().is_some_and(|s| s.read(cx).config.env == Env::Production);
        if production {
            self.review(t, true, window, cx);
        } else {
            t.update(cx, |t, cx| t.commit(cx));
        }
    }

    fn review(&mut self, t: Entity<TableView>, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let stmts = t.read(cx).statements(cx);
        if stmts.is_empty() {
            return;
        }
        let text = format!("BEGIN;\n\n{}\n\nCOMMIT;", stmts.join("\n\n"));
        let sql = read_only_sql(&text, window, cx);
        self.modal = Some(Modal::Review { tab: t, sql, commit });
        self.modal_focus.focus(window);
        cx.notify();
    }

    pub fn toast(&mut self, text: String, cx: &mut Context<Self>) {
        self.push_toast(text, false, cx);
    }

    pub fn toast_error(&mut self, text: String, cx: &mut Context<Self>) {
        self.push_toast(text, true, cx);
    }

    fn push_toast(&mut self, text: String, error: bool, cx: &mut Context<Self>) {
        let id = self.next_toast;
        self.next_toast += 1;
        self.toasts.push(Toast { id, text, error });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(if error { 6000 } else { 2800 })).await;
            let _ = this.update(cx, |w, cx| {
                w.toasts.retain(|t| t.id != id);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn dismiss_toast(&mut self, id: u64, cx: &mut Context<Self>) {
        self.toasts.retain(|t| t.id != id);
        cx.notify();
    }

    pub fn open_menu(&mut self, build: menu::Builder, position: Point<Pixels>, width: f32, search: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let m = cx.new(|cx| Menu::new(build, search, px(width), window, cx));
        m.update(cx, |m, cx| m.focus(window, cx));
        cx.subscribe_in(&m, window, |w, m, _: &DismissEvent, window, cx| {
            w.popup = None;
            // Give focus back only if the menu still has it: an item may have
            // moved it on purpose, like into a new filter's value field.
            if window.focused(cx).is_none() || m.focus_handle(cx).contains_focused(window, cx) {
                w.refocus(window, cx);
            }
            cx.notify();
        })
        .detach();
        self.popup = Some((m, position));
        cx.notify();
    }

    /// Hand focus back to the active tab after a menu or modal closes.
    fn refocus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A dialog that's still open keeps the keys, Escape included.
        if self.modal.is_some() {
            self.modal_focus.focus(window);
            return;
        }
        match self.active_tab() {
            Some(Tab::Table(t)) => {
                let g = t.read(cx).grid.clone();
                window.focus(&g.focus_handle(cx));
            }
            Some(Tab::Sql(s)) => {
                let e = s.read(cx).editor.clone();
                e.update(cx, |s, cx| s.focus(window, cx));
            }
            None => self.focus.focus(window),
        }
    }

    fn close_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.modal = None;
        self.refocus(window, cx);
        cx.notify();
    }

    fn confirm_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.authenticating {
            return;
        }
        let prod = self.session.as_ref().is_some_and(|s| s.read(cx).config.env == Env::Production);
        // Writes to production go through Touch ID (or the account password).
        let reason = match &self.modal {
            Some(Modal::Review { .. }) if prod => Some("commit changes to production"),
            Some(Modal::ConfirmRun { .. }) => Some("run writes against production"),
            _ => None,
        };
        if let Some(reason) = reason {
            self.authenticating = true;
            cx.notify();
            let task = cx.background_spawn(async move { crate::auth::confirm(reason) });
            cx.spawn_in(window, async move |this, cx| {
                let res = task.await;
                let _ = this.update_in(cx, |w, window, cx| {
                    w.authenticating = false;
                    match res {
                        Ok(()) => w.run_confirmed(window, cx),
                        Err(e) => {
                            w.toast_error(format!("Not confirmed: {e}"), cx);
                            cx.notify();
                        }
                    }
                });
            })
            .detach();
            return;
        }
        self.run_confirmed(window, cx);
    }

    fn run_confirmed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.modal.take() {
            Some(Modal::Review { tab, .. }) => tab.update(cx, |t, cx| t.commit(cx)),
            Some(Modal::ConfirmRun { tab, sql, .. }) => tab.update(cx, |t, cx| t.execute(sql, cx)),
            Some(Modal::CloseDirty(i)) => self.close_tab(i, true, window, cx),
            Some(Modal::Export { table, format, columns }) => {
                let picked: Vec<String> = columns.iter().filter(|c| c.2).map(|c| c.0.clone()).collect();
                if picked.is_empty() {
                    self.modal = Some(Modal::Export { table, format, columns });
                    return;
                }
                let all = picked.len() == columns.len();
                self.export_table(table, format, (!all).then_some(picked), cx);
            }
            Some(Modal::Connection(form)) => {
                self.modal = Some(Modal::Connection(form.clone()));
                form.update(cx, |f, cx| f.submit(window, cx));
                return;
            }
            _ => {}
        }
        self.refocus(window, cx);
        cx.notify();
    }

    fn select_tab(&mut self, i: usize, cx: &mut Context<Self>) {
        if i < self.tabs.len() {
            self.active = Some(i);
            cx.notify();
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_tab().cloned() {
            Some(Tab::Table(t)) if t.read(cx).has_pending(cx) => self.request_commit(t, window, cx),
            Some(Tab::Sql(s)) => {
                let sql = s.read(cx).text(cx);
                self.save_query(s, sql, cx);
            }
            _ => {}
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = &self.session {
            s.update(cx, |s, cx| s.refresh(cx));
        }
        // Refreshing the schema reloads the open tables' structure, and with it their rows.
        if let Some(Tab::Table(t)) = self.active_tab().cloned() {
            t.update(cx, |t, cx| t.reload(cx));
        }
    }
}

/// Open a workspace window on a connection that's already open.
pub fn open(config: ConnectionConfig, opened: session::Opened, cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
    let title = format!("{} — Tables", config.name);
    let res = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui_component::TitleBar::title_bar_options()),
            window_min_size: Some(size(px(980.), px(600.))),
            ..Default::default()
        },
        move |window, cx| {
            window.set_window_title(&title);
            crate::theme::sync(window, cx);
            window.observe_window_appearance(crate::theme::sync).detach();
            // Closing the last workspace window goes back to the welcome window.
            window.on_window_should_close(cx, |_, cx| {
                if cx.windows().len() == 1 {
                    cx.set_global(crate::welcome::ShowOnClose(true));
                }
                true
            });
            let view = cx.new(|cx| Workspace::new(config, opened, window, cx));
            cx.new(|cx| gpui_component::Root::new(view, window, cx))
        },
    );
    if let Err(e) = res {
        eprintln!("Could not open the workspace window: {e}");
    }
}

/// A read-only SQL viewer, for the review and confirm dialogs.
fn read_only_sql(text: &str, window: &mut Window, cx: &mut Context<Workspace>) -> Entity<InputState> {
    let text = text.to_string();
    let e = cx.new(|cx| InputState::new(window, cx).code_editor("sql").line_number(true).soft_wrap(true));
    e.update(cx, |s, cx| s.set_value(text, window, cx));
    e
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if window.focused(cx).is_none() {
            self.focus.focus(window);
        }
        let c = palette(cx);
        let body: AnyElement = if self.session.is_some() {
            let dragging = self.resizing.map(|r| r.0);
            let handle = move |id: &'static str, side: Side, width: f32, cx: &mut Context<Self>| {
                div()
                    .id(id)
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .w(px(7.))
                    .when(side == Side::Left, |s| s.left(px(width - 3.)))
                    .when(side == Side::Right, |s| s.right(px(width - 3.)))
                    .cursor_col_resize()
                    .when(dragging == Some(side), |s| s.bg(hx(c.blue).opacity(0.3)))
                    .hover(|s| s.bg(hx(c.blue).opacity(0.3)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |w, e: &MouseDownEvent, _, cx| {
                        w.resizing = Some((side, f(e.position.x), width));
                        cx.stop_propagation();
                    }))
            };
            let (sw, dw) = (self.sidebar_width, self.detail_width);
            row()
                .flex_1()
                .min_h_0()
                .relative()
                .on_mouse_move(cx.listener(|w, e: &MouseMoveEvent, _, cx| {
                    let Some((side, x0, w0)) = w.resizing else { return };
                    if e.pressed_button != Some(MouseButton::Left) {
                        w.resizing = None;
                        cx.notify();
                        return;
                    }
                    let dx = f(e.position.x) - x0;
                    match side {
                        Side::Left => w.sidebar_width = (w0 + dx).clamp(180., 520.),
                        Side::Right => w.detail_width = (w0 - dx).clamp(260., 640.),
                    }
                    cx.notify();
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(|w, _, _, cx| {
                    w.resizing = None;
                    cx.notify();
                }))
                .when(self.sidebar_open, |s| s.child(self.render_sidebar(c, window, cx)))
                .child(col().flex_1().min_w_0().h_full().child(self.render_tabs(c, cx)).child(div().flex_1().min_h_0().child(self.render_active(c, cx))))
                .when(self.detail_open, |s| s.child(self.render_detail(c, window, cx)))
                .when(self.sidebar_open, |s| s.child(handle("resize-sidebar", Side::Left, sw, cx)))
                .when(self.detail_open, |s| s.child(handle("resize-detail", Side::Right, dw, cx)))
                .into_any_element()
        } else {
            div().flex_1().into_any_element()
        };
        col()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .bg(rgb(c.background))
            .text_color(rgb(c.text))
            .font_family(crate::theme::ui_font())
            .text_size(px(crate::theme::scaled_ui(13.)))
            .on_action(cx.listener(|w, _: &NewQuery, window, cx| w.new_query(None, String::new(), false, window, cx)))
            .on_action(cx.listener(|w, _: &CloseTab, window, cx| {
                if let Some(i) = w.active {
                    w.close_tab(i, false, window, cx);
                }
            }))
            .on_action(cx.listener(|w, _: &ToggleSidebar, _, cx| {
                w.sidebar_open = !w.sidebar_open;
                cx.notify();
            }))
            .on_action(cx.listener(|w, _: &ToggleDetail, _, cx| {
                w.detail_open = !w.detail_open;
                cx.notify();
            }))
            .on_action(cx.listener(|w, _: &OpenPalette, window, cx| w.open_palette(window, cx)))
            .on_action(cx.listener(|w, _: &Save, window, cx| w.save(window, cx)))
            .on_action(cx.listener(|w, _: &Reload, _, cx| w.reload(cx)))
            .on_action(cx.listener(|w, _: &FocusFilter, window, cx| {
                w.sidebar_open = true;
                w.sidebar_filter.update(cx, |s, cx| s.focus(window, cx));
                cx.notify();
            }))
            .on_action(cx.listener(|w, _: &NextTab, _, cx| {
                if !w.tabs.is_empty() {
                    let next = w.active.map_or(0, |a| (a + 1) % w.tabs.len());
                    w.select_tab(next, cx);
                }
            }))
            .on_action(cx.listener(|w, _: &PrevTab, _, cx| {
                if !w.tabs.is_empty() {
                    let prev = w.active.map_or(0, |a| (a + w.tabs.len() - 1) % w.tabs.len());
                    w.select_tab(prev, cx);
                }
            }))
            .on_action(cx.listener(|w, _: &NewConnection, window, cx| w.edit_connection(None, window, cx)))
            .on_action(cx.listener(|w, _: &Disconnect, window, cx| w.disconnect(window, cx)))
            .on_action(cx.listener(|w, _: &OpenSettings, window, cx| w.open_settings(window, cx)))
            .on_action(cx.listener(|w, _: &Tab1, _, cx| w.select_tab(0, cx)))
            .on_action(cx.listener(|w, _: &Tab2, _, cx| w.select_tab(1, cx)))
            .on_action(cx.listener(|w, _: &Tab3, _, cx| w.select_tab(2, cx)))
            .on_action(cx.listener(|w, _: &Tab4, _, cx| w.select_tab(3, cx)))
            .on_action(cx.listener(|w, _: &Tab5, _, cx| w.select_tab(4, cx)))
            .on_action(cx.listener(|w, _: &Tab6, _, cx| w.select_tab(5, cx)))
            .on_action(cx.listener(|w, _: &Tab7, _, cx| w.select_tab(6, cx)))
            .on_action(cx.listener(|w, _: &Tab8, _, cx| w.select_tab(7, cx)))
            .on_action(cx.listener(|w, _: &Tab9, _, cx| {
                let last = w.tabs.len().saturating_sub(1);
                w.select_tab(last, cx)
            }))
            .child(self.render_title(c, cx))
            .child(body)
            .when(self.session.is_some(), |s| s.child(self.render_status(c, cx)))
            .children(self.render_overlays(c, window, cx))
    }
}
