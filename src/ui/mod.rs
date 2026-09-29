mod board;
mod code_spans;
mod deleted_projects;
mod deleted_tickets;
mod dialog;
mod markdown_view;
mod pointer;
mod project_dialog;
mod projects;

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gdk;
use gtk::glib::{self, clone};

use crate::db::Store;
use crate::model::Project;
use board::Board;
use projects::ProjectsPage;

/// Sidebar entries: the section's name in the content stack, and its label.
const SECTIONS: [(&str, &str); 2] = [("projects", "Projects"), ("notes", "Notes")];

pub fn build(app: &adw::Application, store: Store) {
    load_css();
    let shell = Shell::new(app, store);
    shell.window.present();
    // Don't start with a button focused, where a stray Enter would open a modal.
    gtk::prelude::GtkWindowExt::set_focus(&shell.window, None::<&gtk::Widget>);

    // Signal handlers only hold weak references, so the window owns the shell until it is
    // destroyed; otherwise the shell would be dropped here and every handler would no-op.
    let keep_alive = RefCell::new(Some(Rc::clone(&shell)));
    shell.window.connect_destroy(move |_| {
        keep_alive.take();
    });
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        // Dialogs open and close instantly. libadwaita has no per-dialog switch, only this
        // app-wide setting, so it also turns off toast and view-switch transitions.
        gtk::Settings::for_display(&display).set_gtk_enable_animations(false);
    }
}

/// The main window: a sidebar of sections, and the selected section's content.
struct Shell {
    window: adw::ApplicationWindow,
    store: Rc<RefCell<Store>>,
    toasts: adw::ToastOverlay,
    sidebar: gtk::ListBox,
    split: adw::OverlaySplitView,
    sections: gtk::Stack,
    /// The projects page, with a project's board pushed on top of it while one is open.
    navigation: adw::NavigationView,
    projects: Rc<ProjectsPage>,
    board: RefCell<Option<Rc<Board>>>,
}

impl Shell {
    fn new(app: &adw::Application, store: Store) -> Rc<Self> {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("orgx")
            .default_width(1100)
            .default_height(700)
            .width_request(600)
            .height_request(400)
            .build();
        let store = Rc::new(RefCell::new(store));
        let toasts = adw::ToastOverlay::new();

        let sidebar = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .css_classes(["navigation-sidebar"])
            .build();
        for (_, label) in SECTIONS {
            let row = gtk::ListBoxRow::builder()
                .child(&gtk::Label::builder().label(label).xalign(0.0).build())
                .build();
            row.set_cursor_from_name(Some("pointer"));
            sidebar.append(&row);
        }
        sidebar.select_row(sidebar.row_at_index(0).as_ref());
        let sidebar_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&sidebar)
            .build();
        let sidebar_page = adw::ToolbarView::new();
        sidebar_page.add_top_bar(
            &adw::HeaderBar::builder()
                .title_widget(&adw::WindowTitle::new("orgx", ""))
                .build(),
        );
        sidebar_page.set_content(Some(&sidebar_scroller));

        let navigation = adw::NavigationView::new();
        let sections = gtk::Stack::new();
        let split = adw::OverlaySplitView::builder()
            .sidebar(&sidebar_page)
            .content(&sections)
            .min_sidebar_width(180.0)
            .max_sidebar_width(220.0)
            .build();

        let notes_header = adw::HeaderBar::builder()
            .title_widget(&adw::WindowTitle::new("Notes", ""))
            .build();
        notes_header.pack_start(&sidebar_toggle(&split));
        let notes_page = adw::ToolbarView::new();
        notes_page.add_top_bar(&notes_header);

        sections.add_named(&navigation, Some(SECTIONS[0].0));
        sections.add_named(&notes_page, Some(SECTIONS[1].0));
        toasts.set_child(Some(&split));
        window.set_content(Some(&toasts));

        let shell = Rc::new_cyclic(|weak: &std::rc::Weak<Self>| {
            let weak = weak.clone();
            let projects = ProjectsPage::new(
                &window,
                &toasts,
                &navigation,
                &sidebar_toggle(&split),
                Rc::clone(&store),
                move |project| {
                    if let Some(shell) = weak.upgrade() {
                        shell.open_board(project);
                    }
                },
            );
            Self {
                window,
                store,
                toasts,
                sidebar,
                split,
                sections,
                navigation,
                projects,
                board: RefCell::default(),
            }
        });
        shell.connect_signals();
        shell
    }

    fn connect_signals(self: &Rc<Self>) {
        let shell = Rc::clone(self);

        self.sidebar.connect_row_activated(clone!(
            #[weak]
            shell,
            move |_, row| {
                let Some((name, _)) = usize::try_from(row.index())
                    .ok()
                    .and_then(|i| SECTIONS.get(i))
                else {
                    return;
                };
                shell.sections.set_visible_child_name(name);
                // Clicking "Projects" from a board goes back to the project cards.
                if *name == SECTIONS[0].0 {
                    shell.navigation.pop_to_tag(projects::PAGE_TAG);
                }
            }
        ));

        self.navigation.connect_popped(clone!(
            #[weak]
            shell,
            move |_, page| {
                let popped = shell
                    .board
                    .borrow()
                    .as_ref()
                    .is_some_and(|board| board.page() == page);
                if popped {
                    shell.board.take();
                }
            }
        ));

        let focus_search = gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>f"),
            Some(gtk::CallbackAction::new(clone!(
                #[weak]
                shell,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, _| {
                    if shell.window.visible_dialog().is_some() {
                        return glib::Propagation::Proceed;
                    }
                    match shell.visible_board() {
                        Some(board) => {
                            board.focus_search();
                            glib::Propagation::Stop
                        }
                        None => glib::Propagation::Proceed,
                    }
                }
            ))),
        );
        let shortcuts = gtk::ShortcutController::new();
        shortcuts.add_shortcut(focus_search);
        self.window.add_controller(shortcuts);
    }

    /// Shows the board of `project` on top of the project cards.
    fn open_board(&self, project: Project) {
        self.navigation.pop_to_page(self.projects.page());
        let board = Board::new(
            &self.window,
            &self.toasts,
            &self.navigation,
            &sidebar_toggle(&self.split),
            Rc::clone(&self.store),
            project,
        );
        self.navigation.push(board.page());
        *self.board.borrow_mut() = Some(board);
    }

    /// The open board, if it is what the window currently shows.
    fn visible_board(&self) -> Option<Rc<Board>> {
        let board = self.board.borrow().clone()?;
        let shown = self.sections.visible_child_name().as_deref() == Some(SECTIONS[0].0)
            && self.navigation.visible_page().as_ref() == Some(board.page());
        shown.then_some(board)
    }
}

/// A header bar button that shows and hides the sidebar. Every content page gets its own,
/// all kept in sync with the split view.
fn sidebar_toggle(split: &adw::OverlaySplitView) -> gtk::ToggleButton {
    let toggle = gtk::ToggleButton::builder()
        .icon_name("sidebar-show-symbolic")
        .tooltip_text("Toggle Sidebar")
        .build();
    toggle.set_cursor_from_name(Some("pointer"));
    split
        .bind_property("show-sidebar", &toggle, "active")
        .bidirectional()
        .sync_create()
        .build();
    toggle
}
