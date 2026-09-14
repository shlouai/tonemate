//! Batch-test the local translation path against a JSON list of cases.
//!
//!   cargo run --example local_test -- examples/test_cases.json
//!
//! Each case runs the full production path — scene classification followed by one
//! request per tone — and every rendered tone is checked for language direction:
//! Chinese input must come back with no Han characters in the translation,
//! non-Chinese input must come back with Han characters. A tone that echoes the
//! input or refuses in the target language is still a failure.
//!
//! Prints one line per case (PASS/FAIL with the offending renderings) and a
//! summary at the end. Reuses the model/binary from the environment the same way
//! `translate.rs` does.

use std::collections::BTreeMap;
use std::io::Read;
use std::time::Instant;

use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    text: String,
    #[serde(default)]
    scene: String,
}

/// The Han blocks that make text Chinese. Mirrors `provider::mod::is_han`.
fn has_han(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(
            c,
            '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}'
        )
    })
}

fn normalized(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '，' | '。' | '．' | '！' | '？' | '、' | '；' | '：' | '.' | ',' | '!' | '?' | ';' | ':' | '"' | '\'' | '“' | '”' | '‘' | '’'))
        .collect()
}

/// Phrases that mark the model answering as a chat partner rather than emitting
/// a translation. Reported as a note, never a failure on its own: a
/// customer-service translation legitimately says "sorry"/"please", so the
/// language-direction check below is the only pass/fail signal.
fn chat_note(s: &str) -> Option<&'static str> {
    let lower = s.to_lowercase();
    if has_han(s) {
        if ["请把", "请用", "请将", "请提供", "翻译成", "语气要"]
            .iter()
            .any(|p| s.contains(p))
        {
            return Some("instruction-echo");
        }
    } else if ["hey there", "hey!", "hello!", "good morning", "good evening", "i'm not sure", "not sure", "let me know", "could you", "can i ask", "did you hear", "as an ai"]
        .iter()
        .any(|p| lower.contains(p))
    {
        return Some("chat");
    }
    None
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args.first().cloned().unwrap_or_else(|| "examples/test_cases.json".to_string());
    let limit: Option<usize> = args.get(1).and_then(|s| s.parse().ok());

    let mut raw = String::new();
    std::fs::File::open(&path)
        .unwrap_or_else(|e| panic!("cannot open {path}: {e}"))
        .read_to_string(&mut raw)
        .expect("read cases");
    let cases: Vec<Case> = serde_json::from_str(&raw).expect("parse cases");
    let cases: Vec<Case> = if let Some(limit) = limit {
        cases.into_iter().take(limit).collect()
    } else {
        cases
    };

    tauri::async_runtime::block_on(async {
        let chosen = tonemate_lib::provider::Provider::from_env();
        let mut passed = 0usize;
        let mut failed = Vec::new();
        let started = Instant::now();

        for (i, case) in cases.iter().enumerate() {
            let expect_english = has_han(&case.text);
            let mut parser = tonemate_lib::tones::Parser::default();
            let mut rows: BTreeMap<usize, (String, String)> = BTreeMap::new();
            let result = tonemate_lib::provider::translate(&chosen, &case.text, |fragment| {
                for tone in parser.push(fragment) {
                    rows.insert(tone.index, (tone.label, tone.text));
                }
            })
            .await;
            for tone in parser.finish() {
                rows.insert(tone.index, (tone.label, tone.text));
            }

            match result {
                Err(err) => {
                    println!("[{:03}] ERROR {} — {err}", i + 1, case.text);
                    failed.push((i + 1, case.text.clone(), vec![err]));
                }
                Ok(_) => {
                    let mut problems = Vec::new();
                    let mut notes = Vec::new();
                    for (label, text) in rows.values() {
                        let text = text.trim();
                        let verdict = if text.is_empty() {
                            "empty"
                        } else if expect_english && has_han(text) {
                            if normalized(text).contains(&normalized(&case.text)) {
                                "echo-source"
                            } else {
                                "source-language"
                            }
                        } else if !expect_english && !has_han(text) {
                            if normalized(text).contains(&normalized(&case.text)) {
                                "echo-source"
                            } else {
                                "source-language"
                            }
                        } else {
                            "ok"
                        };
                        if verdict != "ok" {
                            problems.push(format!("[{label}] {verdict}: {text}"));
                        } else if let Some(note) = chat_note(text) {
                            notes.push(format!("[{label}] {note}: {text}"));
                        }
                    }
                    if problems.is_empty() {
                        passed += 1;
                        if notes.is_empty() {
                            println!("[{:03}] PASS {}", i + 1, case.text);
                        } else {
                            println!("[{:03}] PASS {}  (note:)", i + 1, case.text);
                            for n in &notes {
                                println!("        {n}");
                            }
                        }
                    } else {
                        println!(
                            "[{:03}] FAIL {}  (scene: {})",
                            i + 1,
                            case.text,
                            case.scene
                        );
                        for p in &problems {
                            println!("        {p}");
                        }
                        failed.push((i + 1, case.text.clone(), problems));
                    }
                }
            }
        }

        println!();
        println!(
            "=== {passed}/{} passed, {} failed in {:?} ===",
            cases.len(),
            failed.len(),
            started.elapsed()
        );
        for (i, text, problems) in &failed {
            println!("  FAIL {i:03}: {text}");
            for p in problems {
                println!("      {p}");
            }
        }
    });
}
