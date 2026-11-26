//! The row detail panel, docked on the right: every field of the selected row
//! as a form suited to its type, editable where the grid is.
use super::*;
use crate::db::Kind;
use crate::grid::{DataGrid, GridColumn};
use crate::icons::Glyph;
use crate::theme::{Palette, mono, scaled_mono, scaled_ui};
use gpui_component::Sizable;
use gpui_component::input::Input;
use std::time::Instant;

#[derive(Default)]
pub struct DetailState {
    signature: Option<(EntityId, usize, u64)>,
    /// The grid and columns the editors were made for; they're reused until it changes.
    layout: Option<(EntityId, Vec<(String, Kind)>)>,
    inputs: Vec<Option<Entity<InputState>>>,
    find: Option<Entity<InputState>>,
    _find_sub: Option<Subscription>,
    _subs: Vec<Subscription>,
    scroll: ScrollHandle,
    /// When the pane itself last scrolled. While a gesture keeps it moving, wheel
    /// events stay with the pane rather than being caught by a JSON editor that
    /// slides under the pointer.
    scrolled_at: Rc<Cell<Option<Instant>>>,
}

/// A pause longer than this ends the pane's hold on the scroll.
const SCROLL_LATCH: Duration = Duration::from_millis(180);

fn edit_text(v: &Value) -> String {
    match v {
        Value::Json(j) => serde_json::to_string_pretty(j).unwrap_or_default(),
        other => other.edit_text(),
    }
}

fn placeholder(v: &Value) -> &'static str {
    match v {
        Value::Null => "NULL",
        Value::Default => "DEFAULT",
        _ => "",
    }
}

fn hash_row(g: &DataGrid, row: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for i in 0..g.columns.len() {
        g.value(row, i).map(|v| v.display()).hash(&mut h);
    }
    g.editable.hash(&mut h);
    g.is_deleted(row).hash(&mut h);
    h.finish()
}

fn uses_input(col: &GridColumn) -> bool {
    !matches!(col.kind, Kind::Bool | Kind::Enum)
}

impl Workspace {
    fn detail_grid(&self, cx: &App) -> Option<(Entity<DataGrid>, String, bool)> {
        match self.active_tab()? {
            Tab::Table(t) => Some((t.read(cx).grid.clone(), t.read(cx).table.clone(), true)),
            Tab::Sql(s) => Some((s.read(cx).grid.clone(), "Result".into(), false)),
        }
    }

    /// Keep the field editors in step with the selected row. Editors are made
    /// once per table layout and only have their text swapped as the row changes,
    /// so moving through rows stays cheap.
    fn prepare_detail(&mut self, grid: &Entity<DataGrid>, window: &mut Window, cx: &mut Context<Self>) {
        if self.detail.find.is_none() {
            let find = cx.new(|cx| InputState::new(window, cx).placeholder("Find field"));
            self.detail._find_sub = Some(cx.subscribe(&find, |_, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    cx.notify();
                }
            }));
            self.detail.find = Some(find);
        }
        let g = grid.read(cx);
        let Some((row, _)) = g.sel else {
            self.detail.signature = None;
            return;
        };
        let Some(r) = g.rows.get(row) else { return };
        let sig = (grid.entity_id(), r.key, hash_row(g, row));
        if self.detail.signature == Some(sig) {
            return;
        }
        self.detail.signature = Some(sig);
        let columns = g.columns.clone();
        let values: Vec<Value> = (0..columns.len()).map(|i| g.value(row, i).cloned().unwrap_or(Value::Null)).collect();
        let layout = (grid.entity_id(), columns.iter().map(|c| (c.name.clone(), c.kind)).collect::<Vec<_>>());
        if self.detail.layout.as_ref() != Some(&layout) {
            self.detail.layout = Some(layout);
            self.detail.inputs.clear();
            self.detail._subs.clear();
            for (i, col) in columns.iter().enumerate() {
                if !uses_input(col) {
                    self.detail.inputs.push(None);
                    continue;
                }
                let sample = edit_text(&values[i]);
                let long = col.kind == Kind::Json || (col.kind == Kind::Text && (sample.contains('\n') || sample.chars().count() > 40));
                let input = cx.new(|cx| {
                    let s = InputState::new(window, cx);
                    if col.kind == Kind::Json {
                        s.code_editor("json").line_number(false).soft_wrap(true)
                    } else if long {
                        s.auto_grow(2, 8)
                    } else {
                        s
                    }
                });
                let grid = grid.clone();
                let kind = col.kind;
                self._subs_detail_push(cx.subscribe(&input, move |_, input, e: &InputEvent, cx| {
                    let commit = matches!(e, InputEvent::Blur) || matches!(e, InputEvent::PressEnter { secondary: false } if !long);
                    if !commit {
                        return;
                    }
                    let text = input.read(cx).value().to_string();
                    grid.update(cx, |g, cx| {
                        let Some((row, _)) = g.sel else { return };
                        if !g.editable || g.is_deleted(row) {
                            return;
                        }
                        let old = g.value(row, i).cloned().unwrap_or(Value::Null);
                        if edit_text(&old) == text {
                            return;
                        }
                        if let (Value::Json(a), Ok(b)) = (&old, serde_json::from_str::<serde_json::Value>(&text))
                            && *a == b
                        {
                            return;
                        }
                        let v = Value::parse(&text, kind, old.is_null() || old == Value::Default);
                        let v = if old == Value::Default && text.is_empty() { Value::Default } else { v };
                        g.set_value(row, i, v, cx);
                    });
                }));
                self.detail.inputs.push(Some(input));
            }
        }
        for (i, input) in self.detail.inputs.clone().into_iter().enumerate() {
            let Some(input) = input else { continue };
            if input.read(cx).focus_handle(cx).is_focused(window) {
                continue;
            }
            let text = edit_text(&values[i]);
            let ph = placeholder(&values[i]);
            input.update(cx, |s, cx| {
                if s.value().as_ref() != text {
                    s.set_value(text, window, cx);
                }
                s.set_placeholder(ph, window, cx);
            });
        }
    }

    fn _subs_detail_push(&mut self, sub: Subscription) {
        self.detail._subs.push(sub);
    }

    pub(super) fn render_detail(&mut self, c: Palette, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = self.detail_width;
        let shell = || col().w(px(width)).flex_shrink_0().h_full().bg(rgb(c.chrome)).border_l_1().border_color(rgb(c.border));
        let header = |title: String, sub: String| {
            row()
                .h(px(BAR))
                .flex_shrink_0()
                .pl(px(12.))
                .pr(px(6.))
                .gap(px(6.))
                .border_b_1()
                .border_color(rgb(c.border))
                .child(row().flex_1().min_w_0().gap(px(6.)).child(div().font_weight(FontWeight::SEMIBOLD).child(title)).child(div().truncate().text_size(px(scaled_ui(12.))).text_color(rgb(c.quiet)).child(sub)))
        };
        let empty = |msg: &str| col().flex_1().items_center().justify_center().text_color(rgb(c.quiet)).child(msg.to_string());
        let Some((grid, title, is_table)) = self.detail_grid(cx) else {
            return shell().child(header("Row".into(), String::new())).child(empty("No tab open"));
        };
        self.prepare_detail(&grid, window, cx);
        let g = grid.read(cx);
        let Some((row_ix, _)) = g.sel.filter(|(r, _)| *r < g.rows.len()) else {
            return shell().child(header("Row".into(), String::new())).child(empty(if is_table { "Select a row to see it here" } else { "Run a query and select a row" }));
        };
        let pk = g.columns.iter().position(|c| c.pk);
        let r = &g.rows[row_ix];
        let sub = if r.inserted {
            "new row".to_string()
        } else if let Some(p) = pk {
            format!("{} {}", g.columns[p].name, crate::grid::ellipsize(&g.value(row_ix, p).map(|v| v.display()).unwrap_or_default(), 16))
        } else {
            format!("row {} of {}", row_ix + 1, g.rows.len())
        };
        let editable = g.editable && !g.is_deleted(row_ix);
        let q = self.detail.find.as_ref().map(|f| f.read(cx).value().to_lowercase()).unwrap_or_default();
        let columns = g.columns.clone();
        let values: Vec<Value> = (0..columns.len()).map(|i| g.value(row_ix, i).cloned().unwrap_or(Value::Null)).collect();
        let edited: Vec<bool> = (0..columns.len()).map(|i| g.is_edited(row_ix, i)).collect();
        let pk_value = pk.map(|p| values[p].clone());
        let refs = if is_table && !r.inserted {
            self.session.as_ref().and_then(|s| s.read(cx).details.get(&title).map(|d| d.referenced_by.clone())).unwrap_or_default()
        } else {
            vec![]
        };
        let fields = columns.iter().enumerate().filter(|(_, col)| q.is_empty() || col.name.to_lowercase().contains(&q)).map(|(i, column)| {
                        let v = values[i].clone();
            let grid = grid.clone();
            let field_box = |el: Div| {
                el.w_full()
                    .min_h(px(28.))
                    .px(px(8.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgb(if edited[i] { c.amber } else { c.border }))
                    .bg(rgb(if edited[i] { c.amber_tint } else if editable { c.background } else { c.panel }))
            };
            let body: AnyElement = match column.kind {
                Kind::Bool => {
                    let on = matches!(v, Value::Bool(true));
                    let g2 = grid.clone();
                    row()
                        .id(("bool", i))
                        .h(px(28.))
                        .gap(px(8.))
                        .child(
                            div()
                                .w(px(30.))
                                .h(px(18.))
                                .rounded(px(9.))
                                .relative()
                                .bg(rgb(if on { c.green } else { c.border }))
                                .child(div().absolute().top(px(2.)).left(px(if on { 14. } else { 2. })).size(px(14.)).rounded_full().bg(rgb(0xffffff))),
                        )
                        .child(mono_text(12.).text_color(rgb(c.muted)).child(v.display()))
                        .when(editable, |s| {
                            s.on_click(move |_, _, cx| {
                                g2.update(cx, |g, cx| {
                                    if let Some((r, _)) = g.sel {
                                        g.set_value(r, i, Value::Bool(!on), cx);
                                    }
                                })
                            })
                        })
                        .into_any_element()
                }
                Kind::Enum => {
                    let options = column.enum_values.clone();
                    let nullable = column.nullable;
                    let g2 = grid.clone();
                    let current = v.display();
                    field_box(row())
                        .id(("enum", i))
                        .gap(px(6.))
                        .when(!v.is_null(), |s| s.child(dot(crate::grid::enum_color(c, &current), 7.)))
                        .child(div().flex_1().when(v.is_null(), |s| s.italic().text_color(rgb(c.quiet))).child(current.clone()))
                        .when(editable, |s| s.child(icon(Glyph::Down, 12., c.quiet)))
                        .when(editable, |s| {
                            s.on_mouse_down(MouseButton::Left, cx.listener(move |w, e: &MouseDownEvent, window, cx| {
                                let (opts, g3, cur) = (options.clone(), g2.clone(), current.clone());
                                let build: menu::Builder = std::rc::Rc::new(move |_| {
                                    let mut v: Vec<menu::Entry> = opts
                                        .iter()
                                        .map(|o| {
                                            let (g4, o2) = (g3.clone(), o.clone());
                                            menu::Entry::Item(menu::Item::new(o.clone()).checked(*o == cur).on_click(move |_, cx| {
                                                g4.update(cx, |g, cx| {
                                                    if let Some((r, _)) = g.sel {
                                                        g.set_value(r, i, Value::Text(o2.clone()), cx);
                                                    }
                                                })
                                            }))
                                        })
                                        .collect();
                                    if nullable {
                                        let g4 = g3.clone();
                                        v.push(menu::Entry::Separator);
                                        v.push(menu::Entry::Item(menu::Item::new("NULL").on_click(move |_, cx| {
                                            g4.update(cx, |g, cx| {
                                                if let Some((r, _)) = g.sel {
                                                    g.set_value(r, i, Value::Null, cx);
                                                }
                                            })
                                        })));
                                    }
                                    v
                                });
                                w.open_menu(build, e.position, 240., None, window, cx);
                            }))
                        })
                        .into_any_element()
                }
                _ => {
                    let input = self.detail.inputs.get(i).cloned().flatten();
                    let mono_kind = column.kind.mono() || column.fk.is_some();
                    let is_json = column.kind == Kind::Json;
                    let link = column.fk.clone().filter(|_| !v.is_null());
                    let content: AnyElement = match input {
                        // The input brings its own small inset; the box adds none.
                        Some(input) => Input::new(&input)
                            .appearance(false)
                            // Input sizes its text at 7/8 of this; match the grid's sizes.
                            .with_size(gpui_component::Size::Size(px(if mono_kind { crate::theme::scaled_mono(12.) } else { scaled_ui(12.5) } / 0.875)))
                            .disabled(!editable).when(is_json, |s| s.h_full()).into_any_element(),
                        None => div().px(px(8.)).child(v.display()).into_any_element(),
                    };
                    field_box(row())
                        .px_0()
                        .py(px(if is_json { 2. } else { 0. }))
                        .when(link.is_some(), |s| s.pr_0())
                        .when(mono_kind, |s| s.font_family(mono()).text_size(px(scaled_mono(12.))))
                        .when(is_json, |s| s.h(px((v.edit_text().lines().count().clamp(2, 14) as f32) * 18. + 8.)))
                        .when(is_json, |s| s.items_start())
                        .child(div().flex_1().min_w_0().flex().when(is_json, |s| s.h_full()).when(!is_json, |s| s.items_center()).child(content))
                        .when_some(link, |s, (ft, fc)| {
                            // The referenced table reads as part of the same field, past a divider.
                            let v2 = v.clone();
                            s.child(div().w(px(1.)).h(px(16.)).flex_shrink_0().bg(rgb(c.border)))
                                .child(
                                    row()
                                        .id(("fk-open", i))
                                        .h(px(26.))
                                        .flex_shrink_0()
                                        .px(px(9.))
                                        .gap(px(5.))
                                        .rounded_r(px(6.))
                                        .font_family(crate::theme::ui_font())
                                        .text_color(rgb(c.cyan))
                                        .text_size(px(scaled_ui(12.)))
                                        .hover(|s| s.bg(rgb(c.hover)))
                                        .child(icon(Glyph::Arrow, 12., c.cyan))
                                        .child(ft.clone())
                                        .on_click(cx.listener(move |w, _, window, cx| w.open_table(ft.clone(), false, Some((fc.clone(), v2.clone())), window, cx))),
                                )
                        })
                        .into_any_element()
                }
            };
            let g_null = grid.clone();
            col()
                .px(px(12.))
                .py(px(7.))
                .gap(px(5.))
                .border_b_1()
                .border_color(rgb(c.row_border))
                .child(
                    row()
                        .gap(px(6.))
                        .text_size(px(scaled_ui(11.5)))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(column.name.clone()))
                        .when(edited[i], |s| s.child(dot(c.amber, 5.)))
                        .child(div().font_family(mono()).text_size(px(scaled_mono(10.5))).text_color(rgb(c.quiet)).child(column.type_name.clone()))
                        .when(column.pk, |s| s.child(chip("PK", c.amber, tint(c.amber, CHIP_FILL))))
                        .when(column.fk.is_some(), |s| s.child(chip("FK", c.cyan, tint(c.cyan, CHIP_FILL))))
                        .child(div().flex_1())
                        .when(editable && column.nullable && !column.pk && !v.is_null(), |s| {
                            s.child(
                                div()
                                    .id(("null", i))
                                    .text_size(px(scaled_ui(11.)))
                                    .text_color(rgb(c.quiet))
                                    .hover(|s| s.text_color(rgb(c.text)))
                                    .child("Set NULL")
                                    .on_click(move |_, _, cx| {
                                        g_null.update(cx, |g, cx| {
                                            if let Some((r, _)) = g.sel {
                                                g.set_value(r, i, Value::Null, cx);
                                            }
                                        })
                                    }),
                            )
                        })
                        .child({
                            let text = v.display();
                            div()
                                .id(("copy", i))
                                .hover(|s| s.opacity(1.))
                                .opacity(0.6)
                                .child(icon(Glyph::Copy, 12., c.quiet))
                                .on_click(cx.listener(move |w, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                                    w.toast("Copied value".into(), cx);
                                }))
                        }),
                )
                .child(body)
        });
        let refs_el = (!refs.is_empty()).then(|| {
            col()
                .px(px(12.))
                .pt(px(12.))
                .gap(px(2.))
                .child(div().mb(px(4.)).text_size(px(scaled_ui(11.))).font_weight(FontWeight::SEMIBOLD).text_color(rgb(c.quiet)).child("REFERENCED BY"))
                .children(refs.into_iter().map(|(t, col)| {
                    let pkv = pk_value.clone();
                    let (t2, col2) = (t.clone(), col.clone());
                    row()
                        .id(SharedString::from(format!("ref-{t}-{col}")))
                        .h(px(28.))
                        .px(px(8.))
                        .gap(px(8.))
                        .rounded(px(6.))
                        .hover(|s| s.bg(rgb(c.hover)))
                        .child(icon(Glyph::Table, 14., c.quiet))
                        .child(row().flex_1().gap(px(4.)).child(t.clone()).child(mono_text(11.).text_color(rgb(c.quiet)).child(format!(".{col}"))))
                        .child(icon(Glyph::Right, 12., c.quiet))
                        .on_click(cx.listener(move |w, _, window, cx| w.open_table(t2.clone(), false, pkv.clone().map(|v| (col2.clone(), v)), window, cx)))
                }))
        });
        let find = self.detail.find.clone();
        let total = g.rows.len();
        shell()
            .child(
                header(title, sub)
                    .child(button(c, "row-prev", Some(Glyph::Up), None, Tone::Ghost).on_click(cx.listener(move |w, _, _, cx| {
                        if let Some((g, _, _)) = w.detail_grid(cx) {
                            g.update(cx, |g, cx| {
                                if let Some((r, c)) = g.sel {
                                    g.select(r.saturating_sub(1), c, cx);
                                }
                            });
                        }
                    })))
                    .child(button(c, "row-next", Some(Glyph::Down), None, Tone::Ghost).on_click(cx.listener(move |w, _, _, cx| {
                        if let Some((g, _, _)) = w.detail_grid(cx) {
                            g.update(cx, |g, cx| {
                                if let Some((r, c)) = g.sel {
                                    g.select((r + 1).min(total.saturating_sub(1)), c, cx);
                                }
                            });
                        }
                    })))
                    .child(button(c, "row-json", Some(Glyph::Copy), None, Tone::Ghost).on_click(cx.listener(move |w, _, _, cx| {
                        if let Some((g, _, _)) = w.detail_grid(cx) {
                            let json = g.read(cx).sel.map(|(r, _)| g.read(cx).row_as_json(r)).unwrap_or_default();
                            cx.write_to_clipboard(ClipboardItem::new_string(json));
                            w.toast("Copied row as JSON".into(), cx);
                        }
                    }))),
            )
            .when_some(find, |s, find| {
                s.child(
                    div().px(px(10.)).py(px(8.)).border_b_1().border_color(rgb(c.border)).child(
                        row()
                            .h(px(26.))
                            .px(px(8.))
                            .gap(px(6.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(rgb(c.border))
                            .bg(rgb(c.background))
                            .child(icon(Glyph::Search, 12., c.quiet))
                            .child(div().flex_1().child(Input::new(&find).appearance(false).cleanable(true))),
                    ),
                )
            })
            .child(
                div()
                    .id("detail-fields")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.detail.scroll)
                    .pb(px(20.))
                    .children(fields)
                    .children(refs_el)
                    .child(scroll_latch(self.detail.scroll.clone(), self.detail.scrolled_at.clone()))
                    // Only reached when no editor took the event: the pane is the one scrolling.
                    .on_scroll_wheel({
                        let at = self.detail.scrolled_at.clone();
                        move |_, _, _| at.set(Some(Instant::now()))
                    }),
            )
    }
}

/// Keeps a scroll gesture on the pane once the pane has started moving. It listens
/// in the capture phase, ahead of the editors, and while the latch holds it scrolls
/// the pane itself and stops the event from reaching anything underneath.
fn scroll_latch(handle: ScrollHandle, at: Rc<Cell<Option<Instant>>>) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |_, _, window, _| {
            window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture || !handle.bounds().contains(&e.position) {
                    return;
                }
                if !at.get().is_some_and(|t| t.elapsed() < SCROLL_LATCH) {
                    return;
                }
                let delta = e.delta.pixel_delta(window.line_height());
                let mut offset = handle.offset();
                offset.y = (offset.y + delta.y).clamp(-handle.max_offset().height, px(0.));
                handle.set_offset(offset);
                at.set(Some(Instant::now()));
                window.refresh();
                cx.stop_propagation();
            });
        },
    )
    .absolute()
    .size_0()
}
