//! Exercise the Bedrock translation path without launching the GUI.
//!
//!   cargo run --example translate -- "今天天气不错，我们出去走走吧。"

fn main() {
    let text = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    let text = if text.is_empty() {
        "今天天气不错，我们出去走走吧。".to_string()
    } else {
        text
    };

    println!("[tonemate] in : {text}");
    match tauri::async_runtime::block_on(tonemate_lib::bedrock::translate(&text)) {
        Ok(translated) => println!("[tonemate] out: {translated}"),
        Err(err) => {
            eprintln!("[tonemate] translate failed: {err}");
            std::process::exit(1);
        }
    }
}
