//! Menus, the command palette, modal dialogs and toasts.
use super::*;
use crate::icons::Glyph;
use crate::theme::{Palette, scaled_ui};
use gpui_component::Icon;
use gpui_component::input::Input;

impl Workspace {
    pub(super) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = self.self_weak.clone();
        let session = self.session.clone();
        let saved = self.conn_id(cx).and_then(|id| self.store.state.get(&id)).map(|s| s.saved.clone()).unwrap_or_default();
        let conns = self.store.connections.clone();
        let colors = palette(cx);
        let build: menu::Builder = std::rc::Rc::new(move |cx| {
            let mut v = vec![];
            if let Some(s) = &session {
                let s = s.read(cx);
                if !s.relations.is_empty() {
                    v.push(menu::Entry::Label("Tables".into()));
                }
                for r in &s.relations {
                    let (w, name) = (weak.clone(), r.name.clone());
                    let view = r.kind == crate::db::RelationKind::View;
                    v.push(menu::Entry::Item(
                        menu::Item::new(r.name.clone())
                            .icon(Icon::new(if view { Glyph::View } else { Glyph::Table }))
                            .detail(r.rows.map(|n| format!("{} rows", crate::db::compact(n as f64))).unwrap_or_default())
                            .on_click(move |window, cx| {
                                let _ = w.update(cx, |w, cx| w.open_table(name.clone(), false, None, window, cx));
                            }),
                    ));
                }
            }
            if !saved.is_empty() {
                v.push(menu::Entry::Label("Saved queries".into()));
                for q in &saved {
                    let (w, name) = (weak.clone(), q.name.clone());
                    v.push(menu::Entry::Item(menu::Item::new(q.name.clone()).icon(Icon::new(Glyph::Sql)).on_click(move |window, cx| {
                        let _ = w.update(cx, |w, cx| w.open_saved(&name, window, cx));
                    })));
                }
            }
            v.push(menu::Entry::Label("Commands".into()));
            let cmd = |label: &str, g: Glyph, key: &str, f: fn(&mut Workspace, &mut Window, &mut Context<Workspace>)| {
                let w = weak.clone();
                menu::Entry::Item(menu::Item::new(label.to_string()).icon(Icon::new(g)).detail(key.to_string()).on_click(move |window, cx| {
                    let _ = w.update(cx, |w, cx| f(w, window, cx));
                }))
            };
            v.push(cmd("New SQL query", Glyph::Plus, "⌘T", |w, window, cx| w.new_query(None, String::new(), false, window, cx)));
            v.push(cmd("Toggle sidebar", Glyph::Sidebar, "⌘B", |w, _, cx| {
                w.sidebar_open = !w.sidebar_open;
                cx.notify();
            }));
            v.push(cmd("Toggle row detail", Glyph::Panel, "⌘.", |w, _, cx| {
                w.detail_open = !w.detail_open;
                cx.notify();
            }));
            v.push(cmd("Reload schema and rows", Glyph::Refresh, "⌘R", |w, _, cx| w.reload(cx)));
            v.push(cmd("New connection…", Glyph::Plus, "⌘N", |w, window, cx| w.edit_connection(None, window, cx)));
            v.push(cmd("Settings…", Glyph::Format, "⌘,", |w, window, cx| w.open_settings(window, cx)));
            v.push(cmd("Disconnect", Glyph::Plug, "", |w, window, cx| w.disconnect(window, cx)));
            if conns.len() > 1 {
                v.push(menu::Entry::Label("Switch connection".into()));
                for c in &conns {
                    let (w, c2) = (weak.clone(), c.clone());
                    v.push(menu::Entry::Item(menu::Item::new(c.name.clone()).dot(env_color(colors, c.env).0).detail(c.env.label()).on_click(move |window, cx| {
                        let _ = w.update(cx, |w, cx| w.switch_to(c2.clone(), window, cx));
                    })));
                }
            }
            v
        });
        let x = (f(window.viewport_size().width) - 600.) / 2.;
        self.open_menu(build, point(px(x.max(10.)), px(70.)), 600., Some("Jump to a table, saved query or command"), window, cx);
    }

    pub(super) fn render_overlays(&mut self, c: Palette, _window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = vec![];
        if let Some(modal) = self.render_modal(c, cx) {
            out.push(modal);
        }
        if let Some((m, at)) = self.popup.clone() {
            // Not deferred: a menu's search field has a right-click menu of its own.
            out.push(anchored().anchor(Corner::TopLeft).position(at).snap_to_window_with_margin(px(8.)).child(m).into_any_element());
        }
        if !self.toasts.is_empty() {
            out.push(
                    col().absolute().left(px(20.)).right(px(20.)).bottom(px(40.)).gap(px(8.)).items_end().children(self.toasts.iter().map(|t| {
                        row()
                            .id(("toast", t.id))
                            .occlude()
                            .items_start()
                            .max_w(px(460.))
                            .min_w(px(240.))
                            .gap(px(10.))
                            .px(px(14.))
                            .py(px(10.))
                            .rounded(px(9.))
                            .bg(rgb(c.popover))
                            .border_1()
                            .border_color(rgb(if t.error { c.red } else { c.popover_border }))
                            .shadow_lg()
                            .child(div().flex_shrink_0().mt(px(2.)).child(icon(if t.error { Glyph::Warn } else { Glyph::Check }, 14., if t.error { c.red } else { c.green })))
                            // Long errors wrap, up to a few lines, instead of pushing the close button out.
                            .child(div().flex_1().min_w_0().max_h(px(120.)).overflow_hidden().child(t.text.clone()))
                            .child({
                                let id = t.id;
                                div()
                                    .id(("toast-close", id))
                                    .flex_shrink_0()
                                    .p(px(2.))
                                    .mr(px(-4.))
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(c.hover)))
                                    .child(icon(Glyph::Close, 12., c.muted))
                                    .on_click(cx.listener(move |w, _, _, cx| {
                                        cx.stop_propagation();
                                        w.dismiss_toast(id, cx);
                                    }))
                            })
                    }))
                .into_any_element(),
            );
        }
        out
    }

    fn render_modal(&mut self, c: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let current = self.modal.as_ref()?;
        let prod = self.session.as_ref().is_some_and(|s| s.read(cx).config.env == Env::Production);
        let conn = self.session.as_ref().map(|s| format!("{} · {}", s.read(cx).config.name, s.read(cx).config.env.label())).unwrap_or_default();
        let authenticating = self.authenticating;
        // `(label, danger, fingerprint)`: a fingerprint button asks for Touch ID first.
        let footer = |confirm: Option<(&str, bool, bool)>, cx: &mut Context<Self>| {
            row()
                .px(px(18.))
                .py(px(12.))
                .gap(px(8.))
                .border_t_1()
                .border_color(rgb(c.popover_border))
                .bg(hx(c.background).opacity(0.4))
                .child(div().flex_1())
                .child(button(c, "modal-cancel", None, Some(if confirm.is_some() { "Cancel" } else { "OK" }), Tone::Plain).on_click(cx.listener(|w, _, window, cx| w.close_modal(window, cx))))
                .when_some(confirm, |s, (label, danger, touch)| {
                    s.child(
                        button(c, "modal-confirm", touch.then_some(Glyph::Fingerprint), Some(label), if danger { Tone::Danger } else { Tone::Primary })
                            .when(touch && authenticating, |s| s.opacity(0.6))
                            .child(kbd(c, "⌘↵").border_color(hx(c.primary_text).opacity(0.3)).text_color(rgb(c.primary_text)))
                            .on_click(cx.listener(|w, _, window, cx| w.confirm_modal(window, cx))),
                    )
                })
        };
        let title = |g: Glyph, color: u32, t: String| {
            row().px(px(18.)).pt(px(16.)).pb(px(6.)).gap(px(10.)).text_size(px(scaled_ui(15.))).font_weight(FontWeight::SEMIBOLD).child(icon(g, 16., color)).child(t)
        };
        let warn = |text: String| {
            row()
                .items_start()
                .gap(px(10.))
                .px(px(12.))
                .py(px(10.))
                .rounded(px(8.))
                .bg(rgb(c.red_tint))
                .border_1()
                .border_color(hx(c.red).opacity(0.4))
                .child(icon(Glyph::Warn, 14., c.red))
                .child(div().flex_1().min_w_0().child(text))
        };
        let sql_box = |input: &Entity<InputState>, lines: usize| {
            div()
                .mt(px(12.))
                .h(px((lines as f32 * 20. + 24.).clamp(80., 300.)))
                .rounded(px(8.))
                .border_1()
                .border_color(rgb(c.border))
                .bg(rgb(c.background))
                .p(px(8.))
                .font_family(crate::theme::mono())
                .text_size(px(crate::theme::scaled_mono(12.5)))
                .child(Input::new(input).appearance(false).h_full())
        };
        let (width, content): (f32, AnyElement) = match current {
            Modal::Review { tab, sql, commit } => {
                let n = tab.read(cx).statements(cx).len();
                let lines = sql.read(cx).value().lines().count();
                let danger = prod && *commit;
                (
                    660.,
                    col()
                        .child(if danger {
                            title(Glyph::Shield, c.red, format!("Commit {n} {} to production?", if n == 1 { "change" } else { "changes" }))
                        } else {
                            title(Glyph::Sql, c.muted, "Review pending changes".into())
                        })
                        .child(
                            col()
                                .px(px(18.))
                                .pb(px(16.))
                                .text_color(rgb(c.muted))
                                .child(if danger {
                                    warn(format!("You're connected to {conn}. These statements run in one transaction and can't be undone from here.")).into_any_element()
                                } else {
                                    div().child(format!("{n} {} will run in a single transaction on {conn}.", if n == 1 { "statement" } else { "statements" })).into_any_element()
                                })
                                .child(sql_box(sql, lines)),
                        )
                        .child(footer(Some(("Commit", prod, prod)), cx))
                        .into_any_element(),
                )
            }
            Modal::ConfirmRun { view, .. } => {
                let lines = view.read(cx).value().lines().count();
                (
                    660.,
                    col()
                        .child(title(Glyph::Shield, c.red, "Run writes against production?".into()))
                        .child(col().px(px(18.)).pb(px(16.)).text_color(rgb(c.muted)).child(warn(format!("This changes data or schema on {conn}."))).child(sql_box(view, lines)))
                        .child(footer(Some(("Run", true, true)), cx))
                        .into_any_element(),
                )
            }
            Modal::CloseDirty(_) => (
                440.,
                col()
                    .child(title(Glyph::Warn, c.amber, "Close with uncommitted changes?".into()))
                    .child(div().px(px(18.)).pb(px(16.)).text_color(rgb(c.muted)).child("The pending edits in this tab will be lost."))
                    .child(footer(Some(("Discard and close", true, false)), cx))
                    .into_any_element(),
            ),
            Modal::Message { title: t, body } => (
                520.,
                col()
                    .child(title(Glyph::Warn, c.red, t.clone()))
                    .child(div().id("message-body").max_h(px(320.)).overflow_y_scroll().px(px(18.)).pb(px(16.)).font_family(crate::theme::mono()).text_size(px(12.)).child(body.clone()))
                    .child(footer(None, cx))
                    .into_any_element(),
            ),
            Modal::Export { table, format, columns } => {
                let picked = columns.iter().filter(|col| col.2).count();
                let total = columns.len();
                let link = |id: &'static str, label: &'static str, on: bool, cx: &mut Context<Self>| {
                    div()
                        .id(id)
                        .text_color(rgb(c.blue))
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .child(label)
                        .on_click(cx.listener(move |w, _, _, cx| {
                            if let Some(Modal::Export { columns, .. }) = &mut w.modal {
                                columns.iter_mut().for_each(|col| col.2 = on);
                            }
                            cx.notify();
                        }))
                };
                let list = columns.iter().enumerate().map(|(i, (name, ty, on))| {
                    let on = *on;
                    row()
                        .id(("export-col", i))
                        .gap(px(10.))
                        .px(px(8.))
                        .py(px(5.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(c.hover)))
                        .child(
                            div()
                                .size(px(16.))
                                .flex_shrink_0()
                                .rounded(px(4.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(on, |s| s.bg(rgb(c.blue)).child(icon(Glyph::Check, 12., c.primary_text)))
                                .when(!on, |s| s.border_1().border_color(rgb(c.popover_border)).bg(rgb(c.background))),
                        )
                        .child(div().flex_1().min_w_0().truncate().when(!on, |s| s.text_color(rgb(c.muted))).child(name.clone()))
                        .child(div().flex_shrink_0().text_color(rgb(c.muted)).font_family(crate::theme::mono()).text_size(px(crate::theme::scaled_mono(11.5))).child(ty.clone()))
                        .on_click(cx.listener(move |w, _, _, cx| {
                            if let Some(Modal::Export { columns, .. }) = &mut w.modal
                                && let Some(col) = columns.get_mut(i)
                            {
                                col.2 = !col.2;
                            }
                            cx.notify();
                        }))
                }).collect::<Vec<_>>();
                let header = row()
                    .gap(px(12.))
                    .pb(px(8.))
                    .text_color(rgb(c.muted))
                    .child(div().flex_1().child(format!("{picked} of {total} columns")))
                    .child(link("export-all", "All", true, cx))
                    .child(link("export-none", "None", false, cx));
                (
                    460.,
                    col()
                        .child(title(Glyph::Download, c.muted, format!("Export {table} as {}", format.label())))
                        .child(
                            col()
                                .px(px(18.))
                                .pb(px(16.))
                                .child(header)
                                .child(
                                    col()
                                        .id("export-cols")
                                        .max_h(px(340.))
                                        .overflow_y_scroll()
                                        .p(px(4.))
                                        .rounded(px(8.))
                                        .border_1()
                                        .border_color(rgb(c.border))
                                        .bg(rgb(c.background))
                                        .children(list),
                                ),
                        )
                        .child(footer(Some(("Export…", false, false)), cx))
                        .into_any_element(),
                )
            }
            Modal::Connection(form) => (580., form.clone().into_any_element()),
            Modal::Settings => return Some(self.preferences_view(c, cx)),
        };
        Some(
            div()
                .key_context("Modal")
                .track_focus(&self.modal_focus)
                .absolute()
                .inset_0()
                .on_action(cx.listener(|w, _: &ConfirmModal, window, cx| w.confirm_modal(window, cx)))
                .on_action(cx.listener(|w, _: &CancelModal, window, cx| w.close_modal(window, cx)))
                .child(modal(c, width, content))
                .into_any_element(),
        )
    }
}
