//! Modal for adding and renaming projects.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::glib::{self, clone};

use super::pointer;
use super::projects::ProjectsPage;
use crate::model::Project;

struct ProjectDialog {
    projects: Weak<ProjectsPage>,
    dialog: adw::Dialog,
    toasts: adw::ToastOverlay,
    cancel_button: gtk::Button,
    save_button: gtk::Button,
    name_row: adw::EntryRow,
    /// `None` when adding a project.
    project: Option<Project>,
}

pub fn open(projects: &Rc<ProjectsPage>, project: Option<Project>) {
    let this = Rc::new(ProjectDialog::new(projects, project));
    this.connect_signals();
    this.dialog.present(Some(projects.window()));
    // Focus only takes once the dialog is in the window.
    this.name_row.grab_focus();
}

impl ProjectDialog {
    fn new(projects: &Rc<ProjectsPage>, project: Option<Project>) -> Self {
        let cancel_button = gtk::Button::with_label("Cancel");
        let save_button = gtk::Button::builder()
            .label(if project.is_some() { "Save" } else { "Create" })
            .css_classes(["suggested-action"])
            .sensitive(false)
            .build();
        for button in [&cancel_button, &save_button] {
            button.set_cursor_from_name(Some("pointer"));
        }
        // Cancel replaces the close button.
        let header = adw::HeaderBar::builder()
            .show_start_title_buttons(false)
            .show_end_title_buttons(false)
            .build();
        header.pack_start(&cancel_button);
        header.pack_end(&save_button);

        let name_row = adw::EntryRow::builder().title("Name").build();
        if let Some(project) = &project {
            name_row.set_text(&project.name);
        }
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .margin_start(18)
            .margin_end(18)
            .margin_top(12)
            .margin_bottom(18)
            .build();
        list.append(&name_row);

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&list));
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&toasts));

        let dialog = adw::Dialog::builder()
            .title(if project.is_some() {
                "Edit Project"
            } else {
                "New Project"
            })
            .content_width(420)
            .child(&toolbar)
            .build();

        Self {
            projects: Rc::downgrade(projects),
            dialog,
            toasts,
            cancel_button,
            save_button,
            name_row,
            project,
        }
    }

    fn connect_signals(self: &Rc<Self>) {
        let this = Rc::clone(self);

        self.cancel_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| {
                this.dialog.close();
            }
        ));
        self.save_button.connect_clicked(clone!(
            #[weak]
            this,
            move |_| this.save()
        ));
        self.name_row.connect_changed(clone!(
            #[weak]
            this,
            move |_| this.update_save_button()
        ));
        self.name_row.connect_entry_activated(clone!(
            #[weak]
            this,
            move |_| this.save()
        ));
        // Covers buttons libadwaita builds inside the dialog as well as our own.
        self.dialog
            .connect_map(|dialog| pointer::set_on_buttons(dialog.upcast_ref()));

        // Every other handler holds a weak reference; this one keeps the dialog state alive
        // until the dialog closes.
        let keep_alive = RefCell::new(Some(this));
        self.dialog.connect_closed(move |_| {
            keep_alive.take();
        });
    }

    fn name(&self) -> String {
        self.name_row.text().trim().to_owned()
    }

    fn update_save_button(&self) {
        let name = self.name();
        let unchanged = self.project.as_ref().is_some_and(|p| p.name == name);
        self.save_button
            .set_sensitive(!name.is_empty() && !unchanged);
    }

    fn save(&self) {
        if !self.save_button.is_sensitive() {
            return;
        }
        let Some(projects) = self.projects.upgrade() else {
            return;
        };
        let name = self.name();
        let result = match &self.project {
            Some(project) => projects.update_project(project.id, &name),
            None => projects.create_project(&name),
        };
        match result {
            Ok(_) => {
                self.dialog.close();
            }
            Err(e) => pointer::add_toast(&self.toasts, &format!("Could not save: {e}")),
        }
    }
}
