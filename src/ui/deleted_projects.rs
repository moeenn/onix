//! List of deleted projects, each with a menu for restoring or permanently deleting it.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib::{self, clone};

use super::dialog::format_time;
use super::pointer;
use crate::db::Store;
use crate::model::Project;

/// Tag of the page in the navigation view, for the `navigation.push` action.
pub const PAGE_TAG: &str = "deleted-projects";

pub struct DeletedProjectsPage {
    store: Rc<RefCell<Store>>,
    toasts: adw::ToastOverlay,
    page: adw::NavigationPage,
    /// Shows either the "list" or the "empty" status page.
    content: gtk::Stack,
    list: gtk::ListBox,
}

impl DeletedProjectsPage {
    /// `toasts` belongs to the main window, which the page is shown in.
    pub fn new(toasts: &adw::ToastOverlay, store: Rc<RefCell<Store>>) -> Rc<Self> {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .valign(gtk::Align::Start)
            .css_classes(["boxed-list"])
            .build();
        let clamp = adw::Clamp::builder()
            .maximum_size(720)
            .margin_start(18)
            .margin_end(18)
            .margin_top(18)
            .margin_bottom(18)
            .child(&list)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();
        let empty = adw::StatusPage::builder()
            .title("No Deleted Projects")
            .description("Projects you delete can be restored from here.")
            .build();

        let content = gtk::Stack::new();
        content.add_named(&scroller, Some("list"));
        content.add_named(&empty, Some("empty"));

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&content));
        let page = adw::NavigationPage::builder()
            .title("Deleted Projects")
            .tag(PAGE_TAG)
            .child(&toolbar)
            .build();
        // The back button is built by libadwaita.
        page.connect_shown(|page| pointer::set_on_buttons(page.upcast_ref()));

        let this = Rc::new(Self {
            store,
            toasts: toasts.clone(),
            page,
            content,
            list,
        });
        this.page.connect_showing(clone!(
            #[weak]
            this,
            move |_| this.refresh()
        ));
        this
    }

    pub fn page(&self) -> &adw::NavigationPage {
        &self.page
    }

    fn refresh(self: &Rc<Self>) {
        let projects = match self.store.borrow().list_deleted_projects() {
            Ok(projects) => projects,
            Err(e) => {
                pointer::add_toast(&self.toasts, &format!("Could not load projects: {e}"));
                return;
            }
        };
        self.list.remove_all();
        for (project, deleted_at) in &projects {
            let row = adw::ActionRow::builder()
                .title(&project.name)
                .subtitle(format!("Deleted {}", format_time(*deleted_at)))
                .use_markup(false)
                .build();
            row.add_suffix(&self.build_menu(project));
            self.list.append(&row);
        }
        self.content
            .set_visible_child_name(if projects.is_empty() { "empty" } else { "list" });
    }

    fn build_menu(self: &Rc<Self>, project: &Project) -> gtk::MenuButton {
        let this = Rc::clone(self);
        let menu = gio::Menu::new();
        menu.append(Some("Restore"), Some("deleted.restore"));
        menu.append(Some("Delete Permanently"), Some("deleted.purge"));
        // A text label rather than an icon, so the button shows regardless of icon theme.
        let button = gtk::MenuButton::builder()
            .child(&gtk::Label::new(Some("⋯")))
            .menu_model(&menu)
            .tooltip_text("Project options")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        button.set_cursor_from_name(Some("pointer"));
        pointer::set_on_menu_items(button.popover());

        let restore = gio::SimpleAction::new("restore", None);
        restore.connect_activate(clone!(
            #[weak]
            this,
            #[strong]
            project,
            move |_, _| {
                // The list is rebuilt afterwards; let the menu finish closing first.
                let (this, project) = (Rc::clone(&this), project.clone());
                glib::idle_add_local_once(move || this.restore(&project));
            }
        ));
        let purge = gio::SimpleAction::new("purge", None);
        purge.connect_activate(clone!(
            #[weak]
            this,
            #[strong]
            project,
            move |_, _| this.confirm_purge(&project)
        ));
        let actions = gio::SimpleActionGroup::new();
        actions.add_action(&restore);
        actions.add_action(&purge);
        button.insert_action_group("deleted", Some(&actions));
        button
    }

    fn restore(self: &Rc<Self>, project: &Project) {
        let result = self.store.borrow_mut().restore_project(project.id);
        let message = match result {
            Ok(project) => format!("“{}” restored", project.name),
            Err(e) => format!("Could not restore project: {e}"),
        };
        pointer::add_toast(&self.toasts, &message);
        self.refresh();
    }

    fn confirm_purge(self: &Rc<Self>, project: &Project) {
        let alert = adw::AlertDialog::new(
            Some("Permanently delete project?"),
            Some(&format!(
                "“{}” and all of its tickets will be erased. This cannot be undone.",
                project.name
            )),
        );
        alert.add_responses(&[("cancel", "Cancel"), ("purge", "Delete Permanently")]);
        alert.set_response_appearance("purge", adw::ResponseAppearance::Destructive);
        alert.set_default_response(Some("cancel"));
        alert.set_close_response("cancel");
        alert.connect_map(|alert| pointer::set_on_buttons(alert.upcast_ref()));
        let this = Rc::clone(self);
        let id = project.id;
        // Check the id rather than connecting to the "purge" detail only, so no other
        // response can ever trigger the deletion.
        alert.connect_response(
            None,
            clone!(
                #[weak]
                this,
                move |_, response| {
                    if response == "purge" {
                        this.purge(id);
                    }
                }
            ),
        );
        alert.present(Some(&self.page));
    }

    fn purge(self: &Rc<Self>, id: i64) {
        let result = self.store.borrow_mut().purge_project(id);
        let message = match result {
            Ok(()) => "Project permanently deleted".to_owned(),
            Err(e) => format!("Could not delete project: {e}"),
        };
        pointer::add_toast(&self.toasts, &message);
        self.refresh();
    }
}
