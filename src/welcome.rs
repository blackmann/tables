//! The welcome window: a small, fixed-size window listing saved connections.
//! It shows at launch and whenever the last workspace window closes; opening a
//! connection hands off to a workspace window and closes this one.
use crate::db::Engine;
use crate::db::config::{self, ConnectionConfig, Store};
use crate::icons::Glyph;
use crate::menu::{self, Menu};
use crate::session;
use crate::theme::{Palette, palette, scaled_ui};
use crate::ui::*;
use crate::workspace::{CancelModal, ConfirmModal, ConnectForm, FormEvent, ImportEvent, ImportForm, ImportTablePlus, NewConnection};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Root, TitleBar};

actions!(welcome, [OpenFile, EditSelected, DuplicateSelected]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-o", OpenFile, Some("Welcome")),
        KeyBinding::new("cmd-e", EditSelected, Some("Welcome")),
        KeyBinding::new("cmd-d", DuplicateSelected, Some("Welcome")),
    ]);
}

/// The app icon's artwork: square and full-bleed, so clip it to a rounded rect.
fn app_icon() -> std::sync::Arc<Image> {
    static ICON: std::sync::OnceLock<std::sync::Arc<Image>> = std::sync::OnceLock::new();
    ICON.get_or_init(|| std::sync::Arc::new(Image::from_bytes(ImageFormat::Png, include_bytes!("../assets/icon-256.png").to_vec())))
        .clone()
}

const WIDTH: f32 = 760.;
const HEIGHT: f32 = 480.;
/// The window grows to this while the connection form is open, so the
/// Postgres fields fit without scrolling.
const FORM_HEIGHT: f32 = 680.;
/// How many recently used connections lead the list.
const RECENT: usize = 3;

/// Set when a workspace window closes as the last window, so the app shows
/// this window instead of quitting.
#[derive(Default)]
pub struct ShowOnClose(pub bool);
impl Global for ShowOnClose {}

/// Open the welcome window. `argument` is a SQLite path or postgres:// URL
/// from the command line, connected to straight away.
pub fn open(argument: Option<String>, cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx);
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("Tables".into()), appears_transparent: true, traffic_light_position: Some(point(px(14.), px(14.))) }),
            is_resizable: false,
            is_minimizable: false,
            ..Default::default()
        },
        move |window, cx| {
            window.set_window_title("Tables");
            crate::theme::sync(window, cx);
            window.observe_window_appearance(crate::theme::sync).detach();
            let view = cx.new(|cx| Welcome::new(window, cx));
            if let Some(arg) = argument {
                view.update(cx, |w, cx| w.open_argument(&arg, window, cx));
            }
            cx.new(|cx| Root::new(view, window, cx))
        },
    );
    if let Err(e) = opened {
        eprintln!("Could not open the welcome window: {e}");
    }
}

struct Row {
    config: ConnectionConfig,
    recent: bool,
    when: String,
}

pub struct Welcome {
    store: Store,
    focus: FocusHandle,
    filter: Entity<InputState>,
    selected: usize,
    connecting: Option<(String, Task<()>)>,
    error: Option<(String, String)>,
    form: Option<Entity<ConnectForm>>,
    import: Option<Entity<ImportForm>>,
    modal_focus: FocusHandle,
    popup: Option<(Entity<Menu>, Point<Pixels>)>,
    scroll: ScrollHandle,
    _subs: Vec<Subscription>,
}

impl Welcome {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Search connections"));
        filter.update(cx, |s, cx| s.focus(window, cx));
        let subs = vec![cx.subscribe(&filter, |w: &mut Welcome, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                w.selected = 0;
                cx.notify();
            }
        })];
        Welcome {
            store: Store::load(),
            focus: cx.focus_handle(),
            filter,
            selected: 0,
            connecting: None,
            error: None,
            form: None,
            import: None,
            modal_focus: cx.focus_handle(),
            popup: None,
            scroll: ScrollHandle::new(),
            _subs: subs,
        }
    }

    fn save_store(&self) {
        if let Err(e) = self.store.save() {
            eprintln!("Couldn't save settings: {e}");
        }
    }

    /// The connections to show, most recently used first, narrowed by the search.
    fn rows(&self, cx: &App) -> Vec<Row> {
        let q = self.filter.read(cx).value().trim().to_lowercase();
        let used = |c: &ConnectionConfig| self.store.state.get(&c.id).and_then(|s| s.last_used);
        let mut recent: Vec<&ConnectionConfig> = self.store.connections.iter().filter(|c| used(c).is_some()).collect();
        recent.sort_by_key(|c| std::cmp::Reverse(used(c)));
        recent.truncate(RECENT);
        let rest = self.store.connections.iter().filter(|c| !recent.iter().any(|r| r.id == c.id));
        recent
            .iter()
            .map(|c| (*c, true))
            .chain(rest.map(|c| (c, false)))
            .filter(|(c, _)| q.is_empty() || c.name.to_lowercase().contains(&q) || describe(c).to_lowercase().contains(&q))
            .map(|(c, recent)| Row { config: c.clone(), recent, when: used(c).map(ago).unwrap_or_default() })
            .collect()
    }

    fn selected_config(&self, cx: &App) -> Option<ConnectionConfig> {
        self.rows(cx).into_iter().nth(self.selected).map(|r| r.config)
    }

    fn step(&mut self, down: bool, cx: &mut Context<Self>) {
        let n = self.rows(cx).len();
        if n == 0 {
            return;
        }
        self.selected = if down { (self.selected + 1).min(n - 1) } else { self.selected.saturating_sub(1) };
        // The list's children include a header before each group.
        let rows = self.rows(cx);
        let searching = !self.filter.read(cx).value().trim().is_empty();
        let headers = (0..=self.selected).filter(|&i| i == 0 || group_of(&rows, i, searching) != group_of(&rows, i - 1, searching)).count();
        self.scroll.scroll_to_item(self.selected + headers);
        cx.notify();
    }

    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.form.is_some() || self.import.is_some() || self.popup.is_some() {
            return;
        }
        let k = &e.keystroke;
        if k.modifiers.modified() {
            return;
        }
        match k.key.as_str() {
            "down" => self.step(true, cx),
            "up" => self.step(false, cx),
            "enter" => {
                if let Some(c) = self.selected_config(cx) {
                    self.connect(c, window, cx);
                }
            }
            "escape" => {
                self.filter.update(cx, |s, cx| s.set_value("", window, cx));
                self.selected = 0;
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    pub fn connect(&mut self, config: ConnectionConfig, window: &mut Window, cx: &mut Context<Self>) {
        if self.connecting.is_some() {
            return;
        }
        self.error = None;
        let id = config.id.clone();
        let password = if config.engine.is_server() { config::password(&config.id) } else { None };
        let c2 = config.clone();
        let task = cx.background_spawn(async move { session::open(c2, password) });
        let t = cx.spawn_in(window, async move |this, cx| {
            let res = task.await;
            let _ = this.update_in(cx, |w, window, cx| {
                w.connecting = None;
                match res {
                    Ok(opened) => {
                        w.store.state(&config.id).last_used = Some(chrono::Utc::now().timestamp());
                        w.store.last = Some(config.id.clone());
                        w.save_store();
                        crate::workspace::open(config.clone(), opened, cx);
                        window.remove_window();
                    }
                    Err(e) => w.error = Some((config.id.clone(), e.to_string())),
                }
                cx.notify();
            });
        });
        self.connecting = Some((id, t));
        cx.notify();
    }

    /// `tables some.db` or `tables postgres://…`: save it if it's new, then connect.
    fn open_argument(&mut self, arg: &str, window: &mut Window, cx: &mut Context<Self>) {
        let parsed = if arg.contains("://") {
            config::from_url(arg)
        } else {
            let mut c = ConnectionConfig::new(Engine::Sqlite);
            c.path = std::fs::canonicalize(arg).map(|p| p.display().to_string()).unwrap_or_else(|_| arg.to_string());
            c.name = c.database_name();
            Ok((c, None))
        };
        match parsed {
            Ok((mut c, pass)) => {
                if let Some(existing) = self.store.connections.iter().find(|e| e.engine == c.engine && e.path == c.path && e.host == c.host && e.database == c.database && e.user == c.user) {
                    c = existing.clone();
                } else {
                    if let Some(p) = &pass {
                        let _ = config::set_password(&c.id, p);
                    }
                    self.store.connections.push(c.clone());
                    self.save_store();
                }
                self.connect(c, window, cx);
            }
            Err(e) => {
                self.error = Some((String::new(), e.to_string()));
                cx.notify();
            }
        }
    }

    fn open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Open".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(p) = paths.first()
            {
                let p = p.display().to_string();
                let _ = this.update_in(cx, |w, window, cx| w.open_argument(&p, window, cx));
            }
        })
        .detach();
    }

    fn edit(&mut self, config: Option<ConnectionConfig>, window: &mut Window, cx: &mut Context<Self>) {
        let form = cx.new(|cx| ConnectForm::new(config, window, cx));
        self._subs.push(cx.subscribe_in(&form, window, |w, _, e: &FormEvent, window, cx| {
            match e {
                FormEvent::Saved(c, connect) => {
                    match w.store.connections.iter_mut().find(|x| x.id == c.id) {
                        Some(x) => *x = c.clone(),
                        None => w.store.connections.push(c.clone()),
                    }
                    w.save_store();
                    w.close_form(window, cx);
                    if *connect {
                        w.connect(c.clone(), window, cx);
                    }
                }
                FormEvent::Cancel => w.close_form(window, cx),
                FormEvent::Deleted(id) => {
                    w.delete(id, cx);
                    w.close_form(window, cx);
                }
            }
            cx.notify();
        }));
        self.form = Some(form);
        window.resize(size(px(WIDTH), px(FORM_HEIGHT)));
        self.modal_focus.focus(window);
        cx.notify();
    }

    fn import_tableplus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        crate::workspace::import::pick(window, cx, move |path, window, cx| {
            let _ = this.update(cx, |w, cx| {
                // The sheet takes focus; its password field, if it has one, then takes it from there.
                w.modal_focus.focus(window);
                let form = cx.new(|cx| ImportForm::new(path, window, cx));
                w._subs.push(cx.subscribe_in(&form, window, |w, _, ImportEvent::Close, window, cx| {
                    w.store = Store::load();
                    w.close_form(window, cx);
                }));
                w.import = Some(form);
                cx.notify();
            });
        });
    }

    fn close_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form = None;
        self.import = None;
        window.resize(size(px(WIDTH), px(HEIGHT)));
        self.filter.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn duplicate(&mut self, c: ConnectionConfig, cx: &mut Context<Self>) {
        let mut copy = ConnectionConfig { name: format!("{} copy", c.name), ..c.clone() };
        copy.id = ConnectionConfig::new(c.engine).id;
        if let Some(p) = config::password(&c.id) {
            let _ = config::set_password(&copy.id, &p);
        }
        let at = self.store.connections.iter().position(|x| x.id == c.id).map_or(self.store.connections.len(), |i| i + 1);
        self.store.connections.insert(at, copy);
        self.save_store();
        cx.notify();
    }

    fn delete(&mut self, id: &str, cx: &mut Context<Self>) {
        self.store.connections.retain(|x| x.id != id);
        self.store.state.remove(id);
        config::forget_password(id);
        self.save_store();
        cx.notify();
    }

    fn open_menu(&mut self, conn: ConnectionConfig, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let build: menu::Builder = std::rc::Rc::new(move |_| {
            let item = |label: &str, g: Glyph, key: &str, f: fn(&mut Welcome, ConnectionConfig, &mut Window, &mut Context<Welcome>)| {
                let (w, c) = (weak.clone(), conn.clone());
                menu::Entry::Item(menu::Item::new(label.to_string()).icon(gpui_component::Icon::new(g)).detail(key.to_string()).on_click(move |window, cx| {
                    let _ = w.update(cx, |w, cx| f(w, c.clone(), window, cx));
                }))
            };
            vec![
                item("Connect", Glyph::Plug, "↵", |w, c, window, cx| w.connect(c, window, cx)),
                item("Edit…", Glyph::Edit, "⌘E", |w, c, window, cx| w.edit(Some(c), window, cx)),
                item("Duplicate", Glyph::Duplicate, "⌘D", |w, c, _, cx| w.duplicate(c, cx)),
                menu::Entry::Separator,
                item("Delete", Glyph::Trash, "", |w, c, _, cx| w.delete(&c.id, cx)),
            ]
        });
        let m = cx.new(|cx| Menu::new(build, None, px(200.), window, cx));
        m.update(cx, |m, cx| m.focus(window, cx));
        cx.subscribe_in(&m, window, |w, _, _: &DismissEvent, window, cx| {
            w.popup = None;
            if w.form.is_none() {
                w.filter.update(cx, |s, cx| s.focus(window, cx));
            }
            cx.notify();
        })
        .detach();
        self.popup = Some((m, position));
        cx.notify();
    }

    fn render_brand(&self, c: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let action = |id: &'static str, glyph: Glyph, glyph_color: u32, label: &'static str, keys: &'static str| {
            row()
                .id(id)
                .w_full()
                .h(px(30.))
                .px(px(10.))
                .gap(px(9.))
                .rounded(px(7.))
                .hover(|s| s.bg(rgb(c.hover)))
                .child(icon(glyph, 14., glyph_color))
                .child(div().flex_1().child(label))
                .child(kbd(c, keys))
        };
        let count = self.store.connections.len();
        col()
            .w(px(260.))
            .h_full()
            .flex_none()
            .bg(rgb(c.chrome))
            .border_r_1()
            .border_color(rgb(c.border))
            .px(px(16.))
            .pt(px(36.))
            .pb(px(14.))
            .child(
                col()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap(px(4.))
                    .child(
                        // Same corner ratio as the Dock icon (185/824 of the body).
                        div()
                            .size(px(76.))
                            .mb(px(12.))
                            .rounded(px(17.))
                            .shadow_lg()
                            .child(img(app_icon()).size_full().rounded(px(17.))),
                    )
                    .child(div().text_size(px(scaled_ui(22.))).font_weight(FontWeight::BOLD).child("Tables"))
                    .child(text(12.).text_color(rgb(c.quiet)).child(concat!("Version ", env!("CARGO_PKG_VERSION"))))
                    .child(
                        col()
                            .w_full()
                            .mt(px(22.))
                            .gap(px(2.))
                            .child(action("new", Glyph::Plus, c.blue, "New connection…", "⌘N").on_click(cx.listener(|w, _, window, cx| w.edit(None, window, cx))))
                            .child(action("open", Glyph::Folder, c.muted, "Open SQLite file…", "⌘O").on_click(cx.listener(|w, _, window, cx| w.open_file(window, cx)))),
                    ),
            )
            .child(text(11.).text_color(rgb(c.quiet)).child(match count {
                0 => "No connections".to_string(),
                1 => "1 connection".to_string(),
                n => format!("{n} connections"),
            }))
    }

    fn render_list(&self, c: Palette, cx: &mut Context<Self>) -> AnyElement {
        if self.store.connections.is_empty() {
            return col()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .p(px(30.))
                .text_color(rgb(c.muted))
                .child(
                    div()
                        .size(px(54.))
                        .mb(px(6.))
                        .rounded(px(14.))
                        .border_1()
                        .border_dashed()
                        .border_color(rgb(c.popover_border))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(Glyph::Database, 24., c.quiet)),
                )
                .child(text(14.).font_weight(FontWeight::SEMIBOLD).text_color(rgb(c.text)).child("No connections yet"))
                .child("Add a PostgreSQL server or open a SQLite file to get started.")
                .child(
                    button(c, "empty-new", Some(Glyph::Plus), Some("New connection"), Tone::Primary)
                        .mt(px(8.))
                        .h(px(28.))
                        .px(px(12.))
                        .on_click(cx.listener(|w, _, window, cx| w.edit(None, window, cx))),
                )
                .into_any_element();
        }
        let rows = self.rows(cx);
        let searching = !self.filter.read(cx).value().trim().is_empty();
        let mut items: Vec<AnyElement> = vec![];
        let mut group = "";
        for (i, r) in rows.iter().enumerate() {
            let g = group_of(&rows, i, searching);
            if g != group {
                group = g;
                items.push(
                    div()
                        .px(px(8.))
                        .pt(px(10.))
                        .pb(px(4.))
                        .text_size(px(scaled_ui(11.)))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(c.quiet))
                        .child(g.to_uppercase())
                        .into_any_element(),
                );
            }
            items.push(self.render_row(c, i, r, cx).into_any_element());
        }
        if rows.is_empty() {
            items.push(
                col()
                    .items_center()
                    .pt(px(60.))
                    .gap(px(4.))
                    .text_color(rgb(c.muted))
                    .child(text(14.).font_weight(FontWeight::SEMIBOLD).text_color(rgb(c.text)).child("No matches"))
                    .child(format!("Nothing matches “{}”.", self.filter.read(cx).value().trim()))
                    .into_any_element(),
            );
        }
        let hint = |keys: &[&str], label: &str| row().gap(px(4.)).children(keys.iter().map(|k| kbd(c, k))).child(div().ml(px(2.)).child(label.to_string()));
        col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                row()
                    .gap(px(6.))
                    .px(px(12.))
                    .pt(px(12.))
                    .pb(px(8.))
                    .child(
                        row()
                            .flex_1()
                            .h(px(30.))
                            .px(px(9.))
                            .gap(px(7.))
                            .rounded(px(7.))
                            .bg(rgb(c.chrome))
                            .border_1()
                            .border_color(rgb(c.border))
                            .child(icon(Glyph::Search, 14., c.quiet))
                            .child(div().flex_1().child(Input::new(&self.filter).appearance(false))),
                    )
                    .child(
                        div()
                            .id("add")
                            .size(px(30.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(7.))
                            .border_1()
                            .border_color(rgb(c.border))
                            .bg(rgb(c.chrome))
                            .hover(|s| s.bg(rgb(c.hover)))
                            .child(icon(Glyph::Plus, 14., c.muted))
                            .on_click(cx.listener(|w, _, window, cx| w.edit(None, window, cx))),
                    ),
            )
            .when_some(self.error.as_ref().filter(|(id, _)| id.is_empty()), |s, (_, e)| {
                s.child(
                    row()
                        .mx(px(12.))
                        .mb(px(6.))
                        .px(px(10.))
                        .py(px(7.))
                        .gap(px(8.))
                        .rounded(px(7.))
                        .bg(rgb(c.red_tint))
                        .text_size(px(scaled_ui(12.)))
                        .child(icon(Glyph::Warn, 14., c.red))
                        .child(div().flex_1().child(e.clone())),
                )
            })
            .child(div().id("conns").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.scroll).flex().flex_col().px(px(8.)).pb(px(10.)).children(items))
            .child(
                row()
                    .gap(px(14.))
                    .px(px(14.))
                    .py(px(7.))
                    .border_t_1()
                    .border_color(rgb(c.border))
                    .text_size(px(scaled_ui(11.)))
                    .text_color(rgb(c.quiet))
                    .child(hint(&["↑", "↓"], "select"))
                    .child(hint(&["↵"], "connect"))
                    .child(hint(&["⌘E"], "edit"))
                    .child(hint(&["⌘N"], "new")),
            )
            .into_any_element()
    }

    fn render_row(&self, c: Palette, i: usize, r: &Row, cx: &mut Context<Self>) -> impl IntoElement {
        let conn = &r.config;
        let selected = i == self.selected;
        let busy = self.connecting.as_ref().is_some_and(|(id, _)| *id == conn.id);
        let error = self.error.as_ref().filter(|(id, _)| *id == conn.id).map(|(_, e)| e.clone());
        let (env, env_bg) = env_color(c, conn.env);
        let env_label = match conn.env {
            config::Env::Development => "DEV",
            config::Env::Staging => "STAGING",
            config::Env::Production => "PROD",
        };
        let group = SharedString::from(format!("row-{}", conn.id));
        let (c1, c2, c3) = (conn.clone(), conn.clone(), conn.clone());
        row()
            .id(SharedString::from(format!("conn-{}", conn.id)))
            .group(group.clone())
            .gap(px(10.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(8.))
            .when(selected, |s| s.bg(rgb(c.selected)))
            .when(!selected, |s| s.hover(|s| s.bg(rgb(c.hover))))
            .child(engine_badge(conn.engine, 30.).rounded(px(8.)))
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .child(row().gap(px(6.)).child(div().font_weight(FontWeight::SEMIBOLD).truncate().child(conn.name.clone())).child(chip(env_label, env, env_bg)))
                    .child(match error {
                        Some(e) => text(11.5).text_color(rgb(c.red)).child(e).into_any_element(),
                        None => mono_text(11.).truncate().text_color(rgb(c.muted)).child(describe(conn)).into_any_element(),
                    }),
            )
            .when(busy, |s| s.child(text(11.).text_color(rgb(c.blue)).child("Connecting…")))
            .when(!busy, |s| {
                s.child(
                    div()
                        .text_size(px(scaled_ui(11.)))
                        .text_color(rgb(c.quiet))
                        .when(!selected, |s| s.group_hover(group.clone(), |s| s.invisible()))
                        .when(selected, |s| s.invisible())
                        .child(r.when.clone()),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("more-{}", conn.id)))
                        .size(px(22.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.))
                        .when(!selected, |s| s.invisible().group_hover(group.clone(), |s| s.visible()))
                        .hover(|s| s.bg(rgb(c.popover_hover)))
                        .child(icon(Glyph::Dots, 14., c.quiet))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |w, e: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            w.selected = i;
                            w.open_menu(c3.clone(), e.position, window, cx);
                        })),
                )
            })
            .on_click(cx.listener(move |w, e: &ClickEvent, window, cx| {
                w.selected = i;
                if e.click_count() >= 2 {
                    w.connect(c1.clone(), window, cx);
                }
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |w, e: &MouseDownEvent, window, cx| {
                w.selected = i;
                w.open_menu(c2.clone(), e.position, window, cx);
            }))
    }
}

fn group_of(rows: &[Row], i: usize, searching: bool) -> &'static str {
    if searching {
        "Results"
    } else if rows[i].recent {
        "Recent"
    } else if rows.iter().any(|r| r.recent) {
        "All"
    } else {
        "Connections"
    }
}

/// Where a connection points: `host:port · database`, or the file.
fn describe(c: &ConnectionConfig) -> String {
    match c.engine {
        Engine::Sqlite => c.address(),
        _ if c.database.is_empty() => c.address(),
        _ => format!("{} · {}", c.address(), c.database),
    }
}

/// "2 min ago", "Yesterday", "Nov 12".
fn ago(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(then) = Local.timestamp_opt(ts, 0).single() else { return String::new() };
    let now = Local::now();
    let secs = (now - then).num_seconds().max(0);
    let days = (now.date_naive() - then.date_naive()).num_days();
    match () {
        _ if secs < 60 => "Just now".into(),
        _ if secs < 3600 => format!("{} min ago", secs / 60),
        _ if days == 0 => format!("{} h ago", secs / 3600),
        _ if days == 1 => "Yesterday".into(),
        _ if days < 7 => format!("{days} days ago"),
        _ if then.format("%Y").to_string() == now.format("%Y").to_string() => then.format("%b %-d").to_string(),
        _ => then.format("%b %-d, %Y").to_string(),
    }
}

impl Render for Welcome {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if window.focused(cx).is_none() {
            self.focus.focus(window);
        }
        let c = palette(cx);
        let n = self.rows(cx).len();
        if n > 0 && self.selected >= n {
            self.selected = n - 1;
        }
        row()
            .id("welcome")
            .key_context("Welcome")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .bg(rgb(c.background))
            .text_color(rgb(c.text))
            .font_family(crate::theme::ui_font())
            .text_size(px(scaled_ui(13.)))
            .capture_key_down(cx.listener(Self::key))
            .on_action(cx.listener(|w, _: &NewConnection, window, cx| w.edit(None, window, cx)))
            .on_action(cx.listener(|w, _: &OpenFile, window, cx| w.open_file(window, cx)))
            .on_action(cx.listener(|w, _: &ImportTablePlus, window, cx| w.import_tableplus(window, cx)))
            .on_action(cx.listener(|w, _: &EditSelected, window, cx| {
                if w.form.is_none() && let Some(c) = w.selected_config(cx) {
                    w.edit(Some(c), window, cx);
                }
            }))
            .on_action(cx.listener(|w, _: &DuplicateSelected, _, cx| {
                if w.form.is_none() && let Some(c) = w.selected_config(cx) {
                    w.duplicate(c, cx);
                }
            }))
            // The title bar is transparent: this strip over the brand panel drags the window.
            .child(div().absolute().top_0().left_0().w(px(260.)).h(px(32.)).child(TitleBar::new().bg(transparent_black()).border_0()))
            .child(self.render_brand(c, cx))
            .child(self.render_list(c, cx))
            .when_some(self.form.clone(), |s, form| {
                s.child(
                    div()
                        .key_context("Modal")
                        .track_focus(&self.modal_focus)
                        .absolute()
                        .inset_0()
                        .on_action(cx.listener(|w, _: &ConfirmModal, window, cx| {
                            if let Some(f) = w.form.clone() {
                                f.update(cx, |f, cx| f.submit(window, cx));
                            }
                        }))
                        .on_action(cx.listener(|w, _: &CancelModal, window, cx| w.close_form(window, cx)))
                        .child(modal_fill(c, 560., form)),
                )
            })
            .when_some(self.import.clone(), |s, form| {
                s.child(
                    div()
                        .key_context("Modal")
                        .track_focus(&self.modal_focus)
                        .absolute()
                        .inset_0()
                        .on_action(cx.listener(|w, _: &ConfirmModal, window, cx| {
                            if let Some(f) = w.import.clone() {
                                f.update(cx, |f, cx| f.submit(window, cx));
                            }
                        }))
                        .on_action(cx.listener(|w, _: &CancelModal, window, cx| w.close_form(window, cx)))
                        .child(modal_fill(c, 480., form)),
                )
            })
            .when_some(self.popup.clone(), |s, (m, at)| s.child(anchored().anchor(Corner::TopLeft).position(at).snap_to_window_with_margin(px(8.)).child(m)))
    }
}
