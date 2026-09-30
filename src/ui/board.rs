//! Kanban board of one project's tickets.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib::{self, clone};
use gtk::{gdk, gio, graphene};

use super::deleted_tickets::DeletedTicketsPage;
use super::dialog::{self, Target};
use super::pointer;
use crate::db::Store;
use crate::model::{Project, Status, Ticket};

const COLUMN_WIDTH: i32 = 320;
const SEARCH_DEBOUNCE_MS: u32 = 250;

struct Column {
    status: Status,
    root: gtk::Box,
    scroller: gtk::ScrolledWindow,
    cards_box: gtk::Box,
    count: gtk::Label,
    add_button: gtk::Button,
    /// Card widgets in display order, paired with the ticket they show.
    entries: RefCell<Vec<(gtk::Widget, Ticket)>>,
}

impl Column {
    fn new(status: Status) -> Self {
        let title = gtk::Label::builder()
            .label(status.label())
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let count = gtk::Label::builder()
            .css_classes(["dim-label", "numeric", "caption"])
            .build();
        let header = gtk::Box::builder()
            .spacing(8)
            .css_classes(["column-header"])
            .build();
        header.append(&title);
        header.append(&count);

        // A text label rather than an icon, so the button shows regardless of icon theme.
        let add_button = gtk::Button::builder()
            .label("+  Add ticket")
            .tooltip_text(format!("Add a ticket to {}", status.label()))
            .css_classes(["add-ticket"])
            .margin_start(8)
            .margin_end(8)
            .margin_bottom(4)
            .build();
        add_button.set_cursor_from_name(Some("pointer"));

        let cards_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_start(8)
            .margin_end(8)
            .margin_top(6)
            .margin_bottom(12)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&cards_box)
            .build();

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(COLUMN_WIDTH)
            .hexpand(false)
            .css_classes(["board-column"])
            .build();
        root.append(&header);
        root.append(&add_button);
        root.append(&scroller);

        Self {
            status,
            root,
            scroller,
            cards_box,
            count,
            add_button,
            entries: RefCell::default(),
        }
    }
}

pub struct Board {
    store: Rc<RefCell<Store>>,
    /// The project whose tickets this board shows.
    project: Project,
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    page: adw::NavigationPage,
    /// The view this board's page is in, for pushing the deleted tickets page on top of it.
    navigation: adw::NavigationView,
    /// Kept while pushed, so its handlers stay alive.
    deleted_tickets: RefCell<Option<Rc<DeletedTicketsPage>>>,
    search: gtk::SearchEntry,
    columns: Vec<Column>,
    query: RefCell<String>,
    /// Id of the ticket currently being dragged, if any.
    dragging: Cell<Option<i64>>,
    /// Card currently showing the drop marker, and the marker's CSS class.
    indicator: RefCell<Option<(gtk::Widget, &'static str)>>,
}

impl Board {
    /// `window` and `toasts` belong to the main window, which the board's page is shown in.
    pub fn new(
        window: &adw::ApplicationWindow,
        toasts: &adw::ToastOverlay,
        navigation: &adw::NavigationView,
        section_menu: &gtk::MenuButton,
        store: Rc<RefCell<Store>>,
        project: Project,
    ) -> Rc<Self> {
        // The section menu's "Projects" entry leads back, so it takes the back button's place.
        let header = adw::HeaderBar::builder().show_back_button(false).build();
        header.pack_start(section_menu);

        // GtkSearchEntry emits `search-changed` only after `search-delay` ms without typing.
        let search = gtk::SearchEntry::builder()
            .placeholder_text("Search tickets by title or details…")
            .search_delay(SEARCH_DEBOUNCE_MS)
            .hexpand(true)
            .build();
        let search_bar = gtk::Box::builder()
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        search_bar.append(&search);

        let menu = gio::Menu::new();
        menu.append(Some("Deleted Tickets"), Some("board.deleted-tickets"));
        // A text label rather than an icon, so the button shows regardless of icon theme.
        let menu_button = gtk::MenuButton::builder()
            .child(&gtk::Label::new(Some("⋯")))
            .menu_model(&menu)
            .tooltip_text("More")
            .margin_start(6)
            .build();
        menu_button.set_cursor_from_name(Some("pointer"));
        pointer::set_on_menu_items(menu_button.popover());
        search_bar.append(&menu_button);

        let columns: Vec<Column> = Status::ALL.into_iter().map(Column::new).collect();
        let board_box = gtk::Box::builder()
            .spacing(12)
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .margin_bottom(12)
            .halign(gtk::Align::Start)
            .build();
        for column in &columns {
            board_box.append(&column.root);
        }
        let board_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .vexpand(true)
            .child(&board_box)
            .build();

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_top_bar(&search_bar);
        toolbar.set_content(Some(&board_scroller));
        // Not poppable by swipe gestures, Escape or the mouse's back button; the section
        // menu's "Projects" entry still pops it programmatically.
        let page = adw::NavigationPage::builder()
            .title(&project.name)
            .can_pop(false)
            .child(&toolbar)
            .build();

        let board = Rc::new(Self {
            store,
            project,
            window: window.clone(),
            toasts: toasts.clone(),
            page,
            navigation: navigation.clone(),
            deleted_tickets: RefCell::default(),
            search: search.clone(),
            columns,
            query: RefCell::default(),
            dragging: Cell::new(None),
            indicator: RefCell::new(None),
        });
        board.connect_signals();
        board.refresh();
        board
    }

    fn connect_signals(self: &Rc<Self>) {
        let search = &self.search;
        let board = Rc::clone(self);

        search.connect_search_changed(clone!(
            #[weak]
            board,
            move |entry| {
                *board.query.borrow_mut() = entry.text().trim().to_lowercase();
                board.apply_filter();
            }
        ));
        search.connect_stop_search(|entry| entry.set_text(""));

        let deleted_tickets = gio::SimpleAction::new("deleted-tickets", None);
        deleted_tickets.connect_activate(clone!(
            #[weak]
            board,
            move |_, _| board.show_deleted_tickets()
        ));
        let actions = gio::SimpleActionGroup::new();
        actions.add_action(&deleted_tickets);
        self.page.insert_action_group("board", Some(&actions));

        for (index, column) in self.columns.iter().enumerate() {
            let status = column.status;
            column.add_button.connect_clicked(clone!(
                #[weak]
                board,
                move |_| dialog::open(&board, Target::New(status))
            ));

            let drop = gtk::DropTarget::new(i64::static_type(), gdk::DragAction::MOVE);
            drop.connect_motion(clone!(
                #[weak]
                board,
                #[upgrade_or]
                gdk::DragAction::empty(),
                move |_, x, y| {
                    board.drag_motion(index, x, y);
                    gdk::DragAction::MOVE
                }
            ));
            drop.connect_leave(clone!(
                #[weak]
                board,
                move |_| board.set_indicator(None)
            ));
            drop.connect_drop(clone!(
                #[weak]
                board,
                #[upgrade_or]
                false,
                move |_, value, x, y| board.drop_ticket(index, value, x, y)
            ));
            column.root.add_controller(drop);
        }
    }

    pub fn window(&self) -> &adw::ApplicationWindow {
        &self.window
    }

    pub fn page(&self) -> &adw::NavigationPage {
        &self.page
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    fn show_deleted_tickets(self: &Rc<Self>) {
        let page = DeletedTicketsPage::new(self);
        self.navigation.push(page.page());
        *self.deleted_tickets.borrow_mut() = Some(page);
    }

    pub fn focus_search(&self) {
        self.search.grab_focus();
    }

    pub fn toast(&self, message: &str) {
        pointer::add_toast(&self.toasts, message);
    }

    /// Reloads every column from the database.
    fn refresh(self: &Rc<Self>) {
        let tickets = match self.store.borrow().list(self.project.id) {
            Ok(tickets) => tickets,
            Err(e) => {
                self.toast(&format!("Could not load tickets: {e}"));
                return;
            }
        };
        self.set_indicator(None);

        for column in &self.columns {
            while let Some(child) = column.cards_box.first_child() {
                column.cards_box.remove(&child);
            }
            let entries: Vec<(gtk::Widget, Ticket)> = tickets
                .iter()
                .filter(|t| t.status == column.status)
                .map(|t| {
                    let card = self.build_card(t);
                    column.cards_box.append(&card);
                    (card, t.clone())
                })
                .collect();
            *column.entries.borrow_mut() = entries;
        }
        self.apply_filter();
    }

    fn build_card(self: &Rc<Self>, ticket: &Ticket) -> gtk::Widget {
        let board = Rc::clone(self);
        let id = ticket.id;

        // A tiny max width lets the label wrap to the column instead of widening it.
        let title = gtk::Label::builder()
            .label(&ticket.title)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .max_width_chars(1)
            .xalign(0.0)
            .hexpand(true)
            .css_classes(["ticket-title"])
            .build();
        let card = gtk::Box::builder()
            .css_classes(["card", "activatable", "ticket"])
            .focusable(true)
            .build();
        card.set_cursor_from_name(Some("pointer"));
        card.update_property(&[gtk::accessible::Property::Label(&ticket.title)]);
        card.append(&title);

        let click = gtk::GestureClick::builder()
            .button(gdk::BUTTON_PRIMARY)
            .build();
        click.connect_released(clone!(
            #[weak]
            board,
            move |gesture, n_press, _, _| {
                if n_press == 1 {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    board.open_ticket(id);
                }
            }
        ));
        card.add_controller(click);

        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(clone!(
            #[weak]
            board,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, _| match key {
                gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space => {
                    board.open_ticket(id);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        ));
        card.add_controller(keys);

        let drag = gtk::DragSource::builder()
            .actions(gdk::DragAction::MOVE)
            .build();
        let hotspot = Rc::new(Cell::new((0, 0)));
        drag.connect_prepare(clone!(
            #[strong]
            hotspot,
            move |_, x, y| {
                hotspot.set((x as i32, y as i32));
                Some(gdk::ContentProvider::for_value(&id.to_value()))
            }
        ));
        drag.connect_drag_begin(clone!(
            #[weak]
            board,
            #[weak]
            card,
            #[strong]
            hotspot,
            move |source, _| {
                // Snapshot before dimming the card, so the drag icon stays opaque.
                let icon = gtk::WidgetPaintable::new(Some(&card)).current_image();
                let (x, y) = hotspot.get();
                source.set_icon(Some(&icon), x, y);
                card.add_css_class("dragging");
                board.dragging.set(Some(id));
            }
        ));
        drag.connect_drag_end(clone!(
            #[weak]
            board,
            #[weak]
            card,
            move |_, _, _| {
                card.remove_css_class("dragging");
                board.dragging.set(None);
                board.set_indicator(None);
            }
        ));
        card.add_controller(drag);

        card.upcast()
    }

    fn open_ticket(self: &Rc<Self>, id: i64) {
        let ticket = self.store.borrow().get(id);
        match ticket {
            Ok(ticket) => dialog::open(self, Target::Existing(ticket)),
            Err(e) => {
                self.toast(&e.to_string());
                self.refresh();
            }
        }
    }

    /// Case-insensitive substring match, like `title ILIKE '%q%' OR details ILIKE '%q%'`.
    /// `query` is already lowercased.
    fn matches(ticket: &Ticket, query: &str) -> bool {
        ticket.title.to_lowercase().contains(query) || ticket.details.to_lowercase().contains(query)
    }

    fn apply_filter(&self) {
        let query = self.query.borrow();
        for column in &self.columns {
            let entries = column.entries.borrow();
            let mut shown = 0;
            for (card, ticket) in entries.iter() {
                let visible = query.is_empty() || Self::matches(ticket, &query);
                card.set_visible(visible);
                shown += usize::from(visible);
            }
            let count = if query.is_empty() {
                entries.len().to_string()
            } else {
                format!("{shown}/{}", entries.len())
            };
            column.count.set_label(&count);
        }
    }

    /// Where a drop at (`x`, `y`) in the column lands: the insertion index among the column's
    /// tickets (excluding the dragged one), and the visible card to mark with `true` = above.
    fn drop_position(
        &self,
        column: &Column,
        x: f64,
        y: f64,
    ) -> (usize, Option<(gtk::Widget, bool)>) {
        let dragged = self.dragging.get();
        let entries = column.entries.borrow();
        let Some(point) = column
            .root
            .compute_point(&column.cards_box, &graphene::Point::new(x as f32, y as f32))
        else {
            return (entries.len(), None);
        };

        let mut index = 0;
        let mut last_visible = None;
        for (card, ticket) in entries.iter() {
            if Some(ticket.id) == dragged {
                continue;
            }
            if card.is_visible() {
                if let Some(bounds) = card.compute_bounds(&column.cards_box)
                    && point.y() < bounds.y() + bounds.height() / 2.0
                {
                    return (index, Some((card.clone(), true)));
                }
                last_visible = Some(card.clone());
            }
            index += 1;
        }
        (index, last_visible.map(|card| (card, false)))
    }

    fn drag_motion(&self, column_index: usize, x: f64, y: f64) {
        let (_, target) = self.drop_position(&self.columns[column_index], x, y);
        self.set_indicator(
            target.map(|(card, above)| (card, if above { "drop-above" } else { "drop-below" })),
        );
    }

    fn set_indicator(&self, indicator: Option<(gtk::Widget, &'static str)>) {
        let mut current = self.indicator.borrow_mut();
        if let Some((card, class)) = current.take() {
            card.remove_css_class(class);
        }
        if let Some((card, class)) = &indicator {
            card.add_css_class(class);
        }
        *current = indicator;
    }

    fn drop_ticket(
        self: &Rc<Self>,
        column_index: usize,
        value: &glib::Value,
        x: f64,
        y: f64,
    ) -> bool {
        let Ok(id) = value.get::<i64>() else {
            return false;
        };
        let column = &self.columns[column_index];
        let (index, _) = self.drop_position(column, x, y);
        self.set_indicator(None);

        let result = self
            .store
            .borrow_mut()
            .move_ticket(id, column.status, index);
        if let Err(e) = result {
            self.toast(&format!("Could not move ticket: {e}"));
            return false;
        }
        // Rebuild after the drop completes; the dragged card is still in use until then.
        let board = Rc::clone(self);
        glib::idle_add_local_once(move || board.refresh());
        true
    }

    pub fn create_ticket(
        self: &Rc<Self>,
        status: Status,
        title: &str,
        details: &str,
    ) -> Result<Ticket, String> {
        let ticket = self
            .store
            .borrow_mut()
            .create_ticket(self.project.id, status, title, details)
            .map_err(|e| e.to_string())?;
        self.refresh();
        if let Some(column) = self.columns.iter().find(|c| c.status == status) {
            column.scroller.vadjustment().set_value(0.0);
        }
        Ok(ticket)
    }

    pub fn update_ticket(
        self: &Rc<Self>,
        id: i64,
        title: &str,
        details: &str,
    ) -> Result<Ticket, String> {
        let ticket = self
            .store
            .borrow_mut()
            .update_ticket(id, title, details)
            .map_err(|e| e.to_string())?;
        self.refresh();
        Ok(ticket)
    }

    pub fn list_deleted_tickets(&self) -> Result<Vec<Ticket>, String> {
        self.store
            .borrow()
            .list_deleted_tickets(self.project.id)
            .map_err(|e| e.to_string())
    }

    pub fn restore_ticket(self: &Rc<Self>, id: i64) -> Result<Ticket, String> {
        let result = self
            .store
            .borrow_mut()
            .restore_ticket(id)
            .map_err(|e| e.to_string());
        self.refresh();
        result
    }

    /// Erases a deleted ticket. It isn't on the board, so there is nothing to refresh.
    pub fn purge_ticket(&self, id: i64) -> Result<(), String> {
        self.store
            .borrow_mut()
            .purge_ticket(id)
            .map_err(|e| e.to_string())
    }

    pub fn delete_ticket(self: &Rc<Self>, id: i64) -> Result<(), String> {
        let result = self
            .store
            .borrow_mut()
            .delete_ticket(id)
            .map_err(|e| e.to_string());
        self.refresh();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::Board;
    use crate::model::{Status, Ticket};

    #[test]
    fn search_is_case_insensitive_substring_of_title_or_details() {
        let now = chrono::Utc::now();
        let ticket = Ticket {
            id: 1,
            title: "Set up CI Pipeline".into(),
            details: "Run `clippy` on every push".into(),
            status: Status::Backlog,
            created_at: now,
            updated_at: now,
        };
        assert!(Board::matches(&ticket, "ci pipe"));
        assert!(Board::matches(&ticket, "clippy"));
        assert!(
            !Board::matches(&ticket, "sup ci"),
            "no fuzzy subsequence matches"
        );
        assert!(!Board::matches(&ticket, "deploy"));
    }
}
