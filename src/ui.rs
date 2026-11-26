//! Small building blocks shared by every view, in the mockup's visual roles.
use crate::icons::Glyph;
use crate::theme::{Palette, mono, scaled_mono, scaled_ui};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::Icon;

pub const BAR: f32 = 36.;
pub const TOOLBAR: f32 = 40.;
pub const ROW_H: f32 = 27.;

pub fn row() -> Div {
    div().flex().items_center()
}
pub fn col() -> Div {
    div().flex().flex_col()
}
pub fn hx(c: u32) -> Hsla {
    rgb(c).into()
}
pub fn icon(g: Glyph, size: f32, color: u32) -> Icon {
    Icon::new(g).size(px(size)).text_color(rgb(color))
}
pub fn text(size: f32) -> Div {
    div().text_size(px(scaled_ui(size)))
}
pub fn mono_text(size: f32) -> Div {
    div().font_family(mono()).text_size(px(scaled_mono(size)))
}

#[derive(Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum Tone {
    Plain,
    Ghost,
    Primary,
    Danger,
    On,
}

/// A toolbar button: optional glyph, optional label, optional shortcut.
pub fn button(c: Palette, id: impl Into<ElementId>, glyph: Option<Glyph>, label: Option<&str>, tone: Tone) -> Stateful<Div> {
    let (bg, border, fg, ic) = match tone {
        Tone::Plain => (c.background, c.border, c.text, c.muted),
        Tone::Ghost => (0, 0, c.text, c.muted),
        Tone::Primary => (c.blue, c.blue, c.primary_text, c.primary_text),
        Tone::Danger => (c.red, c.red, c.primary_text, c.primary_text),
        Tone::On => (c.blue_tint, c.blue, c.blue, c.blue),
    };
    let transparent = tone == Tone::Ghost;
    row()
        .id(id)
        .flex_shrink_0()
        .h(px(26.))
        .gap(px(6.))
        .px(px(if label.is_some() { 9. } else { 6. }))
        .rounded(px(6.))
        .border_1()
        .text_size(px(scaled_ui(12.5)))
        .when(!transparent, |s| s.bg(rgb(bg)).border_color(rgb(border)))
        .when(transparent, |s| s.border_color(transparent_black()))
        .text_color(rgb(fg))
        .when(matches!(tone, Tone::Primary | Tone::Danger), |s| s.font_weight(FontWeight::MEDIUM))
        .hover(move |s| match tone {
            Tone::Primary | Tone::Danger => s.opacity(0.92),
            _ => s.bg(rgb(c.hover)),
        })
        .cursor_default()
        .when_some(glyph, |s, g| s.child(icon(g, 14., ic)))
        .when_some(label, |s, l| s.child(l.to_string()))
}

/// A key cap, as in ⌘ K.
pub fn kbd(c: Palette, keys: &str) -> Div {
    div()
        .flex_shrink_0()
        .px(px(4.))
        .h(px(16.))
        .min_w(px(17.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .border_b_2()
        .border_color(rgb(c.border))
        .text_color(rgb(c.quiet))
        .text_size(px(scaled_ui(10.5)))
        .child(keys.to_string())
}

pub fn vsep(c: Palette) -> Div {
    div().w(px(1.)).h(px(18.)).mx(px(4.)).flex_shrink_0().bg(rgb(c.border))
}

/// A small coloured label: PK, FK, UNIQUE, PRODUCTION.
pub fn chip(label: impl Into<SharedString>, fg: u32, bg: Hsla) -> Div {
    div()
        .flex_shrink_0()
        .px(px(6.))
        .py(px(1.))
        .rounded(px(4.))
        .bg(bg)
        .text_color(rgb(fg))
        .text_size(px(scaled_ui(10.)))
        .font_weight(FontWeight::BOLD)
        .child(label.into())
}

/// How much of a chip's colour shows through its fill: just enough to read as a chip.
pub const CHIP_FILL: f32 = 0.08;

/// A tinted version of a colour, for chip backgrounds.
pub fn tint(c: u32, alpha: f32) -> Hsla {
    hx(c).opacity(alpha)
}

pub fn dot(color: u32, size: f32) -> Div {
    div().flex_shrink_0().size(px(size)).rounded_full().bg(rgb(color))
}

/// The colour that marks an environment everywhere it shows, and a see-through fill
/// of it for chips so they sit on any background.
pub fn env_color(c: Palette, env: crate::db::config::Env) -> (u32, Hsla) {
    use crate::db::config::Env::*;
    let fg = match env {
        Production => c.red,
        Staging => c.amber,
        Development => c.green,
    };
    (fg, tint(fg, CHIP_FILL))
}

/// Engine badge: the engine's mark on its colour.
pub fn engine_badge(e: crate::db::Engine, size: f32) -> Div {
    div()
        .flex_shrink_0()
        .size(px(size))
        .rounded(px(5.))
        .bg({
            let (top, bottom) = e.badge_colors();
            linear_gradient(160., linear_color_stop(hx(top), 0.), linear_color_stop(hx(bottom), 1.))
        })
        .flex()
        .items_center()
        .justify_center()
        .child(icon(e.glyph(), (size * 0.72).round(), 0xffffff))
}

pub fn fmt_int(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 { format!("-{out}") } else { out }
}

pub fn fmt_ms(d: std::time::Duration) -> String {
    let ms = d.as_secs_f64() * 1000.;
    if ms < 10. {
        format!("{ms:.1} ms")
    } else if ms < 10_000. {
        format!("{ms:.0} ms")
    } else {
        format!("{:.1} s", ms / 1000.)
    }
}

/// Wrap `content` in a modal card over a dimmed window.
///
/// Drawn in place, last, rather than deferred: text fields open their own
/// deferred menus on right-click, and GPUI can't defer inside a deferral.
pub fn modal(c: Palette, width: f32, content: impl IntoElement) -> impl IntoElement {
    modal_card(c, width, false, content)
}

/// Like [`modal`], but centred and allowed the whole window height, for
/// small windows where the usual top offset would squeeze the card.
pub fn modal_fill(c: Palette, width: f32, content: impl IntoElement) -> impl IntoElement {
    modal_card(c, width, true, content)
}

fn modal_card(c: Palette, width: f32, fill: bool, content: impl IntoElement) -> impl IntoElement {
    div()
            .id("modal-scrim")
            .absolute()
            .inset_0()
            .bg(rgba(c.scrim))
            .flex()
            .justify_center()
            .map(|s| if fill { s.items_center().p(px(16.)) } else { s.items_start().pt(relative(0.12)) })
            .occlude()
            .child(
                div()
                    .id("modal-card")
                    .w(px(width))
                    .map(|s| if fill { s.max_h_full() } else { s.max_h(relative(0.8)) })
                    .flex()
                    .flex_col()
                    .bg(rgb(c.popover))
                    .border_1()
                    .border_color(rgb(c.popover_border))
                    .rounded(px(12.))
                    .shadow_lg()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(content),
            )
}

/// Pixels as a plain number.
pub fn f(p: Pixels) -> f32 {
    p / px(1.)
}
