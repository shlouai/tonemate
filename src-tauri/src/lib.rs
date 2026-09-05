pub mod bedrock;
pub mod settings;
pub mod tones;
mod tray;

use std::io::Write;

use tauri::{Emitter, Manager, WebviewWindow, WindowEvent};

/// Window labels, as declared in tauri.conf.json.
pub(crate) const MAIN_WINDOW: &str = "main";
pub(crate) const SETTINGS_WINDOW: &str = "settings";

/// Translate what was typed, streaming the renderings to both the terminal and
/// the result box under the input. The frontend doesn't await this — it just
/// listens for the events below, so the bar stays responsive while the model
/// answers.
#[tauri::command]
async fn submit(window: WebviewWindow, text: String) {
    println!("[tonemate] in : {text}");
    // The renderings arrive on lines of their own, so the label gets its own
    // line too rather than sitting in front of the first one.
    println!("[tonemate] out:");

    let _ = window.emit("translate:start", ());

    let mut parser = tones::Parser::default();
    let result = bedrock::translate(&text, |fragment| {
        print!("{fragment}");
        // stdout is line-buffered, so each fragment needs an explicit flush to
        // actually appear as it arrives rather than all at once at the newline.
        let _ = std::io::stdout().flush();
        for tone in parser.push(fragment) {
            let _ = window.emit("translate:tone", tone);
        }
    })
    .await;

    println!();
    match result {
        // The frontend needs the end of the stream, not just its fragments: a
        // response that streams nothing would otherwise leave the loading
        // placeholder up forever. `finish` comes first because the model
        // usually omits the trailing newline, so the last rendering is still
        // sitting in the parser at this point.
        Ok(_) => {
            for tone in parser.finish() {
                let _ = window.emit("translate:tone", tone);
            }
            let _ = window.emit("translate:done", ());
        }
        Err(err) => {
            eprintln!("[tonemate] translate failed: {err}");
            let _ = window.emit("translate:error", err);
        }
    }
}

/// Show + focus, or hide. `is_visible` is the source of truth: the window
/// starts hidden (`"visible": false` in tauri.conf.json), so the first
/// hotkey press summons it.
pub(crate) fn toggle(window: &WebviewWindow) {
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        println!("[tonemate] toggle -> hidden");
    } else {
        let _ = window.show();
        let _ = window.set_focus();
        println!("[tonemate] toggle -> shown");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Copying a rendering goes through the native pasteboard rather than
        // `navigator.clipboard`: WebKit refuses that one outside a user gesture
        // it recognises, and a click on a plain div isn't reliably one.
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![
            submit,
            settings::accent,
            settings::set_accent
        ])
        // The settings window is reused rather than rebuilt, so closing it has
        // to mean hiding it: letting the close through destroys the webview, and
        // the next 设置… would pay to start a fresh one. Hiding also keeps this
        // from being a way to lose the menu bar item — the app has no other
        // window that has to stay alive.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == SETTINGS_WINDOW {
                    api.prevent_close();
                    let _ = window.hide();
                    println!("[tonemate] settings -> hidden");
                }
            }
        })
        .setup(|app| {
            // Menu bar only: the bar is summoned by a hotkey and dismissed with
            // Esc, so a Dock icon would stand for a window that is almost never
            // there. Set before anything is shown, or the icon flashes into the
            // Dock on launch.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            tray::init(app)?;

            #[cfg(desktop)]
            {
                use tauri_plugin_global_shortcut::{
                    Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState,
                };

                // SUPER is Cmd on macOS.
                let hotkey = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::Space);

                app.handle()
                    .plugin(tauri_plugin_global_shortcut::Builder::new().build())?;

                app.global_shortcut()
                    .on_shortcut(hotkey, move |app, _shortcut, event| {
                        // Fires for both press and release; only act once.
                        if event.state == ShortcutState::Pressed {
                            if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
                                toggle(&window);
                            }
                        }
                    })?;

                println!("[tonemate] hotkey registered: Cmd+Shift+Space");
            }

            // Off the startup path: the app is usable the moment the hotkey is
            // registered, and by the time anyone types, Bedrock is reachable.
            tauri::async_runtime::spawn(async {
                match bedrock::warm().await {
                    Ok(()) => println!("[tonemate] bedrock warm"),
                    Err(err) => eprintln!("[tonemate] bedrock warmup failed: {err}"),
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
