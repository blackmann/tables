//! A Zed-style popover menu: optional search, section labels, roomy rounded rows,
//! trailing check marks, keyboard navigation and outside-click dismissal.
//!
//! Entries are rebuilt from a closure on every render, so check marks always
//! reflect live workspace state (multi-select menus can stay open while toggling).
// Ported whole from Gitflow; not every item option has a caller here yet.
#![allow(dead_code)]
use crate::theme::palette;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{
    Icon, IconName, Sizable,
    input::{self, Input, InputState},
};
use std::rc::Rc;

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct Item {
    label: SharedString,
    detail: Option<SharedString>,
    description: Option<SharedString>,
    icon: Option<Icon>,
    /// A picture in place of the icon, such as an app's.
    image: Option<std::path::PathBuf>,
    checked: bool,
    disabled: bool,
    keep_open: bool,
    font: Option<SharedString>,
    handler: Option<Handler>,
    /// A small coloured dot before the label, for a status.
    dot: Option<u32>,
    /// Entries shown beside this one when it's highlighted.
    submenu: Option<Builder>,
}
impl Item {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            detail: None,
            description: None,
            icon: None,
            image: None,
            checked: false,
            disabled: false,
            keep_open: false,
            font: None,
            handler: None,
            dot: None,
            submenu: None,
        }
    }
    pub fn dot(mut self, color: u32) -> Self {
        self.dot = Some(color);
        self
    }
    /// Open these entries beside the item when it's highlighted (hover or →).
    pub fn submenu(mut self, build: Builder) -> Self {
        self.submenu = Some(build);
        self
    }
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }
    /// A sentence about the item, on its own line under the label. Unlike
    /// `detail`, which shares the label's line, it can be any length.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }
    /// Show a picture, such as an app's icon, where the icon goes.
    pub fn image(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.image = Some(path.into());
        self
    }
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    /// Keep the menu open after activation, for multi-select lists.
    pub fn keep_open(mut self) -> Self {
        self.keep_open = true;
        self
    }
    /// Render the label in a specific font family (font pickers preview themselves).
    pub fn font(mut self, font: impl Into<SharedString>) -> Self {
        self.font = Some(font.into());
        self
    }
    pub fn on_click(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.handler = Some(Rc::new(f));
        self
    }
}

// Entries are rebuilt per render and short-lived; boxing items would only add noise.
#[allow(clippy::large_enum_variant)]
pub enum Entry {
    Label(SharedString),
    Separator,
    Item(Item),
}

pub type Builder = Rc<dyn Fn(&App) -> Vec<Entry>>;

/// A submenu asks to close and hand focus back (← or Escape), without
/// closing the menu it came from.
pub struct Back;

pub struct Menu {
    focus: FocusHandle,
    search: Option<Entity<InputState>>,
    build: Builder,
    highlighted: Option<usize>,
    width: Pixels,
    scroll: ScrollHandle,
    /// The submenu showing, and the item it belongs to.
    sub: Option<(usize, Entity<Menu>)>,
    /// Where the submenu was last drawn, so a press there isn't "outside".
    sub_bounds: Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    /// Where each item with a submenu was last drawn, to open it beside.
    rows: Rc<std::cell::RefCell<std::collections::HashMap<usize, Bounds<Pixels>>>>,
    /// This menu is another's submenu: its parent decides when it closes.
    nested: bool,
}
impl EventEmitter<DismissEvent> for Menu {}
impl EventEmitter<Back> for Menu {}
impl Focusable for Menu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
impl Menu {
    /// The labels of the items as built now, and of the open submenu's.
    pub fn labels(&self, cx: &App) -> (Vec<String>, Option<Vec<String>>) {
        let labels = (self.build)(cx)
            .into_iter()
            .filter_map(|e| match e {
                Entry::Item(item) => Some(format!("{}{}", item.label, if item.image.is_some() { " [img]" } else { "" })),
                _ => None,
            })
            .collect();
        (labels, self.sub.as_ref().map(|(_, m)| m.read(cx).labels(cx).0))
    }
}

impl Menu {
    pub fn new(
        build: Builder,
        search: Option<&str>,
        width: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = search.map(|placeholder| {
            let input =
                cx.new(|cx| InputState::new(window, cx).placeholder(placeholder.to_string()));
            // Subscribe to edits only: the input also notifies on every caret blink,
            // which would otherwise clear the hover highlight.
            cx.subscribe(&input, |m: &mut Menu, _, e: &input::InputEvent, cx| {
                if matches!(e, input::InputEvent::Change) {
                    m.highlighted = None;
                    cx.notify();
                }
            })
            .detach();
            input
        });
        Self {
            focus: cx.focus_handle(),
            search,
            build,
            highlighted: None,
            width,
            scroll: ScrollHandle::new(),
            sub: None,
            sub_bounds: Rc::default(),
            rows: Rc::default(),
            nested: false,
        }
    }
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        match &self.search {
            Some(s) => s.update(cx, |s, cx| s.focus(window, cx)),
            None => self.focus.focus(window),
        }
    }
    /// While searching, show only matching items, without section chrome.
    fn visible(&self, cx: &App) -> Vec<Entry> {
        let entries = (self.build)(cx);
        let query = self
            .search
            .as_ref()
            .map(|s| s.read(cx).value().trim().to_lowercase())
            .unwrap_or_default();
        if query.is_empty() {
            return entries;
        }
        entries
            .into_iter()
            .filter(|e| match e {
                Entry::Item(i) => {
                    i.label.to_lowercase().contains(&query)
                        || i.detail
                            .as_ref()
                            .is_some_and(|d| d.to_lowercase().contains(&query))
                        || i.description
                            .as_ref()
                            .is_some_and(|d| d.to_lowercase().contains(&query))
                }
                _ => false,
            })
            .collect()
    }
    fn selectable(entries: &[Entry]) -> Vec<usize> {
        entries
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, Entry::Item(i) if !i.disabled))
            .map(|(i, _)| i)
            .collect()
    }
    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.visible(cx);
        let Some(Entry::Item(item)) = entries.get(index) else {
            return;
        };
        if item.disabled {
            return;
        }
        if item.submenu.is_some() {
            self.open_sub(index, true, window, cx);
            return;
        }
        if let Some(handler) = item.handler.clone() {
            handler(window, cx);
        }
        if item.keep_open {
            cx.notify();
        } else {
            cx.emit(DismissEvent);
        }
    }
    /// Show the submenu of item `index`, if it has one, closing any other;
    /// with `focus`, keys go to it.
    fn open_sub(&mut self, index: usize, focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.sub.as_ref().is_some_and(|(i, _)| *i == index) {
            if focus && let Some((_, sub)) = &self.sub {
                sub.update(cx, |m, cx| m.focus(window, cx));
            }
            return;
        }
        self.sub = None;
        let entries = self.visible(cx);
        let Some(Entry::Item(Item { submenu: Some(build), .. })) = entries.get(index) else {
            cx.notify();
            return;
        };
        let build = build.clone();
        let sub = cx.new(|cx| {
            let mut m = Menu::new(build, None, px(220.), window, cx);
            m.nested = true;
            m
        });
        // Picking something in the submenu closes the whole menu; backing out
        // of it returns here.
        cx.subscribe(&sub, |_, _, _: &DismissEvent, cx| cx.emit(DismissEvent)).detach();
        cx.subscribe_in(&sub, window, |m: &mut Menu, _, _: &Back, window, cx| {
            m.sub = None;
            m.focus.focus(window);
            cx.notify();
        })
        .detach();
        if focus {
            sub.update(cx, |m, cx| {
                m.focus(window, cx);
                m.step(true, cx);
            });
        }
        self.sub = Some((index, sub));
        cx.notify();
    }
    /// Draw again, with any submenu: the state it shows has changed.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some((_, sub)) = &self.sub {
            sub.update(cx, |m, cx| m.refresh(cx));
        }
        cx.notify();
    }
    fn step(&mut self, down: bool, cx: &mut Context<Self>) {
        let entries = self.visible(cx);
        let items = Self::selectable(&entries);
        if items.is_empty() {
            return;
        }
        let pos = self
            .highlighted
            .and_then(|h| items.iter().position(|i| *i == h));
        let next = match (pos, down) {
            (None, true) => 0,
            (None, false) => items.len() - 1,
            (Some(p), true) => (p + 1) % items.len(),
            (Some(p), false) => (p + items.len() - 1) % items.len(),
        };
        self.highlighted = Some(items[next]);
        self.scroll.scroll_to_item(items[next]);
        cx.notify();
    }
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let first = Self::selectable(&self.visible(cx)).first().copied();
        if let Some(i) = self.highlighted.or(first) {
            self.activate(i, window, cx);
        }
    }
    /// Without a search field the menu itself holds focus and sees raw keys.
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // The submenu sits inside this menu, so its keys pass here first: leave
        // them to it while it has focus.
        if let Some((_, sub)) = &self.sub
            && sub.read(cx).focus.is_focused(window)
        {
            return;
        }
        match event.keystroke.key.as_str() {
            "down" => self.step(true, cx),
            "up" => self.step(false, cx),
            "enter" => self.confirm(window, cx),
            "right" => match self.highlighted {
                Some(i) => self.open_sub(i, true, window, cx),
                None => return,
            },
            "left" if self.nested => cx.emit(Back),
            "escape" if self.nested => cx.emit(Back),
            "escape" => cx.emit(DismissEvent),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for Menu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = palette(cx);
        let entries = self.visible(cx);
        let searching = self
            .search
            .as_ref()
            .is_some_and(|s| !s.read(cx).value().trim().is_empty());
        let mut list = div()
            .id("menu-list")
            .track_scroll(&self.scroll)
            .flex()
            .flex_col()
            .max_h(px(360.))
            .overflow_y_scroll()
            .p(px(4.));
        let empty = entries.is_empty();
        // The open submenu, once its item has been drawn somewhere to open beside.
        let sub = self
            .sub
            .as_ref()
            .and_then(|(i, m)| Some((m.clone(), *self.rows.borrow().get(i)?)));
        // The submenu opens to the right of its item, or to the left when it
        // wouldn't fit there: snapping it back inside the window would cover
        // the menu it came from.
        let sub = sub.map(|(menu, at)| {
            let width = menu.read(cx).width;
            let fits_right = at.right() + px(6.) + width + px(8.) <= window.viewport_size().width;
            let (corner, x) = if fits_right { (Corner::TopLeft, at.right() + px(6.)) } else { (Corner::TopRight, at.left() - px(6.)) };
            (menu, corner, point(x, at.top() - px(5.)))
        });
        for (i, entry) in entries.into_iter().enumerate() {
            list = list.child(match entry {
                Entry::Label(text) => div()
                    .flex_shrink_0()
                    .px(px(8.))
                    .pt(px(if i == 0 { 2. } else { 6. }))
                    .pb(px(3.))
                    .text_size(px(crate::theme::scaled_ui(12.)))
                    .text_color(rgb(colors.muted))
                    .child(text)
                    .into_any_element(),
                Entry::Separator => div()
                    .my(px(4.))
                    .mx(px(-4.))
                    .h(px(1.))
                    .bg(rgb(colors.popover_border))
                    .into_any_element(),
                Entry::Item(item) => {
                    let highlighted = self.highlighted == Some(i);
                    let has_sub = item.submenu.is_some();
                    let rows = self.rows.clone();
                    div()
                        .id(("menu-item", i))
                        .relative()
                        // The list scrolls; rows keep their height rather than squeeze.
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .min_h(px(28.))
                        .px(px(8.))
                        .rounded(px(6.))
                        .text_size(px(crate::theme::scaled_ui(13.)))
                        .when(item.disabled, |s| s.text_color(rgb(colors.quiet)))
                        .when(!item.disabled, |s| {
                            s.cursor_pointer()
                                .when(highlighted, |s| s.bg(rgb(colors.popover_hover)))
                                .on_mouse_move(cx.listener(move |m, _, window, cx| {
                                    if m.highlighted != Some(i) {
                                        m.highlighted = Some(i);
                                        // Hovering another item closes a submenu, or opens its own.
                                        m.open_sub(i, false, window, cx);
                                        cx.notify();
                                    }
                                }))
                                .on_click(cx.listener(move |m, _, w, cx| m.activate(i, w, cx)))
                        })
                        .when_some(item.image, |s, path| s.child(img(path).flex_shrink_0().size(px(16.))))
                        .when_some(item.icon, |s, icon| {
                            s.child(icon.size(px(14.)).text_color(rgb(colors.muted)))
                        })
                        .when_some(item.dot, |s, dot| {
                            s.child(div().flex_shrink_0().size(px(7.)).rounded_full().bg(rgb(dot)))
                        })
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .when(item.description.is_some(), |s| s.py(px(5.)).gap(px(1.)))
                                .child(
                                    div()
                                        .truncate()
                                        .when_some(item.font, |s, f| s.font_family(f))
                                        .child(item.label),
                                )
                                .when_some(item.description, |s, d| {
                                    s.child(
                                        div()
                                            .text_size(px(crate::theme::scaled_ui(12.)))
                                            .line_height(relative(1.35))
                                            .text_color(rgb(colors.muted))
                                            .child(d),
                                    )
                                }),
                        )
                        .when(item.checked, |s| {
                            s.child(
                                Icon::new(IconName::Check)
                                    .flex_shrink_0()
                                    .size(px(14.))
                                    .text_color(rgb(colors.blue)),
                            )
                        })
                        .when_some(item.detail, |s, d| {
                            s.child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(crate::theme::scaled_ui(12.)))
                                    .text_color(rgb(colors.muted))
                                    .child(d),
                            )
                        })
                        .when(has_sub, |s| {
                            s.child(
                                Icon::new(IconName::ChevronRight)
                                    .flex_shrink_0()
                                    .size(px(14.))
                                    .text_color(rgb(colors.muted)),
                            )
                        })
                        // Note where it is, for its submenu to open beside.
                        .when(has_sub, |s| {
                            s.child(
                                canvas(
                                    move |bounds, _, _| {
                                        rows.borrow_mut().insert(i, bounds);
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .inset_0(),
                            )
                        })
                        .into_any_element()
                }
            });
        }
        if empty {
            list = list.child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .text_color(rgb(colors.muted))
                    .child(if searching {
                        "No matches"
                    } else {
                        "Nothing here yet"
                    }),
            );
        }
        div()
            .track_focus(&self.focus)
            .key_context("ChatMenu")
            .capture_key_down(cx.listener(Self::key))
            .capture_action(cx.listener(|m, _: &input::MoveDown, _, cx| {
                cx.stop_propagation();
                m.step(true, cx)
            }))
            .capture_action(cx.listener(|m, _: &input::MoveUp, _, cx| {
                cx.stop_propagation();
                m.step(false, cx)
            }))
            .capture_action(cx.listener(|m, _: &input::Enter, w, cx| {
                cx.stop_propagation();
                m.confirm(w, cx)
            }))
            .capture_action(cx.listener(|_, _: &input::Escape, _, cx| {
                cx.stop_propagation();
                cx.emit(DismissEvent)
            }))
            .on_mouse_down_out(cx.listener(|m, e: &MouseDownEvent, _, cx| {
                // A submenu is its parent's to close; a press in it isn't outside.
                let in_sub = m.sub.is_some() && m.sub_bounds.get().is_some_and(|b| b.contains(&e.position));
                if !m.nested && !in_sub {
                    cx.emit(DismissEvent)
                }
            }))
            .occlude()
            .debug_selector({
                let nested = self.nested;
                move || if nested { "submenu".into() } else { "menu".into() }
            })
            .w(self.width)
            .flex()
            .flex_col()
            .bg(rgb(colors.popover))
            .border_1()
            .border_color(rgb(colors.popover_border))
            .rounded(px(10.))
            .shadow_lg()
            .text_color(rgb(colors.text))
            .font_family(crate::theme::ui_font())
            .when_some(self.search.clone(), |s, search| {
                s.child(
                    div()
                        .h(px(38.))
                        .px(px(6.))
                        .flex()
                        .items_center()
                        .border_b_1()
                        .border_color(rgb(colors.popover_border))
                        .child(Input::new(&search).appearance(false).small()),
                )
            })
            .child(div().rounded(px(10.)).overflow_hidden().child(list))
            // The submenu, beside its item and level with it. Drawn last and
            // outside the list, so nothing covers or clips it; menus are
            // already drawn deferred, so it can't be deferred again.
            .when_some(sub, |s, (sub, corner, at)| {
                let sub_bounds = self.sub_bounds.clone();
                s.child(
                    anchored()
                        .anchor(corner)
                        .position(at)
                        .snap_to_window_with_margin(px(8.))
                        .child(
                            div().relative().child(sub).child(
                                canvas(move |bounds, _, _| sub_bounds.set(Some(bounds)), |_, _, _, _| {})
                                    .absolute()
                                    .inset_0(),
                            ),
                        ),
                )
            })
    }
}
