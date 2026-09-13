pub mod provider;
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
    let chosen = provider::Provider::from_settings(window.app_handle());
    let result = provider::translate(&chosen, &text, |fragment| {
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
        // A global-hotkey + tray app is single-instance by nature: a second
        // launch would fight the first over the same hotkey and crash in setup
        // ("HotKey already registered"). This plugin exits the second instance
        // and lets us bring the first one's bar forward instead.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        // Copying a rendering goes through the native pasteboard rather than
        // `navigator.clipboard`: WebKit refuses that one outside a user gesture
        // it recognises, and a click on a plain div isn't reliably one.
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![
            submit,
            settings::accent,
            settings::set_accent,
            settings::provider_config,
            settings::set_provider,
            settings::set_kimi_api_key,
            settings::set_deepseek_api_key,
            settings::set_qwen_api_key,
            settings::set_aws_profile
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

                // SUPER is Cmd on macOS, the Windows key elsewhere. Windows
                // reserves Win+Shift+Space for input-method switching, so
                // non-macOS builds use Ctrl+Shift+Space instead.
                #[cfg(target_os = "macos")]
                let hotkey = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::Space);
                #[cfg(not(target_os = "macos"))]
                let hotkey =
                    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space);

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

                println!(
                    "[tonemate] hotkey registered: {}",
                    if cfg!(target_os = "macos") {
                        "Cmd+Shift+Space"
                    } else {
                        "Ctrl+Shift+Space"
                    }
                );
            }

            // Off the startup path: the app is usable the moment the hotkey is
            // registered, and by the time anyone types, the provider is reachable.
            let warm_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let chosen = provider::Provider::from_settings(&warm_handle);
                match provider::warm(&chosen).await {
                    Ok(()) => println!("[tonemate] {} warm", chosen.label()),
                    Err(err) => {
                        eprintln!("[tonemate] {} warmup failed: {err}", chosen.label())
                    }
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            if let tauri::RunEvent::Exit = event {
                provider::local::kill_server();
            }
        });
}
