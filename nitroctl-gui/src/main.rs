mod format;
mod window;

use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;

const APP_ID: &str = "io.github.nitrocontrol.NitroControl";

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(|app| {
        window::build_window(app).present();
    });
    app.run()
}
