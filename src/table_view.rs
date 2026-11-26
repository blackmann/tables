//! A table tab: its rows (filtered, sorted, paged, editable) or its structure.
use crate::db::{self, Filter, Op, Sort, Target, Value};
use crate::grid::{DataGrid, GridColumn, GridEvent};
use crate::icons::Glyph;
use crate::menu::{Entry, Item};
use crate::session::{Session, SessionEvent};
use crate::tab::TabEvent;
use crate::theme::{Palette, mono, palette, scaled_mono, scaled_ui};
use crate::ui::*;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{Icon, Sizable};
use gpui_component::input::{Input, InputEvent, InputState};
use std::rc::Rc;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Data,
    Structure,
}

struct FilterRow {
    enabled: bool,
    target: Target,
    op: Op,
    input: Entity<InputState>,
    _sub: Subscription,
}

impl FilterRow {
    fn read(&self, cx: &App) -> Filter {
        Filter { target: self.target.clone(), op: self.op, value: self.input.read(cx).value().to_string() }
    }
}

pub struct TableView {
    pub session: Entity<Session>,
    pub table: String,
    pub grid: Entity<DataGrid>,
    pub mode: Mode,
    /// A preview tab is replaced by the next table opened with a single click.
    pub preview: bool,
    filters: Vec<FilterRow>,
    show_filters: bool,
    applied: Vec<Filter>,
    sort: Option<Sort>,
    pub page: usize,
    pub page_size: usize,
    pub total: Option<(i64, bool)>,
    pub loading: bool,
    pub error: Option<String>,
    pub elapsed: Duration,
    ddl: Entity<InputState>,
    ddl_for: Option<String>,
    committing: bool,
    _tasks: Vec<Task<()>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<TabEvent> for TableView {}

impl TableView {
    pub fn new(session: Entity<Session>, table: String, preview: bool, filter: Option<(String, Value)>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let grid = cx.new(DataGrid::new);
        let ddl = cx.new(|cx| InputState::new(window, cx).code_editor("sql").line_number(false));
        let subs = vec![
            cx.subscribe_in(&grid, window, Self::on_grid),
            cx.subscribe(&session, |t: &mut TableView, _, e: &SessionEvent, cx| match e {
                SessionEvent::Detail(name) if *name == t.table => t.reload(cx),
                SessionEvent::DetailFailed(name, e) if *name == t.table => {
                    t.loading = false;
                    t.error = Some(e.clone());
                    cx.emit(TabEvent::Changed);
                    cx.notify();
                }
                SessionEvent::Schema => {
                    t.session.update(cx, |s, cx| {
                        s.detail(&t.table, cx);
                    });
                }
                _ => {}
            }),
        ];
        let mut t = TableView {
            session,
            table,
            grid,
            mode: Mode::Data,
            preview,
            filters: vec![],
            show_filters: false,
            applied: vec![],
            sort: None,
            page: 1,
            page_size: 100,
            total: None,
            loading: true,
            error: None,
            elapsed: Duration::ZERO,
            ddl,
            ddl_for: None,
            committing: false,
            _tasks: vec![],
            _subs: subs,
        };
        if let Some((column, value)) = filter {
            let op = if value.is_null() { Op::IsNull } else { Op::Eq };
            let text = if value.is_null() { String::new() } else { value.display() };
            t.applied = vec![Filter::column(column.clone(), op, text.clone())];
            t.filters = vec![t.filter_row(Target::Column(column), op, text, window, cx)];
            t.show_filters = true;
        }
        t.reload(cx);
        t
    }

    fn filter_row(&self, target: Target, op: Op, value: String, window: &mut Window, cx: &mut Context<Self>) -> FilterRow {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(op.hint()));
        input.update(cx, |s, cx| s.set_value(value, window, cx));
        // Return applies this row.
        let _sub = cx.subscribe(&input, |t: &mut TableView, input, e: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = e
                && let Some(i) = t.filters.iter().position(|f| f.input == input)
            {
                t.apply_row(i, cx);
            }
        });
        FilterRow { enabled: true, target, op, input, _sub }
    }

    pub fn title(&self) -> String {
        self.table.clone()
    }

    pub fn is_view(&self, cx: &App) -> bool {
        self.session.read(cx).is_view(&self.table)
    }

    pub fn has_pending(&self, cx: &App) -> bool {
        self.grid.read(cx).has_pending()
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let detail = self.session.update(cx, |s, cx| s.detail(&self.table, cx).cloned());
        let Some(detail) = detail else {
            self.loading = true;
            cx.notify();
            return;
        };
        let session = self.session.read(cx);
        let engine = session.engine();
        let schema = session.schema.clone();
        let estimate = session.relation(&self.table).and_then(|r| r.rows);
        let names: Vec<String> = detail.columns.iter().map(|c| c.name.clone()).collect();
        let where_sql = db::grid_where(engine, &self.applied, &names, "");
        // Without a chosen sort, pages follow the primary key, so they stay put between reloads.
        let pk_sort = detail.columns.iter().find(|c| c.primary_key).map(|c| Sort { column: c.name.clone(), descending: false });
        let sort = self.sort.clone().or(pk_sort);
        let sql = db::select_page(engine, &schema, &self.table, &where_sql, sort.as_ref(), self.page_size, (self.page - 1) * self.page_size);
        // Counting a big table can take seconds; without filters the estimate will do.
        let exact = where_sql.is_empty() && estimate.is_some_and(|n| n > 500_000);
        let count = (!exact).then(|| db::count_sql(engine, &schema, &self.table, &where_sql));
        let task = session.run(cx, move |d| {
            let page = d.query(&sql);
            let n = count.and_then(|q| d.query(&q).ok()).and_then(|p| p.rows.first().and_then(|r| r.first().cloned()));
            (page, n)
        });
        self.loading = true;
        self.error = None;
        cx.notify();
        let view = self.session.read(cx).is_view(&self.table);
        self._tasks.push(cx.spawn(async move |this, cx| {
            let (page, n) = task.await;
            let _ = this.update(cx, |t, cx| {
                t.loading = false;
                match page {
                    Ok(page) => {
                        t.elapsed = page.elapsed;
                        t.total = match n {
                            Some(Value::Int(n)) => Some((n, false)),
                            _ => estimate.map(|e| (e, true)),
                        };
                        let columns = page
                            .columns
                            .iter()
                            .map(|rc| match detail.columns.iter().find(|c| c.name == rc.name) {
                                Some(info) => GridColumn::from_info(info),
                                None => GridColumn::plain(&rc.name, &rc.type_name),
                            })
                            .collect::<Vec<_>>();
                        let has_pk = columns.iter().any(|c| c.pk);
                        let first = (t.page - 1) * t.page_size + 1;
                        let sort = t.sort.as_ref().and_then(|s| columns.iter().position(|c| c.name == s.column).map(|i| (i, s.descending)));
                        t.grid.update(cx, |g, cx| {
                            g.editable = has_pk && !view;
                            g.read_only_reason = Some(if view {
                                "Views are read-only".into()
                            } else {
                                "This table has no primary key, so its rows can't be edited here".into()
                            });
                            g.first_number = first;
                            g.sort = sort;
                            g.set_data(columns, page.rows, cx);
                        });
                    }
                    Err(e) => t.error = Some(e.to_string()),
                }
                cx.emit(TabEvent::Changed);
                cx.notify();
            });
        }));
    }

    fn on_grid(&mut self, _: &Entity<DataGrid>, e: &GridEvent, window: &mut Window, cx: &mut Context<Self>) {
        match e {
            GridEvent::Selected | GridEvent::Edited => {
                if matches!(e, GridEvent::Edited) {
                    self.preview = false;
                }
                cx.emit(TabEvent::Changed);
                cx.notify();
            }
            GridEvent::Sort(i) => {
                let name = self.grid.read(cx).columns[*i].name.clone();
                self.sort = match self.sort.take() {
                    Some(s) if s.column == name && !s.descending => Some(Sort { column: name, descending: true }),
                    Some(s) if s.column == name => None,
                    _ => Some(Sort { column: name, descending: false }),
                };
                self.reload(cx);
            }
            GridEvent::HeaderMenu { col, position } => {
                let build = self.header_menu(*col, cx);
                cx.emit(TabEvent::Menu { position: *position, build, width: 230. });
            }
            GridEvent::ContextMenu { position } => {
                let build = self.cell_menu(window, cx);
                cx.emit(TabEvent::Menu { position: *position, build, width: 250. });
            }
            GridEvent::OpenForeign { table, column, value } => cx.emit(TabEvent::OpenTable { table: table.clone(), filter: Some((column.clone(), value.clone())) }),
            GridEvent::Copied(msg) => cx.emit(TabEvent::Toast(msg.clone())),
        }
    }

    fn cell_menu(&mut self, _: &mut Window, cx: &mut Context<Self>) -> crate::menu::Builder {
        let this = cx.entity().downgrade();
        let g = self.grid.read(cx);
        let Some((r, c)) = g.sel else { return Rc::new(|_| vec![]) };
        let column = g.columns[c].clone();
        let value = g.value(r, c).cloned().unwrap_or(Value::Null);
        let editable = g.editable;
        let deleted = g.is_deleted(r);
        let json = g.row_as_json(r);
        let engine = self.session.read(cx).engine();
        let qualified = self.session.read(cx).qualified(&self.table);
        let insert = db::Change::Insert {
            values: (0..g.columns.len()).filter(|i| !g.columns[*i].pk).map(|i| (g.columns[i].name.clone(), g.value(r, i).cloned().unwrap_or(Value::Null))).collect(),
        }
        .to_sql(engine, &qualified);
        Rc::new(move |_| {
            let this = this.clone();
            let mut e = vec![];
            let v = value.clone();
            e.push(Entry::Item(Item::new("Copy value").icon(Icon::new(Glyph::Copy)).detail("⌘C").on_click({
                let this = this.clone();
                move |_, cx| {
                    let _ = this.update(cx, |t, cx| t.grid.update(cx, |g, cx| g.copy_selected(cx)));
                }
            })));
            let j = json.clone();
            e.push(Entry::Item(Item::new("Copy row as JSON").icon(Icon::new(Glyph::Copy)).on_click(move |_, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(j.clone()));
            })));
            let ins = insert.clone();
            e.push(Entry::Item(Item::new("Copy row as INSERT").icon(Icon::new(Glyph::Sql)).on_click(move |_, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(ins.clone()));
            })));
            e.push(Entry::Separator);
            let (col, val) = (column.name.clone(), v.clone());
            let label = if v.is_null() { format!("Filter {} is NULL", column.name) } else { format!("Filter {} = {}", column.name, crate::grid::ellipsize(&v.display(), 18)) };
            e.push(Entry::Item(Item::new(label).icon(Icon::new(Glyph::Filter)).on_click({
                let this = this.clone();
                move |window, cx| {
                    let _ = this.update(cx, |t, cx| t.filter_to(col.clone(), val.clone(), window, cx));
                }
            })));
            if let Some((ft, fc)) = column.fk.clone().filter(|_| !v.is_null()) {
                let val = v.clone();
                e.push(Entry::Item(Item::new(format!("Open {ft} where {fc} = {}", crate::grid::ellipsize(&v.display(), 14))).icon(Icon::new(Glyph::Arrow)).on_click({
                    let this = this.clone();
                    move |_, cx| {
                        let _ = this.update(cx, |_, cx| cx.emit(TabEvent::OpenTable { table: ft.clone(), filter: Some((fc.clone(), val.clone())) }));
                    }
                })));
            }
            if editable {
                e.push(Entry::Separator);
                e.push(Entry::Item(Item::new("Edit cell").icon(Icon::new(Glyph::Edit)).detail("↵").disabled(deleted).on_click({
                    let this = this.clone();
                    move |window, cx| {
                        let _ = this.update(cx, |t, cx| t.grid.update(cx, |g, cx| g.start_edit(None, window, cx)));
                    }
                })));
                if column.nullable {
                    e.push(Entry::Item(Item::new("Set NULL").icon(Icon::new(Glyph::Null)).disabled(deleted || v.is_null()).on_click({
                        let this = this.clone();
                        move |_, cx| {
                            let _ = this.update(cx, |t, cx| {
                                t.grid.update(cx, |g, cx| {
                                    if let Some((r, c)) = g.sel {
                                        g.set_value(r, c, Value::Null, cx);
                                    }
                                })
                            });
                        }
                    })));
                }
                e.push(Entry::Item(Item::new("Duplicate row").icon(Icon::new(Glyph::Duplicate)).on_click({
                    let this = this.clone();
                    move |window, cx| {
                        let _ = this.update(cx, |t, cx| t.grid.update(cx, |g, cx| g.duplicate_selected(window, cx)));
                    }
                })));
                e.push(Entry::Item(Item::new(if deleted { "Restore row" } else { "Delete row" }).icon(Icon::new(if deleted { Glyph::Refresh } else { Glyph::Trash })).detail("⌘⌫").on_click({
                    let this = this.clone();
                    move |_, cx| {
                        let _ = this.update(cx, |t, cx| t.grid.update(cx, |g, cx| g.toggle_delete(cx)));
                    }
                })));
            }
            e
        })
    }

    pub fn filter_to(&mut self, column: String, value: Value, window: &mut Window, cx: &mut Context<Self>) {
        let op = if value.is_null() { Op::IsNull } else { Op::Eq };
        let text = if value.is_null() { String::new() } else { value.display() };
        self.filters = vec![self.filter_row(Target::Column(column), op, text, window, cx)];
        self.show_filters = true;
        self.apply_all(cx);
    }

    /// Apply every checked row.
    fn apply_all(&mut self, cx: &mut Context<Self>) {
        self.applied = self.filters.iter().filter(|f| f.enabled).map(|f| f.read(cx)).filter(|f| f.is_complete()).collect();
        self.page = 1;
        self.reload(cx);
    }

    /// Apply just one row, as its own Apply button does.
    fn apply_row(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(f) = self.filters.get_mut(i) else { return };
        f.enabled = true;
        let f = f.read(cx);
        self.applied = if f.is_complete() { vec![f] } else { vec![] };
        self.page = 1;
        self.reload(cx);
    }

    /// Add a filter row on `column` and put the caret in its value.
    pub fn filter_column(&mut self, column: String, window: &mut Window, cx: &mut Context<Self>) {
        let row = self.filter_row(Target::Column(column), Op::Eq, String::new(), window, cx);
        row.input.update(cx, |s, cx| s.focus(window, cx));
        // An untouched default row is replaced rather than kept beside the new one.
        if self.filters.len() == 1 && self.filters[0].target == Target::AnyColumn && self.filters[0].input.read(cx).value().is_empty() {
            self.filters.clear();
        }
        self.filters.push(row);
        self.show_filters = true;
        cx.notify();
    }

    fn header_menu(&self, col: usize, cx: &mut Context<Self>) -> crate::menu::Builder {
        let name = self.grid.read(cx).columns.get(col).map(|c| c.name.clone()).unwrap_or_default();
        let sorted = self.sort.as_ref().filter(|s| s.column == name).map(|s| s.descending);
        let has_filters = !self.filters.is_empty();
        let this = cx.entity().downgrade();
        Rc::new(move |_| {
            let (t1, t2, t3, t4, t5, t6) = (this.clone(), this.clone(), this.clone(), this.clone(), this.clone(), this.clone());
            let (n1, n2, n3, n4) = (name.clone(), name.clone(), name.clone(), name.clone());
            let mut v = vec![
                Entry::Item(Item::new(format!("Filter by {name}…")).icon(Icon::new(Glyph::Filter)).on_click(move |window, cx| {
                    let _ = t1.update(cx, |t, cx| t.filter_column(n1.clone(), window, cx));
                })),
                Entry::Separator,
                Entry::Item(Item::new("Sort ascending").icon(Icon::new(Glyph::Up)).checked(sorted == Some(false)).on_click(move |_, cx| {
                    let _ = t2.update(cx, |t, cx| {
                        t.sort = Some(Sort { column: n2.clone(), descending: false });
                        t.reload(cx);
                    });
                })),
                Entry::Item(Item::new("Sort descending").icon(Icon::new(Glyph::Down)).checked(sorted == Some(true)).on_click(move |_, cx| {
                    let _ = t3.update(cx, |t, cx| {
                        t.sort = Some(Sort { column: n3.clone(), descending: true });
                        t.reload(cx);
                    });
                })),
            ];
            if sorted.is_some() {
                v.push(Entry::Item(Item::new("Clear sort").on_click(move |_, cx| {
                    let _ = t4.update(cx, |t, cx| {
                        t.sort = None;
                        t.reload(cx);
                    });
                })));
            }
            v.push(Entry::Separator);
            v.push(Entry::Item(Item::new("Copy column name").icon(Icon::new(Glyph::Copy)).on_click(move |_, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(n4.clone()));
            })));
            if has_filters {
                v.push(Entry::Item(Item::new("Clear filters").icon(Icon::new(Glyph::Close)).on_click(move |_, cx| {
                    let _ = t5.update(cx, |t, cx| {
                        t.filters.clear();
                        t.show_filters = false;
                        t.apply_all(cx);
                    });
                })));
            }
            let _ = t6;
            v
        })
    }

    fn new_filter(&self, window: &mut Window, cx: &mut Context<Self>) -> FilterRow {
        let row = self.filter_row(Target::AnyColumn, Op::ContainsCi, String::new(), window, cx);
        row.input.update(cx, |s, cx| s.focus(window, cx));
        row
    }

    pub fn add_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode != Mode::Data {
            self.mode = Mode::Data;
        }
        let detail = self.session.read(cx).details.get(&self.table).cloned();
        self.grid.update(cx, |g, cx| {
            if !g.editable {
                if let Some(why) = g.read_only_reason.clone() {
                    cx.emit(GridEvent::Copied(why));
                }
                return;
            }
            let values = g
                .columns
                .iter()
                .map(|c| {
                    let info = detail.as_ref().and_then(|d| d.columns.iter().find(|i| i.name == c.name));
                    if info.is_some_and(|i| i.default.is_some()) || c.pk { Value::Default } else { Value::Null }
                })
                .collect();
            g.insert_row(values, window, cx);
        });
    }

    pub fn statements(&self, cx: &App) -> Vec<String> {
        let s = self.session.read(cx);
        let table = s.qualified(&self.table);
        self.grid.read(cx).changes().iter().map(|c| c.to_sql(s.engine(), &table)).collect()
    }

    pub fn commit(&mut self, cx: &mut Context<Self>) {
        let stmts = self.statements(cx);
        if stmts.is_empty() || self.committing {
            return;
        }
        self.committing = true;
        let n = stmts.len();
        let start = std::time::Instant::now();
        let task = self.session.read(cx).run(cx, move |d| d.apply(&stmts));
        self._tasks.push(cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |t, cx| {
                t.committing = false;
                match res {
                    Ok(()) => {
                        t.grid.update(cx, |g, cx| g.accept(cx));
                        cx.emit(TabEvent::Toast(format!("Committed {n} {} to {} · {}", if n == 1 { "change" } else { "changes" }, t.table, fmt_ms(start.elapsed()))));
                        t.reload(cx);
                    }
                    Err(e) => cx.emit(TabEvent::Error(format!("Nothing was committed.\n{e}"))),
                }
                cx.notify();
            });
        }));
    }

    pub fn discard(&mut self, cx: &mut Context<Self>) {
        self.grid.update(cx, |g, cx| g.discard(cx));
        cx.emit(TabEvent::Toast("Discarded pending changes".into()));
    }

    fn column_menu(&self, ix: usize, cx: &mut Context<Self>) -> crate::menu::Builder {
        let columns: Vec<(String, String)> = self.grid.read(cx).columns.iter().map(|c| (c.name.clone(), c.type_name.clone())).collect();
        let current = self.filters.get(ix).map(|f| f.target.clone());
        let this = cx.entity().downgrade();
        let pick = move |this: WeakEntity<TableView>, target: Target| {
            move |window: &mut Window, cx: &mut App| {
                let _ = this.update(cx, |t, cx| {
                    if let Some(f) = t.filters.get_mut(ix) {
                        f.target = target.clone();
                        f.input.update(cx, |s, cx| {
                            s.set_placeholder(if target == Target::Raw { "status = 'paid' AND total > 100" } else { f.op.hint() }, window, cx);
                            s.focus(window, cx);
                        });
                    }
                    cx.notify();
                });
            }
        };
        Rc::new(move |_| {
            let mut v: Vec<Entry> = columns
                .iter()
                .map(|(n, ty)| {
                    let target = Target::Column(n.clone());
                    Entry::Item(Item::new(n.clone()).detail(ty.clone()).checked(current.as_ref() == Some(&target)).on_click(pick(this.clone(), target)))
                })
                .collect();
            v.push(Entry::Separator);
            v.push(Entry::Item(Item::new("Any column").checked(current == Some(Target::AnyColumn)).on_click(pick(this.clone(), Target::AnyColumn))));
            v.push(Entry::Item(Item::new("Raw SQL").checked(current == Some(Target::Raw)).on_click(pick(this.clone(), Target::Raw))));
            v
        })
    }

    fn op_menu(&self, ix: usize, cx: &mut Context<Self>) -> crate::menu::Builder {
        let current = self.filters.get(ix).map(|f| f.op);
        let this = cx.entity().downgrade();
        Rc::new(move |_| {
            let mut v = vec![];
            for (g, group) in Op::GROUPS.iter().enumerate() {
                if g > 0 {
                    v.push(Entry::Separator);
                }
                for op in group.iter().copied() {
                    let this = this.clone();
                    v.push(Entry::Item(Item::new(op.label()).checked(Some(op) == current).on_click(move |window, cx| {
                        let _ = this.update(cx, |t, cx| {
                            if let Some(f) = t.filters.get_mut(ix) {
                                f.op = op;
                                f.input.update(cx, |s, cx| {
                                    s.set_placeholder(op.hint(), window, cx);
                                    s.focus(window, cx);
                                });
                            }
                            cx.notify();
                        });
                    })));
                }
            }
            v
        })
    }

    fn limit_menu(&self, cx: &mut Context<Self>) -> crate::menu::Builder {
        let this = cx.entity().downgrade();
        let current = self.page_size;
        Rc::new(move |_| {
            [100usize, 300, 1000, 5000]
                .into_iter()
                .map(|n| {
                    let this = this.clone();
                    Entry::Item(Item::new(format!("{n} rows")).checked(n == current).on_click(move |_, cx| {
                        let _ = this.update(cx, |t, cx| {
                            t.page_size = n;
                            t.page = 1;
                            t.reload(cx);
                        });
                    }))
                })
                .collect()
        })
    }

    fn pages(&self) -> Option<usize> {
        self.total.map(|(n, _)| ((n.max(0) as usize).div_ceil(self.page_size)).max(1))
    }

    fn go_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let max = self.pages().unwrap_or(usize::MAX);
        let p = page.clamp(1, max);
        if p != self.page {
            self.page = p;
            self.reload(cx);
        }
    }

    fn render_filters(&self, c: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let picker = |id: (&'static str, usize), label: String, width: f32| {
            row()
                .id(id)
                .w(px(width))
                .flex_shrink_0()
                .h(px(24.))
                .pl(px(8.))
                .pr(px(4.))
                .gap(px(4.))
                .rounded(px(5.))
                .border_1()
                .border_color(rgb(c.border))
                .bg(rgb(c.background))
                .hover(|s| s.border_color(rgb(c.popover_border)))
                .child(div().flex_1().min_w_0().truncate().child(label))
                .child(icon(Glyph::UpDown, 12., c.quiet))
        };
        let small = |id: (&'static str, usize), g: Glyph| {
            div()
                .id(id)
                .size(px(22.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .border_1()
                .border_color(rgb(c.border))
                .bg(rgb(c.background))
                .hover(|s| s.bg(rgb(c.hover)))
                .child(icon(g, 12., c.muted))
        };
        col()
            .flex_shrink_0()
            .px(px(10.))
            .pt(px(6.))
            .pb(px(6.))
            .gap(px(5.))
            .bg(rgb(c.panel))
            .border_b_1()
            .border_color(rgb(c.border))
            .text_size(px(scaled_ui(12.5)))
            .children(self.filters.iter().enumerate().map(|(i, f)| {
                let target_label = match &f.target {
                    Target::Column(n) => n.clone(),
                    Target::AnyColumn => "Any column".into(),
                    Target::Raw => "Raw SQL".into(),
                };
                let raw = f.target == Target::Raw;
                row()
                    .gap(px(6.))
                    .when(!f.enabled, |s| s.opacity(0.55))
                    .child(
                        div()
                            .id(("fon", i))
                            .size(px(16.))
                            .flex_shrink_0()
                            .rounded(px(4.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(f.enabled, |s| s.bg(rgb(c.blue)))
                            .when(!f.enabled, |s| s.border_1().border_color(rgb(c.popover_border)).bg(rgb(c.background)))
                            .when(f.enabled, |s| s.child(icon(Glyph::Check, 12., c.primary_text)))
                            .on_click(cx.listener(move |t, _, _, cx| {
                                if let Some(f) = t.filters.get_mut(i) {
                                    f.enabled = !f.enabled;
                                }
                                t.apply_all(cx);
                            })),
                    )
                    .child(picker(("fcol", i), target_label, 150.).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |t, e: &MouseDownEvent, _, cx| {
                            let build = t.column_menu(i, cx);
                            cx.emit(TabEvent::Menu { position: e.position, build, width: 240. });
                        }),
                    ))
                    .when(!raw, |s| {
                        s.child(picker(("fop", i), f.op.label().to_string(), 210.).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |t, e: &MouseDownEvent, _, cx| {
                                let build = t.op_menu(i, cx);
                                cx.emit(TabEvent::Menu { position: e.position, build, width: 250. });
                            }),
                        ))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(px(24.))
                            .px(px(6.))
                            .rounded(px(5.))
                            .border_1()
                            .border_color(rgb(c.border))
                            .bg(rgb(c.background))
                            .font_family(mono())
                            .text_size(px(scaled_mono(12.)))
                            .flex()
                            .items_center()
                            .when(!raw && !f.op.takes_value(), |s| s.opacity(0.4))
                            .child(Input::new(&f.input).appearance(false).small().disabled(!raw && !f.op.takes_value())),
                    )
                    .child(button(c, ("fapply", i), None, Some("Apply"), Tone::Plain).h(px(24.)).on_click(cx.listener(move |t, _, _, cx| t.apply_row(i, cx))))
                    .child(small(("frm", i), Glyph::Minus).on_click(cx.listener(move |t, _, _, cx| {
                        t.filters.remove(i);
                        if t.filters.is_empty() {
                            t.show_filters = false;
                        }
                        t.apply_all(cx);
                    })))
                    .child(small(("fadd", i), Glyph::Plus).on_click(cx.listener(move |t, _, window, cx| {
                        let row = t.new_filter(window, cx);
                        t.filters.insert(i + 1, row);
                        cx.notify();
                    })))
            }))
            .child(
                row()
                    .gap(px(6.))
                    .pt(px(1.))
                    .child(div().flex_1())
                    .child(button(c, "fclear", None, Some("Clear"), Tone::Plain).h(px(24.)).on_click(cx.listener(|t, _, _, cx| {
                        t.filters.clear();
                        t.show_filters = false;
                        t.apply_all(cx);
                    })))
                    .child(button(c, "fapply-all", None, Some("Apply All"), Tone::Primary).h(px(24.)).on_click(cx.listener(|t, _, _, cx| t.apply_all(cx)))),
            )
    }

    fn render_changes(&self, c: Palette, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (e, a, d) = self.grid.read(cx).pending();
        let n = e + a + d;
        if n == 0 || self.mode != Mode::Data {
            return None;
        }
        Some(
            div().absolute().bottom(px(14.)).left_0().right_0().flex().justify_center().child(
                row()
                    .id("pending-bar")
                    .occlude()
                    .gap(px(6.))
                    .pl(px(12.))
                    .pr(px(5.))
                    .py(px(5.))
                    .rounded(px(10.))
                    .bg(rgb(c.popover))
                    .border_1()
                    .border_color(rgb(c.popover_border))
                    .shadow_lg()
                    .child(dot(c.amber, 7.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{n} pending {}", if n == 1 { "change" } else { "changes" })))
                    .child(
                        row()
                            .gap(px(8.))
                            .mx(px(6.))
                            .text_size(px(scaled_ui(12.)))
                            .when(e > 0, |s| s.child(div().text_color(rgb(c.amber)).child(format!("~{e} edited"))))
                            .when(a > 0, |s| s.child(div().text_color(rgb(c.green)).child(format!("+{a} new"))))
                            .when(d > 0, |s| s.child(div().text_color(rgb(c.red)).child(format!("−{d} deleted")))),
                    )
                    .child(button(c, "review", Some(Glyph::Sql), Some("Review SQL"), Tone::Ghost).on_click(cx.listener(|_, _, _, cx| cx.emit(TabEvent::RequestReview))))
                    .child(button(c, "discard", None, Some("Discard"), Tone::Ghost).on_click(cx.listener(|t, _, _, cx| t.discard(cx))))
                    .child(
                        button(c, "commit", None, Some(if self.committing { "Committing…" } else { "Commit" }), Tone::Primary)
                            .child(kbd(c, "⌘S").border_color(hx(c.primary_text).opacity(0.3)).text_color(rgb(c.primary_text)))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(TabEvent::RequestCommit))),
                    ),
            ),
        )
    }

    fn render_footer(&self, c: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let seg = |id: &'static str, g: Glyph, label: &'static str, on: bool| {
            row()
                .id(id)
                .h_full()
                .px(px(10.))
                .gap(px(5.))
                .when(on, |s| s.bg(rgb(c.selected)).text_color(rgb(c.text)))
                .hover(|s| s.text_color(rgb(c.text)))
                .child(icon(g, 12., if on { c.blue } else { c.muted }))
                .child(label)
        };
        let rows = self.grid.read(cx).rows.len();
        let first = (self.page - 1) * self.page_size + 1;
        let info = match self.total {
            _ if rows == 0 => "No rows".to_string(),
            Some((n, approx)) => format!("Rows {}–{} of {}{}", fmt_int(first as i64), fmt_int((first + rows - 1) as i64), if approx { "~" } else { "" }, fmt_int(n)),
            None => format!("{rows} rows"),
        };
        let detail = self.session.read(cx).details.get(&self.table);
        let size = detail.and_then(|d| d.size.clone());
        let pages = self.pages();
        row()
            .h(px(32.))
            .flex_shrink_0()
            .px(px(8.))
            .gap(px(8.))
            .bg(rgb(c.chrome))
            .border_t_1()
            .border_color(rgb(c.border))
            .text_color(rgb(c.muted))
            .text_size(px(scaled_ui(12.)))
            .child(
                row()
                    .h(px(22.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgb(c.border))
                    .overflow_hidden()
                    .child(seg("mode-data", Glyph::Table, "Data", self.mode == Mode::Data).on_click(cx.listener(|t, _, _, cx| {
                        t.mode = Mode::Data;
                        cx.notify();
                    })))
                    .child(div().w(px(1.)).h_full().bg(rgb(c.border)))
                    .child(seg("mode-structure", Glyph::Key, "Structure", self.mode == Mode::Structure).on_click(cx.listener(|t, _, _, cx| {
                        t.mode = Mode::Structure;
                        cx.notify();
                    }))),
            )
            .when(self.is_view(cx), |s| s.child(chip("VIEW · READ-ONLY", c.purple, tint(c.purple, CHIP_FILL))))
            .when(!self.is_view(cx), |s| {
                s.child(button(c, "add-row", Some(Glyph::Plus), Some("Row"), Tone::Ghost).h(px(22.)).on_click(cx.listener(|t, _, window, cx| t.add_row(window, cx))))
            })
            .child(div().flex_1())
            .when(self.mode == Mode::Data, |s| {
                s.child(div().child(info))
                    .child(vsep(c))
                    .child(button(c, "prev", Some(Glyph::Left), None, Tone::Ghost).h(px(22.)).on_click(cx.listener(|t, _, _, cx| t.go_page(t.page.saturating_sub(1), cx))))
                    .child(div().child(match pages {
                        Some(p) => format!("Page {} of {}", fmt_int(self.page as i64), fmt_int(p as i64)),
                        None => format!("Page {}", self.page),
                    }))
                    .child(button(c, "next", Some(Glyph::Right), None, Tone::Ghost).h(px(22.)).on_click(cx.listener(|t, _, _, cx| t.go_page(t.page + 1, cx))))
                    .child(vsep(c))
                    .child(
                        row()
                            .id("limit")
                            .gap(px(4.))
                            .child("Limit")
                            .child(div().text_color(rgb(c.text)).font_weight(FontWeight::MEDIUM).child(self.page_size.to_string()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|t, e: &MouseDownEvent, _, cx| {
                                    let build = t.limit_menu(cx);
                                    cx.emit(TabEvent::Menu { position: e.position, build, width: 140. });
                                }),
                            ),
                    )
            })
            .when(self.mode == Mode::Structure, |s| {
                let n = detail.map_or(0, |d| d.columns.len());
                s.child(format!(
                    "{n} columns · {} {}{}",
                    if self.is_view(cx) { "view" } else { "table" },
                    self.total.map(|(n, a)| format!("· {}{} rows ", if a { "~" } else { "" }, db::compact(n as f64))).unwrap_or_default(),
                    size.map(|s| format!("· {s}")).unwrap_or_default()
                ))
            })
    }

    fn render_structure(&mut self, c: Palette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let detail = self.session.update(cx, |s, cx| s.detail(&self.table, cx).cloned());
        let Some(d) = detail else {
            return div().size_full().flex().items_center().justify_center().text_color(rgb(c.quiet)).child("Loading structure…").into_any_element();
        };
        if self.ddl_for.as_deref() != Some(d.ddl.as_str()) {
            self.ddl_for = Some(d.ddl.clone());
            let ddl = d.ddl.clone();
            self.ddl.update(cx, |s, cx| s.set_value(ddl, window, cx));
        }
        let heading = |title: &str, n: usize| {
            row().gap(px(8.)).mt(px(22.)).mb(px(8.)).child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(scaled_ui(12.))).child(title.to_string())).child(text(12.).text_color(rgb(c.quiet)).child(n.to_string()))
        };
        let th = |s: &str, w: Option<f32>| {
            let d = div().px(px(10.)).pb(px(6.)).whitespace_nowrap().overflow_hidden().text_size(px(scaled_ui(11.))).text_color(rgb(c.quiet)).font_weight(FontWeight::MEDIUM).child(s.to_string());
            match w {
                Some(w) => d.w(px(w)).flex_shrink_0(),
                None => d.flex_1().min_w_0(),
            }
        };
        let td = |w: Option<f32>| {
            let d = row().px(px(10.)).min_h(px(30.)).py(px(4.)).gap(px(4.)).overflow_hidden().whitespace_nowrap();
            match w {
                Some(w) => d.w(px(w)).flex_shrink_0(),
                None => d.flex_1().min_w_0(),
            }
        };
        let tr = || row().border_b_1().border_color(rgb(c.row_border)).hover(|s| s.bg(rgb(c.hover)));
        let head_row = || row().border_b_1().border_color(rgb(c.border));
        let m = |s: String, color: u32| div().truncate().font_family(mono()).text_size(px(scaled_mono(11.5))).text_color(rgb(color)).child(s);
        let column_rows = d.columns.iter().enumerate().map(|(i, col)| {
            tr().child(td(Some(34.)).text_color(rgb(c.quiet)).child((i + 1).to_string()))
                .child(td(Some(170.)).font_weight(FontWeight::MEDIUM).child(col.name.clone()))
                .child(td(Some(150.)).child(m(col.data_type.clone(), c.purple)))
                .child(td(Some(90.)).child(if col.nullable { div().text_color(rgb(c.muted)).child("YES") } else { chip("NOT NULL", c.muted, hx(c.hover)) }))
                .child(td(Some(160.)).child(m(col.default.clone().unwrap_or_else(|| "—".into()), if col.default.is_some() { c.muted } else { c.quiet })))
                .child(
                    td(None)
                        .when(col.primary_key, |s| s.child(chip("PK", c.amber, tint(c.amber, CHIP_FILL))))
                        .when(col.unique, |s| s.child(chip("UNIQUE", c.purple, tint(c.purple, CHIP_FILL))))
                        .when_some(col.foreign.clone(), |s, (t, fc)| {
                            s.child(
                                row()
                                    .id(SharedString::from(format!("fk-{}", col.name)))
                                    .gap(px(4.))
                                    .px(px(6.))
                                    .py(px(1.))
                                    .rounded(px(4.))
                                    .bg(tint(c.cyan, CHIP_FILL))
                                    .text_color(rgb(c.cyan))
                                    .text_size(px(scaled_ui(10.5)))
                                    .font_weight(FontWeight::BOLD)
                                    .child(icon(Glyph::Link, 11., c.cyan))
                                    .child(format!("{t}.{fc}"))
                                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(TabEvent::OpenTable { table: t.clone(), filter: None }))),
                            )
                        })
                        .when(!col.enum_values.is_empty(), |s| s.child(div().text_color(rgb(c.quiet)).truncate().text_size(px(scaled_ui(11.5))).child(col.enum_values.join(" · ")))),
                )
                .child(td(None).text_color(rgb(c.quiet)).truncate().child(col.comment.clone().unwrap_or_default()))
        });
        let index_rows = d.indexes.iter().map(|ix| {
            let unused = ix.scans == Some(0) && !ix.primary && !ix.unique;
            tr().child(td(Some(260.)).child(m(ix.name.clone(), c.text)))
                .child(td(None).child(m(ix.columns.join(", "), c.text)))
                .child(td(Some(90.)).child(m(ix.method.clone(), c.purple)))
                .child(td(Some(90.)).child(if ix.primary { chip("PRIMARY", c.amber, tint(c.amber, CHIP_FILL)) } else if ix.unique { chip("UNIQUE", c.purple, tint(c.purple, CHIP_FILL)) } else { div().text_color(rgb(c.quiet)).child("—") }))
                .child(td(Some(90.)).justify_end().text_color(rgb(c.muted)).child(ix.size.clone().unwrap_or_else(|| "—".into())))
                .child(td(Some(120.)).justify_end().text_color(rgb(c.muted)).child(match ix.scans {
                    _ if unused => row().gap(px(4.)).px(px(6.)).rounded(px(4.)).bg(rgb(c.amber_tint)).text_color(rgb(c.amber)).text_size(px(scaled_ui(11.))).child(icon(Glyph::Warn, 11., c.amber)).child("0 · unused"),
                    Some(n) => div().child(fmt_int(n)),
                    None => div().child("—"),
                }))
        });
        let ref_rows = d.referenced_by.iter().map(|(t, col)| {
            let t2 = t.clone();
            tr().id(SharedString::from(format!("ref-{t}-{col}")))
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(TabEvent::OpenTable { table: t2.clone(), filter: None })))
                .child(td(Some(300.)).text_color(rgb(c.amber)).font_weight(FontWeight::MEDIUM).child(t.clone()))
                .child(td(None).child(m(col.clone(), c.text)))
        });
        div()
            .id("structure")
            .size_full()
            .overflow_y_scroll()
            .px(px(22.))
            .pt(px(0.))
            .pb(px(40.))
            .text_size(px(scaled_ui(12.5)))
            .child(heading("Columns", d.columns.len()))
            .child(head_row().child(th("#", Some(34.))).child(th("Name", Some(170.))).child(th("Type", Some(150.))).child(th("Nullable", Some(90.))).child(th("Default", Some(160.))).child(th("Constraints", None)).child(th("Comment", None)))
            .children(column_rows)
            .child(heading("Indexes", d.indexes.len()))
            .child(head_row().child(th("Name", Some(260.))).child(th("Columns", None)).child(th("Method", Some(90.))).child(th("Unique", Some(90.))).child(th("Size", Some(90.)).text_right()).child(th("Scans", Some(120.)).text_right()))
            .children(index_rows)
            .when(!d.referenced_by.is_empty(), |s| {
                s.child(heading("Referenced by", d.referenced_by.len()))
                    .child(head_row().child(th("Table", Some(300.))).child(th("Column", None)))
                    .children(ref_rows)
            })
            .child(
                row()
                    .mt(px(22.))
                    .mb(px(8.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(scaled_ui(12.))).child("Definition"))
                    .child(div().flex_1())
                    .child(button(c, "copy-ddl", Some(Glyph::Copy), Some("Copy"), Tone::Ghost).on_click(cx.listener(move |t, _, _, cx| {
                        if let Some(d) = &t.ddl_for {
                            cx.write_to_clipboard(ClipboardItem::new_string(d.clone()));
                        }
                        cx.emit(TabEvent::Toast("Copied definition".into()));
                    }))),
            )
            .child(
                div()
                    .h(px((d.ddl.lines().count() as f32 * 20. + 30.).min(520.)))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(c.border))
                    .bg(rgb(c.panel))
                    .overflow_hidden()
                    .p(px(6.))
                    .font_family(mono())
                    .text_size(px(scaled_mono(12.5)))
                    .child(Input::new(&self.ddl).appearance(false).h_full()),
            )
            .into_any_element()
    }
}

impl Render for TableView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = palette(cx);
        let body: AnyElement = match self.mode {
            Mode::Structure => self.render_structure(c, window, cx),
            Mode::Data => match &self.error {
                Some(e) => col()
                    .m(px(18.))
                    .p(px(14.))
                    .gap(px(6.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(hx(c.red).opacity(0.4))
                    .bg(rgb(c.red_tint))
                    .child(row().gap(px(8.)).text_color(rgb(c.red)).font_weight(FontWeight::SEMIBOLD).child(icon(Glyph::Warn, 14., c.red)).child("Couldn't load rows"))
                    .child(mono_text(12.).child(e.clone()))
                    .into_any_element(),
                None => self.grid.clone().into_any_element(),
            },
        };
        col()
            .size_full()
            .when(self.show_filters && self.mode == Mode::Data, |s| s.child(self.render_filters(c, cx)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(body)
                    .when(self.loading && self.mode == Mode::Data, |s| {
                        s.child(
                            div()
                                .absolute()
                                .top(px(40.))
                                .left_0()
                                .right_0()
                                .flex()
                                .justify_center()
                                .child(row().gap(px(8.)).px(px(10.)).py(px(4.)).rounded(px(6.)).bg(rgb(c.popover)).border_1().border_color(rgb(c.popover_border)).text_color(rgb(c.muted)).child("Loading…")),
                        )
                    })
                    .children(self.render_changes(c, cx)),
            )
            .child(self.render_footer(c, cx))
    }
}
