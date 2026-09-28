//! Modal for viewing, editing and creating tickets.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use chrono::{DateTime, Local, Utc};
use gtk::gdk;
use gtk::glib::{self, clone};

use super::{Board, markdown_view, pointer};
use crate::markdown;
use crate::model::{Status, Ticket};

pub enum Target {
    /// Create a ticket at the top of this column.
    New(Status),
    Existing(Ticket),
}

struct TicketDialog {
    /// For callbacks created after construction, such as preview checkboxes.
    this: Weak<TicketDialog>,
    board: Weak<Board>,
    dialog: adw::Dialog,
    header: adw::HeaderBar,
    edit_button: gtk::Button,
    delete_button: gtk::Button,
    cancel_button: gtk::Button,
    save_button: gtk::Button,
    stack: gtk::Stack,
    toasts: adw::ToastOverlay,
    title_label: gtk::Label,
    meta_label: gtk::Label,
    /// Holds the rendered markdown, rebuilt each time the preview is shown.
    details: gtk::Box,
    title_row: adw::EntryRow,
    details_buffer: gtk::TextBuffer,
    /// Column a new ticket is created in.
    status: Status,
    /// `None` until a new ticket has been saved.
    ticket: RefCell<Option<Ticket>>,
}

pub fn open(board: &Rc<Board>, target: Target) {
    let (status, ticket) = match target {
        Target::New(status) => (status, None),
        Target::Existing(ticket) => (ticket.status, Some(ticket)),
    };
    let this =
        Rc::new_cyclic(|weak| TicketDialog::new(weak.clone(), board, status, ticket.clone()));
    this.connect_signals();
    match &ticket {
        Some(ticket) => this.show_preview(ticket),
        None => this.show_editor(),
    }
    this.dialog.present(Some(board.window()));
    if ticket.is_none() {
        // Focus only takes once the dialog is in the window.
        this.title_row.grab_focus();
    }
}

impl TicketDialog {
    fn new(this: Weak<Self>, board: &Rc<Board>, status: Status, ticket: Option<Ticket>) -> Self {
        let edit_button = gtk::Button::builder()
            .label("Edit")
            .tooltip_text("Edit (Ctrl+E)")
            .build();
        let delete_button = gtk::Button::builder()
            .label("Delete")
            .tooltip_text("Delete ticket")
            .css_classes(["destructive-action"])
            .build();
        let cancel_button = gtk::Button::with_label("Cancel");
        let save_button = gtk::Button::builder()
            .label("Save")
            .tooltip_text("Save (Ctrl+S)")
            .css_classes(["suggested-action"])
            .build();
        let header = adw::HeaderBar::new();
        header.pack_start(&cancel_button);
        header.pack_start(&delete_button);
        header.pack_end(&edit_button);
        header.pack_end(&save_button);
        for button in [&edit_button, &delete_button, &cancel_button, &save_button] {
            button.set_cursor_from_name(Some("pointer"));
        }

        // Preview
        let title_label = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .xalign(0.0)
            .selectable(true)
            .css_classes(["title-2"])
            .build();
        let meta_label = gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .css_classes(["dim-label", "caption"])
            .build();
        let details = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["ticket-preview"])
            .build();
        let preview_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_start(24)
            .margin_end(24)
            .margin_top(12)
            .margin_bottom(24)
            .build();
        preview_box.append(&title_label);
        preview_box.append(&meta_label);
        preview_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        preview_box.append(&details);
        let preview = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&preview_box)
            .build();

        // Editor
        let title_row = adw::EntryRow::builder().title("Title").build();
        let title_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .margin_bottom(6)
            .build();
        title_list.append(&title_row);
        if let Some(text) = title_row
            .delegate()
            .and_then(|d| d.downcast::<gtk::Text>().ok())
        {
            trim_pasted_text_in_entry(&text);
        }
        let details_heading = gtk::Label::builder()
            .label("Details (Markdown)")
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let details_view = gtk::TextView::builder()
            .monospace(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(12)
            .bottom_margin(12)
            .left_margin(12)
            .right_margin(12)
            // Extra space between lines, and between wrapped rows of one line.
            .pixels_below_lines(4)
            .pixels_inside_wrap(2)
            .accepts_tab(true)
            .css_classes(["ticket-editor"])
            .build();
        let details_buffer = details_view.buffer();
        trim_pasted_text_in_view(&details_view);
        indent_with_spaces(&details_view);
        // Tabs can still arrive by pasting. Tab stops need the editor's font, which is only
        // known once it's realized.
        details_view.connect_realize(|view| set_tab_width(view, TAB_WIDTH_CHARS));
        let details_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&details_view)
            .build();
        // Styled like the boxed title row above it.
        details_scroller.add_css_class("card");
        details_scroller.add_css_class("details-card");
        let editor = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_start(18)
            .margin_end(18)
            .margin_top(12)
            .margin_bottom(18)
            .build();
        editor.append(&title_list);
        editor.append(&details_heading);
        editor.append(&details_scroller);

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&preview, Some("preview"));
        stack.add_named(&editor, Some("edit"));

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&stack));
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&toasts));

        let dialog = adw::Dialog::builder()
            .content_width(760)
            .content_height(640)
            .child(&toolbar)
            .build();

        Self {
            this,
            board: Rc::downgrade(board),
            dialog,
            header,
            edit_button,
            delete_button,
            cancel_button,
            save_button,
            stack,
            toasts,
            title_label,
            meta_label,
            details,
            title_row,
            details_buffer,
            status,
            ticket: RefCell::new(ticket),
        }
    }

    fn connect_signals(self: &Rc<Self>) {
        let this = Rc::clone(self);

        self.edit_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| this.show_editor()
        ));
        self.save_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| this.save()
        ));
        self.delete_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| this.confirm_delete()
        ));
        self.cancel_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| this.cancel()
        ));
        self.title_row.connect_changed(clone!(
            #[weak]
            this,
            move |_| this.update_editor_state()
        ));
        self.title_row.connect_entry_activated(clone!(
            #[weak]
            this,
            move |_| this.save()
        ));
        self.details_buffer.connect_changed(clone!(
            #[weak]
            this,
            move |_| this.update_editor_state()
        ));
        // libadwaita builds the close button's container lazily, so set its cursor once shown.
        self.dialog
            .connect_map(|dialog| set_pointer_on_window_controls(dialog.upcast_ref()));
        self.dialog.connect_close_attempt(clone!(
            #[weak]
            this,
            move |dialog| {
                let dialog = dialog.clone();
                this.confirm_discard(move || dialog.force_close());
            }
        ));

        let shortcuts = gtk::ShortcutController::new();
        shortcuts.set_propagation_phase(gtk::PropagationPhase::Capture);
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>s|<Control>Return"),
            Some(gtk::CallbackAction::new(clone!(
                #[weak]
                this,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, _| {
                    this.save();
                    glib::Propagation::Stop
                }
            ))),
        ));
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>e"),
            Some(gtk::CallbackAction::new(clone!(
                #[weak]
                this,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, _| {
                    if this.is_editing() {
                        return glib::Propagation::Proceed;
                    }
                    this.show_editor();
                    glib::Propagation::Stop
                }
            ))),
        ));
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("Escape"),
            Some(gtk::CallbackAction::new(clone!(
                #[weak]
                this,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, _| this.escape()
            ))),
        ));
        self.dialog.add_controller(shortcuts);

        // Every other handler holds a weak reference; this one keeps the dialog state alive
        // until the dialog closes.
        let keep_alive = RefCell::new(Some(this));
        self.dialog.connect_closed(move |_| {
            keep_alive.take();
        });
    }

    fn is_editing(&self) -> bool {
        self.stack.visible_child_name().as_deref() == Some("edit")
    }

    fn show_preview(&self, ticket: &Ticket) {
        self.dialog
            .set_title(&format!("#{} · {}", ticket.id, ticket.status.label()));
        self.title_label.set_label(&ticket.title);
        self.meta_label.set_label(&meta_text(ticket));
        while let Some(child) = self.details.first_child() {
            self.details.remove(&child);
        }
        if ticket.details.trim().is_empty() {
            let empty = gtk::Label::builder()
                .label("No details.")
                .xalign(0.0)
                .css_classes(["dim-label"])
                .build();
            self.details.append(&empty);
        } else {
            let this = self.this.clone();
            let on_task_toggled = Rc::new(move |marker: &Range<usize>, checked: bool| {
                if let Some(this) = this.upgrade() {
                    this.set_task(marker, checked);
                }
            });
            self.details
                .append(&markdown_view::build(&ticket.details, on_task_toggled));
        }

        self.stack.set_visible_child_name("preview");
        self.header.set_show_end_title_buttons(true);
        set_pointer_on_window_controls(self.header.upcast_ref());
        self.edit_button.set_visible(true);
        self.delete_button.set_visible(true);
        self.cancel_button.set_visible(false);
        self.save_button.set_visible(false);
        // Move focus off the editor fields being hidden, but keep it inside the dialog so
        // keys like Escape still reach it.
        self.edit_button.grab_focus();
        self.dialog.set_can_close(true);
    }

    /// Saves a checkbox clicked in the preview by rewriting its marker in the details.
    fn set_task(&self, marker: &Range<usize>, checked: bool) {
        let (Some(board), Some(ticket)) = (self.board.upgrade(), self.ticket.borrow().clone())
        else {
            return;
        };
        let details = markdown::set_task(&ticket.details, marker, checked);
        match board.update_ticket(ticket.id, &ticket.title, &details) {
            Ok(updated) => {
                self.meta_label.set_label(&meta_text(&updated));
                *self.ticket.borrow_mut() = Some(updated);
            }
            Err(e) => {
                pointer::add_toast(&self.toasts, &format!("Could not update task: {e}"));
                // Re-render from the stored ticket so the checkbox shows the saved state.
                let this = self.this.clone();
                glib::idle_add_local_once(move || {
                    if let Some(this) = this.upgrade() {
                        this.show_preview(&ticket);
                    }
                });
            }
        }
    }

    fn show_editor(&self) {
        let (title, details) = self.original_content();
        self.dialog.set_title(&match &*self.ticket.borrow() {
            Some(ticket) => format!("Edit #{}", ticket.id),
            None => format!("New {} Ticket", self.status.label()),
        });

        self.stack.set_visible_child_name("edit");
        self.title_row.set_text(&title);
        self.details_buffer.set_text(&details);
        self.header.set_show_end_title_buttons(false);
        self.edit_button.set_visible(false);
        self.delete_button.set_visible(false);
        self.cancel_button.set_visible(true);
        self.save_button.set_visible(true);
        self.update_editor_state();
        self.title_row.grab_focus();
    }

    fn original_content(&self) -> (String, String) {
        self.ticket
            .borrow()
            .as_ref()
            .map(|t| (t.title.clone(), t.details.clone()))
            .unwrap_or_default()
    }

    fn edited_content(&self) -> (String, String) {
        let buffer = &self.details_buffer;
        let details = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
        (self.title_row.text().to_string(), details.to_string())
    }

    fn is_dirty(&self) -> bool {
        self.is_editing() && self.edited_content() != self.original_content()
    }

    fn update_editor_state(&self) {
        if !self.is_editing() {
            return;
        }
        let (title, _) = self.edited_content();
        let dirty = self.is_dirty();
        self.save_button
            .set_sensitive(!title.trim().is_empty() && dirty);
        // Closing with unsaved edits goes through `close-attempt` for confirmation.
        self.dialog.set_can_close(!dirty);
    }

    fn save(&self) {
        if !self.is_editing() || !self.save_button.is_sensitive() {
            return;
        }
        let Some(board) = self.board.upgrade() else {
            return;
        };
        let (title, details) = self.edited_content();
        let title = title.trim();
        let existing = self.ticket.borrow().as_ref().map(|t| t.id);

        let result = match existing {
            Some(id) => board.update_ticket(id, title, &details),
            None => board.create_ticket(self.status, title, &details),
        };
        match result {
            Ok(ticket) if existing.is_some() => {
                self.show_preview(&ticket);
                *self.ticket.borrow_mut() = Some(ticket);
            }
            Ok(_) => {
                self.dialog.set_can_close(true);
                self.dialog.close();
            }
            Err(e) => pointer::add_toast(&self.toasts, &format!("Could not save: {e}")),
        }
    }

    /// Escape while editing a saved ticket returns to its preview instead of closing the
    /// dialog. Anywhere else it falls through to the default close.
    fn escape(&self) -> glib::Propagation {
        if !self.is_editing() {
            return glib::Propagation::Proceed;
        }
        let Some(ticket) = self.ticket.borrow().clone() else {
            return glib::Propagation::Proceed;
        };
        if self.is_dirty() {
            let this = self.this.clone();
            self.confirm_discard(move || {
                // After the alert closes and hands focus back to the editor, so that
                // `show_preview` moves it last.
                let (this, ticket) = (this.clone(), ticket.clone());
                glib::idle_add_local_once(move || {
                    if let Some(this) = this.upgrade() {
                        this.show_preview(&ticket);
                    }
                });
            });
        } else {
            self.show_preview(&ticket);
        }
        glib::Propagation::Stop
    }

    fn cancel(&self) {
        let ticket = self.ticket.borrow().clone();
        match ticket {
            Some(ticket) => self.show_preview(&ticket),
            None => {
                self.dialog.close();
            }
        }
    }

    fn confirm_delete(self: &Rc<Self>) {
        let Some(ticket) = self.ticket.borrow().clone() else {
            return;
        };
        let alert = adw::AlertDialog::new(
            Some("Delete ticket?"),
            Some(&format!(
                "“{}” will be permanently deleted. This cannot be undone.",
                ticket.title
            )),
        );
        alert.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
        alert.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        alert.set_default_response(Some("cancel"));
        alert.set_close_response("cancel");
        alert.connect_map(|alert| pointer::set_on_buttons(alert.upcast_ref()));
        let this = Rc::clone(self);
        // Check the id rather than connecting to the "delete" detail only, so no other
        // response can ever trigger the deletion.
        alert.connect_response(
            None,
            clone!(
                #[weak]
                this,
                move |_, response| {
                    if response == "delete" {
                        this.delete(ticket.id);
                    }
                }
            ),
        );
        alert.present(Some(&self.dialog));
    }

    fn delete(&self, id: i64) {
        let Some(board) = self.board.upgrade() else {
            return;
        };
        match board.delete_ticket(id) {
            Ok(()) => {
                self.dialog.force_close();
                board.toast("Ticket deleted");
            }
            Err(e) => pointer::add_toast(&self.toasts, &format!("Could not delete: {e}")),
        }
    }

    fn confirm_discard(&self, on_discard: impl Fn() + 'static) {
        let alert = adw::AlertDialog::new(
            Some("Discard changes?"),
            Some("Your edits to this ticket have not been saved."),
        );
        alert.add_responses(&[("keep", "Keep Editing"), ("discard", "Discard")]);
        alert.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        alert.set_default_response(Some("keep"));
        alert.set_close_response("keep");
        alert.connect_map(|alert| pointer::set_on_buttons(alert.upcast_ref()));
        alert.connect_response(None, move |_, response| {
            if response == "discard" {
                on_discard();
            }
        });
        alert.present(Some(&self.dialog));
    }
}

/// Replaces the default paste in a single-line entry with one that trims the pasted text.
fn trim_pasted_text_in_entry(text: &gtk::Text) {
    text.connect_paste_clipboard(|text| {
        text.stop_signal_emission_by_name("paste-clipboard");
        let clipboard = text.clipboard();
        glib::spawn_future_local(clone!(
            #[weak]
            text,
            async move {
                let Ok(Some(pasted)) = clipboard.read_text_future().await else {
                    return;
                };
                if !text.is_editable() {
                    return;
                }
                text.delete_selection();
                let mut position = text.position();
                text.insert_text(pasted.trim(), &mut position);
                text.set_position(position);
            }
        ));
    });
}

/// Replaces the default paste in a text view with one that trims the pasted text.
fn trim_pasted_text_in_view(view: &gtk::TextView) {
    view.connect_paste_clipboard(|view| {
        view.stop_signal_emission_by_name("paste-clipboard");
        let clipboard = view.clipboard();
        glib::spawn_future_local(clone!(
            #[weak]
            view,
            async move {
                let Ok(Some(pasted)) = clipboard.read_text_future().await else {
                    return;
                };
                let buffer = view.buffer();
                let editable = view.is_editable();
                // One undo step for replacing the selection with the pasted text.
                buffer.begin_user_action();
                buffer.delete_selection(true, editable);
                buffer.insert_interactive_at_cursor(pasted.trim(), editable);
                buffer.end_user_action();
                view.scroll_mark_onscreen(&buffer.get_insert());
            }
        ));
    });
}

/// Gives the header bar's built-in close button a pointer cursor. The button is created
/// inside libadwaita, so it is found through its `windowcontrols` container.
fn set_pointer_on_window_controls(widget: &gtk::Widget) {
    if widget.css_name() == "windowcontrols" {
        widget.set_cursor_from_name(Some("pointer"));
        return;
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        set_pointer_on_window_controls(&current);
        child = current.next_sibling();
    }
}

/// Makes Tab insert two spaces (replacing any selection) instead of a tab character.
fn indent_with_spaces(view: &gtk::TextView) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(clone!(
        #[weak]
        view,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, modifiers| {
            let chorded = modifiers.intersects(
                gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::ALT_MASK
                    | gdk::ModifierType::SHIFT_MASK
                    | gdk::ModifierType::SUPER_MASK,
            );
            if key != gdk::Key::Tab || chorded || !view.is_editable() {
                return glib::Propagation::Proceed;
            }
            let buffer = view.buffer();
            buffer.begin_user_action();
            buffer.delete_selection(true, true);
            buffer.insert_interactive_at_cursor(INDENT, true);
            buffer.end_user_action();
            view.scroll_mark_onscreen(&buffer.get_insert());
            glib::Propagation::Stop
        }
    ));
    view.add_controller(keys);
}

/// What Tab inserts in the details editor.
const INDENT: &str = "  ";

/// Width of a tab in the details editor, in characters.
const TAB_WIDTH_CHARS: usize = 2;

fn set_tab_width(view: &gtk::TextView, chars: usize) {
    let (width, _) = view
        .create_pango_layout(Some(&" ".repeat(chars)))
        .pixel_size();
    let mut tabs = gtk::pango::TabArray::new(1, true);
    tabs.set_tab(0, gtk::pango::TabAlign::Left, width);
    view.set_tabs(&tabs);
}

fn meta_text(ticket: &Ticket) -> String {
    format!(
        "{} · Created {} · Updated {}",
        ticket.status.label(),
        format_time(ticket.created_at),
        format_time(ticket.updated_at),
    )
}

fn format_time(time: DateTime<Utc>) -> String {
    time.with_timezone(&Local)
        .format("%b %-d, %Y %H:%M")
        .to_string()
}
