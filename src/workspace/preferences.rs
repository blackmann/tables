//! Preferences, laid out like Chat's: sections down the left under a search
//! box, and the section picked as a titled page of rows, each a name, a
//! description and a control. Searching lists matching rows from every section.
use super::*;
use crate::icons::Glyph;
use crate::theme::{DEFAULT_MONO, Palette, mono, scaled_ui};
use gpui_component::Sizable;
use gpui_component::input::Input;

/// Monospace families worth offering first, where installed.
const SUGGESTED_MONO: &[&str] = &[
    "SF Mono", "Menlo", "Monaco", "JetBrains Mono", "Fira Code", "Berkeley Mono", "IBM Plex Mono", "Cascadia Code", "Source Code Pro", "Geist Mono", "Commit Mono", "Iosevka", "Hack",
];
/// Interface families worth offering first, where installed.
const SUGGESTED_UI: &[&str] = &["SF Pro", "Inter", "Helvetica Neue", "IBM Plex Sans", "Avenir Next", "Geist"];

const PAGE: f32 = 620.;
const CONTROL: f32 = 220.;
const GAP: f32 = 24.;
const SIDEBAR: f32 = 200.;

const SAMPLE: &str = "SELECT u.email, count(o.id) AS orders\nFROM users u\nJOIN orders o ON o.user_id = u.id\nWHERE o.status = 'paid'  -- 0O 1lI {}[]\nGROUP BY 1;";

/// The pages of preferences, in the sidebar's order.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Appearance,
}

impl Section {
    const ALL: [Section; 1] = [Section::Appearance];
    fn title(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
        }
    }
    fn icon(self) -> Glyph {
        match self {
            Section::Appearance => Glyph::Sun,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FontRole {
    Ui,
    Mono,
}

pub struct Prefs {
    section: Section,
    search: Entity<InputState>,
    _sub: Subscription,
}

impl Prefs {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search settings"));
        let _sub = cx.subscribe(&search, |_, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                cx.notify();
            }
        });
        Prefs { section: Section::Appearance, search, _sub }
    }
}

/// One setting: where it lives, what it's called and does, and its control.
struct Row {
    section: Section,
    group: &'static str,
    title: &'static str,
    description: &'static str,
    control: AnyElement,
}

impl Row {
    fn matches(&self, q: &str) -> bool {
        [self.title, self.description, self.group, self.section.title()].iter().any(|t| t.to_lowercase().contains(q))
    }
}

fn setting(c: Palette, title: &str, description: &str, control: AnyElement) -> Div {
    row()
        .w(px(PAGE))
        .gap(px(GAP))
        .py(px(14.))
        .border_b_1()
        .border_color(rgb(c.row_border))
        .child(
            col()
                .w(px(PAGE - CONTROL - GAP))
                .flex_shrink_0()
                .gap(px(3.))
                .child(div().text_size(px(scaled_ui(14.))).child(title.to_string()))
                .child(div().text_size(px(scaled_ui(13.))).line_height(relative(1.45)).text_color(rgb(c.muted)).child(description.to_string())),
        )
        .child(row().flex_1().min_w_0().justify_end().child(control))
}

fn group(c: Palette, title: &str) -> Div {
    div()
        .pt(px(24.))
        .pb(px(4.))
        .font_family(mono())
        .text_size(px(crate::theme::mono_size()))
        .text_color(rgb(c.muted))
        .border_b_1()
        .border_color(rgb(c.row_border))
        .child(title.to_string())
}

impl Workspace {
    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.modal = Some(Modal::Settings);
        self.modal_focus.focus(window);
        cx.notify();
    }

    fn apply_typography(&mut self, cx: &mut Context<Self>) {
        self.save_store();
        let s = &self.store.settings;
        crate::theme::set_fonts(s.ui_font.as_deref(), s.mono_font.as_deref(), s.ui_size, s.mono_size, cx);
        cx.notify();
    }

    fn set_font(&mut self, role: FontRole, family: Option<String>, cx: &mut Context<Self>) {
        match role {
            FontRole::Ui => self.store.settings.ui_font = family,
            FontRole::Mono => self.store.settings.mono_font = family,
        }
        self.apply_typography(cx);
    }

    fn font_size(&self, role: FontRole) -> f32 {
        match role {
            FontRole::Ui => crate::theme::ui_size(),
            FontRole::Mono => crate::theme::mono_size(),
        }
    }

    fn set_font_size(&mut self, role: FontRole, size: f32, cx: &mut Context<Self>) {
        let size = size.clamp(9., 22.);
        match role {
            FontRole::Ui => self.store.settings.ui_size = Some(size),
            FontRole::Mono => self.store.settings.mono_size = Some(size),
        }
        self.apply_typography(cx);
    }

    fn reset_typography(&mut self, cx: &mut Context<Self>) {
        self.store.settings = config::Settings::default();
        self.apply_typography(cx);
    }

    fn font_menu(&self, role: FontRole, cx: &App) -> menu::Builder {
        let mut names = cx.text_system().all_font_names();
        names.retain(|n| !n.starts_with('.'));
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup();
        let suggested: Vec<String> = match role {
            FontRole::Ui => SUGGESTED_UI,
            FontRole::Mono => SUGGESTED_MONO,
        }
        .iter()
        .filter(|s| names.iter().any(|n| n == *s))
        .map(|s| s.to_string())
        .collect();
        let current = match role {
            FontRole::Ui => self.store.settings.ui_font.clone(),
            FontRole::Mono => self.store.settings.mono_font.clone(),
        };
        let weak = self.self_weak.clone();
        std::rc::Rc::new(move |_| {
            let item = |label: String, family: Option<String>| {
                let w = weak.clone();
                let checked = family == current;
                let mut it = menu::Item::new(label).checked(checked);
                if let Some(f) = &family {
                    it = it.font(f.clone());
                }
                menu::Entry::Item(it.on_click(move |_, cx| {
                    let _ = w.update(cx, |w, cx| w.set_font(role, family.clone(), cx));
                }))
            };
            let mut v = vec![item(if role == FontRole::Ui { "System default".into() } else { format!("{DEFAULT_MONO} (default)") }, None)];
            if !suggested.is_empty() {
                v.push(menu::Entry::Label("Suggested".into()));
                v.extend(suggested.iter().map(|n| item(n.clone(), Some(n.clone()))));
            }
            v.push(menu::Entry::Label("All fonts".into()));
            v.extend(names.iter().filter(|n| !suggested.contains(n)).map(|n| item(n.clone(), Some(n.clone()))));
            v
        })
    }

    fn font_picker(&self, c: Palette, role: FontRole, cx: &mut Context<Self>) -> AnyElement {
        let (id, family, label) = match role {
            FontRole::Ui => ("pref-ui-font", crate::theme::ui_font(), self.store.settings.ui_font.clone().unwrap_or_else(|| "System default".into())),
            FontRole::Mono => ("pref-mono-font", mono(), self.store.settings.mono_font.clone().unwrap_or_else(|| DEFAULT_MONO.into())),
        };
        row()
            .id(id)
            .h(px(28.))
            .w(px(CONTROL))
            .pl(px(10.))
            .pr(px(8.))
            .gap(px(8.))
            .rounded(px(6.))
            .border_1()
            .border_color(rgb(c.border))
            .hover(|s| s.bg(rgb(c.hover)))
            .child(div().flex_1().min_w_0().truncate().font_family(family).child(label))
            .child(icon(Glyph::UpDown, 13., c.muted))
            .on_mouse_down(MouseButton::Left, cx.listener(move |w, e: &MouseDownEvent, window, cx| {
                let build = w.font_menu(role, cx);
                w.open_menu(build, e.position, 300., Some("Search fonts…"), window, cx);
            }))
            .into_any_element()
    }

    fn font_size_picker(&self, c: Palette, role: FontRole, cx: &mut Context<Self>) -> AnyElement {
        let size = self.font_size(role);
        let step = |id: (&'static str, usize), g: Glyph| {
            div()
                .id(id)
                .size(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .border_1()
                .border_color(rgb(c.border))
                .hover(|s| s.bg(rgb(c.hover)))
                .child(icon(g, 12., c.muted))
        };
        let r = role as usize;
        row()
            .gap(px(6.))
            .child(step(("smaller", r), Glyph::Minus).on_click(cx.listener(move |w, _, _, cx| w.set_font_size(role, size - 1., cx))))
            .child(div().w(px(54.)).text_center().child(format!("{size:.0} px")))
            .child(step(("larger", r), Glyph::Plus).on_click(cx.listener(move |w, _, _, cx| w.set_font_size(role, size + 1., cx))))
            .into_any_element()
    }

    /// Prose in the interface font and SQL in the code font.
    fn font_preview(&self, c: Palette) -> Div {
        col()
            .mt(px(20.))
            .w(px(PAGE))
            .p(px(16.))
            .gap(px(12.))
            .rounded(px(8.))
            .bg(rgb(c.panel))
            .border_1()
            .border_color(rgb(c.row_border))
            .child(text(12.).text_color(rgb(c.muted)).child("Preview"))
            .child(div().w(px(PAGE - 34.)).line_height(relative(1.55)).child("Tables, filters and the row detail use the interface font; the SQL editor, the grid's numbers and JSON use the code font."))
            .child(
                div()
                    .p(px(12.))
                    .rounded(px(6.))
                    .bg(rgb(c.background))
                    .border_1()
                    .border_color(rgb(c.row_border))
                    .font_family(mono())
                    .text_size(px(crate::theme::mono_size()))
                    .line_height(relative(1.5))
                    .text_color(rgb(c.code))
                    .children(SAMPLE.lines().map(|l| div().whitespace_nowrap().child(l.to_string()))),
            )
    }

    /// Every setting, in the order the pages list them.
    fn pref_rows(&self, c: Palette, cx: &mut Context<Self>) -> Vec<Row> {
        use Section::*;
        let row = |section, group, title, description, control: AnyElement| Row { section, group, title, description, control };
        vec![
            row(Appearance, "Fonts", "Interface font", "Everything outside code: the sidebar, tabs, menus, forms and panels.", self.font_picker(c, FontRole::Ui, cx)),
            row(Appearance, "Fonts", "Code font", "The SQL editor, numbers, timestamps and JSON in the grid and the row detail.", self.font_picker(c, FontRole::Mono, cx)),
            row(Appearance, "Fonts", "Interface font size", "The base size for text, controls, menus, tabs and panels.", self.font_size_picker(c, FontRole::Ui, cx)),
            row(Appearance, "Fonts", "Code font size", "The base size for SQL and data shown in the code font.", self.font_size_picker(c, FontRole::Mono, cx)),
        ]
    }

    fn prefs_sidebar(&self, c: Palette, searching: bool, cx: &mut Context<Self>) -> Div {
        let mut nav = col().px(px(8.)).gap(px(2.));
        for section in Section::ALL {
            let on = !searching && self.prefs.section == section;
            nav = nav.child(
                row()
                    .id(SharedString::from(format!("prefs-section-{}", section.title())))
                    .h(px(30.))
                    .px(px(8.))
                    .gap(px(8.))
                    .rounded(px(6.))
                    .text_color(rgb(if on { c.text } else { c.muted }))
                    .when(on, |s| s.bg(rgb(c.selected)))
                    .when(!on, |s| s.hover(|s| s.bg(rgb(c.hover)).text_color(rgb(c.text))))
                    .child(icon(section.icon(), 14., if on { c.blue } else { c.muted }))
                    .child(section.title())
                    .on_click(cx.listener(move |w, _, window, cx| {
                        w.prefs.section = section;
                        w.prefs.search.update(cx, |s, cx| s.set_value("", window, cx));
                        cx.notify();
                    })),
            );
        }
        col()
            .w(px(SIDEBAR))
            .flex_shrink_0()
            .h_full()
            .bg(rgb(c.chrome))
            .border_r_1()
            .border_color(rgb(c.border))
            .child(
                div().p(px(12.)).child(
                    row()
                        .h(px(28.))
                        .px(px(8.))
                        .gap(px(6.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(rgb(c.border))
                        .bg(rgb(c.background))
                        .child(icon(Glyph::Search, 13., c.quiet))
                        .child(div().flex_1().child(Input::new(&self.prefs.search).appearance(false).small().cleanable(true))),
                ),
            )
            .child(nav)
            .child(div().flex_1())
            .child(
                row()
                    .mx(px(12.))
                    .py(px(12.))
                    .gap(px(6.))
                    .border_t_1()
                    .border_color(rgb(c.border))
                    .text_size(px(scaled_ui(11.5)))
                    .text_color(rgb(c.quiet))
                    .child(kbd(c, "esc"))
                    .child("to close"),
            )
    }

    pub(super) fn preferences_view(&mut self, c: Palette, cx: &mut Context<Self>) -> AnyElement {
        let query = self.prefs.search.read(cx).value().trim().to_lowercase();
        let searching = !query.is_empty();
        let rows = self.pref_rows(c, cx);
        let title = |t: String| div().pt(px(4.)).text_size(px(scaled_ui(20.))).font_weight(FontWeight::SEMIBOLD).child(t);
        let mut page = col().w(px(PAGE));
        if searching {
            let found: Vec<Row> = rows.into_iter().filter(|r| r.matches(&query)).collect();
            page = page.child(title("Search results".into()));
            if found.is_empty() {
                page = page.child(div().pt(px(16.)).text_color(rgb(c.muted)).child(format!("No settings match “{query}”.")));
            }
            let mut last = None;
            for r in found {
                if last != Some(r.section) {
                    last = Some(r.section);
                    page = page.child(group(c, &format!("{} · {}", r.section.title(), r.group)));
                }
                page = page.child(setting(c, r.title, r.description, r.control));
            }
        } else {
            let section = self.prefs.section;
            page = page.child(title(section.title().into()));
            let mut last = None;
            for r in rows.into_iter().filter(|r| r.section == section) {
                if last != Some(r.group) {
                    last = Some(r.group);
                    page = page.child(group(c, r.group));
                }
                page = page.child(setting(c, r.title, r.description, r.control));
            }
            if section == Section::Appearance {
                let s = &self.store.settings;
                let customised = s.ui_font.is_some() || s.mono_font.is_some() || s.ui_size.is_some() || s.mono_size.is_some();
                page = page.child(self.font_preview(c)).child(row().pt(px(16.)).child(div().flex_1()).when(customised, |s| {
                    s.child(button(c, "reset-fonts", None, Some("Restore defaults"), Tone::Ghost).on_click(cx.listener(|w, _, _, cx| w.reset_typography(cx))))
                }));
            }
        }
        let main = col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                row().h(px(48.)).flex_shrink_0().pl(px(40.)).pr(px(12.)).child(div().flex_1()).child(
                    button(c, "close-prefs", Some(Glyph::Close), None, Tone::Ghost).on_click(cx.listener(|w, _, window, cx| w.close_modal(window, cx))),
                ),
            )
            .child(div().id("prefs-content").flex_1().min_h_0().overflow_y_scroll().px(px(40.)).pb(px(40.)).child(page));
        div()
            .key_context("Modal")
            .track_focus(&self.modal_focus)
            .absolute()
            .inset_0()
            .on_action(cx.listener(|w, _: &CancelModal, window, cx| w.close_modal(window, cx)))
            .child(
                    div()
                        .id("prefs-scrim")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(rgba(c.scrim))
                        // A click outside the dialog closes it.
                        .on_mouse_down(MouseButton::Left, cx.listener(|w, _, window, cx| w.close_modal(window, cx)))
                        .child(
                            row()
                                .id("prefs-card")
                                .w(px(SIDEBAR + PAGE + 80.))
                                .max_w(relative(0.94))
                                .h(px(600.))
                                .max_h(relative(0.92))
                                .bg(rgb(c.background))
                                .border_1()
                                .border_color(rgb(c.border))
                                .rounded(px(12.))
                                .shadow_xl()
                                .overflow_hidden()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(self.prefs_sidebar(c, searching, cx))
                                .child(main),
                        ),
            )
            .into_any_element()
    }
}
