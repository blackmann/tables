//! A SQL tab: an editor with schema-aware completion over a results pane with
//! one tab per result set, a message log, and the query plan.
use crate::completion::{SqlCompletion, format_keywords};
use crate::db::{self, Outcome, Plan, Value};
use crate::grid::{DataGrid, GridColumn, GridEvent};
use crate::icons::Glyph;
use crate::menu::{Entry, Item};
use crate::session::Session;
use crate::tab::TabEvent;
use crate::theme::{Palette, mono, palette, scaled_mono, scaled_ui};
use crate::ui::*;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::Icon;
use gpui_component::input::{Input, InputEvent, InputState};
use std::rc::Rc;
use std::time::Instant;

actions!(sql, [RunStatement, RunAll, ExplainStatement, FormatSql, CancelQuery]);

pub fn bind_keys(cx: &mut App) {
    let s = Some("SqlView");
    cx.bind_keys([
        KeyBinding::new("cmd-enter", RunStatement, s),
        KeyBinding::new("cmd-shift-enter", RunAll, s),
        KeyBinding::new("cmd-e", ExplainStatement, s),
        KeyBinding::new("cmd-shift-f", FormatSql, s),
        KeyBinding::new("ctrl-c", CancelQuery, s),
        // Inside the editor too, where the input would otherwise take ⌘↵.
        KeyBinding::new("cmd-enter", RunStatement, Some("SqlView > Input")),
        KeyBinding::new("cmd-shift-enter", RunAll, Some("SqlView > Input")),
    ]);
}

#[derive(Clone, Copy, PartialEq)]
pub enum Pane {
    Result(usize),
    Messages,
    Explain,
}

#[derive(Clone)]
struct LogLine {
    at: String,
    text: String,
    tone: u8, // 0 note, 1 ok, 2 error
}

pub struct SqlView {
    pub session: Entity<Session>,
    pub title: String,
    pub editor: Entity<InputState>,
    pub dirty: bool,
    /// The text as last saved, to tell whether it changed since.
    saved_text: String,
    outcomes: Vec<(String, Outcome)>,
    error: Option<(String, String)>,
    pane: Pane,
    pub grid: Entity<DataGrid>,
    log: Vec<LogLine>,
    plan: Option<Plan>,
    pub running: Option<Instant>,
    editor_height: Option<f32>,
    dragging: Option<(Pixels, f32)>,
    focus: FocusHandle,
    _tasks: Vec<Task<()>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<TabEvent> for SqlView {}

impl SqlView {
    pub fn new(session: Entity<Session>, title: String, text: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            let mut s = InputState::new(window, cx).code_editor("sql").line_number(true).soft_wrap(false);
            s.lsp.completion_provider = Some(Rc::new(SqlCompletion { session: session.clone() }));
            s
        });
        editor.update(cx, |s, cx| s.set_value(text.clone(), window, cx));
        let grid = cx.new(DataGrid::new);
        let subs = vec![
            cx.subscribe(&editor, |t: &mut SqlView, e, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    let dirty = e.read(cx).value().as_ref() != t.saved_text;
                    if dirty != t.dirty {
                        t.dirty = dirty;
                        cx.emit(TabEvent::Changed);
                    }
                }
            }),
            cx.subscribe(&grid, |_, _, e: &GridEvent, cx| match e {
                GridEvent::Selected => cx.emit(TabEvent::Changed),
                GridEvent::Copied(m) => cx.emit(TabEvent::Toast(m.clone())),
                GridEvent::OpenForeign { table, column, value } => cx.emit(TabEvent::OpenTable { table: table.clone(), filter: Some((column.clone(), value.clone())) }),
                _ => {}
            }),
        ];
        SqlView {
            session,
            title,
            editor,
            dirty: false,
            saved_text: text,
            outcomes: vec![],
            error: None,
            pane: Pane::Result(0),
            grid,
            log: vec![],
            plan: None,
            running: None,
            editor_height: None,
            dragging: None,
            focus: cx.focus_handle(),
            _tasks: vec![],
            _subs: subs,
        }
    }

    pub fn focus_editor(&self, window: &mut Window, cx: &mut App) {
        self.editor.update(cx, |s, cx| s.focus(window, cx));
    }

    pub fn text(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }

    pub fn mark_saved(&mut self, cx: &mut Context<Self>) {
        self.saved_text = self.text(cx);
        self.dirty = false;
        cx.emit(TabEvent::Changed);
    }

    pub fn insert(&mut self, sql: &str, window: &mut Window, cx: &mut Context<Self>) {
        let cur = self.text(cx);
        let next = if cur.trim().is_empty() { sql.to_string() } else { format!("{}\n\n{sql}", cur.trim_end()) };
        self.editor.update(cx, |s, cx| s.set_value(next, window, cx));
    }

    /// The selection if there is one, else the statement under the cursor.
    fn target(&self, window: &mut Window, cx: &mut Context<Self>) -> String {
        let text = self.text(cx);
        let selected = self.editor.update(cx, |s, cx| {
            let sel = s.selected_text_range(true, window, cx)?;
            if sel.range.is_empty() {
                return None;
            }
            s.text_for_range(sel.range, &mut None, window, cx)
        });
        if let Some(s) = selected.filter(|s| db::has_code(s)) {
            return s;
        }
        let cursor = self.editor.read(cx).cursor();
        db::statement_at(&text, cursor).map(|r| text[r].to_string()).unwrap_or_default()
    }

    fn log(&mut self, text: impl Into<String>, tone: u8) {
        let at = chrono::Local::now().format("%H:%M:%S").to_string();
        self.log.push(LogLine { at, text: text.into(), tone });
    }

    pub fn run(&mut self, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        let sql = if all { self.text(cx) } else { self.target(window, cx) };
        if !db::has_code(&sql) {
            cx.emit(TabEvent::Toast("Nothing to run here".into()));
            return;
        }
        let production = self.session.read(cx).config.env == crate::db::config::Env::Production;
        if production && !db::split_statements(&sql).iter().all(|s| db::reads_only(s)) {
            cx.emit(TabEvent::ConfirmRun(sql));
            return;
        }
        self.execute(sql, cx);
    }

    /// Run `sql` as is, with no production check.
    pub fn execute(&mut self, sql: String, cx: &mut Context<Self>) {
        cx.emit(TabEvent::Ran(sql.trim().to_string()));
        let start = Instant::now();
        self.running = Some(start);
        self.error = None;
        let one_line = sql.split_whitespace().collect::<Vec<_>>().join(" ");
        self.log(crate::grid::ellipsize(&one_line, 140), 0);
        cx.emit(TabEvent::Changed);
        cx.notify();
        let task = self.session.read(cx).run(cx, move |d| d.execute(&sql));
        self._tasks.push(cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |t, cx| {
                t.running = None;
                let (done, err) = match res {
                    Ok(done) => (done, None),
                    Err((done, stmt, e)) => (done, Some((stmt, e.to_string()))),
                };
                for (_, o) in &done {
                    match o {
                        Outcome::Rows(p) => t.log(format!("{} {} in {}", fmt_int(p.rows.len() as i64), if p.rows.len() == 1 { "row" } else { "rows" }, fmt_ms(p.elapsed)), 1),
                        Outcome::Done { command, rows, elapsed } => {
                            t.log(format!("{command}{} — {}", rows.map(|n| format!(" {n}")).unwrap_or_default(), fmt_ms(*elapsed)), 1)
                        }
                    }
                }
                if let Some((stmt, e)) = &err {
                    t.log(format!("ERROR: {}", e.lines().next().unwrap_or("")), 2);
                    t.error = Some((stmt.clone(), e.clone()));
                }
                // Writes may have changed the schema or row counts.
                if done.iter().any(|(_, o)| matches!(o, Outcome::Done { command, .. } if matches!(command.as_str(), "CREATE" | "ALTER" | "DROP" | "TRUNCATE"))) {
                    t.session.update(cx, |s, cx| s.refresh(cx));
                }
                t.outcomes = done;
                let first_rows = t.outcomes.iter().position(|(_, o)| matches!(o, Outcome::Rows(_)));
                t.pane = if t.error.is_some() { Pane::Result(0) } else { first_rows.map(Pane::Result).unwrap_or(Pane::Messages) };
                t.show_result(cx);
                cx.emit(TabEvent::Changed);
                cx.notify();
            });
        }));
    }

    pub fn explain(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        let sql = self.target(window, cx);
        if !db::has_code(&sql) {
            return;
        }
        self.running = Some(Instant::now());
        cx.notify();
        let task = self.session.read(cx).run(cx, move |d| d.explain(&sql));
        self._tasks.push(cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |t, cx| {
                t.running = None;
                match res {
                    Ok(p) => {
                        t.log(format!("EXPLAIN — {} nodes", p.nodes.len()), 1);
                        t.plan = Some(p);
                    }
                    Err(e) => {
                        t.log(format!("ERROR: {e}"), 2);
                        t.plan = None;
                        t.error = Some(("EXPLAIN".into(), e.to_string()));
                    }
                }
                t.pane = Pane::Explain;
                cx.emit(TabEvent::Changed);
                cx.notify();
            });
        }));
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.running.is_some() {
            self.session.read(cx).cancel();
            self.log("Cancel requested", 0);
            cx.notify();
        }
    }

    fn show_result(&mut self, cx: &mut Context<Self>) {
        let Pane::Result(i) = self.pane else { return };
        let page = self.outcomes.get(i).and_then(|(_, o)| if let Outcome::Rows(p) = o { Some(p.clone()) } else { None });
        let Some(page) = page else { return };
        let session = self.session.read(cx);
        // Columns that are a table's own keep their key and link marks.
        let columns: Vec<GridColumn> = page
            .columns
            .iter()
            .map(|rc| {
                let mut g = GridColumn::plain(&rc.name, &rc.type_name);
                if let Some(info) = session.details.values().flat_map(|d| d.columns.iter()).find(|c| c.name == rc.name && c.foreign.is_some()) {
                    g.fk = info.foreign.clone();
                }
                g
            })
            .collect();
        self.grid.update(cx, |g, cx| {
            g.editable = false;
            g.read_only_reason = Some("Query results are read-only — open the table to edit rows".into());
            g.first_number = 1;
            g.sort = None;
            g.set_data(columns, page.rows, cx);
        });
    }

    pub fn format(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t = format_keywords(&self.text(cx));
        self.editor.update(cx, |s, cx| s.set_value(t, window, cx));
    }

    fn export_menu(&self, cx: &App) -> crate::menu::Builder {
        let g = self.grid.read(cx);
        let cols: Vec<String> = g.columns.iter().map(|c| c.name.clone()).collect();
        let rows: Vec<Vec<Value>> = g.rows.iter().map(|r| r.values.clone()).collect();
        Rc::new(move |_| {
            let (c1, r1, c2, r2) = (cols.clone(), rows.clone(), cols.clone(), rows.clone());
            let (c3, r3) = (cols.clone(), rows.clone());
            vec![
                Entry::Item(Item::new("Copy as CSV").icon(Icon::new(Glyph::Copy)).on_click(move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(crate::export::csv(&c1, &r1))))),
                Entry::Item(Item::new("Copy as JSON").icon(Icon::new(Glyph::Copy)).on_click(move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(crate::export::json(&c2, &r2))))),
                Entry::Separator,
                Entry::Item(Item::new("Save as CSV…").icon(Icon::new(Glyph::Download)).on_click(move |_, cx| {
                    let csv = crate::export::csv(&c3, &r3);
                    let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Downloads");
                    let rx = cx.prompt_for_new_path(&dir, Some("result.csv"));
                    cx.spawn(async move |_| {
                        if let Ok(Ok(Some(path))) = rx.await {
                            let _ = std::fs::write(path, csv);
                        }
                    })
                    .detach();
                })),
            ]
        })
    }

    pub fn status(&self) -> Option<String> {
        if let Some(s) = self.running {
            return Some(format!("Running… {}", fmt_ms(s.elapsed())));
        }
        None
    }

    fn render_toolbar(&self, c: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.running.is_some();
        row()
            .h(px(TOOLBAR))
            .flex_shrink_0()
            .px(px(10.))
            .gap(px(6.))
            .border_b_1()
            .border_color(rgb(c.border))
            .child(if running {
                button(c, "stop", Some(Glyph::Stop), Some("Stop"), Tone::Danger)
                    .child(kbd(c, "⌃C").border_color(hx(c.primary_text).opacity(0.3)).text_color(rgb(c.primary_text)))
                    .on_click(cx.listener(|t, _, _, cx| t.cancel(cx)))
            } else {
                button(c, "run", Some(Glyph::Play), Some("Run"), Tone::Primary)
                    .child(kbd(c, "⌘↵").border_color(hx(c.primary_text).opacity(0.3)).text_color(rgb(c.primary_text)))
                    .on_click(cx.listener(|t, _, window, cx| t.run(false, window, cx)))
            })
            .child(button(c, "run-all", None, Some("Run all"), Tone::Ghost).on_click(cx.listener(|t, _, window, cx| t.run(true, window, cx))))
            .child(button(c, "explain", Some(Glyph::Explain), Some("Explain"), Tone::Ghost).on_click(cx.listener(|t, _, window, cx| t.explain(window, cx))))
            .child(button(c, "format", Some(Glyph::Format), Some("Format"), Tone::Ghost).on_click(cx.listener(|t, _, window, cx| t.format(window, cx))))
            .child(div().flex_1())
            .child(button(c, "history", Some(Glyph::History), Some("History"), Tone::Ghost).on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, e: &MouseDownEvent, _, cx| cx.emit(TabEvent::HistoryMenu(e.position))),
            ))
            .child(button(c, "save", Some(Glyph::Save), Some("Save"), Tone::Ghost).on_click(cx.listener(|t, _, _, cx| {
                let sql = t.text(cx);
                cx.emit(TabEvent::Save(sql));
            })))
    }

    fn render_results_head(&self, c: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let rtab = |id: SharedString, g: Glyph, label: String, n: Option<String>, on: bool, err: bool| {
            row()
                .id(id)
                .h(px(24.))
                .px(px(9.))
                .gap(px(6.))
                .rounded(px(6.))
                .text_size(px(scaled_ui(12.)))
                .text_color(rgb(if err { c.red } else if on { c.text } else { c.muted }))
                .when(on, |s| s.bg(rgb(c.selected)))
                .hover(|s| s.bg(rgb(c.hover)))
                .child(icon(g, 12., if err { c.red } else { c.muted }))
                .child(label)
                .when_some(n, |s, n| s.child(div().text_size(px(scaled_ui(10.5))).text_color(rgb(c.quiet)).child(n)))
        };
        let mut tabs: Vec<AnyElement> = vec![];
        let row_sets: Vec<(usize, usize)> = self.outcomes.iter().enumerate().filter_map(|(i, (_, o))| if let Outcome::Rows(p) = o { Some((i, p.rows.len())) } else { None }).collect();
        if self.error.is_some() && !matches!(self.pane, Pane::Explain) {
            tabs.push(rtab("res-err".into(), Glyph::Warn, "Error".into(), None, matches!(self.pane, Pane::Result(_)), true).into_any_element());
        }
        let multi = row_sets.len() > 1;
        for (k, (i, n)) in row_sets.iter().enumerate() {
            let i = *i;
            let label = if multi { format!("Result {}", k + 1) } else { "Result".into() };
            tabs.push(
                rtab(format!("res-{i}").into(), Glyph::Table, label, Some(fmt_int(*n as i64)), self.pane == Pane::Result(i) && self.error.is_none(), false)
                    .on_click(cx.listener(move |t, _, _, cx| {
                        t.pane = Pane::Result(i);
                        t.error = None;
                        t.show_result(cx);
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        if row_sets.is_empty() && self.error.is_none() {
            tabs.push(rtab("res-none".into(), Glyph::Table, "Result".into(), None, matches!(self.pane, Pane::Result(_)), false).into_any_element());
        }
        tabs.push(
            rtab("res-msg".into(), Glyph::Terminal, "Messages".into(), Some(self.log.len().to_string()), self.pane == Pane::Messages, false)
                .on_click(cx.listener(|t, _, _, cx| {
                    t.pane = Pane::Messages;
                    cx.notify();
                }))
                .into_any_element(),
        );
        tabs.push(
            rtab("res-plan".into(), Glyph::Explain, "Explain".into(), None, self.pane == Pane::Explain, false)
                .on_click(cx.listener(|t, _, _, cx| {
                    t.pane = Pane::Explain;
                    cx.notify();
                }))
                .into_any_element(),
        );
        let meta = match self.pane {
            Pane::Result(i) if self.error.is_none() => self.outcomes.get(i).map(|(_, o)| match o {
                Outcome::Rows(p) => (format!("{} rows", fmt_int(p.rows.len() as i64)), fmt_ms(p.elapsed)),
                Outcome::Done { command, rows, elapsed } => (format!("{command}{}", rows.map(|n| format!(" {n}")).unwrap_or_default()), fmt_ms(*elapsed)),
            }),
            _ => None,
        };
        let has_rows = matches!(self.pane, Pane::Result(i) if matches!(self.outcomes.get(i), Some((_, Outcome::Rows(_)))));
        row()
            .h(px(34.))
            .flex_shrink_0()
            .px(px(8.))
            .gap(px(2.))
            .bg(rgb(c.chrome))
            .border_b_1()
            .border_color(rgb(c.border))
            .children(tabs)
            .child(div().flex_1())
            .when_some(meta, |s, (a, b)| {
                s.child(
                    row()
                        .gap(px(10.))
                        .mr(px(6.))
                        .text_size(px(scaled_ui(12.)))
                        .text_color(rgb(c.quiet))
                        .child(a)
                        .child(row().gap(px(4.)).child(icon(Glyph::Bolt, 12., c.quiet)).child(b)),
                )
            })
            .when(has_rows, |s| {
                s.child(button(c, "res-export", Some(Glyph::Download), Some("Export"), Tone::Ghost).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|t, e: &MouseDownEvent, _, cx| {
                        let build = t.export_menu(cx);
                        cx.emit(TabEvent::Menu { position: e.position, build, width: 200. });
                    }),
                ))
            })
    }

    fn render_body(&self, c: Palette, _cx: &mut Context<Self>) -> AnyElement {
        let empty = |big: &str, small: &str| {
            col()
                .size_full()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .text_color(rgb(c.quiet))
                .child(div().text_color(rgb(c.muted)).child(big.to_string()))
                .child(small.to_string())
                .into_any_element()
        };
        match self.pane {
            Pane::Messages => div()
                .id("messages")
                .size_full()
                .overflow_y_scroll()
                .px(px(14.))
                .py(px(10.))
                .font_family(mono())
                .text_size(px(scaled_mono(12.)))
                .children(self.log.iter().map(|l| {
                    row()
                        .items_start()
                        .gap(px(14.))
                        .py(px(2.))
                        .child(div().flex_shrink_0().text_color(rgb(c.quiet)).child(l.at.clone()))
                        .child(div().text_color(rgb(match l.tone {
                            1 => c.green,
                            2 => c.red,
                            _ => c.muted,
                        })).child(l.text.clone()))
                }))
                .into_any_element(),
            Pane::Explain => match &self.plan {
                None => empty("No plan yet", "Explain (⌘E) shows how the database runs the statement under the cursor."),
                Some(p) => render_plan(c, p),
            },
            Pane::Result(i) => {
                if let Some((stmt, e)) = &self.error {
                    return col()
                        .m(px(18.))
                        .p(px(14.))
                        .gap(px(8.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(hx(c.red).opacity(0.4))
                        .bg(rgb(c.red_tint))
                        .child(row().gap(px(8.)).text_color(rgb(c.red)).font_weight(FontWeight::SEMIBOLD).child(icon(Glyph::Warn, 14., c.red)).child("ERROR"))
                        .child(mono_text(12.).child(e.clone()))
                        .child(mono_text(11.5).text_color(rgb(c.muted)).child(crate::grid::ellipsize(&stmt.split_whitespace().collect::<Vec<_>>().join(" "), 300)))
                        .into_any_element();
                }
                match self.outcomes.get(i) {
                    Some((_, Outcome::Rows(_))) => self.grid.clone().into_any_element(),
                    Some((_, Outcome::Done { command, rows, elapsed })) => empty(
                        &format!("{command}{}", rows.map(|n| format!(" — {n} {}", if n == 1 { "row" } else { "rows" })).unwrap_or_default()),
                        &format!("Ran in {}", fmt_ms(*elapsed)),
                    ),
                    None if self.running.is_some() => empty("Running…", "⌃C or Stop cancels it"),
                    None => empty("No results yet", "⌘↵ runs the statement under the cursor, or the selection. ⌘⇧↵ runs everything."),
                }
            }
        }
    }
}

fn render_plan(c: Palette, p: &Plan) -> AnyElement {
    let max = p.nodes.iter().filter_map(|n| n.self_ms).fold(0f64, f64::max);
    let timed = p.nodes.iter().any(|n| n.self_ms.is_some());
    let header = row()
        .px(px(16.))
        .py(px(6.))
        .gap(px(12.))
        .border_b_1()
        .border_color(rgb(c.border))
        .text_size(px(scaled_ui(11.)))
        .text_color(rgb(c.quiet))
        .child(div().flex_1().child("Node"))
        .child(div().w(px(90.)).text_right().child("Rows"))
        .when(timed, |s| s.child(div().w(px(200.)).child("Self time")).child(div().w(px(80.)).text_right().child("ms")));
    div()
        .id("plan")
        .size_full()
        .overflow_y_scroll()
        .when_some(p.advice.clone(), |s, (title, sql)| {
            s.child(
                row()
                    .items_start()
                    .m(px(12.))
                    .px(px(14.))
                    .py(px(10.))
                    .gap(px(10.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(hx(c.amber).opacity(0.35))
                    .bg(rgb(c.amber_tint))
                    .child(icon(Glyph::Warn, 14., c.amber))
                    .child(
                        col()
                            .flex_1()
                            .gap(px(4.))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                            .child(text(12.).text_color(rgb(c.muted)).child("An index on the filtered columns would let the database skip most of these rows:"))
                            .child(mono_text(11.5).child(sql.clone())),
                    )
                    .child(button(c, "copy-advice", Some(Glyph::Copy), Some("Copy"), Tone::Plain).on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(sql.clone())))),
            )
        })
        .child(header)
        .children(p.nodes.iter().enumerate().map(|(i, n)| {
            let hot = max > 0. && n.self_ms.is_some_and(|m| m >= max * 0.5) && p.nodes.len() > 1;
            let width = n.self_ms.map(|m| if max > 0. { (m / max).max(0.015) } else { 0. }).unwrap_or(0.);
            row()
                .id(("plan-node", i))
                .px(px(16.))
                .py(px(6.))
                .gap(px(12.))
                .border_b_1()
                .border_color(rgb(c.row_border))
                .hover(|s| s.bg(rgb(c.hover)))
                .child(
                    row()
                        .flex_1()
                        .min_w_0()
                        .gap(px(8.))
                        .pl(px(n.depth as f32 * 18.))
                        .when(n.depth > 0, |s| s.child(div().text_color(rgb(c.quiet)).font_family(mono()).child("└")))
                        .child(div().flex_shrink_0().font_weight(FontWeight::MEDIUM).child(n.operation.clone()))
                        .when_some(n.relation.clone(), |s, r| s.child(mono_text(11.5).flex_shrink_0().text_color(rgb(c.amber)).child(r)))
                        .child(mono_text(11.).text_color(rgb(c.quiet)).truncate().child(n.detail.clone()))
                        .when(hot, |s| {
                            s.child(row().flex_shrink_0().gap(px(4.)).px(px(6.)).rounded(px(4.)).bg(rgb(c.amber_tint)).text_color(rgb(c.amber)).text_size(px(scaled_ui(11.))).child(icon(Glyph::Warn, 11., c.amber)).child("hot"))
                        }),
                )
                .child(div().w(px(90.)).text_right().text_color(rgb(c.muted)).child(n.rows.map(|r| fmt_int(r as i64)).unwrap_or_default()))
                .when(timed, |s| s.child(
                    div()
                        .w(px(200.))
                        .h(px(6.))
                        .rounded(px(3.))
                        .bg(rgb(c.hover))
                        .child(div().h_full().rounded(px(3.)).w(relative(width as f32)).bg(rgb(if hot { c.amber } else { c.blue }))),
                ).child(div().w(px(80.)).text_right().text_color(rgb(c.muted)).child(n.self_ms.map(|m| format!("{m:.1}")).unwrap_or_else(|| "—".into()))))
        }))
        .child(div().px(px(16.)).py(px(10.)).text_size(px(scaled_ui(12.))).text_color(rgb(c.quiet)).child(p.summary.clone()))
        .into_any_element()
}

impl Focusable for SqlView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SqlView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = palette(cx);
        if self.running.is_some() {
            // Keep the elapsed time ticking in the status bar.
            window.request_animation_frame();
        }
        let editor_h = self.editor_height;
        col()
            .id("sql-view")
            .key_context("SqlView")
            .track_focus(&self.focus)
            .size_full()
            .on_action(cx.listener(|t, _: &RunStatement, window, cx| t.run(false, window, cx)))
            .on_action(cx.listener(|t, _: &RunAll, window, cx| t.run(true, window, cx)))
            .on_action(cx.listener(|t, _: &ExplainStatement, window, cx| t.explain(window, cx)))
            .on_action(cx.listener(|t, _: &FormatSql, window, cx| t.format(window, cx)))
            .on_action(cx.listener(|t, _: &CancelQuery, _, cx| t.cancel(cx)))
            .on_mouse_move(cx.listener(|t, e: &MouseMoveEvent, _, cx| {
                if let Some((y0, h0)) = t.dragging {
                    if e.pressed_button != Some(MouseButton::Left) {
                        t.dragging = None;
                        return;
                    }
                    t.editor_height = Some((h0 + f(e.position.y - y0)).max(80.));
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|t, _, _, _| t.dragging = None))
            .child(self.render_toolbar(c, cx))
            .child(
                div()
                    .when_some(editor_h, |s, h| s.h(px(h)))
                    .when(editor_h.is_none(), |s| s.h(relative(0.42)))
                    .min_h(px(80.))
                    .flex_shrink_0()
                    .bg(rgb(c.background))
                    .py(px(6.))
                    .font_family(mono())
                    .text_size(px(scaled_mono(13.)))
                    .child(Input::new(&self.editor).appearance(false).h_full()),
            )
            .child(
                div()
                    .id("splitter")
                    .h(px(7.))
                    .my(px(-3.))
                    .flex_shrink_0()
                    .relative()
                    .cursor_row_resize()
                    .child(div().absolute().left_0().right_0().top(px(3.)).h(px(1.)).bg(rgb(c.border)))
                    .hover(|s| s.bg(hx(c.blue).opacity(0.3)))
                    .on_mouse_down(MouseButton::Left, cx.listener(|t, e: &MouseDownEvent, window, cx| {
                        let h = t.editor_height.unwrap_or(f(window.viewport_size().height) * 0.3);
                        t.dragging = Some((e.position.y, h));
                        cx.stop_propagation();
                    })),
            )
            .child(self.render_results_head(c, cx))
            .child(div().flex_1().min_h_0().child(self.render_body(c, cx)))
    }
}
