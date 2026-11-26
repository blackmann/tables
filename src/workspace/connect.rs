//! The new/edit connection form, shared by the welcome and workspace windows.
use super::*;
use crate::db::config::SslMode;
use crate::icons::Glyph;
use crate::theme::{Palette, scaled_ui};
use gpui_component::input::Input;

pub enum FormEvent {
    Saved(ConnectionConfig, bool),
    Cancel,
    Deleted(String),
}

pub struct ConnectForm {
    pub config: ConnectionConfig,
    is_new: bool,
    name: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
    database: Entity<InputState>,
    path: Entity<InputState>,
    url: Entity<InputState>,
    testing: Option<Task<()>>,
    result: Option<(bool, String)>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<FormEvent> for ConnectForm {}

impl ConnectForm {
    pub fn new(config: Option<ConnectionConfig>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let is_new = config.is_none();
        let config = config.unwrap_or_else(|| ConnectionConfig::new(Engine::Postgres));
        let field = |placeholder: &str, value: String, window: &mut Window, cx: &mut Context<Self>| {
            let e = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.to_string()));
            e.update(cx, |s, cx| s.set_value(value, window, cx));
            e
        };
        let password = cx.new(|cx| InputState::new(window, cx).placeholder("Stored in your Keychain").masked(true));
        if !is_new && let Some(p) = config::password(&config.id) {
            password.update(cx, |s, cx| s.set_value(p, window, cx));
        }
        let name = field("My database", config.name.clone(), window, cx);
        let host = field("localhost", config.host.clone(), window, cx);
        let port = field("5432", config.port.to_string(), window, cx);
        let user = field("postgres", config.user.clone(), window, cx);
        let database = field("postgres", config.database.clone(), window, cx);
        let path = field("~/path/to/database.sqlite", config.path.clone(), window, cx);
        let url = field("Paste a postgres:// URL to fill the form", String::new(), window, cx);
        let subs = vec![cx.subscribe_in(&url, window, |f: &mut ConnectForm, input, e: &InputEvent, window, cx| {
            if matches!(e, InputEvent::Change) {
                let text = input.read(cx).value().to_string();
                if let Ok((c, pass)) = config::from_url(&text) {
                    f.fill(c, pass, window, cx);
                }
            }
        })];
        ConnectForm { config, is_new, name, host, port, user, password, database, path, url, testing: None, result: None, _subs: subs }
    }

    fn fill(&mut self, c: ConnectionConfig, pass: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.config.engine = c.engine;
        self.config.ssl = c.ssl;
        let set = |e: &Entity<InputState>, v: String, window: &mut Window, cx: &mut Context<Self>| e.update(cx, |s, cx| s.set_value(v, window, cx));
        set(&self.host, c.host, window, cx);
        set(&self.port, c.port.to_string(), window, cx);
        set(&self.user, c.user, window, cx);
        set(&self.database, c.database, window, cx);
        set(&self.path, c.path, window, cx);
        if self.name.read(cx).value().is_empty() {
            set(&self.name, c.name, window, cx);
        }
        if let Some(p) = pass {
            set(&self.password, p, window, cx);
        }
        cx.notify();
    }

    /// The form's current values as a config.
    fn read(&self, cx: &App) -> ConnectionConfig {
        let v = |e: &Entity<InputState>| e.read(cx).value().trim().to_string();
        let mut c = self.config.clone();
        c.host = v(&self.host);
        c.port = v(&self.port).parse().unwrap_or(5432);
        c.user = v(&self.user);
        c.database = v(&self.database);
        c.path = v(&self.path);
        c.name = v(&self.name);
        if c.name.is_empty() {
            c.name = match c.engine {
                Engine::Sqlite => c.database_name(),
                Engine::Postgres if !c.database.is_empty() => c.database.clone(),
                Engine::Postgres => c.host.clone(),
            };
        }
        c
    }

    fn test(&mut self, cx: &mut Context<Self>) {
        let c = self.read(cx);
        let pass = self.password.read(cx).value().to_string();
        self.result = None;
        let task = cx.background_spawn(async move { session::open(c, Some(pass)).map(|o| o.version) });
        self.testing = Some(cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |f, cx| {
                f.testing = None;
                f.result = Some(match res {
                    Ok(v) => (true, format!("Connected — {v}")),
                    Err(e) => (false, e.to_string()),
                });
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub fn submit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let c = self.read(cx);
        if c.engine == Engine::Postgres {
            let pass = self.password.read(cx).value().to_string();
            if let Err(e) = config::set_password(&c.id, &pass) {
                self.result = Some((false, e.to_string()));
                cx.notify();
                return;
            }
        }
        cx.emit(FormEvent::Saved(c, true));
    }

    pub fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Open".into()) });
        let path = self.path.clone();
        let name = self.name.clone();
        cx.spawn_in(window, async move |_, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(p) = paths.first()
            {
                let p = p.display().to_string();
                let stem = std::path::Path::new(&p).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let _ = cx.update(|window, cx| {
                    path.update(cx, |s, cx| s.set_value(p, window, cx));
                    if name.read(cx).value().is_empty() {
                        name.update(cx, |s, cx| s.set_value(stem, window, cx));
                    }
                });
            }
        })
        .detach();
    }
}

fn label(c: Palette, s: &str) -> Div {
    div().text_size(px(scaled_ui(11.5))).text_color(rgb(c.muted)).font_weight(FontWeight::MEDIUM).child(s.to_string())
}

fn field(c: Palette, input: &Entity<InputState>) -> Div {
    div().h(px(30.)).px(px(6.)).flex().items_center().rounded(px(6.)).border_1().border_color(rgb(c.border)).bg(rgb(c.background)).child(Input::new(input).appearance(false))
}

fn segmented<T: Copy + PartialEq + 'static>(c: Palette, id: &'static str, options: &[(T, &'static str, Option<u32>)], current: T, cx: &mut Context<ConnectForm>, set: fn(&mut ConnectForm, T)) -> Div {
    row().h(px(28.)).rounded(px(6.)).border_1().border_color(rgb(c.border)).overflow_hidden().children(options.iter().enumerate().map(|(i, (v, l, color))| {
        let v = *v;
        let on = v == current;
        row()
            .id((id, i))
            .h_full()
            .px(px(12.))
            .gap(px(6.))
            .when(i > 0, |s| s.border_l_1().border_color(rgb(c.border)))
            .when(on, |s| s.bg(rgb(c.selected)).text_color(rgb(c.text)))
            .when(!on, |s| s.text_color(rgb(c.muted)).hover(|s| s.bg(rgb(c.hover))))
            .when_some(*color, |s, col| s.child(dot(col, 7.)))
            .child(l.to_string())
            .on_click(cx.listener(move |f, _, _, cx| {
                set(f, v);
                cx.notify();
            }))
    }))
}

impl Render for ConnectForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = palette(cx);
        let pg = self.config.engine == Engine::Postgres;
        let (env, _) = env_color(c, self.config.env);
        // Header and footer stay fixed; the fields scroll when the window is too short.
        col()
            .flex_shrink()
            .min_h_0()
            .child(
                row()
                    .flex_none()
                    .px(px(18.))
                    .pt(px(16.))
                    .pb(px(10.))
                    .gap(px(10.))
                    .child(engine_badge(self.config.engine, 22.))
                    .child(div().text_size(px(scaled_ui(15.))).font_weight(FontWeight::SEMIBOLD).child(if self.is_new { "New connection" } else { "Edit connection" })),
            )
            .child(
                col()
                    .id("connect-body")
                    .flex_shrink()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(18.))
                    .pb(px(14.))
                    .gap(px(12.))
                    .child(row().gap(px(10.)).child(
                        segmented(c, "engine", &[(Engine::Postgres, "PostgreSQL", None), (Engine::Sqlite, "SQLite", None)], self.config.engine, cx, |f, v| {
                            f.config.engine = v;
                            if v == Engine::Postgres && f.config.port == 0 {
                                f.config.port = 5432;
                            }
                        }),
                    ))
                    .when(pg, |s| s.child(col().gap(px(5.)).child(label(c, "URL")).child(field(c, &self.url))))
                    .child(
                        row()
                            .gap(px(10.))
                            .items_end()
                            .child(col().flex_1().gap(px(5.)).child(label(c, "Name")).child(field(c, &self.name)))
                            .child(col().gap(px(5.)).child(label(c, "Environment")).child(segmented(
                                c,
                                "env",
                                &[(Env::Development, "Dev", Some(c.green)), (Env::Staging, "Staging", Some(c.amber)), (Env::Production, "Prod", Some(c.red))],
                                self.config.env,
                                cx,
                                |f, v| f.config.env = v,
                            ))),
                    )
                    .when(pg, |s| {
                        s.child(
                            row()
                                .gap(px(10.))
                                .child(col().flex_1().gap(px(5.)).child(label(c, "Host")).child(field(c, &self.host)))
                                .child(col().w(px(90.)).gap(px(5.)).child(label(c, "Port")).child(field(c, &self.port))),
                        )
                        .child(
                            row()
                                .gap(px(10.))
                                .child(col().flex_1().gap(px(5.)).child(label(c, "User")).child(field(c, &self.user)))
                                .child(col().flex_1().gap(px(5.)).child(label(c, "Password")).child(field(c, &self.password))),
                        )
                        .child(col().gap(px(5.)).child(label(c, "Database")).child(field(c, &self.database)))
                        .child(col().gap(px(5.)).child(label(c, "SSL")).child(segmented(
                            c,
                            "ssl",
                            &[(SslMode::Disable, "Off", None), (SslMode::Prefer, "Prefer", None), (SslMode::Require, "Require", None), (SslMode::RequireNoVerify, "Require, any cert", None)],
                            self.config.ssl,
                            cx,
                            |f, v| f.config.ssl = v,
                        )))
                    })
                    .when(!pg, |s| {
                        s.child(
                            col().gap(px(5.)).child(label(c, "Database file")).child(
                                row()
                                    .gap(px(8.))
                                    .child(field(c, &self.path).flex_1())
                                    .child(button(c, "browse", Some(Glyph::Folder), Some("Browse…"), Tone::Plain).on_click(cx.listener(|f, _, window, cx| f.browse(window, cx)))),
                            ),
                        )
                    })
                    .when(self.config.env == Env::Production, |s| {
                        s.child(
                            row()
                                .gap(px(8.))
                                .px(px(10.))
                                .py(px(8.))
                                .rounded(px(8.))
                                .bg(rgb(c.red_tint))
                                .text_size(px(scaled_ui(12.)))
                                .child(icon(Glyph::Shield, 14., env))
                                .child("Production: a red stripe marks the window, and every write asks before it runs."),
                        )
                    })
                    .when_some(self.result.clone(), |s, (ok, msg)| {
                        s.child(
                            row()
                                .items_start()
                                .gap(px(8.))
                                .px(px(10.))
                                .py(px(8.))
                                .rounded(px(8.))
                                .bg(rgb(if ok { c.green_tint } else { c.red_tint }))
                                .text_size(px(scaled_ui(12.)))
                                .child(icon(if ok { Glyph::Check } else { Glyph::Warn }, 14., if ok { c.green } else { c.red }))
                                .child(div().flex_1().child(msg)),
                        )
                    }),
            )
            .child(
                row()
                    .flex_none()
                    .px(px(18.))
                    .py(px(12.))
                    .gap(px(8.))
                    .border_t_1()
                    .border_color(rgb(c.popover_border))
                    .bg(hx(c.background).opacity(0.4))
                    .child(button(c, "test", Some(Glyph::Plug), Some(if self.testing.is_some() { "Testing…" } else { "Test" }), Tone::Plain).on_click(cx.listener(|f, _, _, cx| f.test(cx))))
                    .when(!self.is_new, |s| {
                        s.child(button(c, "delete", Some(Glyph::Trash), Some("Delete"), Tone::Ghost).on_click(cx.listener(|f, _, _, cx| cx.emit(FormEvent::Deleted(f.config.id.clone())))))
                    })
                    .child(div().flex_1())
                    .child(button(c, "cancel", None, Some("Cancel"), Tone::Plain).on_click(cx.listener(|_, _, _, cx| cx.emit(FormEvent::Cancel))))
                    .child(
                        button(c, "save", None, Some("Save & connect"), Tone::Primary)
                            .child(kbd(c, "⌘↵").border_color(hx(c.primary_text).opacity(0.3)).text_color(rgb(c.primary_text)))
                            .on_click(cx.listener(|f, _, window, cx| f.submit(window, cx))),
                    ),
            )
    }
}

impl Workspace {
    pub(super) fn edit_connection(&mut self, config: Option<ConnectionConfig>, window: &mut Window, cx: &mut Context<Self>) {
        let form = cx.new(|cx| ConnectForm::new(config, window, cx));
        self._subs.push(cx.subscribe_in(&form, window, |w, _, e: &FormEvent, window, cx| {
            match e {
                FormEvent::Saved(c, connect) => {
                    match w.store.connections.iter_mut().find(|x| x.id == c.id) {
                        Some(x) => *x = c.clone(),
                        None => w.store.connections.push(c.clone()),
                    }
                    w.save_store();
                    w.modal = None;
                    if *connect {
                        // Reconnect, even to the same connection: its settings may have changed.
                        if w.tabs.iter().any(|t| matches!(t, Tab::Table(t) if t.read(cx).has_pending(cx))) {
                            w.toast_error("Saved. Commit or discard pending changes, then reconnect".into(), cx);
                        } else {
                            w.connect(c.clone(), window, cx);
                        }
                    }
                }
                FormEvent::Cancel => w.modal = None,
                FormEvent::Deleted(id) => {
                    w.store.connections.retain(|x| &x.id != id);
                    w.store.state.remove(id);
                    config::forget_password(id);
                    w.save_store();
                    w.modal = None;
                    if w.conn_id(cx).as_deref() == Some(id.as_str()) {
                        w.disconnect(window, cx);
                    }
                }
            }
            cx.notify();
        }));
        self.modal = Some(Modal::Connection(form));
        cx.notify();
    }
}
