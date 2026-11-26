mod auth;
mod completion;
mod db;
mod export;
mod grid;
mod icons;
mod menu;
mod session;
mod sql_view;
mod tab;
mod table_view;
mod theme;
mod ui;
mod welcome;
mod workspace;

use gpui::*;
use workspace::*;

fn main() {
    Application::new().with_assets(icons::Assets).run(move |cx| {
        gpui_component::init(cx);
        theme::init(cx);
        cx.bind_keys(key_bindings());
        grid::bind_keys(cx);
        sql_view::bind_keys(cx);
        welcome::bind_keys(cx);
        let settings = db::config::Store::load().settings;
        theme::set_fonts(settings.ui_font.as_deref(), settings.mono_font.as_deref(), settings.ui_size, settings.mono_size, cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.set_menus(vec![
            Menu {
                name: "Tables".into(),
                items: vec![
                    MenuItem::action("Settings…", OpenSettings),
                    MenuItem::action("New Connection…", NewConnection),
                    MenuItem::separator(),
                    MenuItem::action("Quit Tables", Quit),
                ],
            },
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("New Query", NewQuery),
                    MenuItem::action("Save", Save),
                    MenuItem::separator(),
                    MenuItem::action("Import from TablePlus…", ImportTablePlus),
                    MenuItem::separator(),
                    MenuItem::action("Close Tab", CloseTab),
                    MenuItem::action("Disconnect", Disconnect),
                ],
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Command Palette", OpenPalette),
                    MenuItem::separator(),
                    MenuItem::action("Toggle Sidebar", ToggleSidebar),
                    MenuItem::action("Toggle Row Detail", ToggleDetail),
                    MenuItem::separator(),
                    MenuItem::action("Reload", Reload),
                    MenuItem::action("Next Tab", NextTab),
                    MenuItem::action("Previous Tab", PrevTab),
                ],
            },
        ]);
        // Start on the welcome window; a path or URL argument connects straight away.
        welcome::open(std::env::args().nth(1), cx);
        cx.on_window_closed(|cx| {
            if !cx.windows().is_empty() {
                return;
            }
            // The last workspace window went back to the welcome window; closing
            // the welcome window itself quits.
            if cx.try_global::<welcome::ShowOnClose>().is_some_and(|s| s.0) {
                cx.set_global(welcome::ShowOnClose(false));
                welcome::open(None, cx);
            } else {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
