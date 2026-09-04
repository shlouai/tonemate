pub mod bedrock;

use tauri::{Manager, WebviewWindow};

/// Translate what was typed and print both sides to the terminal. The frontend
/// doesn't await this, so the bar can hide the instant Enter is pressed while
/// the model call finishes in the background.
#[tauri::command]
async fn submit(text: String) {
    println!("[tonemate] in : {text}");
    match bedrock::translate(&text).await {
        Ok(translated) => println!("[tonemate] out: {translated}"),
        Err(err) => eprintln!("[tonemate] translate failed: {err}"),
    }
}

/// Show + focus, or hide. `is_visible` is the source of truth: the window
/// starts hidden (`"visible": false` in tauri.conf.json), so the first
/// hotkey press summons it.
fn toggle(window: &WebviewWindow) {
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
        .invoke_handler(tauri::generate_handler![submit])
        .setup(|app| {
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
                            if let Some(window) = app.get_webview_window("main") {
                                toggle(&window);
                            }
                        }
                    })?;

                println!("[tonemate] hotkey registered: Cmd+Shift+Space");
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
