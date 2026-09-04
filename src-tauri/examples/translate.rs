//! Exercise the Bedrock translation path without launching the GUI.
//!
//!   cargo run --example translate -- "今天天气不错，我们出去走走吧。"
//!
//! Prints time-to-first-word and total time, and runs the same warm-up the app
//! does at startup so the numbers reflect a warm client rather than a cold one.

use std::io::Write;
use std::time::Instant;

fn main() {
    let text = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    let text = if text.is_empty() {
        "今天天气不错，我们出去走走吧。".to_string()
    } else {
        text
    };

    tauri::async_runtime::block_on(async {
        let warmup = Instant::now();
        if let Err(err) = tonemate_lib::bedrock::warm().await {
            eprintln!("[tonemate] bedrock warmup failed: {err}");
            std::process::exit(1);
        }
        println!("[tonemate] warm in {:?}", warmup.elapsed());

        println!("[tonemate] in : {text}");
        print!("[tonemate] out: ");
        let _ = std::io::stdout().flush();

        let started = Instant::now();
        let mut first_word = None;
        let result = tonemate_lib::bedrock::translate(&text, |fragment| {
            first_word.get_or_insert_with(|| started.elapsed());
            print!("{fragment}");
            let _ = std::io::stdout().flush();
        })
        .await;
        println!();

        match result {
            Ok(_) => println!(
                "[tonemate] first word in {:?}, total {:?}",
                first_word.unwrap_or_default(),
                started.elapsed()
            ),
            Err(err) => {
                eprintln!("[tonemate] translate failed: {err}");
                std::process::exit(1);
            }
        }
    });
}
