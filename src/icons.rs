//! The app's glyphs, drawn on a 16px grid to match the design mockup, served
//! ahead of GPUI Component's embedded icon set.
use gpui::{AssetSource, Result, SharedString};
use gpui_component::IconNamed;
use std::borrow::Cow;

macro_rules! glyphs {
    ($($name:ident => $file:literal : $body:literal,)*) => {
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        #[allow(dead_code)]
        pub enum Glyph { $($name,)* }
        impl Glyph {
            #[cfg(test)]
            pub const ALL: &[Glyph] = &[$(Glyph::$name,)*];
            fn file(self) -> &'static str { match self { $(Glyph::$name => $file,)* } }
        }
        fn body(file: &str) -> Option<&'static str> {
            match file { $($file => Some($body),)* _ => None }
        }
    };
}

glyphs! {
    Table => "table.svg": r#"<rect x="2" y="2.5" width="12" height="11" rx="1.5"/><path d="M2 6h12M2 9.5h12M6.5 6v7.5"/>"#,
    View => "view.svg": r#"<path d="M1.5 8s2.4-4.5 6.5-4.5S14.5 8 14.5 8 12.1 12.5 8 12.5 1.5 8 1.5 8z"/><circle cx="8" cy="8" r="1.8"/>"#,
    Function => "fn.svg": r#"<path d="M10.5 2.5c-1.6-.4-2.7.3-3 2L6 12c-.3 1.6-1.3 2-2.7 1.6M4.5 6.5h5"/>"#,
    Sql => "sql.svg": r#"<path d="M5 4.5 1.5 8 5 11.5M11 4.5 14.5 8 11 11.5M9.2 3 6.8 13"/>"#,
    Key => "key.svg": r#"<circle cx="5.5" cy="10.5" r="3"/><path d="m7.6 8.4 6-6M11.5 4.5l1.5 1.5M10 6l1.2 1.2"/>"#,
    Link => "link.svg": r#"<path d="M6.5 9.5 9.5 6.5M7 4.5l1-1a2.8 2.8 0 0 1 4 4l-1 1M9 11.5l-1 1a2.8 2.8 0 0 1-4-4l1-1"/>"#,
    Unlink => "unlink.svg": r#"<path d="M7 4.5l1-1a2.8 2.8 0 0 1 4 4l-1 1M9 11.5l-1 1a2.8 2.8 0 0 1-4-4l1-1M5.5 1.5V3M1.5 5.5H3M10.5 14.5V13M14.5 10.5H13"/>"#,
    Search => "search.svg": r#"<circle cx="7" cy="7" r="4.5"/><path d="m10.5 10.5 3.5 3.5"/>"#,
    Filter => "filter.svg": r#"<path d="M2 3.5h12l-4.5 5.2v4.3l-3 1.5V8.7z"/>"#,
    Plus => "plus.svg": r#"<path d="M8 3v10M3 8h10"/>"#,
    Minus => "minus.svg": r#"<path d="M3 8h10"/>"#,
    UpDown => "updown.svg": r#"<path d="m5 6 3-3 3 3M5 10l3 3 3-3"/>"#,
    Close => "x.svg": r#"<path d="m4 4 8 8M12 4l-8 8"/>"#,
    Down => "down.svg": r#"<path d="m4 6 4 4 4-4"/>"#,
    Up => "up.svg": r#"<path d="m4 10 4-4 4 4"/>"#,
    Left => "left.svg": r#"<path d="m10 4-4 4 4 4"/>"#,
    Right => "right.svg": r#"<path d="m6 4 4 4-4 4"/>"#,
    Refresh => "refresh.svg": r#"<path d="M13.5 8a5.5 5.5 0 1 1-1.6-3.9M13.5 2.5v3h-3"/>"#,
    Sun => "sun.svg": r#"<circle cx="8" cy="8" r="3"/><path d="M8 1.5v1.5M8 13v1.5M1.5 8H3M13 8h1.5M3.4 3.4l1 1M11.6 11.6l1 1M3.4 12.6l1-1M11.6 4.4l1-1"/>"#,
    Moon => "moon.svg": r#"<path d="M13.5 9.5A5.5 5.5 0 0 1 6.5 2.5a5.5 5.5 0 1 0 7 7z"/>"#,
    Sidebar => "sidebar.svg": r#"<rect x="1.5" y="2.5" width="13" height="11" rx="1.5"/><path d="M6 2.5v11"/>"#,
    Panel => "panel.svg": r#"<rect x="1.5" y="2.5" width="13" height="11" rx="1.5"/><path d="M10 2.5v11"/>"#,
    Play => "play.svg": r#"<path d="M4.5 2.8v10.4L13 8z" fill="currentColor" stroke="none"/>"#,
    Stop => "stop.svg": r#"<rect x="4" y="4" width="8" height="8" rx="1.5" fill="currentColor" stroke="none"/>"#,
    History => "history.svg": r#"<path d="M2.5 8a5.5 5.5 0 1 0 1.6-3.9M2.5 2.5v3h3M8 5v3.2l2 1.3"/>"#,
    Save => "save.svg": r#"<path d="M3 2.5h8l2.5 2.5v8.5h-11zM5 2.5v3.5h5V2.5M5 13.5V9.5h6v4"/>"#,
    Database => "db.svg": r#"<ellipse cx="8" cy="3.8" rx="5.5" ry="2"/><path d="M2.5 3.8v8.4c0 1.1 2.5 2 5.5 2s5.5-.9 5.5-2V3.8M2.5 8c0 1.1 2.5 2 5.5 2s5.5-.9 5.5-2"/>"#,
    Columns => "cols.svg": r#"<rect x="2" y="2.5" width="12" height="11" rx="1.5"/><path d="M6 2.5v11M10 2.5v11"/>"#,
    Download => "download.svg": r#"<path d="M8 2.5v8M4.5 7 8 10.5 11.5 7M2.5 13.5h11"/>"#,
    Trash => "trash.svg": r#"<path d="M2.5 4.5h11M6 4.5V3h4v1.5M4 4.5l.7 9h6.6l.7-9"/>"#,
    Copy => "copy.svg": r#"<rect x="5" y="5" width="8.5" height="8.5" rx="1.5"/><path d="M11 5V3.5A1 1 0 0 0 10 2.5H3.5a1 1 0 0 0-1 1V10a1 1 0 0 0 1 1H5"/>"#,
    Arrow => "arrow.svg": r#"<path d="M4.5 11.5 11.5 4.5M6 4.5h5.5V10"/>"#,
    Warn => "warn.svg": r#"<path d="M8 2 14.5 13.5h-13zM8 6.5v3.2M8 11.6v.1"/>"#,
    Check => "check.svg": r#"<path d="m3 8.5 3 3 7-7"/>"#,
    Bolt => "bolt.svg": r#"<path d="M9 1.5 3.5 9H8l-1 5.5L12.5 7H8z"/>"#,
    Pin => "pin.svg": r#"<path d="M6 2.5h4l-.5 4 2.5 2.5H4L6.5 6.5zM8 9v5"/>"#,
    Format => "format.svg": r#"<path d="M2.5 3.5h11M2.5 6.5h7M2.5 9.5h11M2.5 12.5h7"/>"#,
    Explain => "explain.svg": r#"<path d="M3 2.5v11h10.5M5.5 10.5l2.5-3 2 2 3-4.5"/>"#,
    Duplicate => "dup.svg": r#"<rect x="2.5" y="2.5" width="8" height="8" rx="1.5"/><path d="M13.5 5.5v8h-8"/>"#,
    Shield => "shield.svg": r#"<path d="M8 1.5 13.5 3.5v4c0 3.5-2.5 6-5.5 7-3-1-5.5-3.5-5.5-7v-4z"/>"#,
    Fingerprint => "fingerprint.svg": r#"<path d="M3.6 4.2A5.5 5.5 0 0 1 13.5 7.5v1M2.6 6.6c-.1.5-.1.9-.1 1.4v1.5M5 13.3c-.4-.9-.5-1.9-.5-3V8a3.5 3.5 0 0 1 7 0v2M8 8v2.5c0 1.3.4 2.4 1.1 3.3M11.4 12.4c-.1.4-.2.8-.4 1.1"/>"#,
    Lock => "lock.svg": r#"<rect x="3" y="7" width="10" height="7" rx="1.5"/><path d="M5 7V5a3 3 0 0 1 6 0v2"/>"#,
    Terminal => "terminal.svg": r#"<rect x="1.5" y="2.5" width="13" height="11" rx="1.5"/><path d="m4.5 6 2 2-2 2M8 10.5h3.5"/>"#,
    Sort => "sort.svg": r#"<path d="M5 3v10M2.5 10.5 5 13l2.5-2.5M11 13V3M8.5 5.5 11 3l2.5 2.5"/>"#,
    Null => "null.svg": r#"<circle cx="8" cy="8" r="5.5"/><path d="m4.2 11.8 7.6-7.6"/>"#,
    Dots => "dots.svg": r#"<circle cx="3.5" cy="8" r=".9" fill="currentColor"/><circle cx="8" cy="8" r=".9" fill="currentColor"/><circle cx="12.5" cy="8" r=".9" fill="currentColor"/>"#,
    Edit => "edit.svg": r#"<path d="M10.5 2.5 13.5 5.5 6 13H3v-3z"/>"#,
    Plug => "plug.svg": r#"<path d="M8 14.5v-3M6 5V1.5M10 5V1.5M12 5v3a3.5 3.5 0 0 1-3.5 3.5h-1A3.5 3.5 0 0 1 4 8V5z"/>"#,
    Elephant => "elephant.svg": r#"<path fill="currentColor" stroke="none" fill-rule="evenodd" d="M1.6 12.8C1.2 11 1.8 9.2 2.6 7.8 3.2 5.2 5 3.6 7.4 3.6c1 0 1.8.3 2.4.7 2-.4 4.6.5 4.8 3.3V13h-2v-2.2H9.4V13h-2v-2.6c-.8 0-1.8-.5-2.4-1.2-.6.8-1.1 2-1.6 3.4zM5.2 5.45a.55.55 0 1 0 0 1.1.55.55 0 1 0 0-1.1zM7 5.1c1.3.1 2 1.2 1.9 2.4-.1 1-.9 1.8-2 1.8.7-.5 1.1-1.1 1.1-2S7.6 5.6 7 5.1z"/>"#,
    Feather => "feather.svg": r#"<path d="M13.8 2.2C8.6 2.6 4.6 6.4 3.6 12.8M13.8 2.2c.6 4.6-2.6 8.6-8.2 8.9M3.6 12.8 2.2 14.2M7.4 6.6l1.9 1.9M9.6 4.6l1.6 1.6"/>"#,
    Folder => "folder.svg": r#"<path d="M1.5 4a1 1 0 0 1 1-1h3.6l1.6 1.8h5.8a1 1 0 0 1 1 1V12.5a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1z"/>"#,
}

const PREFIX: &str = "tables/";

impl IconNamed for Glyph {
    fn path(self) -> SharedString {
        format!("{PREFIX}{}", self.file()).into()
    }
}

/// Serves app glyphs, then falls back to GPUI Component's embedded icons.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(b) = path.strip_prefix(PREFIX).and_then(body) {
            return Ok(Some(Cow::Owned(
                format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">{b}</svg>"#)
                    .into_bytes(),
            )));
        }
        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_component_assets::Assets.list(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_glyph_resolves() {
        for g in Glyph::ALL {
            assert!(Assets.load(&g.path()).unwrap().is_some(), "{g:?}");
        }
    }
}
