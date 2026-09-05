//! The menu bar item — the only part of tonemate you can point at.
//!
//! The app has no Dock icon and no window of its own until the hotkey summons
//! one, so without this there is nothing to click: no way to reach the settings
//! and no way to quit. The menu also names the hotkey, which is otherwise
//! something you have to already know.

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    App, AppHandle, Manager,
};

/// Menu item ids. They travel as strings through the menu event, so they are
/// named here once rather than spelled out at both ends.
const TOGGLE: &str = "toggle";
const SETTINGS: &str = "settings";
const QUIT: &str = "quit";

/// Builds the menu bar item and wires its three items up. Called once, from
/// `setup`.
pub fn init(app: &App) -> tauri::Result<()> {
    // The accelerator is spelled into the label rather than passed as one: the
    // hotkey is registered globally, so giving the menu item its own would put a
    // second, competing registration on the same chord. This way the menu only
    // tells you about the shortcut that already works.
    let toggle = MenuItem::with_id(app, TOGGLE, "打开输入条  ⌘⇧Space", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, SETTINGS, "设置…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, QUIT, "退出 tonemate", true, None::<&str>)?;
    // Quit is the one item with consequences, held apart from the two that
    // merely open something.
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&toggle, &settings, &separator, &quit])?;

    TrayIconBuilder::new()
        // The app icon, for now: a monochrome template image is what makes a
        // menu bar icon look native, and there isn't one in `icons/` yet.
        .icon(app.default_window_icon().expect("no default icon").clone())
        .menu(&menu)
        // A menu bar item whose left click does nothing reads as broken — there
        // is no other click for it to be saving itself for.
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            TOGGLE => {
                if let Some(window) = app.get_webview_window(crate::MAIN_WINDOW) {
                    crate::toggle(&window);
                }
            }
            SETTINGS => show_settings(app),
            QUIT => app.exit(0),
            _ => {}
        })
        .build(app)?;

    println!("[tonemate] menu bar item ready");
    Ok(())
}

/// The window is declared in tauri.conf.json and only ever hidden, so this is
/// show-and-focus rather than a build: reopening it costs nothing and it comes
/// back where it was left.
fn show_settings(app: &AppHandle) {
    let Some(window) = app.get_webview_window(crate::SETTINGS_WINDOW) else {
        eprintln!("[tonemate] settings window missing");
        return;
    };
    let _ = window.show();
    // Without this the window can come back behind whatever you were using: the
    // app is an accessory, so clicking the menu bar item doesn't bring it
    // forward on its own.
    let _ = window.set_focus();
    // Both halves of the window's life are logged, for the same reason `toggle`
    // is: this is a GUI-only path — there is no way to exercise it from a script
    // — so the terminal is where you check that it happened.
    println!("[tonemate] settings -> shown");
}
