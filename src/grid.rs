//! The data grid: virtualized rows, a sticky header and row-number gutter, cell
//! selection, inline editing, and pending edits/inserts/deletes tinted in place.
//!
//! The grid only tracks changes; whoever owns it decides what they mean (the
//! table tab turns them into statements, the SQL tab keeps them read-only).
use crate::db::{Change, ColumnInfo, Kind, Value};
use crate::icons::Glyph;
use crate::theme::{Palette, mono, palette, scaled_mono, scaled_ui};
use crate::ui::*;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::Scrollbar;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// The grid's sideways scroll. The rows list only scrolls up and down; sideways is
/// ours, so the header, the cells and the bar all move in the same frame.
#[derive(Default)]
struct Side {
    offset: f32,
    viewport: f32,
    content: f32,
    /// The axis the current trackpad swipe is held to.
    axis: Option<Axis>,
    /// When the last wheel event came in; a pause ends the axis lock, since mouse
    /// wheels never report a gesture starting.
    last: Option<std::time::Instant>,
    /// Where a drag on the scrollbar began, and the offset then.
    drag: Option<(Pixels, f32)>,
}

impl Side {
    fn max(&self) -> f32 {
        (self.content - self.viewport).max(0.)
    }
    fn thumb(&self, track: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
        let max = self.max();
        if max <= 0. || self.content <= 0. {
            return None;
        }
        let tw = f(track.size.width);
        let w = (tw * self.viewport / self.content).clamp(24., tw);
        let left = f(track.left()) + (tw - w) * (self.offset / max);
        Some(Bounds::new(point(px(left), track.top()), size(px(w), track.size.height)))
    }
}

actions!(
    grid,
    [MoveUp, MoveDown, MoveLeft, MoveRight, PageUp, PageDown, MoveTop, MoveBottom, NextCell, PrevCell, EditCell, ClearCell, DeleteRow, CopyCell, CancelEdit, CommitNext, CommitPrev, CommitDown, CommitUp]
);

pub fn bind_keys(cx: &mut App) {
    let g = Some("Grid");
    let e = Some("GridCell > Input");
    cx.bind_keys([
        KeyBinding::new("up", MoveUp, g),
        KeyBinding::new("down", MoveDown, g),
        KeyBinding::new("left", MoveLeft, g),
        KeyBinding::new("right", MoveRight, g),
        KeyBinding::new("pageup", PageUp, g),
        KeyBinding::new("pagedown", PageDown, g),
        KeyBinding::new("cmd-up", MoveTop, g),
        KeyBinding::new("cmd-down", MoveBottom, g),
        KeyBinding::new("tab", NextCell, g),
        KeyBinding::new("shift-tab", PrevCell, g),
        KeyBinding::new("enter", EditCell, g),
        KeyBinding::new("f2", EditCell, g),
        KeyBinding::new("backspace", ClearCell, g),
        KeyBinding::new("cmd-backspace", DeleteRow, g),
        KeyBinding::new("cmd-c", CopyCell, g),
        KeyBinding::new("escape", CancelEdit, e),
        KeyBinding::new("tab", CommitNext, e),
        KeyBinding::new("shift-tab", CommitPrev, e),
        KeyBinding::new("enter", CommitDown, e),
        KeyBinding::new("shift-enter", CommitUp, e),
    ]);
}

pub const GUTTER: f32 = 46.;
const HEADER: f32 = 30.;

#[derive(Clone, Debug)]
pub struct GridColumn {
    pub name: String,
    pub type_name: String,
    pub kind: Kind,
    pub pk: bool,
    pub fk: Option<(String, String)>,
    pub nullable: bool,
    pub enum_values: Vec<String>,
    pub width: f32,
}

impl GridColumn {
    pub fn plain(name: &str, type_name: &str) -> Self {
        GridColumn { name: name.into(), type_name: type_name.into(), kind: Kind::from_type(type_name), pk: false, fk: None, nullable: true, enum_values: vec![], width: 120. }
    }
    pub fn from_info(c: &ColumnInfo) -> Self {
        GridColumn {
            name: c.name.clone(),
            type_name: short_type(&c.data_type),
            kind: c.kind(),
            pk: c.primary_key,
            fk: c.foreign.clone(),
            nullable: c.nullable,
            enum_values: c.enum_values.clone(),
            width: 120.,
        }
    }
}

/// Long Postgres type names, shortened for a column header.
pub fn short_type(t: &str) -> String {
    t.replace("timestamp with time zone", "timestamptz")
        .replace("timestamp without time zone", "timestamp")
        .replace("character varying", "varchar")
        .replace("double precision", "float8")
        .replace("integer", "int4")
        .replace("bigint", "int8")
        .replace("smallint", "int2")
        .replace("boolean", "bool")
}

#[derive(Clone, Debug)]
pub struct GridRow {
    pub key: usize,
    pub values: Vec<Value>,
    pub inserted: bool,
}

pub enum GridEvent {
    Selected,
    /// Pending edits, inserts or deletes changed.
    Edited,
    Sort(usize),
    /// A column title was right-clicked.
    HeaderMenu { col: usize, position: Point<Pixels> },
    ContextMenu { position: Point<Pixels> },
    OpenForeign { table: String, column: String, value: Value },
    Copied(String),
}

struct Editing {
    row: usize,
    col: usize,
    input: Entity<InputState>,
    _sub: Subscription,
}

pub struct DataGrid {
    focus: FocusHandle,
    pub columns: Vec<GridColumn>,
    pub rows: Vec<GridRow>,
    pub edits: HashMap<(usize, usize), Value>,
    pub deleted: HashSet<usize>,
    pub editable: bool,
    /// Why the grid can't be edited, shown when someone tries.
    pub read_only_reason: Option<String>,
    pub sel: Option<(usize, usize)>,
    pub sort: Option<(usize, bool)>,
    /// The number shown on the first row, for paging.
    pub first_number: usize,
    editing: Option<Editing>,
    scroll: UniformListScrollHandle,
    side: Rc<RefCell<Side>>,
    resizing: Option<(usize, Pixels, f32)>,
    next_key: usize,
}

impl EventEmitter<GridEvent> for DataGrid {}
impl Focusable for DataGrid {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl DataGrid {
    pub fn new(cx: &mut Context<Self>) -> Self {
        DataGrid {
            focus: cx.focus_handle(),
            columns: vec![],
            rows: vec![],
            edits: HashMap::new(),
            deleted: HashSet::new(),
            editable: false,
            read_only_reason: None,
            sel: None,
            sort: None,
            first_number: 1,
            editing: None,
            scroll: UniformListScrollHandle::new(),
            side: Rc::default(),
            resizing: None,
            next_key: 0,
        }
    }

    /// Replace the contents. Keeps column widths when the columns are the same.
    pub fn set_data(&mut self, mut columns: Vec<GridColumn>, rows: Vec<Vec<Value>>, cx: &mut Context<Self>) {
        let same = columns.len() == self.columns.len() && columns.iter().zip(&self.columns).all(|(a, b)| a.name == b.name);
        if same {
            for (a, b) in columns.iter_mut().zip(&self.columns) {
                a.width = b.width;
            }
        } else {
            for (i, c) in columns.iter_mut().enumerate() {
                c.width = default_width(c, rows.iter().take(40).map(|r| &r[i]));
            }
            self.scroll.0.borrow().base_handle.set_offset(point(px(0.), px(0.)));
            self.side.borrow_mut().offset = 0.;
        }
        self.columns = columns;
        self.rows = rows.into_iter().enumerate().map(|(key, values)| GridRow { key, values, inserted: false }).collect();
        self.next_key = self.rows.len();
        self.edits.clear();
        self.deleted.clear();
        self.editing = None;
        self.sel = match self.sel {
            Some((r, c)) if !self.rows.is_empty() => Some((r.min(self.rows.len() - 1), c.min(self.columns.len().saturating_sub(1)))),
            _ if !self.rows.is_empty() && !self.columns.is_empty() => Some((0, 0)),
            _ => None,
        };
        cx.emit(GridEvent::Selected);
        cx.notify();
    }

    pub fn value(&self, row: usize, col: usize) -> Option<&Value> {
        let r = self.rows.get(row)?;
        self.edits.get(&(r.key, col)).or_else(|| r.values.get(col))
    }

    pub fn is_edited(&self, row: usize, col: usize) -> bool {
        self.rows.get(row).is_some_and(|r| !r.inserted && self.edits.contains_key(&(r.key, col)))
    }

    pub fn is_deleted(&self, row: usize) -> bool {
        self.rows.get(row).is_some_and(|r| self.deleted.contains(&r.key))
    }

    /// (rows edited, rows added, rows deleted).
    pub fn pending(&self) -> (usize, usize, usize) {
        let inserted: HashSet<usize> = self.rows.iter().filter(|r| r.inserted).map(|r| r.key).collect();
        let edited: HashSet<usize> = self.edits.keys().map(|(k, _)| *k).filter(|k| !inserted.contains(k) && !self.deleted.contains(k)).collect();
        (edited.len(), inserted.len(), self.deleted.len())
    }

    pub fn has_pending(&self) -> bool {
        let (a, b, c) = self.pending();
        a + b + c > 0
    }

    /// The pending changes as row operations, keyed by the primary key columns.
    pub fn changes(&self) -> Vec<Change> {
        let pk: Vec<usize> = (0..self.columns.len()).filter(|&i| self.columns[i].pk).collect();
        let key_of = |r: &GridRow| pk.iter().map(|&i| (self.columns[i].name.clone(), r.values[i].clone())).collect::<Vec<_>>();
        let mut out = Vec::new();
        for r in &self.rows {
            if r.inserted {
                let values = (0..self.columns.len())
                    .map(|i| (self.columns[i].name.clone(), self.edits.get(&(r.key, i)).cloned().unwrap_or_else(|| r.values[i].clone())))
                    .collect();
                out.push(Change::Insert { values });
            } else if self.deleted.contains(&r.key) {
                out.push(Change::Delete { key: key_of(r) });
            } else {
                let mut set: Vec<(usize, Value)> = self.edits.iter().filter(|((k, _), _)| *k == r.key).map(|((_, c), v)| (*c, v.clone())).collect();
                if !set.is_empty() {
                    set.sort_by_key(|(c, _)| *c);
                    out.push(Change::Update { key: key_of(r), set: set.into_iter().map(|(c, v)| (self.columns[c].name.clone(), v)).collect() });
                }
            }
        }
        out
    }

    /// Make the pending changes the new truth, after they were committed.
    pub fn accept(&mut self, cx: &mut Context<Self>) {
        for r in self.rows.iter_mut() {
            for (i, v) in r.values.iter_mut().enumerate() {
                if let Some(e) = self.edits.get(&(r.key, i)) {
                    *v = e.clone();
                }
            }
            r.inserted = false;
        }
        let deleted = std::mem::take(&mut self.deleted);
        self.rows.retain(|r| !deleted.contains(&r.key));
        self.edits.clear();
        self.clamp_selection();
        cx.emit(GridEvent::Edited);
        cx.notify();
    }

    pub fn discard(&mut self, cx: &mut Context<Self>) {
        self.rows.retain(|r| !r.inserted);
        self.edits.clear();
        self.deleted.clear();
        self.editing = None;
        self.clamp_selection();
        cx.emit(GridEvent::Edited);
        cx.emit(GridEvent::Selected);
        cx.notify();
    }

    fn clamp_selection(&mut self) {
        if self.rows.is_empty() {
            self.sel = None;
        } else if let Some((r, c)) = self.sel {
            self.sel = Some((r.min(self.rows.len() - 1), c));
        }
    }

    pub fn set_value(&mut self, row: usize, col: usize, v: Value, cx: &mut Context<Self>) {
        let Some(r) = self.rows.get(row) else { return };
        let key = (r.key, col);
        if !r.inserted && r.values.get(col) == Some(&v) {
            self.edits.remove(&key);
        } else {
            self.edits.insert(key, v);
        }
        cx.emit(GridEvent::Edited);
        cx.emit(GridEvent::Selected);
        cx.notify();
    }

    pub fn insert_row(&mut self, values: Vec<Value>, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.next_key;
        self.next_key += 1;
        self.rows.insert(0, GridRow { key, values, inserted: true });
        let first = self.columns.iter().position(|c| !c.pk).unwrap_or(0);
        self.sel = Some((0, first));
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.emit(GridEvent::Edited);
        cx.emit(GridEvent::Selected);
        self.start_edit(None, window, cx);
    }

    pub fn duplicate_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((r, _)) = self.sel else { return };
        let values = (0..self.columns.len())
            .map(|i| if self.columns[i].pk { Value::Default } else { self.value(r, i).cloned().unwrap_or(Value::Null) })
            .collect();
        self.insert_row(values, window, cx);
    }

    /// Mark the selected row for deletion, or bring it back; a new row just goes.
    pub fn toggle_delete(&mut self, cx: &mut Context<Self>) {
        if !self.check_editable(cx) {
            return;
        }
        let Some((r, _)) = self.sel else { return };
        let row = &self.rows[r];
        if row.inserted {
            let key = row.key;
            self.rows.remove(r);
            self.edits.retain(|(k, _), _| *k != key);
            self.clamp_selection();
        } else if !self.deleted.remove(&row.key) {
            self.deleted.insert(row.key);
        }
        cx.emit(GridEvent::Edited);
        cx.emit(GridEvent::Selected);
        cx.notify();
    }

    fn check_editable(&self, cx: &mut Context<Self>) -> bool {
        if !self.editable {
            if let Some(why) = &self.read_only_reason {
                cx.emit(GridEvent::Copied(why.clone()));
            }
        }
        self.editable
    }

    pub fn select(&mut self, row: usize, col: usize, cx: &mut Context<Self>) {
        if self.rows.is_empty() || self.columns.is_empty() {
            return;
        }
        let r = row.min(self.rows.len() - 1);
        let c = col.min(self.columns.len() - 1);
        if self.sel != Some((r, c)) {
            self.sel = Some((r, c));
            self.scroll.scroll_to_item(r, ScrollStrategy::Top);
            self.scroll_to_column(c);
            cx.emit(GridEvent::Selected);
            cx.notify();
        }
    }

    fn scroll_to_column(&self, c: usize) {
        let left: f32 = self.columns[..c].iter().map(|c| c.width).sum();
        let right = left + self.columns[c].width;
        let mut side = self.side.borrow_mut();
        if left < side.offset {
            side.offset = left;
        } else if side.viewport > 0. && right > side.offset + side.viewport {
            side.offset = right - side.viewport;
        }
    }

    fn step(&mut self, dr: isize, dc: isize, cx: &mut Context<Self>) {
        let (r, c) = self.sel.unwrap_or((0, 0));
        let r = (r as isize + dr).clamp(0, self.rows.len().saturating_sub(1) as isize) as usize;
        let c = (c as isize + dc).clamp(0, self.columns.len().saturating_sub(1) as isize) as usize;
        self.select(r, c, cx);
    }

    fn page_rows(&self) -> isize {
        let h = f(self.scroll.0.borrow().base_handle.bounds().size.height);
        ((h / ROW_H) as isize - 2).max(1)
    }

    pub fn copy_selected(&mut self, cx: &mut Context<Self>) {
        if let Some((r, c)) = self.sel
            && let Some(v) = self.value(r, c)
        {
            let text = v.display();
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            cx.emit(GridEvent::Copied(format!("Copied {}", ellipsize(&text, 40))));
        }
    }

    pub fn row_as_json(&self, row: usize) -> String {
        let mut m = serde_json::Map::new();
        for (i, c) in self.columns.iter().enumerate() {
            let v = match self.value(row, i) {
                Some(Value::Null) | None => serde_json::Value::Null,
                Some(Value::Bool(b)) => (*b).into(),
                Some(Value::Int(n)) => (*n).into(),
                Some(Value::Float(f)) => serde_json::Number::from_f64(*f).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null),
                Some(Value::Json(j)) => j.clone(),
                Some(other) => other.display().into(),
            };
            m.insert(c.name.clone(), v);
        }
        serde_json::to_string_pretty(&serde_json::Value::Object(m)).unwrap_or_default()
    }

    /// Start editing the selected cell, optionally replacing its text with `initial`.
    pub fn start_edit(&mut self, initial: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.check_editable(cx) {
            return;
        }
        let Some((row, col)) = self.sel else { return };
        if self.is_deleted(row) {
            return;
        }
        let current = self.value(row, col).cloned().unwrap_or(Value::Null);
        let kind = self.columns[col].kind;
        let text = initial.clone().unwrap_or_else(|| if kind == Kind::Json { current.display() } else { current.edit_text() });
        let input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            if current.is_null() {
                s = s.placeholder("NULL");
            }
            s
        });
        input.update(cx, |s, cx| {
            s.set_value(text, window, cx);
            s.focus(window, cx);
        });
        let sub = cx.subscribe_in(&input, window, |g: &mut DataGrid, _, e: &InputEvent, window, cx| match e {
            InputEvent::Blur => g.finish_edit(true, window, cx),
            _ => {}
        });
        self.editing = Some(Editing { row, col, input, _sub: sub });
        cx.notify();
    }

    fn finish_edit(&mut self, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.editing.take() else { return };
        if save {
            let text = ed.input.read(cx).value().to_string();
            let old = self.value(ed.row, ed.col).cloned().unwrap_or(Value::Null);
            let kind = self.columns[ed.col].kind;
            let v = Value::parse(&text, kind, old.is_null() || old == Value::Default);
            let v = if old == Value::Default && text.is_empty() { Value::Default } else { v };
            if v != old {
                self.set_value(ed.row, ed.col, v, cx);
            }
        }
        self.focus.focus(window);
        cx.notify();
    }

    fn foreign_of(&self, row: usize, col: usize) -> Option<GridEvent> {
        let (table, column) = self.columns.get(col)?.fk.clone()?;
        let value = self.value(row, col)?.clone();
        (!value.is_null()).then_some(GridEvent::OpenForeign { table, column, value })
    }

    fn render_cell(&self, c: Palette, row: usize, col: usize, cx: &mut Context<Self>) -> AnyElement {
        let column = &self.columns[col];
        if let Some(ed) = self.editing.as_ref().filter(|e| e.row == row && e.col == col) {
            return div()
                .key_context("GridCell")
                .w(px(column.width))
                .h_full()
                .flex_shrink_0()
                .border_2()
                .border_color(rgb(c.blue))
                .bg(rgb(c.background))
                .child(Input::new(&ed.input).appearance(false).h_full())
                .into_any_element();
        }
        let v = self.value(row, col).cloned().unwrap_or(Value::Null);
        let selected = self.sel == Some((row, col));
        let edited = self.is_edited(row, col);
        let fk = column.fk.is_some() && !v.is_null();
        let content = cell_content(c, &v, column);
        let this = cx.entity().downgrade();
        row_cell(c, column.width, column.kind.right_aligned() || fk)
            .id(("cell", row * 10_000 + col))
            .when(edited, |s| s.bg(rgb(c.amber_tint)))
            .when(selected, |s| s.border_1().border_color(rgb(c.blue)))
            .on_mouse_down(MouseButton::Left, cx.listener(move |g, e: &MouseDownEvent, window, cx| {
                g.focus.focus(window);
                if g.editing.is_some() {
                    g.finish_edit(true, window, cx);
                }
                g.select(row, col, cx);
                if e.click_count == 2 {
                    g.start_edit(None, window, cx);
                }
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |g, e: &MouseDownEvent, window, cx| {
                g.focus.focus(window);
                g.select(row, col, cx);
                cx.emit(GridEvent::ContextMenu { position: e.position });
            }))
            .when(fk, |s| {
                s.child(
                    div()
                        .id(("fk", row * 10_000 + col))
                        .flex_shrink_0()
                        .opacity(if selected { 1. } else { 0.55 })
                        .hover(|s| s.opacity(1.))
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            cx.stop_propagation();
                            let _ = this.update(cx, |g, cx| {
                                if let Some(ev) = g.foreign_of(row, col) {
                                    cx.emit(ev);
                                }
                            });
                        })
                        .child(icon(Glyph::Arrow, 12., c.cyan)),
                )
            })
            .child(content)
            .into_any_element()
    }

    fn render_row(&self, c: Palette, ix: usize, offset: f32, cx: &mut Context<Self>) -> AnyElement {
        let r = &self.rows[ix];
        let selected = self.sel.is_some_and(|(s, _)| s == ix);
        let deleted = self.deleted.contains(&r.key);
        let (bg, mark) = if r.inserted {
            (Some(hx(c.green_tint)), Some(c.green))
        } else if deleted {
            (Some(hx(c.red_tint)), Some(c.red))
        } else if selected {
            (Some(hx(c.selected)), Some(c.blue))
        } else if ix % 2 == 1 {
            (Some(hx(c.text).opacity(if c.dark { 0.012 } else { 0.014 })), None)
        } else {
            (None, None)
        };
        let number = if r.inserted { "+".to_string() } else if deleted { "−".to_string() } else { (self.first_number + ix).to_string() };
        // Only the columns in view are built.
        let (mut x, view) = (0f32, self.side.borrow().viewport.max(2000.));
        let mut cells: Vec<AnyElement> = Vec::new();
        let mut lead = 0f32;
        for col in 0..self.columns.len() {
            let w = self.columns[col].width;
            if x + w < offset {
                lead += w;
            } else if x <= offset + view {
                cells.push(self.render_cell(c, ix, col, cx));
            }
            x += w;
        }
        div()
            .id(("row", ix))
            .w_full()
            .h(px(ROW_H))
            .flex()
            .overflow_hidden()
            .when_some(bg, |s, bg| s.bg(bg))
            .when(!selected && !r.inserted && !deleted, |s| s.hover(|s| s.bg(hx(c.hover).opacity(0.55))))
            .when(deleted, |s| s.text_color(rgb(c.quiet)).line_through())
            .child(
                div()
                    .relative()
                    .w(px(GUTTER))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_end()
                    .pr(px(8.))
                    .bg(rgb(c.chrome))
                    .when_some(bg.filter(|_| selected || r.inserted || deleted), |s, bg| s.bg(bg))
                    .border_r_1()
                    .border_b_1()
                    .border_color(rgb(c.row_border))
                    .when_some(mark, |s, m| s.child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(rgb(m))))
                    .text_size(px(scaled_ui(11.)))
                    .text_color(rgb(mark.unwrap_or(c.quiet)))
                    .child(number),
            )
            .child(div().flex_1().h_full().overflow_hidden().child(row().h_full().ml(px(lead - offset)).children(cells)))
            .into_any_element()
    }

    fn render_header(&self, c: Palette, offset: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let cells = self.columns.iter().enumerate().map(|(i, col)| {
            let sort = self.sort.filter(|(s, _)| *s == i).map(|(_, d)| d);
            row()
                .id(("th", i))
                .relative()
                .w(px(col.width))
                .h_full()
                .flex_shrink_0()
                .px(px(9.))
                .gap(px(5.))
                .border_r_1()
                .border_color(rgb(c.row_border))
                .hover(|s| s.bg(hx(c.hover).opacity(0.7)))
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(GridEvent::Sort(i))))
                .on_mouse_down(MouseButton::Right, cx.listener(move |_, e: &MouseDownEvent, _, cx| cx.emit(GridEvent::HeaderMenu { col: i, position: e.position })))
                .when(col.pk, |s| s.child(icon(Glyph::Key, 12., c.amber)))
                .when(col.fk.is_some() && !col.pk, |s| s.child(icon(Glyph::Link, 12., c.cyan)))
                .child(div().flex_shrink_0().font_weight(FontWeight::MEDIUM).child(col.name.clone()))
                .child(div().flex_1().min_w_0().truncate().text_color(rgb(c.quiet)).text_size(px(scaled_ui(11.))).child(col.type_name.clone()))
                .when_some(sort, |s, desc| s.child(icon(if desc { Glyph::Down } else { Glyph::Up }, 12., c.blue)))
                .child(
                    div()
                        .id(("rz", i))
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right(px(-3.))
                        .w(px(7.))
                        .cursor_col_resize()
                        .hover(|s| s.bg(hx(c.blue).opacity(0.5)))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |g, e: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            g.resizing = Some((i, e.position.x, g.columns[i].width));
                        }))
                        .on_click(|_, _, cx| cx.stop_propagation()),
                )
        });
        row()
            .h(px(HEADER))
            .flex_shrink_0()
            .bg(rgb(c.chrome))
            .border_b_1()
            .border_color(rgb(c.border))
            .text_size(px(scaled_ui(12.5)))
            .child(div().w(px(GUTTER)).h_full().flex_shrink_0().border_r_1().border_color(rgb(c.row_border)))
            .child(div().flex_1().h_full().overflow_hidden().child(row().h_full().ml(px(-offset)).children(cells)))
    }
}

/// A cell's box: fixed width, borders, truncation.
fn row_cell(c: Palette, width: f32, right: bool) -> Div {
    row()
        .w(px(width))
        .h_full()
        .flex_shrink_0()
        .px(px(9.))
        .gap(px(4.))
        .overflow_hidden()
        .border_r_1()
        .border_b_1()
        .border_color(rgb(c.row_border))
        .when(right, |s| s.justify_end())
}

/// What a value looks like in a cell, by type.
pub fn cell_content(c: Palette, v: &Value, column: &GridColumn) -> AnyElement {
    let m = |s: String, color: u32| div().min_w_0().truncate().font_family(mono()).text_size(px(scaled_mono(11.5))).text_color(rgb(color)).child(s).into_any_element();
    match v {
        Value::Null => div().text_color(rgb(c.quiet)).italic().text_size(px(scaled_ui(11.))).child("NULL").into_any_element(),
        Value::Default => div().text_color(rgb(c.quiet)).italic().text_size(px(scaled_ui(11.))).child("DEFAULT").into_any_element(),
        Value::Bool(b) => m(b.to_string(), if *b { c.green } else { c.quiet }),
        Value::Int(_) | Value::Float(_) | Value::Number(_) => m(v.display(), c.text),
        Value::Json(j) => m(ellipsize(&j.to_string(), 300), c.code),
        Value::Bytes(_) => m(v.display(), c.muted),
        Value::Text(s) => match column.kind {
            Kind::Time => m(s.clone(), c.muted),
            Kind::Uuid => m(s.clone(), c.muted),
            Kind::Array => m(s.clone(), c.muted),
            Kind::Int | Kind::Float | Kind::Number => m(s.clone(), c.text),
            Kind::Enum => row()
                .gap(px(6.))
                .min_w_0()
                .child(dot(enum_color(c, s), 6.))
                .child(div().truncate().child(s.clone()))
                .into_any_element(),
            _ => div().min_w_0().truncate().child(ellipsize(&s.replace('\n', "↵ "), 300)).into_any_element(),
        },
    }
}

/// A stable colour for an enum value: common states by meaning, others by hash.
pub fn enum_color(c: Palette, v: &str) -> u32 {
    match v.to_ascii_lowercase().as_str() {
        "paid" | "active" | "done" | "complete" | "completed" | "success" | "succeeded" | "approved" | "enabled" | "open" | "published" => c.green,
        "pending" | "trialing" | "draft" | "processing" | "waiting" | "review" | "queued" => c.amber,
        "failed" | "error" | "refunded" | "cancelled" | "canceled" | "past_due" | "rejected" | "disabled" | "deleted" | "banned" => c.red,
        "shipped" | "sent" | "delivered" => c.cyan,
        _ => {
            let h = v.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
            c.lane(h as usize)
        }
    }
}

pub fn ellipsize(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

fn default_width<'a>(c: &GridColumn, sample: impl Iterator<Item = &'a Value>) -> f32 {
    let head = c.name.chars().count() as f32 * 7.6 + c.type_name.chars().count() as f32 * 6. + 44.;
    let fixed = match c.kind {
        Kind::Bool => Some(96.),
        Kind::Uuid => Some(290.),
        Kind::Time => Some(172.),
        Kind::Json => Some(300.),
        _ => None,
    };
    let body = fixed.unwrap_or_else(|| {
        let longest = sample.map(|v| v.display().chars().count().min(60)).max().unwrap_or(4) as f32;
        longest * 7.4 + 26. + if c.fk.is_some() { 16. } else { 0. }
    });
    head.max(body).clamp(72., 340.)
}

impl DataGrid {
    /// Sideways scrolling over the rows: swipes and shift-wheel move the columns,
    /// and a bar along the bottom shows where you are and can be dragged.
    fn sideways(&self, c: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let side = self.side.clone();
        let view = cx.entity_id();
        let (thumb, thumb_hover) = (hx(c.quiet).opacity(0.45), hx(c.quiet).opacity(0.75));
        canvas(
            {
                let side = side.clone();
                move |bounds, window, _| {
                    side.borrow_mut().viewport = f(bounds.size.width) - GUTTER;
                    window.insert_hitbox(bounds, HitboxBehavior::Normal)
                }
            },
            move |bounds, hitbox, window, _| {
                let track = Bounds::new(point(bounds.left() + px(GUTTER), bounds.bottom() - px(9.)), size(bounds.size.width - px(GUTTER + 12.), px(6.)));
                if let Some(bar) = side.borrow().thumb(track) {
                    let lit = side.borrow().drag.is_some() || track.contains(&window.mouse_position());
                    window.paint_quad(fill(bar, if lit { thumb_hover } else { thumb }).corner_radii(px(3.)));
                }
                let (s1, h1) = (side.clone(), hitbox.clone());
                window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture || !h1.should_handle_scroll(window) {
                        return;
                    }
                    let mut side = s1.borrow_mut();
                    let d = e.delta.pixel_delta(px(ROW_H));
                    // A swipe keeps to the axis it starts along, so rows don't drift diagonally.
                    let sideways = match e.delta {
                        ScrollDelta::Lines(_) => {
                            side.axis = None;
                            e.modifiers.shift || d.x.abs() > d.y.abs()
                        }
                        ScrollDelta::Pixels(_) => {
                            let now = std::time::Instant::now();
                            let idle = side.last.is_none_or(|t| now.duration_since(t).as_millis() > 150);
                            side.last = Some(now);
                            if matches!(e.touch_phase, TouchPhase::Started) || idle {
                                side.axis = None;
                            }
                            if side.axis.is_none() && (d.x != px(0.) || d.y != px(0.)) {
                                side.axis = Some(if e.modifiers.shift || d.x.abs() > d.y.abs() { Axis::Horizontal } else { Axis::Vertical });
                            }
                            side.axis == Some(Axis::Horizontal)
                        }
                    };
                    if !sideways {
                        return;
                    }
                    cx.stop_propagation();
                    let dx = f(if d.x == px(0.) { d.y } else { d.x });
                    let next = (side.offset - dx).clamp(0., side.max());
                    if next != side.offset {
                        side.offset = next;
                        cx.notify(view);
                    }
                });
                let s2 = side.clone();
                window.on_mouse_event(move |e: &MouseDownEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture || e.button != MouseButton::Left || !track.contains(&e.position) {
                        return;
                    }
                    let mut side = s2.borrow_mut();
                    let Some(bar) = side.thumb(track) else { return };
                    if !bar.contains(&e.position) {
                        let room = f(track.size.width - bar.size.width);
                        let at = (f(e.position.x - track.left()) - f(bar.size.width) / 2.).clamp(0., room);
                        side.offset = if room > 0. { side.max() * at / room } else { 0. };
                    }
                    side.drag = Some((e.position.x, side.offset));
                    cx.stop_propagation();
                    cx.notify(view);
                });
                let s3 = side.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, _, _, cx| {
                    let mut side = s3.borrow_mut();
                    let (Some((x0, from)), Some(bar)) = (side.drag, side.thumb(track)) else { return };
                    let room = f(track.size.width - bar.size.width);
                    if room > 0. {
                        side.offset = (from + f(e.position.x - x0) * side.max() / room).clamp(0., side.max());
                        cx.notify(view);
                    }
                });
                let s4 = side.clone();
                window.on_mouse_event(move |e: &MouseUpEvent, _, _, cx| {
                    if e.button == MouseButton::Left && s4.borrow_mut().drag.take().is_some() {
                        cx.notify(view);
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }
}

impl Render for DataGrid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = palette(cx);
        let content: f32 = self.columns.iter().map(|c| c.width).sum();
        let offset = {
            let mut side = self.side.borrow_mut();
            side.content = content;
            side.offset = side.offset.clamp(0., side.max());
            side.offset
        };
        let count = self.rows.len();
        let list = uniform_list(
            "grid-rows",
            count,
            cx.processor(move |g: &mut DataGrid, range: std::ops::Range<usize>, _window, cx| {
                let c = palette(cx);
                let x = g.side.borrow().offset;
                range.map(|ix| g.render_row(c, ix, x, cx)).collect::<Vec<_>>()
            }),
        )
        .track_scroll(self.scroll.clone())
        .flex_1()
        .text_size(px(scaled_ui(12.5)));
        let _ = window;
        col()
            .id("grid")
            .key_context("Grid")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .bg(rgb(c.background))
            .on_action(cx.listener(|g, _: &MoveUp, _, cx| g.step(-1, 0, cx)))
            .on_action(cx.listener(|g, _: &MoveDown, _, cx| g.step(1, 0, cx)))
            .on_action(cx.listener(|g, _: &MoveLeft, _, cx| g.step(0, -1, cx)))
            .on_action(cx.listener(|g, _: &MoveRight, _, cx| g.step(0, 1, cx)))
            .on_action(cx.listener(|g, _: &NextCell, _, cx| g.step(0, 1, cx)))
            .on_action(cx.listener(|g, _: &PrevCell, _, cx| g.step(0, -1, cx)))
            .on_action(cx.listener(|g, _: &PageUp, _, cx| g.step(-g.page_rows(), 0, cx)))
            .on_action(cx.listener(|g, _: &PageDown, _, cx| g.step(g.page_rows(), 0, cx)))
            .on_action(cx.listener(|g, _: &MoveTop, _, cx| g.step(-(g.rows.len() as isize), 0, cx)))
            .on_action(cx.listener(|g, _: &MoveBottom, _, cx| g.step(g.rows.len() as isize, 0, cx)))
            .on_action(cx.listener(|g, _: &EditCell, window, cx| {
                if g.editing.is_none() {
                    g.start_edit(None, window, cx)
                }
            }))
            .on_action(cx.listener(|g, _: &CommitDown, window, cx| {
                g.finish_edit(true, window, cx);
                g.step(1, 0, cx);
            }))
            .on_action(cx.listener(|g, _: &CommitUp, window, cx| {
                g.finish_edit(true, window, cx);
                g.step(-1, 0, cx);
            }))
            .on_action(cx.listener(|g, _: &ClearCell, window, cx| g.start_edit(Some(String::new()), window, cx)))
            .on_action(cx.listener(|g, _: &DeleteRow, _, cx| g.toggle_delete(cx)))
            .on_action(cx.listener(|g, _: &CopyCell, _, cx| g.copy_selected(cx)))
            .on_action(cx.listener(|g, _: &CancelEdit, window, cx| g.finish_edit(false, window, cx)))
            .on_action(cx.listener(|g, _: &CommitNext, window, cx| {
                g.finish_edit(true, window, cx);
                g.step(0, 1, cx);
            }))
            .on_action(cx.listener(|g, _: &CommitPrev, window, cx| {
                g.finish_edit(true, window, cx);
                g.step(0, -1, cx);
            }))
            .on_key_down(cx.listener(|g, e: &KeyDownEvent, window, cx| {
                // Typing on a selected cell starts editing it with what was typed.
                let m = &e.keystroke.modifiers;
                if g.editing.is_some() || m.platform || m.control || m.alt || !g.focus.is_focused(window) {
                    return;
                }
                if let Some(ch) = e.keystroke.key_char.as_ref().filter(|s| s.chars().all(|c| !c.is_control()) && !s.is_empty()) {
                    cx.stop_propagation();
                    g.start_edit(Some(ch.clone()), window, cx);
                }
            }))
            .on_mouse_move(cx.listener(|g, e: &MouseMoveEvent, _, cx| {
                if let Some((i, x0, w0)) = g.resizing {
                    if e.pressed_button != Some(MouseButton::Left) {
                        g.resizing = None;
                        return;
                    }
                    g.columns[i].width = (w0 + f(e.position.x - x0)).max(56.);
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|g, _, _, _| g.resizing = None))
            .child(self.render_header(c, offset, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(list)
                    .child(div().absolute().top_0().right_0().bottom_0().w(px(12.)).child(Scrollbar::vertical(&self.scroll)))
                    .child(self.sideways(c, cx))
                    .when(count == 0 && !self.columns.is_empty(), |s| {
                        s.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(rgb(c.quiet))
                                .child("No rows"),
                        )
                    }),
            )
    }
}
