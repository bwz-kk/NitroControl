mod format;
mod window;

use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;

const APP_ID: &str = "io.github.nitrocontrol.NitroControl";

/// M16: force dark mode and recolor Adwaita's named colors using the
/// "Nocturne" design system's real tokens (read from its `theme.json`/
/// `styles.css` via the design-handoff project) instead of the ad-hoc
/// cyan M15 picked: ground `#161826`, surface `#232532`, text `#e9e9ed`,
/// accent `#9184d9` (a blurple), accent-600 `#796cbf` for tinted/pressed
/// fills. Applied once at startup via a CSS provider at
/// `STYLE_PROVIDER_PRIORITY_APPLICATION` — every stock widget that already
/// renders with `@accent_color`/`@accent_bg_color`/`@accent_fg_color`/
/// `@window_bg_color`/etc. (switches, selected `ComboRow` items, row
/// backgrounds, the header bar) picks this up automatically, no
/// per-widget styling code needed. Forcing dark (rather than hand-tuning a
/// full light+dark palette) reuses Adwaita's own already-tested dark
/// layout for everything else, so contrast/legibility stays sound outside
/// the colors this pass deliberately changes.
fn apply_style() {
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);

    let provider = gtk4::CssProvider::new();
    provider.load_from_data(
        "@define-color accent_color #9184d9;\n\
         @define-color accent_bg_color #796cbf;\n\
         @define-color accent_fg_color #f5f4ff;\n\
         @define-color window_bg_color #161826;\n\
         @define-color window_fg_color #e9e9ed;\n\
         @define-color view_bg_color #232532;\n\
         @define-color view_fg_color #e9e9ed;\n\
         @define-color headerbar_bg_color #161826;\n\
         @define-color headerbar_fg_color #e9e9ed;\n\
         @define-color card_bg_color #232532;\n\
         /* Adwaita's stock togglebutton :checked state doesn't route \
            through the accent named colors above -- the ProfilePills \
            row (M15) and the AdwViewSwitcherBar's active tab need it \
            explicit. Nocturne's own interaction pattern is a translucent \
            accent tint plus a solid accent border, never a flooded fill \
            (\"the accent is a line and a glow, never a flood\", its \
            readme) -- GTK4's CSS engine has no color-mix()/rgba-from-hex-\
            var shorthand, so the tint is spelled out as literal rgba(). */\n\
         button:checked {\n\
           background-color: rgba(145, 132, 217, 0.22);\n\
           border-color: #9184d9;\n\
           color: #f3f5fe;\n\
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
