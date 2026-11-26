//! Title bar, tab strip, the active tab and the status bar.
use super::*;
use crate::icons::Glyph;
use crate::theme::{Palette, scaled_ui};
use gpui_component::TitleBar;

impl Workspace {
    pub(super) fn render_title(&self, c: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.session.as_ref().map(|s| s.read(cx));
        TitleBar::new().bg(rgb(c.chrome)).border_b_1().border_color(rgb(c.border)).child(
            row()
                .w_full()
                .min_w_0()
                .pr(px(8.))
                .gap(px(8.))
                .text_size(px(scaled_ui(13.)))
                .when_some(session, |s, session| {
                    let cfg = &session.config;
                    let (env, env_bg) = env_color(c, cfg.env);
                    s.child(
                        row()
                            .id("connection")
                            .relative()
                            .gap(px(8.))
                            .h(px(28.))
                            .pl(px(6.))
                            .pr(px(8.))
                            .rounded(px(7.))
                            .hover(|s| s.bg(rgb(c.hover)))
                            .child(engine_badge(cfg.engine, 20.))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(cfg.name.clone()))
                            .child(chip(cfg.env.label().to_uppercase(), env, env_bg))
                            .child(icon(Glyph::Down, 12., c.quiet))
                            .child(measure(&self.connection_button))
                            .on_mouse_down(MouseButton::Left, cx.listener(|w, e: &MouseDownEvent, window, cx| {
                                let build = w.connections_menu(cx);
                                let at = below(&w.connection_button, e.position);
                                w.open_menu(build, at, 300., None, window, cx);
                            })),
                    )
                    .child(div().text_color(rgb(c.quiet)).child("/"))
                    // ClickHouse's databases are its schemas: the picker stands in for the name.
                    .when(session.engine() != Engine::ClickHouse, |s| s.child(div().text_color(rgb(c.muted)).child(cfg.database_name())))
                    .when(session.engine().has_schemas(), |s| {
                        s.when(session.engine() != Engine::ClickHouse, |s| s.child(div().text_color(rgb(c.quiet)).child("/"))).child(
                            row()
                                .id("schema")
                                .relative()
                                .gap(px(4.))
                                .px(px(6.))
                                .py(px(2.))
                                .rounded(px(5.))
                                .text_color(rgb(c.muted))
                                .hover(|s| s.bg(rgb(c.hover)).text_color(rgb(c.text)))
                                .child(session.schema.clone())
                                .child(icon(Glyph::Down, 12., c.quiet))
                                .child(measure(&self.schema_button))
                                .on_mouse_down(MouseButton::Left, cx.listener(|w, e: &MouseDownEvent, window, cx| {
                                    let build = w.schema_menu(cx);
                                    let at = below(&w.schema_button, e.position);
                                    let title = if w.session.as_ref().is_some_and(|s| s.read(cx).engine() == Engine::ClickHouse) { "Databases" } else { "Schemas" };
                                    w.open_menu(build, at, 220., Some(title), window, cx);
                                })),
                        )
                    })
                })
                .when(session.is_none(), |s| s.child(div().font_weight(FontWeight::SEMIBOLD).child("Tables")))
                .child(div().flex_1())
                .when(self.session.is_some(), |s| {
                    s.child(
                        row()
                            .id("omni")
                            .w(px(380.))
                            .h(px(26.))
                            .px(px(8.))
                            .gap(px(8.))
                            .rounded(px(7.))
                            .border_1()
                            .border_color(rgb(c.border))
                            .bg(rgb(c.background))
                            .text_color(rgb(c.quiet))
                            .hover(|s| s.border_color(rgb(c.popover_border)))
                            .child(icon(Glyph::Search, 14., c.quiet))
                            .child(div().flex_1().child("Search tables, queries, commands…"))
                            .child(kbd(c, "⌘"))
                            .child(kbd(c, "K"))
                            .on_click(cx.listener(|w, _, window, cx| w.open_palette(window, cx))),
                    )
                    .child(div().flex_1())
                })
                .when(self.session.is_some(), |s| {
                    s.child(button(c, "toggle-detail", Some(Glyph::Panel), None, Tone::Ghost).on_click(cx.listener(|w, _, _, cx| {
                        w.detail_open = !w.detail_open;
                        cx.notify();
                    })))
                }),
        )
    }

    pub(super) fn connections_menu(&self, cx: &App) -> menu::Builder {
        let conns = self.store.connections.clone();
        let current = self.conn_id(cx);
        let weak = self.weak(cx);
        let colors = palette(cx);
        std::rc::Rc::new(move |_| {
            let mut v = vec![menu::Entry::Label("Connections".into())];
            for conn in &conns {
                let (weak, conn2) = (weak.clone(), conn.clone());
                v.push(menu::Entry::Item(
                    menu::Item::new(conn.name.clone())
                        .dot(env_color(colors, conn.env).0)
                        .detail(format!("{} · {}", conn.env.label(), conn.engine.label()))
                        .checked(Some(&conn.id) == current.as_ref())
                        .on_click(move |window, cx| {
                            let _ = weak.update(cx, |w, cx| w.switch_to(conn2.clone(), window, cx));
                        }),
                ));
            }
            let (w1, w2, w3) = (weak.clone(), weak.clone(), weak.clone());
            let cur = current.clone();
            v.push(menu::Entry::Separator);
            v.push(menu::Entry::Item(menu::Item::new("New connection…").icon(gpui_component::Icon::new(Glyph::Plus)).detail("⌘N").on_click(move |window, cx| {
                let _ = w1.update(cx, |w, cx| w.edit_connection(None, window, cx));
            })));
            v.push(menu::Entry::Item(menu::Item::new("Edit this connection…").icon(gpui_component::Icon::new(Glyph::Edit)).on_click(move |window, cx| {
                let _ = w2.update(cx, |w, cx| {
                    let c = cur.as_ref().and_then(|id| w.store.connections.iter().find(|c| &c.id == id).cloned());
                    w.edit_connection(c, window, cx)
                });
            })));
            v.push(menu::Entry::Item(menu::Item::new("Disconnect").icon(gpui_component::Icon::new(Glyph::Plug)).on_click(move |window, cx| {
                let _ = w3.update(cx, |w, cx| w.disconnect(window, cx));
            })));
            v
        })
    }

    pub(super) fn weak(&self, cx: &App) -> WeakEntity<Workspace> {
        let _ = cx;
        self.self_weak.clone()
    }

    pub(super) fn switch_to(&mut self, conn: ConnectionConfig, window: &mut Window, cx: &mut Context<Self>) {
        if self.conn_id(cx).as_deref() == Some(conn.id.as_str()) {
            return;
        }
        if self.tabs.iter().any(|t| matches!(t, Tab::Table(t) if t.read(cx).has_pending(cx))) {
            self.toast_error("Commit or discard pending changes before switching".into(), cx);
            return;
        }
        self.connect(conn, window, cx);
    }

    pub(super) fn render_tabs(&self, c: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.tabs.iter().enumerate().map(|(i, tab)| {
            let active = self.active == Some(i);
            let (glyph, color, title, dirty, preview) = match tab {
                Tab::Table(t) => {
                    let t = t.read(cx);
                    let view = t.is_view(cx);
                    (if view { Glyph::View } else { Glyph::Table }, if view { c.purple } else { c.blue }, t.title(), t.has_pending(cx), t.preview)
                }
                Tab::Sql(s) => {
                    let s = s.read(cx);
                    (Glyph::Sql, c.amber, s.title.clone(), s.dirty, false)
                }
            };
            let running = matches!(tab, Tab::Sql(s) if s.read(cx).running.is_some());
            row()
                .id(("tab", i))
                .group("tab")
                .relative()
                .h_full()
                .min_w(px(120.))
                .max_w(px(220.))
                .flex_shrink_0()
                .pl(px(12.))
                .pr(px(6.))
                .gap(px(7.))
                .border_r_1()
                .border_color(rgb(c.border))
                .text_color(rgb(if active { c.text } else { c.muted }))
                .when(active, |s| s.bg(rgb(c.background)))
                .when(!active, |s| s.hover(|s| s.bg(hx(c.hover).opacity(0.6))))
                .when(active, |s| s.child(div().absolute().top_0().left_0().right_0().h(px(2.)).bg(rgb(c.blue))))
                .child(icon(glyph, 14., color))
                .child(div().flex_1().min_w_0().truncate().when(preview, |s| s.italic()).child(title))
                .when(running, |s| s.child(dot(c.amber, 6.)))
                .child(
                    div()
                        .id(("close", i))
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .hover(|s| s.bg(rgb(c.hover)))
                        .when(dirty, |s| s.child(div().group_hover("tab", |s| s.invisible()).child(dot(c.muted, 8.))))
                        .when(!dirty, |s| s.child(div().invisible().when(active, |s| s.visible()).group_hover("tab", |s| s.visible()).child(icon(Glyph::Close, 12., c.quiet))))
                        .when(dirty, |s| s.child(div().absolute().invisible().group_hover("tab", |s| s.visible()).child(icon(Glyph::Close, 12., c.quiet))))
                        .on_click(cx.listener(move |w, _, window, cx| {
                            cx.stop_propagation();
                            w.close_tab(i, false, window, cx);
                        })),
                )
                .on_mouse_down(MouseButton::Left, cx.listener(move |w, e: &MouseDownEvent, window, cx| {
                    w.active = Some(i);
                    if e.click_count == 2
                        && let Some(Tab::Table(t)) = w.tabs.get(i)
                    {
                        t.update(cx, |t, _| t.preview = false);
                    }
                    w.refocus(window, cx);
                    cx.notify();
                }))
                .on_mouse_down(MouseButton::Middle, cx.listener(move |w, _, window, cx| w.close_tab(i, false, window, cx)))
        });
        row()
            .h(px(BAR))
            .flex_shrink_0()
            
            .bg(rgb(c.chrome))
            .border_b_1()
            .border_color(rgb(c.border))
            .child(div().id("tab-strip").flex_1().min_w_0().h_full().flex().overflow_x_scroll().children(tabs))
            .child(
                div()
                    .id("new-query")
                    .w(px(36.))
                    .h_full()
                    .flex_shrink_0()
                    .border_l_1()
                    .border_color(rgb(c.border))
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|s| s.bg(rgb(c.hover)))
                    .child(icon(Glyph::Plus, 14., c.quiet))
                    .on_click(cx.listener(|w, _, window, cx| w.new_query(None, String::new(), false, window, cx))),
            )
    }

    pub(super) fn render_active(&self, c: Palette, cx: &mut Context<Self>) -> AnyElement {
        match self.active_tab() {
            Some(Tab::Table(t)) => t.clone().into_any_element(),
            Some(Tab::Sql(s)) => s.clone().into_any_element(),
            None => {
                let loading = self.session.as_ref().is_some_and(|s| s.read(cx).loading);
                col()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap(px(10.))
                    .text_color(rgb(c.quiet))
                    .child(div().text_color(rgb(c.muted)).child(if loading { "Reading the schema…" } else { "No tabs open" }))
                    .child(row().gap(px(6.)).child("Pick a table from the sidebar, press").child(kbd(c, "⌘")).child(kbd(c, "K")).child("to jump, or").child(kbd(c, "⌘")).child(kbd(c, "T")).child("for a query."))
                    .into_any_element()
            }
        }
    }

    pub(super) fn render_status(&self, c: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(session) = self.session.as_ref().map(|s| s.read(cx)) else { return div() };
        let s = |el: Div| el.flex().items_center().gap(px(6.));
        let right: Vec<AnyElement> = match self.active_tab() {
            Some(Tab::Table(t)) => {
                let t = t.read(cx);
                let cols = t.grid.read(cx).columns.len();
                let mut v = vec![];
                if let Some((n, approx)) = t.total {
                    v.push(s(div()).child(format!("{}{} rows", if approx { "~" } else { "" }, fmt_int(n))).into_any_element());
                }
                v.push(s(div()).child(format!("{cols} columns")).into_any_element());
                v.push(s(div()).child(icon(Glyph::Bolt, 12., c.quiet)).child(fmt_ms(t.elapsed)).into_any_element());
                v
            }
            Some(Tab::Sql(q)) => {
                let q = q.read(cx);
                match q.status() {
                    Some(st) => vec![s(div()).text_color(rgb(c.amber)).child(dot(c.amber, 6.)).child(st).into_any_element()],
                    None => vec![s(div()).child(format!("{} queries this session", self.query_count)).into_any_element()],
                }
            }
            None => vec![],
        };
        row()
            .h(px(24.))
            .flex_shrink_0()
            .px(px(12.))
            .gap(px(14.))
            .bg(rgb(c.chrome))
            .border_t_1()
            .border_color(rgb(c.border))
            .text_size(px(scaled_ui(11.5)))
            .text_color(rgb(c.muted))
            .child(
                div()
                    .id("toggle-sidebar")
                    .ml(px(-6.))
                    .p(px(3.))
                    .rounded(px(4.))
                    .hover(|s| s.bg(rgb(c.hover)))
                    .child(icon(Glyph::Sidebar, 13., if self.sidebar_open { c.blue } else { c.muted }))
                    .on_click(cx.listener(|w, _, _, cx| {
                        w.sidebar_open = !w.sidebar_open;
                        cx.notify();
                    })),
            )
            .child(if session.reconnecting {
                s(div()).text_color(rgb(c.amber)).child(dot(c.amber, 6.)).child("Reconnecting…").into_any_element()
            } else if session.disconnected {
                s(div())
                    .id("reconnect")
                    .ml(px(-4.))
                    .px(px(4.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_color(rgb(c.red))
                    .hover(|s| s.bg(rgb(c.hover)))
                    .child(icon(Glyph::Unlink, 12., c.red))
                    .child("Disconnected — click to reconnect")
                    .on_click(cx.listener(|w, _, _, cx| {
                        if let Some(s) = &w.session {
                            s.update(cx, |s, cx| s.reconnect(cx));
                        }
                    }))
                    .into_any_element()
            } else {
                s(div()).child(dot(c.green, 6.)).child("Connected").into_any_element()
            })
            .child(div().child(session.version.clone()))
            .child(div().flex_1())
            .children(right)
    }
}

/// Records where its parent is drawn into `cell`.
fn measure(cell: &Rc<Cell<Option<Bounds<Pixels>>>>) -> impl IntoElement {
    let cell = cell.clone();
    canvas(move |bounds, _, _| cell.set(Some(bounds)), |_, _, _, _| {}).absolute().inset_0()
}

/// Just under the bottom-left corner of a measured button; the click point if it hasn't been drawn yet.
fn below(cell: &Rc<Cell<Option<Bounds<Pixels>>>>, fallback: Point<Pixels>) -> Point<Pixels> {
    cell.get().map(|b| point(b.origin.x, b.bottom() + px(4.))).unwrap_or(fallback)
}
