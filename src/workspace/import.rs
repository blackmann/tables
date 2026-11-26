//! Importing connections from TablePlus, shared by the welcome and workspace windows.
use super::connect::{field, label};
use super::*;
use crate::db::tableplus::{self, Report};
use crate::icons::Glyph;
use crate::theme::scaled_ui;
use std::path::PathBuf;

/// The sheet is done. The import may have added connections, so reload the store.
pub enum ImportEvent {
    Close,
}

pub struct ImportForm {
    path: PathBuf,
    password: Entity<InputState>,
    needs_password: bool,
    error: Option<String>,
    report: Option<Report>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ImportEvent> for ImportForm {}

/// Ask for a TablePlus export (or `Connections.plist`), then call `then` with it.
pub fn pick(window: &mut Window, cx: &mut App, then: impl FnOnce(PathBuf, &mut Window, &mut App) + 'static) {
    let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Import".into()) });
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        if let Ok(Ok(Some(paths))) = rx.await
            && let Some(p) = paths.into_iter().next()
        {
            let _ = handle.update(cx, |_, window, cx| then(p, window, cx));
        }
    })
    .detach();
}

impl ImportForm {
    pub fn new(path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let password = cx.new(|cx| InputState::new(window, cx).placeholder("Chosen when exporting from TablePlus").masked(true));
        let needs_password = std::fs::read(&path).map(|b| tableplus::is_encrypted(&b)).unwrap_or(false);
        let subs = vec![cx.subscribe_in(&password, window, |f: &mut ImportForm, _, e: &InputEvent, window, cx| match e {
            InputEvent::PressEnter { .. } => f.submit(window, cx),
            InputEvent::Change if f.error.is_some() => {
                f.error = None;
                cx.notify();
            }
            _ => {}
        })];
        let mut form = ImportForm { path, password, needs_password, error: None, report: None, _subs: subs };
        if needs_password {
            form.password.update(cx, |s, cx| s.focus(window, cx));
        } else {
            form.run(cx);
        }
        form
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        let pass = self.password.read(cx).value().to_string();
        match tableplus::import(&self.path, &pass) {
            Ok(r) => self.report = Some(r),
            Err(e) => self.error = Some(e.to_string()),
        }
        cx.notify();
    }

    pub fn submit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.report.is_some() || !self.needs_password {
            cx.emit(ImportEvent::Close);
        } else {
            self.run(cx);
        }
    }
}

fn names(list: &[String]) -> String {
    const SHOWN: usize = 6;
    let mut s = list.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
    if list.len() > SHOWN {
        s.push_str(&format!(" and {} more", list.len() - SHOWN));
    }
    s
}

impl Render for ImportForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = palette(cx);
        let file = self.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let note = |glyph: Glyph, color: u32, bg: u32, title: String, detail: Option<String>| {
            row()
                .items_start()
                .gap(px(8.))
                .px(px(10.))
                .py(px(8.))
                .rounded(px(8.))
                .bg(rgb(bg))
                .text_size(px(scaled_ui(12.)))
                .child(div().pt(px(1.)).child(icon(glyph, 14., color)))
                .child(col().flex_1().gap(px(2.)).child(title).when_some(detail, |s, d| s.child(div().text_color(rgb(c.muted)).child(d))))
        };
        let mut body = col().id("import-body").flex_shrink().min_h_0().overflow_y_scroll().px(px(18.)).pb(px(14.)).gap(px(10.));
        match &self.report {
            None if self.needs_password => {
                body = body.child(col().gap(px(5.)).child(label(c, "Export password")).child(field(c, &self.password)));
            }
            None => {}
            Some(r) => {
                let s = |n: usize| if n == 1 { "" } else { "s" };
                body = body.child(if r.added.is_empty() {
                    note(Glyph::Check, c.muted, c.hover, "No new connections".into(), None)
                } else {
                    let pw = match r.with_passwords {
                        0 => String::new(),
                        n if n == r.added.len() => ", with their passwords".into(),
                        n => format!(", {n} with passwords"),
                    };
                    note(Glyph::Check, c.green, c.green_tint, format!("Added {} connection{}{pw}", r.added.len(), s(r.added.len())), Some(names(&r.added)))
                });
                if !r.duplicates.is_empty() {
                    body = body.child(note(Glyph::Check, c.muted, c.hover, format!("{} already saved, left as they are", r.duplicates.len()), Some(names(&r.duplicates))));
                }
                if !r.ssh.is_empty() {
                    body = body.child(note(Glyph::Warn, c.amber, c.amber_tint, format!("Skipped {} over SSH: tunnels aren't supported yet", r.ssh.len()), Some(names(&r.ssh))));
                }
                if !r.unsupported.is_empty() {
                    let mut engines: Vec<&str> = r.unsupported.iter().map(|(_, e)| e.as_str()).collect();
                    engines.sort();
                    engines.dedup();
                    let list: Vec<String> = r.unsupported.iter().map(|(n, _)| n.clone()).collect();
                    body = body.child(note(Glyph::Warn, c.amber, c.amber_tint, format!("Skipped {} for {}: not supported yet", r.unsupported.len(), engines.join(", ")), Some(names(&list))));
                }
            }
        }
        if let Some(e) = &self.error {
            body = body.child(note(Glyph::Warn, c.red, c.red_tint, e.clone(), None));
        }
        let done = self.report.is_some() || !self.needs_password;
        // Header and footer stay put; the summary scrolls when the window is short.
        col()
            .flex_shrink()
            .min_h_0()
            .child(
                col()
                    .flex_none()
                    .px(px(18.))
                    .pt(px(16.))
                    .pb(px(10.))
                    .gap(px(3.))
                    .child(row().gap(px(8.)).child(icon(Glyph::Download, 15., c.muted)).child(div().text_size(px(scaled_ui(15.))).font_weight(FontWeight::SEMIBOLD).child("Import from TablePlus")))
                    .child(div().text_size(px(scaled_ui(12.))).text_color(rgb(c.muted)).child(file)),
            )
            .child(body)
            .child(
                row()
                    .flex_none()
                    .px(px(18.))
                    .py(px(12.))
                    .gap(px(8.))
                    .border_t_1()
                    .border_color(rgb(c.popover_border))
                    .bg(hx(c.background).opacity(0.4))
                    .child(div().flex_1())
                    .when(!done, |s| s.child(button(c, "import-cancel", None, Some("Cancel"), Tone::Plain).on_click(cx.listener(|_, _, _, cx| cx.emit(ImportEvent::Close)))))
                    .child(
                        button(c, "import-go", None, Some(if done { "Done" } else { "Import" }), Tone::Primary)
                            .child(kbd(c, "⌘↵").border_color(hx(c.primary_text).opacity(0.3)).text_color(rgb(c.primary_text)))
                            .on_click(cx.listener(|f, _, window, cx| f.submit(window, cx))),
                    ),
            )
    }
}

impl Workspace {
    pub(super) fn import_tableplus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        pick(window, cx, move |path, window, cx| {
            let _ = this.update(cx, |w, cx| {
                w.modal_focus.focus(window);
                let form = cx.new(|cx| ImportForm::new(path, window, cx));
                w._subs.push(cx.subscribe_in(&form, window, |w, _, ImportEvent::Close, window, cx| {
                    w.store = Store::load();
                    w.close_modal(window, cx);
                }));
                w.modal = Some(Modal::Import(form));
                cx.notify();
            });
        });
    }
}
