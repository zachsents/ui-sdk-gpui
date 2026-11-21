//! Port of `components/navigation-menu/navigation-menu-focus.ts`: where
//! keys move focus in a navigation menu: the APG's optional arrows, into a
//! panel an arrow key opened, and Tab around a panel that renders
//! elsewhere. Otherwise Tab stays gpui's, except where the popup layer
//! takes it.
//!
//! The original reads page order and containment from the DOM. Here the
//! menu's triggers and links register as they render, which is page order,
//! and what's inside what comes from the focus handles their elements
//! track, as of the last frame. A panel rendered above the window, as every
//! surface is, comes last in Tab's order, as one portaled to the end of the
//! page does in the original.

use gpui::{Context, FocusHandle, KeyDownEvent, SharedString, Window};
use ui_sdk_focus::{next_tabbable, tabbables};
use ui_sdk_kernel::{InputEvent, Key, Reason};
use ui_sdk_native::is_typing;
use ui_sdk_platform::is_rtl;
use ui_sdk_popups::{PopupOpener, owns_event};

use crate::navigation_menu_api::{ListEntry, NavigationMenu};
use crate::navigation_menu_keyboard::{KeyStep, is_modified, list_key, panel_key, step_index};

impl NavigationMenu {
    // Whether the element `handle` tracks is inside one of the menu's
    // contents, which hold links of their own.
    fn in_content(&self, handle: &FocusHandle, window: &Window) -> bool {
        self.content_regions
            .borrow()
            .values()
            .any(|(region, _)| region.contains_focus(handle, window))
    }

    /// Top-level triggers and links in this menu's list, in page order: not
    /// inside a content in it, and not disabled. Contents kept closed in
    /// place hold links too, and a nested menu's list holds its own
    /// (`topLevel`).
    pub(crate) fn top_level(&self, window: &Window) -> Vec<(ListEntry, FocusHandle)> {
        self.list_items
            .keys()
            .into_iter()
            .filter_map(|entry| {
                let handle = match &entry {
                    ListEntry::Trigger(key) => {
                        if self.triggers.info(key).is_some_and(|props| props.disabled) {
                            return None;
                        }
                        self.trigger_handle(key)?
                    }
                    ListEntry::Link(key) => self.link_regions.borrow().get(key)?.focus_handle()?,
                };
                (handle.is_rendered(window) && !self.in_content(&handle, window)).then_some((entry, handle))
            })
            .collect()
    }

    // The top-level links as of the last frame, for the indicator, which
    // renders the menu again when they changed.
    pub(crate) fn settle_top_level(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let links: Vec<SharedString> = self
            .top_level(window)
            .into_iter()
            .filter_map(|(entry, _)| match entry {
                ListEntry::Link(key) => Some(key),
                ListEntry::Trigger(_) => None,
            })
            .collect();
        if *self.top_links.borrow() != links {
            *self.top_links.borrow_mut() = links;
            cx.notify();
        }
    }

    /// Focuses the first link in an item's panel, now if it's shown, or
    /// once its content renders (`enterPanel`).
    pub(crate) fn enter_panel(&mut self, key: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let content = self
            .content_regions
            .borrow()
            .get(key)
            .and_then(|(region, _)| region.focus_handle());
        match content {
            Some(content)
                if self.value.get().as_ref() == Some(key) && self.is_shown(key) && content.is_rendered(window) =>
            {
                self.pending = None;
                if let Some(first) = tabbables(&content, window, cx).into_iter().next() {
                    window.focus(&first, cx);
                }
            }
            _ => self.pending = Some(key.clone()),
        }
    }

    /// A content rendered: focus moves in if an arrow key opened its item
    /// (`contentBound`).
    pub(crate) fn content_bound(&mut self, key: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.as_ref() != Some(key) || self.value.get().as_ref() != Some(key) {
            return;
        }
        self.pending = None;
        let content = self
            .content_regions
            .borrow()
            .get(key)
            .and_then(|(region, _)| region.focus_handle());
        if let Some(content) = content
            && let Some(first) = tabbables(&content, window, cx).into_iter().next()
        {
            window.focus(&first, cx);
        }
    }

    /// Arrow keys, Home and End on a top-level trigger or link move along
    /// the list, and Down, or the inline end in a vertical menu, opens a
    /// trigger's panel and moves into it. Nothing wraps (`listKeydown`).
    pub(crate) fn list_keydown(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // A key a nested menu moved focus for already went: focus moves at
        // once here, where the original's next handler reads the same target.
        if is_modified(&event.keystroke.modifiers) || window.default_prevented() {
            return;
        }
        let Some(focused) = window.focused(cx) else {
            return;
        };
        // Keys from a nested menu's list, or from a panel, aren't on one of
        // this list's items.
        let items = self.top_level(window);
        let Some(index) = items
            .iter()
            .position(|(_, item)| *item == focused || item.contains(&focused, window))
        else {
            return;
        };
        let Some(step) = list_key(&Key::of(&event.keystroke), self.orientation, is_rtl(cx)) else {
            return;
        };
        if step != KeyStep::Enter {
            window.prevent_default();
            let (_, next) = &items[step_index(index, step, items.len())];
            window.focus(next, cx);
            return;
        }
        let ListEntry::Trigger(key) = items[index].0.clone() else {
            return;
        };
        if self.triggers.info(&key).is_some_and(|props| props.disabled) {
            return;
        }
        window.prevent_default();
        self.open(
            key.clone(),
            Reason::TriggerPress,
            Some(InputEvent::KeyDown(event.clone())),
            PopupOpener::Keyboard,
            window,
            cx,
        );
        self.enter_panel(&key, window, cx);
    }

    /// Arrow keys, Home and End on a link inside an open panel move between
    /// the panel's tab stops. Keys from a nested menu's panel are its own,
    /// and a text field in the panel, like a search box, keeps the keys that
    /// move its caret (`panelKeydown`).
    pub(crate) fn panel_keydown(
        &mut self,
        key: &SharedString,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(region) = self.content_regions.borrow().get(key).map(|(region, _)| region.clone()) else {
            return;
        };
        if is_modified(&event.keystroke.modifiers)
            || window.default_prevented()
            || !owns_event(&region, window, cx)
            || is_typing(window)
        {
            return;
        }
        let Some(focused) = window.focused(cx) else {
            return;
        };
        let Some(step) = panel_key(&Key::of(&event.keystroke), is_rtl(cx)) else {
            return;
        };
        let Some(panel) = region.focus_handle() else {
            return;
        };
        let links = tabbables(&panel, window, cx);
        let Some(index) = links
            .iter()
            .position(|link| *link == focused || link.contains(&focused, window))
        else {
            return;
        };
        window.prevent_default();
        window.focus(&links[step_index(index, step, links.len())], cx);
    }

    /// Shift+Tab from the item after an open panel goes back into the
    /// panel, as it would if the panel sat right after its trigger in the
    /// page. Tab past the last item skips the panel it already went through
    /// (`rootKeydown`).
    pub(crate) fn root_keydown(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(value) = self.value.get().clone() else {
            return;
        };
        if Key::of(&event.keystroke) != Key::Tab {
            return;
        }
        let Some(panel) = self.surface_element().and_then(|panel| panel.focus_handle()) else {
            return;
        };
        let Some(trigger) = self.trigger_handle(&value) else {
            return;
        };
        let Some(target) = window.focused(cx) else {
            return;
        };
        if panel == target || panel.contains(&target, window) {
            return;
        }
        // Where each sits in the order Tab visits the navigation.
        let Some(root) = self.shared.root.focus_handle() else {
            return;
        };
        let order = tabbables(&root, window, cx);
        let place = |handle: &FocusHandle| order.iter().position(|other| other == handle);
        let panel_place = order
            .iter()
            .position(|other| *other == panel || panel.contains(other, window));
        let after = |a: Option<usize>, b: Option<usize>| matches!((a, b), (Some(a), Some(b)) if a > b);
        if !event.keystroke.modifiers.shift {
            let next = next_tabbable(&target, None, window, cx);
            let beyond = next_tabbable(&panel, Some(&panel), window, cx);
            if next.is_some_and(|next| next == panel || panel.contains(&next, window))
                && after(place(&target), place(&trigger))
                && let Some(beyond) = beyond
            {
                window.prevent_default();
                window.focus(&beyond, cx);
            }
            return;
        }
        // Already between the trigger and here, Tab gets there on its own.
        if after(panel_place, place(&trigger)) && after(place(&target), panel_place) {
            return;
        }
        if next_tabbable(&trigger, Some(&panel), window, cx).as_ref() != Some(&target) {
            return;
        }
        let Some(last) = tabbables(&panel, window, cx).into_iter().last() else {
            return;
        };
        window.prevent_default();
        window.focus(&last, cx);
    }
}
