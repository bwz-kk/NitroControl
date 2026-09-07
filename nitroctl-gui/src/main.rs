mod format;
mod window;

use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;

const APP_ID: &str = "io.github.nitrocontrol.NitroControl";

/// M15 follow-up: force dark mode and recolor Adwaita's named accent colors
/// toward the Alienware Command Center reference's cyan-blue (user-picked
/// over a red/NitroSense-style alternative, M15's roadmap note). Applied
/// once at startup via a CSS provider at `STYLE_PROVIDER_PRIORITY_APPLICATION`
/// — every stock widget that already renders with `@accent_color`/
/// `@accent_bg_color`/`@accent_fg_color` (switches, selected `ComboRow`
/// items, the linked `ToggleButton` pill row's active state, etc.) picks
/// this up automatically, no per-widget styling code needed. Forcing dark
/// (rather than hand-tuning a full light+dark palette) reuses Adwaita's own
/// already-tested dark colors for everything else, so contrast/legibility
/// stays sound outside the one color this pass deliberately changes.
fn apply_style() {
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);

    let provider = gtk4::CssProvider::new();
    provider.load_from_data(
        "@define-color accent_color #00d4e0;\n\
         @define-color accent_bg_color #00b8c4;\n\
         @define-color accent_fg_color #00272c;\n\
         /* Adwaita's stock togglebutton :checked state doesn't route \
            through the accent named colors above -- the ProfilePills \
            row (M15) needs it explicit to actually show cyan. */\n\
         button:checked {\n\
           background-color: #00b8c4;\n\
           color: #00272c;\n\
         }\n",
    );
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(|app| {
        apply_style();
        window::build_window(app).present();
    });
    app.run()
}
