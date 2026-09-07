mod window;

use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;

const APP_ID: &str = "io.github.nitrocontrol.NitroControl";

/// M17: the full token set from the design-handoff bundle
/// (`design_handoff_dashboard_redesign/README.md`'s "Design tokens" table) —
/// keeps the app's existing forced cyan accent (from M15) but now covers
/// window/card/header backgrounds too, plus the custom classes the M17
/// layout needs (`.card`, `.tab-btn`, `.profile-card`, `.pill`,
/// `.kicker-label`/`.meta-label`) since GTK has no equivalent stock widget
/// for any of those shapes. Applied once at startup via a CSS provider at
/// `STYLE_PROVIDER_PRIORITY_APPLICATION`. Forcing dark (rather than
/// hand-tuning a full light+dark palette) reuses Adwaita's own
/// already-tested dark layout for everything else this pass doesn't touch.
fn apply_style() {
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);

    let provider = gtk4::CssProvider::new();
    provider.load_from_data(
        "@define-color accent_color #00d4e0;\n\
         @define-color accent_bg_color #00b8c4;\n\
         @define-color accent_fg_color #00272c;\n\
         @define-color window_bg_color #161826;\n\
         @define-color window_fg_color #e9e9ed;\n\
         @define-color view_bg_color #232532;\n\
         @define-color view_fg_color #e9e9ed;\n\
         @define-color headerbar_bg_color #161826;\n\
         @define-color headerbar_fg_color #e9e9ed;\n\
         @define-color card_bg_color #232532;\n\
         \n\
         headerbar {\n\
           background-image: linear-gradient(to bottom, #1d2032, #161826);\n\
           border-bottom: 1px solid rgba(233,233,237,0.16);\n\
           box-shadow: none;\n\
         }\n\
         \n\
         /* Adwaita's stock togglebutton :checked state doesn't route \
            through the accent named colors above. Nocturne's own pattern \
            (design_handoff_dashboard_redesign/styles.css's .btn-primary) is \
            a translucent accent tint plus a solid accent border, never a \
            flooded fill -- GTK4's CSS engine has no color-mix(), so the \
            tint is spelled out as a literal rgba(). */\n\
         button:checked {\n\
           background-color: rgba(0, 212, 224, 0.12);\n\
           border-color: #00d4e0;\n\
           color: #e9e9ed;\n\
         }\n\
         \n\
         /* Top tab bar (replaces the M15 bottom AdwViewSwitcherBar): plain \
            flat buttons with a 2px accent underline on the active one -- \
            AdwViewSwitcher's own stock look is a segmented pill row, not \
            this underline style, so it's a custom class instead. */\n\
         .tab-btn {\n\
           background: transparent;\n\
           border: none;\n\
           border-bottom: 2px solid transparent;\n\
           border-radius: 0;\n\
           padding: 0 12px;\n\
           color: #9397ab;\n\
           font-weight: 500;\n\
           box-shadow: none;\n\
         }\n\
         .tab-btn:checked {\n\
           background: transparent;\n\
           border-bottom: 2px solid #00d4e0;\n\
           color: #e9e9ed;\n\
           box-shadow: none;\n\
         }\n\
         .tab-btn:hover { background: transparent; color: #e9e9ed; }\n\
         \n\
         /* Metric/stat cards (Overview, CPU/GPU, Battery, System) */\n\
         .card {\n\
           background-color: #232532;\n\
           border-radius: 14px;\n\
           box-shadow: 0 0 0 1px #3f424d;\n\
         }\n\
         \n\
         /* The header's DMI-product-name pill, and Battery Calibration's \
            read-only state pill. */\n\
         .pill {\n\
           border: 1px solid #3f424d;\n\
           border-radius: 999px;\n\
           padding: 3px 10px;\n\
           color: #b2b6ca;\n\
           font-size: 12px;\n\
         }\n\
         \n\
         /* Power Profile's selectable cards (replaces M15's linked \
            ToggleButton pill row) -- a GtkToggleButton with .profile-card, \
            :checked matching Nocturne's translucent-tint-plus-border \
            pattern again. */\n\
         .profile-card {\n\
           background-color: #232532;\n\
           border: 1px solid #3f424d;\n\
           border-radius: 8px;\n\
           padding: 11px;\n\
           box-shadow: none;\n\
         }\n\
         .profile-card:checked {\n\
           border-color: #00d4e0;\n\
           background-color: rgba(0, 212, 224, 0.12);\n\
           box-shadow: 0 0 0 1px #00d4e0;\n\
         }\n\
         .profile-card:hover { background-color: rgba(0, 212, 224, 0.08); }\n\
         \n\
         /* RAM/Battery progress bars -- GtkProgressBar's stock trough/\
            fill nodes, retargeted to the flat 6px track+fill look. */\n\
         progressbar > trough {\n\
           background-color: #292b31;\n\
           border-radius: 3px;\n\
           min-height: 6px;\n\
         }\n\
         progressbar > trough > progress {\n\
           background-color: #00d4e0;\n\
           border-radius: 3px;\n\
           min-height: 6px;\n\
         }\n\
         \n\
         /* Header brand mark: a 3x20 accent bar with a glow, and the DMI \
            pill's small accent dot. */\n\
         .accent-bar {\n\
           background-color: #00d4e0;\n\
           border-radius: 2px;\n\
           box-shadow: 0 0 12px #00d4e0;\n\
         }\n\
         .accent-dot {\n\
           background-color: #00d4e0;\n\
           border-radius: 999px;\n\
         }\n\
         \n\
         /* Small uppercase section/card labels, and dimmer meta/caption \
            text -- used throughout every card and row. */\n\
         .kicker-label {\n\
           color: #9397ab;\n\
           font-size: 12px;\n\
           letter-spacing: 0.08em;\n\
         }\n\
         .meta-label {\n\
           color: #9397ab;\n\
           font-size: 12px;\n\
         }\n\
         \n\
         /* Top tab bar's own bottom hairline, spanning the full row. */\n\
         .tab-bar {\n\
           border-bottom: 1px solid rgba(233,233,237,0.16);\n\
           padding: 0 17px;\n\
         }\n\
         \n\
         /* CPU/GPU tab rows: hairline between rows, none on the last. */\n\
         .metric-row {\n\
           border-bottom: 1px solid #292b31;\n\
         }\n\
         .metric-row:last-child {\n\
           border-bottom: none;\n\
         }\n\
         \n\
         /* Acer Firmware Profile's dropdown -- an accent-outlined pill \
            instead of AdwComboRow's plain default chrome. */\n\
         .accent-combo {\n\
           border: 1px solid #00d4e0;\n\
           border-radius: 8px;\n\
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
