//! Local translation through a downloaded GGUF model running under llama-server.
//!
//! Both the model weights and the llama.cpp `llama-server` binary are fetched on
//! demand into the app's data directory. Selecting the local provider starts the
//! model download; the server is spawned lazily on the first translation.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use tauri::{AppHandle, Manager};

use super::env_or;

/// The model weights, downloaded from Hugging Face (a mirror first — the
/// canonical host is not reachable from everywhere).
const MODEL_FILE: &str = "Hy-MT2-1.8B-1.25Bit.gguf";
const MODEL_SIZE: u64 = 461_860_800;
const MODEL_URLS: [&str; 2] = [
    "https://hf-mirror.com/tencent/Hy-MT2-1.8B-1.25Bit-GGUF/resolve/main/Hy-MT2-1.8B-1.25Bit.gguf",
    "https://huggingface.co/tencent/Hy-MT2-1.8B-1.25Bit-GGUF/resolve/main/Hy-MT2-1.8B-1.25Bit.gguf",
];

/// The llama.cpp release this build pins. It must be recent enough to carry both
/// the `hunyuan-dense` architecture and 1.25-bit quantization.
const LLAMA_CPP_TAG: &str = "b10936";

/// The archive holding llama.cpp for this platform, and its expected size.
#[cfg(target_os = "windows")]
const BINARY_ASSET: &str = "llama-b10936-bin-win-cpu-x64.zip";
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const BINARY_ASSET: &str = "llama-b10936-bin-macos-arm64.tar.gz";
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const BINARY_ASSET: &str = "llama-b10936-bin-macos-x64.tar.gz";
#[cfg(target_os = "windows")]
const BINARY_SIZE: u64 = 18_426_326;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const BINARY_SIZE: u64 = 11_146_651;
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const BINARY_SIZE: u64 = 11_194_813;

/// The `llama-server` executable name after extraction.
#[cfg(target_os = "windows")]
const SERVER_BIN: &str = "llama-server.exe";
#[cfg(not(target_os = "windows"))]
const SERVER_BIN: &str = "llama-server";

fn binary_url() -> String {
    env_or(
        "TONEMATE_LLAMA_SERVER_URL",
        &format!(
            "https://github.com/ggml-org/llama.cpp/releases/download/{LLAMA_CPP_TAG}/{BINARY_ASSET}"
        ),
    )
}

/// Where the model weights live.
pub fn model_path(app: &AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_default()
        .join("models")
        .join(MODEL_FILE)
}

/// Where the llama.cpp binaries are unpacked.
pub fn bin_dir(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().unwrap_or_default().join("bin")
}

pub fn binary_path(app: &AppHandle) -> PathBuf {
    bin_dir(app).join(SERVER_BIN)
}

/// What a download is doing, if anything. The filesystem is the source of truth
/// for "downloaded"; this only distinguishes the transient moments.
#[derive(Default)]
enum DownloadState {
    #[default]
    Idle,
    Downloading,
    Error(String),
}

static DOWNLOAD_STATE: Mutex<DownloadState> = Mutex::new(DownloadState::Idle);

fn is_downloading() -> bool {
    matches!(*DOWNLOAD_STATE.lock().unwrap(), DownloadState::Downloading)
}

pub fn model_size(app: &AppHandle) -> u64 {
    fs::metadata(model_path(app)).map(|m| m.len()).unwrap_or(0)
}

/// The state string the settings window renders. Pure of the download state so
/// it can be unit-tested.
fn state_string(size_on_disk: u64, downloading: bool) -> String {
    if downloading {
        "downloading".to_string()
    } else if size_on_disk == MODEL_SIZE {
        "downloaded".to_string()
    } else {
        "missing".to_string()
    }
}

pub fn model_state(app: &AppHandle) -> String {
    match &*DOWNLOAD_STATE.lock().unwrap() {
        DownloadState::Error(_) => "error".to_string(),
        _ => state_string(model_size(app), is_downloading()),
    }
}

pub fn model_error() -> Option<String> {
    match &*DOWNLOAD_STATE.lock().unwrap() {
        DownloadState::Error(message) => Some(message.clone()),
        _ => None,
    }
}

/// Where a resumed download continues from. `status_206` says the server
/// honoured the `Range` header; a 200 means it ignored it and the file must be
/// restarted from zero.
fn resume_offset(existing: u64, status_206: bool) -> u64 {
    if status_206 {
        existing
    } else {
        0
    }
}

use std::io::Write as _;

use futures_util::StreamExt;
use serde::Serialize;
use tauri::Emitter;

#[derive(Clone, Serialize)]
struct Progress {
    bytes: u64,
    total: u64,
}

/// Streams `url` to `dest`, resuming from a `.part` file when possible.
/// `app` is `None` for the small binary download, which reports nothing; the
/// model download passes the app handle to broadcast progress.
async fn download(
    app: Option<&AppHandle>,
    url: &str,
    dest: &Path,
    total: u64,
) -> Result<(), String> {
    let part = PathBuf::from(format!("{}.part", dest.display()));
    let existing = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);

    // Fully downloaded but the rename never ran (a crash at the very end).
    if existing >= total {
        fs::rename(&part, dest).map_err(|e| e.to_string())?;
        return Ok(());
    }

    let mut response = reqwest::Client::new()
        .get(url)
        .header("Range", format!("bytes={existing}-"))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let resumed = resume_offset(existing, response.status().as_u16() == 206);
    let mut file = if resumed > 0 {
        fs::OpenOptions::new()
            .append(true)
            .open(&part)
            .map_err(|e| e.to_string())?
    } else {
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&part)
            .map_err(|e| e.to_string())?
    };
    let mut downloaded = resumed;

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        file.write_all(&chunk).map_err(|e| e.to_string())?;
        downloaded += chunk.len() as u64;
        if let Some(app) = app {
            let _ = app.emit("model:download-progress", Progress { bytes: downloaded, total });
        }
    }
    file.flush().map_err(|e| e.to_string())?;

    if downloaded != total {
        return Err(format!("下载不完整：{downloaded}/{total} 字节"));
    }
    fs::rename(&part, dest).map_err(|e| e.to_string())?;
    Ok(())
}

/// Downloads the model if it is not already present, resuming an interrupted
/// download. Called when the local provider is selected.
pub async fn ensure_model_downloaded(app: &AppHandle) -> Result<(), String> {
    if model_size(app) == MODEL_SIZE {
        return Ok(());
    }
    *DOWNLOAD_STATE.lock().unwrap() = DownloadState::Downloading;

    let result = async {
        let mut last = String::from("no URL tried");
        let override_url = env_or("TONEMATE_LOCAL_MODEL_URL", "");
        let urls = std::iter::once(override_url.as_str())
            .filter(|u| !u.is_empty())
            .chain(MODEL_URLS);
        for url in urls {
            match download(Some(app), url, &model_path(app), MODEL_SIZE).await {
                Ok(()) => return Ok(()),
                Err(err) => last = err,
            }
        }
        Err(last)
    }
    .await;

    *DOWNLOAD_STATE.lock().unwrap() = match &result {
        Ok(()) => DownloadState::Idle,
        Err(err) => DownloadState::Error(err.clone()),
    };
    match &result {
        Ok(()) => {
            let _ = app.emit(
                "model:download-done",
                serde_json::json!({ "path": model_path(app).to_string_lossy() }),
            );
        }
        Err(err) => {
            let _ = app.emit("model:download-error", serde_json::json!({ "message": err }));
        }
    }
    result
}

use super::openai_compat::Endpoint;

async fn ensure_binary(binary: &Path) -> Result<(), String> {
    if binary.exists() {
        return Ok(());
    }
    let dir = binary.parent().ok_or("no bin directory")?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;

    let archive = dir.join(BINARY_ASSET);
    let have_archive = fs::metadata(&archive).map(|m| m.len()).unwrap_or(0) == BINARY_SIZE;
    if !have_archive {
        download(None, &binary_url(), &archive, BINARY_SIZE).await?;
    }
    extract(&archive, dir)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(binary, fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn extract(archive: &Path, dest: &Path) -> Result<(), String> {
    if archive.extension().and_then(|e| e.to_str()) == Some("zip") {
        let file = fs::File::open(archive).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        zip.extract(dest).map_err(|e| e.to_string())?;
    } else {
        let file = fs::File::open(archive).map_err(|e| e.to_string())?;
        let gz = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(gz);
        for entry in tar.entries().map_err(|e| e.to_string())? {
            let mut entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path().map_err(|e| e.to_string())?.to_path_buf();
            // Strip the archive's single top-level directory so `llama-server`
            // lands directly in `dest`, not in `dest/llama-b10936/`.
            let mut components = path.components();
            components.next();
            let stripped: PathBuf = components.collect();
            if stripped.as_os_str().is_empty() {
                continue;
            }
            entry.unpack(dest.join(&stripped)).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

static SERVER: tokio::sync::Mutex<Option<std::process::Child>> =
    tokio::sync::Mutex::const_new(None);

fn port() -> u16 {
    env_or("TONEMATE_LOCAL_PORT", "8931").parse().unwrap_or(8931)
}

fn endpoint(port: u16) -> Endpoint {
    Endpoint {
        base_url: format!("http://127.0.0.1:{port}/v1"),
        model: "local".to_string(),
        api_key: String::new(),
        max_tokens_field: "max_tokens",
        extra: serde_json::json!({}),
    }
}

async fn healthy(port: u16) -> bool {
    reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/health"))
        .timeout(std::time::Duration::from_millis(500))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

async fn poll_healthy(port: u16) -> Result<(), String> {
    for _ in 0..60 {
        if healthy(port).await {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    Err("llama-server 启动超时".to_string())
}

/// Ensures the model and binary are present and the server is running, and
/// returns the OpenAI-compatible endpoint to talk to.
pub async fn ensure_server(model: &Path, binary: &Path) -> Result<Endpoint, String> {
    if !model.exists() {
        return Err("本地模型尚未下载，请先在设置中选择「本地模型」触发下载".to_string());
    }
    ensure_binary(binary).await?;

    let port = port();
    let mut guard = SERVER.lock().await;
    if guard.is_none() || !healthy(port).await {
        let mut cmd = std::process::Command::new(binary);
        cmd.arg("--model")
            .arg(model)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg("--ctx-size")
            .arg("8192");
        let ngl = env_or("TONEMATE_LOCAL_N_GPU_LAYERS", "0");
        if ngl != "0" {
            cmd.arg("--n-gpu-layers").arg(ngl);
        }
        let child = cmd.spawn().map_err(|e| format!("启动 llama-server 失败：{e}"))?;
        *guard = Some(child);
        poll_healthy(port).await?;
    }
    Ok(endpoint(port))
}

/// Kills the llama-server child so it does not outlive the app.
pub fn kill_server() {
    if let Ok(mut guard) = SERVER.try_lock() {
        if let Some(child) = guard.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_continues_only_when_the_server_honours_range() {
        assert_eq!(resume_offset(100, true), 100);
        assert_eq!(resume_offset(100, false), 0);
    }

    #[test]
    fn state_string_distinguishes_the_download_moments() {
        assert_eq!(state_string(0, false), "missing");
        assert_eq!(state_string(MODEL_SIZE, false), "downloaded");
        assert_eq!(state_string(0, true), "downloading");
        assert_eq!(state_string(MODEL_SIZE - 1, false), "missing");
    }
}
