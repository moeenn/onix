//! Pointer cursors and styling for widgets that GTK and libadwaita create internally.

use adw::prelude::*;

/// Sets a pointer cursor on `root` and every button inside it.
pub fn set_on_buttons(root: &gtk::Widget) {
    if root.is::<gtk::Button>() {
        root.set_cursor_from_name(Some("pointer"));
    }
    let mut child = root.first_child();
    while let Some(current) = child {
        set_on_buttons(&current);
        child = current.next_sibling();
    }
}

/// Gives the header bar's built-in close button a pointer cursor. The button is created
/// inside libadwaita, so it is found through its `windowcontrols` container.
pub fn set_on_window_controls(widget: &gtk::Widget) {
    if widget.css_name() == "windowcontrols" {
        widget.set_cursor_from_name(Some("pointer"));
        return;
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        set_on_window_controls(&current);
        child = current.next_sibling();
    }
}

/// Gives the options of a drop-down menu a pointer cursor. Menus built from a menu model
/// create their option widgets internally, so they are found by CSS name each time the
/// menu opens, which also covers options added later.
pub fn set_on_menu_items(popover: Option<gtk::Popover>) {
    if let Some(popover) = popover {
        popover.connect_map(|popover| set_on_css_name(popover.upcast_ref(), "modelbutton"));
    }
}

/// Shows the options of a drop-down menu whose text is one of `labels` in red, for actions
/// that delete something. Like `set_on_menu_items`, the options are found each time the
/// menu opens.
pub fn mark_destructive_menu_items(popover: Option<gtk::Popover>, labels: &'static [&'static str]) {
    if let Some(popover) = popover {
        popover.connect_map(move |popover| mark_destructive(popover.upcast_ref(), labels));
    }
}

fn mark_destructive(root: &gtk::Widget, labels: &[&str]) {
    if root.css_name() == "modelbutton" && has_label(root, labels) {
        root.add_css_class("destructive-menu-item");
        return;
    }
    let mut child = root.first_child();
    while let Some(current) = child {
        mark_destructive(&current, labels);
        child = current.next_sibling();
    }
}

/// Whether a label inside `root` shows one of `labels`.
fn has_label(root: &gtk::Widget, labels: &[&str]) -> bool {
    if let Some(label) = root.downcast_ref::<gtk::Label>() {
        return labels.contains(&label.label().as_str());
    }
    let mut child = root.first_child();
    while let Some(current) = child {
        if has_label(&current, labels) {
            return true;
        }
        child = current.next_sibling();
    }
    false
}

fn set_on_css_name(root: &gtk::Widget, name: &str) {
    if root.css_name() == name {
        root.set_cursor_from_name(Some("pointer"));
    }
    let mut child = root.first_child();
    while let Some(current) = child {
        set_on_css_name(&current, name);
        child = current.next_sibling();
    }
}

/// Shows a toast whose close button has a pointer cursor.
pub fn add_toast(overlay: &adw::ToastOverlay, message: &str) {
    let toast = adw::Toast::new(message);
    // A toast queued behind another gets its widget only when the earlier one goes away.
    let weak_overlay = overlay.downgrade();
    toast.connect_dismissed(move |_| {
        if let Some(overlay) = weak_overlay.upgrade() {
            gtk::glib::idle_add_local_once(move || set_on_toasts(&overlay));
        }
    });
    overlay.add_toast(toast);
    set_on_toasts(overlay);
}

/// Toast widgets are direct children of the overlay, next to its content.
fn set_on_toasts(overlay: &adw::ToastOverlay) {
    let mut child = overlay.first_child();
    while let Some(current) = child {
        if current.css_name() == "toast" {
            set_on_buttons(&current);
        }
        child = current.next_sibling();
    }
}
