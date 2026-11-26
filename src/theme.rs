#![allow(dead_code)]
//! Shared semantic colors, carried over from Gitflow: quiet chrome frames a brighter
//! content surface, and tints carry state so color is never the only signal.
use gpui::*;
use gpui_component::{Theme, ThemeMode};
use std::cell::RefCell;

/// The platform's own interface font.
pub const SYSTEM_UI: &str = ".SystemUIFont";
pub const DEFAULT_MONO: &str = "Menlo";
pub const UI_SIZE: f32 = 13.;
pub const MONO_SIZE: f32 = 12.;

thread_local! {
    /// The interface and code fonts. Kept here rather than passed around because
    /// views read them deep inside list closures that have no `App` to ask;
    /// everything that draws runs on the one UI thread.
    static TYPOGRAPHY: RefCell<(SharedString, SharedString, f32, f32)> =
        RefCell::new((SYSTEM_UI.into(), DEFAULT_MONO.into(), UI_SIZE, MONO_SIZE));
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    /// Plain code, in One Dark's or One Light's foreground.
    pub code: u32,
    pub background: u32,
    pub chrome: u32,
    pub panel: u32,
    pub border: u32,
    pub row_border: u32,
    pub text: u32,
    pub muted: u32,
    pub quiet: u32,
    pub blue: u32,
    pub green: u32,
    pub amber: u32,
    /// Work in progress: a project whose agent is running.
    pub orange: u32,
    pub red: u32,
    pub cyan: u32,
    pub purple: u32,
    pub selected: u32,
    pub hover: u32,
    pub primary_text: u32,
    pub blue_tint: u32,
    pub green_tint: u32,
    pub amber_tint: u32,
    pub red_tint: u32,
    pub popover: u32,
    /// Dims the window behind a dialog.
    pub scrim: u32,
    pub popover_hover: u32,
    pub popover_border: u32,
    /// Large areas of state color (usage meters), lighter than the text colors in light mode.
    pub fill_blue: u32,
    pub fill_amber: u32,
    pub fill_red: u32,
    /// Project colors for the rail, cycled per project.
    pub lanes: [u32; 8],
}
impl Global for Palette {}

impl Palette {
    fn new(dark: bool) -> Self {
        if dark {
            Self {
                dark: true,
                code: 0xabb2bf,
                background: 0x24272d,
                chrome: 0x2b2e35,
                panel: 0x2f333a,
                border: 0x3a3e47,
                row_border: 0x31353c,
                text: 0xdcdfe4,
                muted: 0x959ba6,
                quiet: 0x6c727d,
                blue: 0x74ade8,
                green: 0x98c379,
                amber: 0xe5c07b,
                orange: 0xf0883e,
                red: 0xe06c75,
                cyan: 0x56b6c2,
                purple: 0xc678dd,
                selected: 0x2f3d52,
                hover: 0x33373f,
                primary_text: 0x1e2127,
                blue_tint: 0x2a3647,
                green_tint: 0x2c3a2e,
                amber_tint: 0x3b3427,
                red_tint: 0x3e2b2f,
                popover: 0x2e3138,
                scrim: 0x0000005c,
                popover_hover: 0x3b3f48,
                popover_border: 0x434852,
                fill_blue: 0x74ade8,
                fill_amber: 0xe5c07b,
                fill_red: 0xe06c75,
                lanes: [
                    0x74ade8, 0xc678dd, 0x98c379, 0xe5c07b, 0xe06c75, 0x56b6c2, 0xd19a66, 0xa0a8f0,
                ],
            }
        } else {
            Self {
                dark: false,
                code: 0x383a42,
                background: 0xfcfcfc,
                chrome: 0xf3f3f4,
                panel: 0xf6f6f7,
                border: 0xe0e0e3,
                row_border: 0xededef,
                text: 0x2c2d31,
                muted: 0x76787f,
                quiet: 0xa2a4aa,
                blue: 0x2f6fde,
                green: 0x3b7d3e,
                amber: 0xa86a00,
                orange: 0xd9620f,
                red: 0xc2414b,
                cyan: 0x1f8a99,
                purple: 0x8a3fc7,
                selected: 0xe2e8f1,
                hover: 0xebebed,
                primary_text: 0xffffff,
                blue_tint: 0xe7effc,
                green_tint: 0xe8f3e9,
                amber_tint: 0xfbf3e3,
                red_tint: 0xfbe9ea,
                popover: 0xf5f5f6,
                scrim: 0x0000001f,
                popover_hover: 0xe5e5e8,
                popover_border: 0xd8d8dc,
                fill_blue: 0x7fa9ef,
                fill_amber: 0xf0c36a,
                fill_red: 0xec8a91,
                lanes: [
                    0x2f6fde, 0x8a3fc7, 0x3b8a3e, 0xb07800, 0xc2414b, 0x1f8a99, 0xc0601a, 0x5b62c9,
                ],
            }
        }
    }
    pub fn lane(&self, i: usize) -> u32 {
        self.lanes[i % self.lanes.len()]
    }
}

pub fn palette(cx: &App) -> Palette {
    *cx.global::<Palette>()
}
pub fn ui_font() -> SharedString {
    TYPOGRAPHY.with_borrow(|t| t.0.clone())
}
pub fn mono() -> SharedString {
    TYPOGRAPHY.with_borrow(|t| t.1.clone())
}
pub fn ui_size() -> f32 {
    TYPOGRAPHY.with_borrow(|t| t.2)
}
pub fn mono_size() -> f32 {
    TYPOGRAPHY.with_borrow(|t| t.3)
}
/// Scale a deliberately smaller or larger interface label with the selected base size.
pub fn scaled_ui(size: f32) -> f32 {
    size * ui_size() / UI_SIZE
}
/// Scale code and chip labels with the selected monospace base size.
pub fn scaled_mono(size: f32) -> f32 {
    size * mono_size() / MONO_SIZE
}

/// Switch fonts and their base sizes; `None` means the default. Every window redraws with them.
pub fn set_fonts(ui: Option<&str>, mono: Option<&str>, ui_size: Option<f32>, mono_size: Option<f32>, cx: &mut App) {
    let (ui, mono) = (SharedString::from(ui.unwrap_or(SYSTEM_UI).to_string()), SharedString::from(mono.unwrap_or(DEFAULT_MONO).to_string()));
    TYPOGRAPHY.with_borrow_mut(|t| *t = (ui, mono, ui_size.unwrap_or(UI_SIZE), mono_size.unwrap_or(MONO_SIZE)));
    let t = Theme::global_mut(cx);
    t.font_family = ui_font();
    t.mono_font_family = self::mono();
    // Component chrome is based on a 15px rem while workspace copy is 13px.
    t.font_size = px(self::ui_size() + 2.);
    t.mono_font_size = px(self::mono_size());
    cx.refresh_windows();
}

/// Read the actual window appearance on startup and whenever macOS changes it.
pub fn sync(window: &mut Window, cx: &mut App) {
    apply(window.appearance(), cx);
    window.refresh();
}

pub fn init(cx: &mut App) {
    apply(cx.window_appearance(), cx);
}

fn apply(appearance: WindowAppearance, cx: &mut App) {
    // `TABLES_THEME=dark` or `light` pins the appearance, for checking the other one.
    let appearance = match std::env::var("TABLES_THEME").as_deref() {
        Ok("dark") => WindowAppearance::Dark,
        Ok("light") => WindowAppearance::Light,
        _ => appearance,
    };
    Theme::change(appearance, None, cx);
    let p = Palette::new(ThemeMode::from(appearance).is_dark());
    cx.set_global(p);
    let c = |v: u32| -> Hsla { rgb(v).into() };
    let t = Theme::global_mut(cx);
    // GPUI Component sizes controls in rem, and Root sets rem to this value.
    t.font_size = px(ui_size() + 2.);
    t.font_family = ui_font();
    t.mono_font_family = mono();
    t.mono_font_size = px(mono_size());
    t.radius = px(5.);
    t.radius_lg = px(8.);
    t.shadow = false;
    t.background = c(p.background);
    t.foreground = c(p.text);
    t.border = c(p.border);
    t.input = c(p.border);
    t.muted = c(p.chrome);
    t.muted_foreground = c(p.muted);
    t.primary = c(p.blue);
    t.primary_foreground = c(p.primary_text);
    t.primary_hover = c(p.blue).opacity(0.9);
    t.primary_active = c(p.blue).opacity(0.8);
    t.secondary = c(p.panel);
    t.secondary_foreground = c(p.text);
    t.secondary_hover = c(p.hover);
    t.secondary_active = c(p.border);
    t.accent = c(p.hover);
    t.accent_foreground = c(p.text);
    t.selection = c(p.selected);
    t.caret = c(p.blue);
    t.popover = c(p.popover);
    t.popover_foreground = c(p.text);
    t.ring = c(p.blue);
    t.link = c(p.blue);
    t.title_bar = c(p.chrome);
    t.title_bar_border = c(p.border);
    t.list_active = c(p.selected);
    t.list_active_border = c(p.selected);
    t.list_hover = c(p.hover);
    t.scrollbar = c(p.background).opacity(0.);
    t.scrollbar_thumb = c(p.quiet).opacity(0.45);
    t.scrollbar_thumb_hover = c(p.quiet).opacity(0.7);
    // Code editors sit on the window's own background, with a quiet current line.
    let mut hl = (*t.highlight_theme).clone();
    hl.style.editor_background = Some(c(p.background));
    hl.style.editor_active_line = Some(c(p.hover).opacity(0.55));
    hl.style.editor_line_number = Some(c(p.quiet));
    hl.style.editor_active_line_number = Some(c(p.text));
    t.highlight_theme = std::sync::Arc::new(hl);
}
