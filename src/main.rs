mod db;
mod markdown;
mod model;
mod ui;

use std::cell::RefCell;
use std::path::PathBuf;

use adw::prelude::*;
use clap::Parser;
use gtk::{gio, glib};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[arg(short, long, value_name = "PATH", default_value = "./project.db")]
    project: PathBuf,
}

fn main() -> glib::ExitCode {
    let cli = Cli::parse();

    let store = match db::Store::open(&cli.project) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("error: {}: {e}", cli.project.display());
            return glib::ExitCode::FAILURE;
        }
    };

    let app = adw::Application::builder()
        .application_id("dev.orgx.Orgx")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    let store = RefCell::new(Some(store));
    let path = cli.project;
    app.connect_activate(move |app| {
        if let Some(store) = store.take() {
            ui::build(app, store, &path);
        }
    });

    // Arguments were already handled by clap; don't let GApplication parse them again.
    let program: Vec<String> = std::env::args().take(1).collect();
    app.run_with_args(&program)
}
