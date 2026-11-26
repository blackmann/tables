//! An open connection: the driver behind a mutex, and what we've learned about
//! the schema (tables, columns, functions) for the sidebar and completion.
use crate::db::config::ConnectionConfig;
use crate::db::{Driver, Engine, Relation, RelationKind, TableDetail};
use anyhow::Result;
use gpui::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type Canceller = Arc<dyn Fn() + Send + Sync>;

pub enum SessionEvent {
    /// Tables, columns or functions were (re)loaded.
    Schema,
    /// A table's structure arrived.
    Detail(String),
    /// A table's structure couldn't be read: (table, why).
    DetailFailed(String, String),
    /// The connection dropped, or came back.
    Link,
    /// A reconnect the user asked for worked.
    Reconnected,
    Error(String),
}

pub struct Session {
    pub config: ConnectionConfig,
    driver: Arc<Mutex<Box<dyn Driver>>>,
    cancel: Option<Canceller>,
    pub version: String,
    pub schemas: Vec<String>,
    pub schema: String,
    pub relations: Vec<Relation>,
    pub functions: Vec<(String, String)>,
    /// Every column in the schema, by table, for completion.
    pub columns: HashMap<String, Vec<(String, String)>>,
    pub details: HashMap<String, TableDetail>,
    pub loading: bool,
    loading_details: std::collections::HashSet<String>,
    /// Set from the background thread after each piece of work and by the watcher.
    lost: Arc<AtomicBool>,
    /// What the UI last saw of `lost`.
    pub disconnected: bool,
    pub reconnecting: bool,
    _tasks: Vec<Task<()>>,
    _watch: Task<()>,
}

impl EventEmitter<SessionEvent> for Session {}

pub struct Opened {
    pub driver: Box<dyn Driver>,
    pub cancel: Option<Canceller>,
    pub version: String,
    pub schemas: Vec<String>,
}

/// Connect and read the basics, off the UI thread.
pub fn open(config: ConnectionConfig, password: Option<String>) -> Result<Opened> {
    let (mut driver, cancel) = config.open(password)?;
    let version = driver.version().unwrap_or_default();
    let schemas = driver.schemas().unwrap_or_default();
    Ok(Opened { driver, cancel: cancel.map(Arc::from), version, schemas })
}

impl Session {
    pub fn new(config: ConnectionConfig, opened: Opened, schema: Option<String>, cx: &mut Context<Self>) -> Self {
        let default = opened.driver.default_schema();
        let schema = schema.filter(|s| opened.schemas.contains(s)).unwrap_or_else(|| {
            if opened.schemas.contains(&default) { default } else { opened.schemas.first().cloned().unwrap_or(default) }
        });
        let mut s = Session {
            config,
            driver: Arc::new(Mutex::new(opened.driver)),
            cancel: opened.cancel,
            version: opened.version,
            schemas: opened.schemas,
            schema,
            relations: vec![],
            functions: vec![],
            columns: HashMap::new(),
            details: HashMap::new(),
            loading: true,
            loading_details: Default::default(),
            lost: Arc::new(AtomicBool::new(false)),
            disconnected: false,
            reconnecting: false,
            _tasks: vec![],
            _watch: Task::ready(()),
        };
        s._watch = s.watch(cx);
        s.refresh(cx);
        s
    }

    /// Every few seconds, ask the server whether it's still there, so the
    /// status bar shows a dropped connection without waiting for a query.
    fn watch(&self, cx: &mut Context<Self>) -> Task<()> {
        let (d, lost) = (self.driver.clone(), self.lost.clone());
        cx.spawn(async move |this, cx| {
            // The flag is read every second (queries set it too); the server is asked every ten.
            for tick in 1u64.. {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if tick % 10 == 0 {
                    let (d, lost) = (d.clone(), lost.clone());
                    cx.background_spawn(async move {
                        // Busy means a query is running; it reports for itself when done.
                        if let Ok(mut guard) = d.try_lock() {
                            lost.store(!guard.ping(), Ordering::Relaxed);
                        }
                    })
                    .await;
                }
                let now = lost.load(Ordering::Relaxed);
                let alive = this.update(cx, |s, cx| s.sync_link(now, cx));
                if alive.is_err() {
                    break;
                }
            }
        })
    }

    fn sync_link(&mut self, lost: bool, cx: &mut Context<Self>) {
        if self.disconnected != lost && !self.reconnecting {
            self.disconnected = lost;
            cx.emit(SessionEvent::Link);
            cx.notify();
        }
    }

    /// Connect again now, then reload the schema (and with it every open table).
    pub fn reconnect(&mut self, cx: &mut Context<Self>) {
        if self.reconnecting {
            return;
        }
        self.reconnecting = true;
        cx.notify();
        let task = self.run(cx, |d| d.reconnect());
        self._tasks.push(cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |s, cx| {
                s.reconnecting = false;
                s.disconnected = s.lost.load(Ordering::Relaxed);
                match res {
                    Ok(()) => {
                        cx.emit(SessionEvent::Reconnected);
                        s.refresh(cx);
                    }
                    Err(e) => cx.emit(SessionEvent::Error(e.to_string())),
                }
                cx.emit(SessionEvent::Link);
                cx.notify();
            });
        }));
    }

    pub fn engine(&self) -> Engine {
        self.config.engine
    }

    /// Run `f` against the driver on a background thread.
    pub fn run<T: Send + 'static>(&self, cx: &App, f: impl FnOnce(&mut dyn Driver) -> T + Send + 'static) -> Task<T> {
        let (d, lost) = (self.driver.clone(), self.lost.clone());
        cx.background_spawn(async move {
            let mut guard = d.lock().unwrap_or_else(|e| e.into_inner());
            let out = f(guard.as_mut());
            lost.store(guard.is_closed(), Ordering::Relaxed);
            out
        })
    }

    /// Ask the server to stop the running statement. Called from the UI thread
    /// while the driver mutex is held by the query.
    pub fn cancel(&self) {
        if let Some(c) = self.cancel.clone() {
            std::thread::spawn(move || c());
        }
    }

    pub fn set_schema(&mut self, schema: String, cx: &mut Context<Self>) {
        if schema != self.schema {
            self.schema = schema;
            self.details.clear();
            self.refresh(cx);
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let schema = self.schema.clone();
        let task = self.run(cx, move |d| {
            let rel = d.relations(&schema);
            let fns = d.functions(&schema).unwrap_or_default();
            let cols = d.all_columns(&schema).unwrap_or_default();
            (rel, fns, cols)
        });
        self._tasks.push(cx.spawn(async move |this, cx| {
            let (rel, fns, cols) = task.await;
            let _ = this.update(cx, |s, cx| {
                s.loading = false;
                match rel {
                    Ok(r) => s.relations = r,
                    Err(e) => cx.emit(SessionEvent::Error(e.to_string())),
                }
                s.functions = fns;
                s.columns.clear();
                for (t, c, ty) in cols {
                    s.columns.entry(t).or_default().push((c, ty));
                }
                s.details.clear();
                cx.emit(SessionEvent::Schema);
                cx.notify();
            });
        }));
    }

    pub fn relation(&self, name: &str) -> Option<&Relation> {
        self.relations.iter().find(|r| r.name == name)
    }

    pub fn is_view(&self, name: &str) -> bool {
        self.relation(name).is_some_and(|r| r.kind == RelationKind::View)
    }

    /// The table's structure if loaded; starts loading it otherwise.
    pub fn detail(&mut self, table: &str, cx: &mut Context<Self>) -> Option<&TableDetail> {
        if !self.details.contains_key(table) && self.loading_details.insert(table.to_string()) {
            let (schema, t) = (self.schema.clone(), table.to_string());
            let task = self.run(cx, move |d| d.detail(&schema, &t));
            let t = table.to_string();
            self._tasks.push(cx.spawn(async move |this, cx| {
                let res = task.await;
                let _ = this.update(cx, |s, cx| {
                    s.loading_details.remove(&t);
                    match res {
                        Ok(d) => {
                            s.details.insert(t.clone(), d);
                            cx.emit(SessionEvent::Detail(t));
                        }
                        Err(e) => cx.emit(SessionEvent::DetailFailed(t, e.to_string())),
                    }
                    cx.notify();
                });
            }));
        }
        self.details.get(table)
    }

    /// Forget a table's structure so the next look reloads it.
    #[allow(dead_code)]
    pub fn invalidate(&mut self, table: &str) {
        self.details.remove(table);
    }

    pub fn qualified(&self, table: &str) -> String {
        crate::db::qualified(self.engine(), &self.schema, table)
    }
}
