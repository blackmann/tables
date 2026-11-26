//! The table list: pinned, tables, views, functions and saved queries.
use super::*;
use crate::db::RelationKind;
use crate::icons::Glyph;
use crate::theme::{Palette, scaled_ui};
use gpui_component::Icon;
use gpui_component::input::Input;

impl Workspace {
    pub(super) fn render_sidebar(&self, c: Palette, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(session) = self.session.clone() else { return div() };
        let s = session.read(cx);
        let q = self.sidebar_filter.read(cx).value().to_lowercase();
        let matches = |n: &str| q.is_empty() || n.to_lowercase().contains(&q);
        let conn_id = s.config.id.clone();
        let state = self.store.state.get(&conn_id).cloned().unwrap_or_default();
        let open: HashSet<String> = self.tabs.iter().filter_map(|t| if let Tab::Table(t) = t { Some(t.read(cx).table.clone()) } else { None }).collect();
        let active_table = match self.active_tab() {
            Some(Tab::Table(t)) => Some(t.read(cx).table.clone()),
            _ => None,
        };
        let item = |key: String, name: String, glyph: Glyph, count: Option<String>, cx: &mut Context<Self>| {
            let active = active_table.as_deref() == Some(name.as_str());
            let is_open = open.contains(&name);
            let (n1, n2) = (name.clone(), name.clone());
            row()
                .id(SharedString::from(key))
                .h(px(26.))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(6.))
                .when(active, |s| s.bg(rgb(c.selected)))
                .when(!active, |s| s.hover(|s| s.bg(rgb(c.hover))))
                .child(icon(glyph, 14., if active { c.blue } else { c.quiet }))
                .child(highlight(c, &name, &q).flex_1().min_w_0().when(is_open, |s| s.font_weight(FontWeight::MEDIUM)))
                .when_some(count, |s, n| s.child(div().text_size(px(scaled_ui(11.))).text_color(rgb(c.quiet)).child(n)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |w, e: &MouseDownEvent, window, cx| {
                    w.open_table(n1.clone(), e.click_count < 2, None, window, cx);
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(move |w, e: &MouseDownEvent, window, cx| {
                    let build = w.table_menu(n2.clone(), cx);
                    w.open_menu(build, e.position, 230., None, window, cx);
                }))
        };
        let count = |r: &crate::db::Relation| r.rows.map(|n| crate::db::compact(n as f64));
        let pinned: Vec<_> = s.relations.iter().filter(|r| state.pinned.contains(&r.name) && matches(&r.name)).cloned().collect();
        let tables: Vec<_> = s.relations.iter().filter(|r| r.kind == RelationKind::Table && matches(&r.name)).cloned().collect();
        let views: Vec<_> = s.relations.iter().filter(|r| r.kind == RelationKind::View && matches(&r.name)).cloned().collect();
        let fns: Vec<_> = s.functions.iter().filter(|f| matches(&f.0)).cloned().collect();
        let saved: Vec<_> = state.saved.iter().filter(|q| matches(&q.name)).cloned().collect();
        let loading = s.loading;
        let nothing = !q.is_empty() && pinned.is_empty() && tables.is_empty() && views.is_empty() && fns.is_empty() && saved.is_empty();

        let mut sections: Vec<AnyElement> = vec![];
        let mut section = |id: &'static str, title: &str, items: Vec<AnyElement>, cx: &mut Context<Self>| {
            if items.is_empty() {
                return;
            }
            let closed = self.closed_sections.contains(id);
            let n = items.len();
            sections.push(
                col()
                    .mt(px(6.))
                    .child(
                        row()
                            .id(id)
                            .px(px(6.))
                            .py(px(5.))
                            .gap(px(4.))
                            .text_size(px(scaled_ui(11.)))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(c.quiet))
                            .child(icon(if closed { Glyph::Right } else { Glyph::Down }, 12., c.quiet))
                            .child(title.to_uppercase())
                            .child(div().flex_1())
                            .child(div().font_weight(FontWeight::NORMAL).child(n.to_string()))
                            .on_click(cx.listener(move |w, _, _, cx| {
                                if !w.closed_sections.remove(id) {
                                    w.closed_sections.insert(id);
                                }
                                cx.notify();
                            })),
                    )
                    .when(!closed, |s| s.children(items))
                    .into_any_element(),
            );
        };
        let pinned_items = pinned.iter().map(|r| item(format!("pin-{}", r.name), r.name.clone(), if r.kind == RelationKind::View { Glyph::View } else { Glyph::Table }, count(r), cx).into_any_element()).collect();
        section("pinned", "Pinned", pinned_items, cx);
        let table_items = tables.iter().map(|r| item(format!("t-{}", r.name), r.name.clone(), Glyph::Table, count(r), cx).into_any_element()).collect();
        section("tables", "Tables", table_items, cx);
        let view_items = views.iter().map(|r| item(format!("v-{}", r.name), r.name.clone(), Glyph::View, count(r), cx).into_any_element()).collect();
        section("views", "Views", view_items, cx);
        let fn_items = fns
            .iter()
            .map(|(name, args)| {
                let (n, a) = (name.clone(), args.clone());
                row()
                    .id(SharedString::from(format!("f-{name}{args}")))
                    .h(px(26.))
                    .px(px(8.))
                    .gap(px(8.))
                    .rounded(px(6.))
                    .hover(|s| s.bg(rgb(c.hover)))
                    .child(icon(Glyph::Function, 14., c.quiet))
                    .child(row().flex_1().min_w_0().overflow_hidden().child(highlight(c, name, &q)).child(div().truncate().text_color(rgb(c.quiet)).child(args.clone())))
                    .on_click(cx.listener(move |w, _, window, cx| {
                        let call = if a == "()" { format!("SELECT {n}();\n") } else { format!("-- {n}{a}\nSELECT {n}();\n") };
                        w.new_query(Some(format!("{n}()")), call, false, window, cx)
                    }))
                    .into_any_element()
            })
            .collect();
        section("functions", "Functions", fn_items, cx);
        let saved_items = saved
            .iter()
            .map(|sq| {
                let (n, n2) = (sq.name.clone(), sq.name.clone());
                row()
                    .id(SharedString::from(format!("s-{}", sq.name)))
                    .h(px(26.))
                    .px(px(8.))
                    .gap(px(8.))
                    .rounded(px(6.))
                    .hover(|s| s.bg(rgb(c.hover)))
                    .child(icon(Glyph::Sql, 14., c.amber))
                    .child(highlight(c, &sq.name, &q).flex_1().min_w_0())
                    .on_click(cx.listener(move |w, _, window, cx| w.open_saved(&n, window, cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |w, e: &MouseDownEvent, window, cx| {
                        let weak = w.self_weak.clone();
                        let name = n2.clone();
                        let build: menu::Builder = std::rc::Rc::new(move |_| {
                            let (w1, w2, n1, n2) = (weak.clone(), weak.clone(), name.clone(), name.clone());
                            vec![
                                menu::Entry::Item(menu::Item::new("Open").icon(Icon::new(Glyph::Sql)).on_click(move |window, cx| {
                                    let _ = w1.update(cx, |w, cx| w.open_saved(&n1, window, cx));
                                })),
                                menu::Entry::Item(menu::Item::new("Delete").icon(Icon::new(Glyph::Trash)).on_click(move |_, cx| {
                                    let _ = w2.update(cx, |w, cx| {
                                        if let Some(id) = w.conn_id(cx) {
                                            w.store.state(&id).saved.retain(|q| q.name != n2);
                                            w.save_store();
                                        }
                                        cx.notify();
                                    });
                                })),
                            ]
                        });
                        w.open_menu(build, e.position, 180., None, window, cx);
                    }))
                    .into_any_element()
            })
            .collect();
        section("saved", "Saved queries", saved_items, cx);

        col()
            .w(px(self.sidebar_width))
            .flex_shrink_0()
            .h_full()
            .bg(rgb(c.chrome))
            .border_r_1()
            .border_color(rgb(c.border))
            .child(
                col()
                    .p(px(10.))
                    .pb(px(8.))
                    .gap(px(8.))
                    .child(
                        row()
                            .h(px(28.))
                            .px(px(8.))
                            .gap(px(7.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(rgb(c.border))
                            .bg(rgb(c.background))
                            .child(icon(Glyph::Search, 14., c.quiet))
                            .child(div().flex_1().child(Input::new(&self.sidebar_filter).appearance(false).cleanable(true)))
                            .child(kbd(c, "⌘F")),
                    ),
            )
            .child(
                div()
                    .id("sidebar-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(6.))
                    .pb(px(10.))
                    .when(loading && sections.is_empty(), |s| s.child(div().p(px(10.)).text_color(rgb(c.quiet)).child("Loading…")))
                    .children(sections)
                    .when(nothing, |s| s.child(div().py(px(20.)).text_center().text_color(rgb(c.quiet)).child(format!("Nothing matches “{q}”")))),
            )
    }

    pub(super) fn schema_menu(&self, cx: &App) -> menu::Builder {
        let Some(session) = self.session.clone() else { return std::rc::Rc::new(|_| vec![]) };
        let schemas = session.read(cx).schemas.clone();
        let current = session.read(cx).schema.clone();
        let weak = self.self_weak.clone();
        std::rc::Rc::new(move |_| {
            schemas
                .iter()
                .map(|s| {
                    let (session, s2, weak) = (session.clone(), s.clone(), weak.clone());
                    menu::Entry::Item(menu::Item::new(s.clone()).checked(*s == current).on_click(move |_, cx| {
                        session.update(cx, |sess, cx| sess.set_schema(s2.clone(), cx));
                        let id = session.read(cx).config.id.clone();
                        let _ = weak.update(cx, |w, cx| {
                            w.store.state(&id).schema = Some(s2.clone());
                            w.save_store();
                            w.tabs.retain(|t| matches!(t, Tab::Sql(_)));
                            w.active = (!w.tabs.is_empty()).then_some(0);
                            cx.notify();
                        });
                    }))
                })
                .collect()
        })
    }

    fn table_menu(&self, table: String, cx: &App) -> menu::Builder {
        let weak = self.self_weak.clone();
        let pinned = self.conn_id(cx).and_then(|id| self.store.state.get(&id)).is_some_and(|s| s.pinned.contains(&table));
        let qualified = self.session.as_ref().map(|s| s.read(cx).qualified(&table)).unwrap_or_default();
        std::rc::Rc::new(move |_| {
            let t = table.clone();
            let q = qualified.clone();
            let w = weak.clone();
            let open = {
                let (w, t) = (w.clone(), t.clone());
                move |window: &mut Window, cx: &mut App| {
                    let _ = w.update(cx, |w, cx| w.open_table(t.clone(), false, None, window, cx));
                }
            };
            let structure = {
                let (w, t) = (w.clone(), t.clone());
                move |window: &mut Window, cx: &mut App| {
                    let _ = w.update(cx, |w, cx| {
                        w.open_table(t.clone(), false, None, window, cx);
                        if let Some(Tab::Table(v)) = w.active_tab().cloned() {
                            v.update(cx, |v, cx| {
                                v.mode = Mode::Structure;
                                cx.notify();
                            });
                        }
                    });
                }
            };
            let query = {
                let (w, t, q) = (w.clone(), t.clone(), q.clone());
                move |window: &mut Window, cx: &mut App| {
                    let _ = w.update(cx, |w, cx| w.new_query(Some(t.clone()), format!("SELECT *\nFROM {q}\nLIMIT 100;\n"), true, window, cx));
                }
            };
            let pin = {
                let (w, t) = (w.clone(), t.clone());
                move |_: &mut Window, cx: &mut App| {
                    let _ = w.update(cx, |w, cx| {
                        if let Some(id) = w.conn_id(cx) {
                            let p = &mut w.store.state(&id).pinned;
                            if !p.remove(&t) {
                                p.insert(t.clone());
                            }
                            w.save_store();
                        }
                        cx.notify();
                    });
                }
            };
            let copy = {
                let t = t.clone();
                move |_: &mut Window, cx: &mut App| cx.write_to_clipboard(ClipboardItem::new_string(t.clone()))
            };
            // Destructive statements are written into a query tab, never run directly.
            let draft = |sql: String| {
                let (w, t) = (w.clone(), t.clone());
                move |window: &mut Window, cx: &mut App| {
                    let _ = w.update(cx, |w, cx| w.new_query(Some(format!("{t} · careful")), sql.clone(), false, window, cx));
                }
            };
            vec![
                menu::Entry::Item(menu::Item::new("Open in new tab").icon(Icon::new(Glyph::Table)).on_click(open)),
                menu::Entry::Item(menu::Item::new("Show structure").icon(Icon::new(Glyph::Key)).on_click(structure)),
                menu::Entry::Item(menu::Item::new("New query on table").icon(Icon::new(Glyph::Sql)).on_click(query)),
                menu::Entry::Item(menu::Item::new(if pinned { "Unpin" } else { "Pin to top" }).icon(Icon::new(Glyph::Pin)).on_click(pin)),
                menu::Entry::Separator,
                menu::Entry::Item(menu::Item::new("Export").icon(Icon::new(Glyph::Download)).submenu({
                    let (w, t) = (w.clone(), t.clone());
                    std::rc::Rc::new(move |_| {
                        [(ExportFormat::Csv, "CSV…"), (ExportFormat::Json, "JSON…"), (ExportFormat::Sql, "SQL INSERT statements…")]
                            .into_iter()
                            .map(|(fmt, label)| {
                                let (w, t) = (w.clone(), t.clone());
                                menu::Entry::Item(menu::Item::new(label).on_click(move |window, cx| {
                                    let _ = w.update(cx, |w, cx| w.choose_export_columns(t.clone(), fmt, window, cx));
                                }))
                            })
                            .collect()
                    })
                })),
                menu::Entry::Item(menu::Item::new("Copy name").icon(Icon::new(Glyph::Copy)).on_click(copy)),
                menu::Entry::Item(menu::Item::new("Truncate…").icon(Icon::new(Glyph::Refresh)).on_click(draft(format!("-- Removes every row of {q}. Review, then run with ⌘↵.\nTRUNCATE TABLE {q};\n")))),
                menu::Entry::Item(menu::Item::new("Drop…").icon(Icon::new(Glyph::Trash)).on_click(draft(format!("-- Deletes {q} and its data. Review, then run with ⌘↵.\nDROP TABLE {q};\n")))),
            ]
        })
    }
}

#[derive(Clone, Copy)]
pub enum ExportFormat {
    Csv,
    Json,
    Sql,
}

impl ExportFormat {
    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Csv => "CSV",
            ExportFormat::Json => "JSON",
            ExportFormat::Sql => "SQL INSERTs",
        }
    }
}

impl Workspace {
    /// Ask which columns to export; straight to the file prompt when the columns aren't known yet.
    pub(super) fn choose_export_columns(&mut self, table: String, format: ExportFormat, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.session else { return };
        let columns: Vec<(String, String, bool)> =
            session.read(cx).columns.get(&table).map(|cols| cols.iter().map(|(n, t)| (n.clone(), t.clone(), true)).collect()).unwrap_or_default();
        if columns.is_empty() {
            return self.export_table(table, format, None, cx);
        }
        self.modal = Some(Modal::Export { table, format, columns });
        self.modal_focus.focus(window);
        cx.notify();
    }

    /// Write every row of a table to a file the user picks; only `columns`, in that order, when given.
    pub(super) fn export_table(&mut self, table: String, format: ExportFormat, columns: Option<Vec<String>>, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else { return };
        let (engine, qualified) = {
            let s = session.read(cx);
            (s.engine(), s.qualified(&table))
        };
        let ext = match format {
            ExportFormat::Csv => "csv",
            ExportFormat::Json => "json",
            ExportFormat::Sql => "sql",
        };
        let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Downloads");
        let rx = cx.prompt_for_new_path(&dir, Some(&format!("{table}.{ext}")));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            let Ok(task) = session.read_with(cx, |s, cx| {
                let list = match &columns {
                    Some(cols) => cols.iter().map(|c| engine.quote(c)).collect::<Vec<_>>().join(", "),
                    None => "*".into(),
                };
                let sql = format!("SELECT {list} FROM {qualified}");
                s.run(cx, move |d| d.query(&sql))
            }) else {
                return;
            };
            let page = task.await;
            let msg = match page {
                Ok(page) => {
                    let cols: Vec<String> = page.columns.iter().map(|c| c.name.clone()).collect();
                    let body = match format {
                        ExportFormat::Csv => crate::export::csv(&cols, &page.rows),
                        ExportFormat::Json => crate::export::json(&cols, &page.rows),
                        ExportFormat::Sql => crate::export::inserts(engine, &qualified, &cols, &page.rows),
                    };
                    match std::fs::write(&path, body) {
                        Ok(()) => Ok(format!("Exported {} rows to {}", crate::ui::fmt_int(page.rows.len() as i64), path.display())),
                        Err(e) => Err(format!("Couldn't write {}: {e}", path.display())),
                    }
                }
                Err(e) => Err(format!("Couldn't read {table}: {e}")),
            };
            let _ = this.update(cx, |w, cx| match msg {
                Ok(m) => w.toast(m, cx),
                Err(m) => w.toast_error(m, cx),
            });
        })
        .detach();
    }
}

/// A name with the filter's match in blue.
fn highlight(c: Palette, name: &str, q: &str) -> Div {
    let base = div().truncate();
    if q.is_empty() {
        return base.child(name.to_string());
    }
    match name.to_lowercase().find(q) {
        Some(i) if name.is_char_boundary(i) && name.is_char_boundary(i + q.len()) => {
            let hl = HighlightStyle { color: Some(hx(c.blue)), font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() };
            base.child(StyledText::new(name.to_string()).with_highlights([(i..i + q.len(), hl)]))
        }
        _ => base.child(name.to_string()),
    }
}
