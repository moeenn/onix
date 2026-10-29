//! Page with a table of a board's archived tickets, each with a menu for restoring it to the
//! board or deleting it.

use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::gio;
use gtk::glib::{self, BoxedAnyObject, clone};

use super::board::Board;
use super::dialog::format_time;
use super::pointer;
use crate::model::Ticket;

pub struct ArchivedTicketsPage {
    board: Weak<Board>,
    page: adw::NavigationPage,
    /// Shows either the "table" or the "empty" status page.
    content: gtk::Stack,
    /// `Ticket`s wrapped in `BoxedAnyObject`.
    tickets: gio::ListStore,
}

impl ArchivedTicketsPage {
    /// The page for `board`'s project. It loads the tickets each time it is shown.
    pub fn new(board: &Rc<Board>) -> Rc<Self> {
        let this = Rc::new_cyclic(|weak| Self::build(weak.clone(), board));
        this.page.connect_showing(clone!(
            #[weak]
            this,
            move |_| this.reload()
        ));
        this
    }

    fn build(this: Weak<Self>, board: &Rc<Board>) -> Self {
        let tickets = gio::ListStore::new::<BoxedAnyObject>();
        let table = gtk::ColumnView::builder()
            .model(&gtk::NoSelection::new(Some(tickets.clone())))
            .show_row_separators(true)
            .reorderable(false)
            .css_classes(["data-table"])
            .build();
        table.append_column(&label_column("ID", false, |t| format!("#{}", t.id)));
        table.append_column(&label_column("Title", true, |t| t.title.clone()));
        table.append_column(&label_column("Completed", false, |t| {
            t.completed_at.map(format_time).unwrap_or_default()
        }));
        table.append_column(&actions_column(this));

        // The table always takes its full height and the page scrolls instead. Scrolling the
        // table itself would size it to at least the scrollbar's minimum height, leaving a gap
        // under the rows when there are only one or two.
        let table_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .child(&table)
            .build();
        let frame = gtk::Frame::builder()
            .child(&table_scroller)
            .valign(gtk::Align::Start)
            .build();
        let clamp = adw::Clamp::builder()
            .maximum_size(900)
            .margin_start(18)
            .margin_end(18)
            .margin_top(18)
            .margin_bottom(18)
            .child(&frame)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();
        let empty = adw::StatusPage::builder()
            .title("No Archived Tickets")
            .description("Completed tickets move here a week after they were completed.")
            .build();
        let content = gtk::Stack::new();
        content.add_named(&scroller, Some("table"));
        content.add_named(&empty, Some("empty"));

        let header = adw::HeaderBar::builder()
            .title_widget(&adw::WindowTitle::new(
                "Archived Tickets",
                &board.project().name,
            ))
            .build();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&content));
        let page = adw::NavigationPage::builder()
            .title("Archived Tickets")
            .child(&toolbar)
            .build();
        // The back button is built by libadwaita.
        page.connect_shown(|page| pointer::set_on_buttons(page.upcast_ref()));

        let shortcuts = gtk::ShortcutController::new();
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Alt>Left"),
            Some(gtk::NamedAction::new("navigation.pop")),
        ));
        page.add_controller(shortcuts);

        Self {
            board: Rc::downgrade(board),
            page,
            content,
            tickets,
        }
    }

    pub fn page(&self) -> &adw::NavigationPage {
        &self.page
    }

    fn reload(&self) {
        let Some(board) = self.board.upgrade() else {
            return;
        };
        match board.list_archived_tickets() {
            Ok(tickets) => {
                let items: Vec<BoxedAnyObject> =
                    tickets.into_iter().map(BoxedAnyObject::new).collect();
                self.tickets.splice(0, self.tickets.n_items(), &items);
            }
            Err(e) => board.toast(&format!("Could not load tickets: {e}")),
        }
        self.update_empty();
    }

    fn update_empty(&self) {
        self.content
            .set_visible_child_name(if self.tickets.n_items() == 0 {
                "empty"
            } else {
                "table"
            });
    }

    fn restore(&self, id: i64) {
        let Some(board) = self.board.upgrade() else {
            return;
        };
        match board.unarchive_ticket(id) {
            Ok(ticket) => {
                self.remove_row(id);
                board.toast(&format!("Restored to {}", ticket.status.label()));
            }
            Err(e) => {
                board.toast(&format!("Could not restore: {e}"));
                self.reload();
            }
        }
    }

    /// Soft-deletes the ticket, so it can still be restored from the deleted tickets.
    fn delete(&self, id: i64) {
        let Some(board) = self.board.upgrade() else {
            return;
        };
        match board.delete_ticket(id) {
            Ok(()) => {
                self.remove_row(id);
                board.toast("Ticket deleted");
            }
            Err(e) => {
                board.toast(&format!("Could not delete ticket: {e}"));
                self.reload();
            }
        }
    }

    fn remove_row(&self, id: i64) {
        let position = (0..self.tickets.n_items()).find(|&i| {
            self.tickets
                .item(i)
                .and_downcast::<BoxedAnyObject>()
                .is_some_and(|item| item.borrow::<Ticket>().id == id)
        });
        if let Some(position) = position {
            self.tickets.remove(position);
        }
        self.update_empty();
    }
}

fn ticket_of(item: &gtk::ListItem) -> Option<Ticket> {
    let item = item.item()?.downcast::<BoxedAnyObject>().ok()?;
    let ticket = item.borrow::<Ticket>().clone();
    Some(ticket)
}

/// A column showing one line of text per ticket.
fn label_column(
    title: &str,
    expand: bool,
    text: impl Fn(&Ticket) -> String + 'static,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        item.set_child(Some(&label));
    });
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        if let (Some(ticket), Some(label)) =
            (ticket_of(item), item.child().and_downcast::<gtk::Label>())
        {
            label.set_label(&text(&ticket));
            label.set_tooltip_text(expand.then_some(ticket.title.as_str()));
        }
    });
    gtk::ColumnViewColumn::builder()
        .title(title)
        .factory(&factory)
        .expand(expand)
        .build()
}

/// A column with a menu per ticket, for restoring it to the board or deleting it.
fn actions_column(page: Weak<ArchivedTicketsPage>) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let menu = gio::Menu::new();
        menu.append(Some("Restore"), Some("ticket.restore"));
        menu.append(Some("Delete"), Some("ticket.delete"));
        // A text label rather than an icon, so the button shows regardless of icon theme.
        let button = gtk::MenuButton::builder()
            .child(&gtk::Label::new(Some("⋯")))
            .menu_model(&menu)
            .tooltip_text("Ticket options")
            .halign(gtk::Align::End)
            .css_classes(["flat"])
            .build();
        button.set_cursor_from_name(Some("pointer"));
        pointer::set_on_menu_items(button.popover());
        pointer::mark_destructive_menu_items(button.popover(), &["Delete"]);

        // Rows are recycled, so look up the ticket the row shows when an option is chosen.
        let restore = gio::SimpleAction::new("restore", None);
        restore.connect_activate(clone!(
            #[weak]
            item,
            #[strong]
            page,
            move |_, _| {
                let (Some(page), Some(ticket)) = (page.upgrade(), ticket_of(&item)) else {
                    return;
                };
                // Removing the row destroys this menu; let it finish closing first.
                glib::idle_add_local_once(move || page.restore(ticket.id));
            }
        ));
        let delete = gio::SimpleAction::new("delete", None);
        delete.connect_activate(clone!(
            #[weak]
            item,
            #[strong]
            page,
            move |_, _| {
                let (Some(page), Some(ticket)) = (page.upgrade(), ticket_of(&item)) else {
                    return;
                };
                glib::idle_add_local_once(move || page.delete(ticket.id));
            }
        ));
        let actions = gio::SimpleActionGroup::new();
        actions.add_action(&restore);
        actions.add_action(&delete);
        button.insert_action_group("ticket", Some(&actions));
        item.set_child(Some(&button));
    });
    gtk::ColumnViewColumn::builder().factory(&factory).build()
}
