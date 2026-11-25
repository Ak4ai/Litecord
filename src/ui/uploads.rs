//! Files picked to send with the next message. They come from the system's
//! own picker (Windows common dialog; zenity or kdialog on Linux), run on a
//! short lived thread, or from files dropped on the window. Nothing here runs
//! while no file is being picked or sent.

use std::path::PathBuf;
use std::sync::Mutex;

use slint::ComponentHandle;

use crate::utils::i18n_ui::en_pt;

/// Discord accepts at most 10 files per message.
pub const MAX_FILES: usize = 10;

struct Pending {
    path: PathBuf,
    spoiler: bool,
    /// Name chosen on the card, sent instead of the file's own.
    name: Option<String>,
}

impl Pending {
    fn shown_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| file_name(&self.path))
    }
}

static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());

fn pending() -> std::sync::MutexGuard<'static, Vec<Pending>> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Opens the system file picker and adds the chosen files.
pub fn pick(app_weak: slint::Weak<crate::AppWindow>) {
    let _ = std::thread::Builder::new().name("file-picker".into()).spawn(move || {
        let picked = open_dialog();
        if !picked.is_empty() {
            add(picked, &app_weak);
        }
    });
}

/// Adds files (from the picker or dropped on the window); folders are skipped.
pub fn add(paths: Vec<PathBuf>, app_weak: &slint::Weak<crate::AppWindow>) {
    {
        let mut list = pending();
        for p in paths {
            if list.len() < MAX_FILES && p.is_file() && !list.iter().any(|f| f.path == p) {
                list.push(Pending { path: p, spoiler: false, name: None });
            }
        }
    }
    refresh_ui(app_weak, String::new());
}

pub fn remove(index: usize, app_weak: &slint::Weak<crate::AppWindow>) {
    {
        let mut list = pending();
        if index < list.len() {
            list.remove(index);
        }
    }
    refresh_ui(app_weak, String::new());
}

/// Marks a file as a spoiler (Discord blurs it until clicked), or unmarks it.
pub fn toggle_spoiler(index: usize, app_weak: &slint::Weak<crate::AppWindow>) {
    if let Some(f) = pending().get_mut(index) {
        f.spoiler = !f.spoiler;
    }
    refresh_ui(app_weak, String::new());
}

/// Renames a file for sending (the file on disk is untouched). An empty
/// name, or one with characters Windows forbids, keeps the current name.
pub fn rename(index: usize, name: &str, app_weak: &slint::Weak<crate::AppWindow>) {
    let name = name.trim();
    let valid = !name.is_empty() && !name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']);
    if valid {
        if let Some(f) = pending().get_mut(index) {
            f.name = Some(name.to_string());
        }
    }
    refresh_ui(app_weak, String::new());
}

/// Takes the pending files for sending (the strip empties), each with the
/// name it goes out under: spoilers get Discord's `SPOILER_` prefix.
pub fn take() -> Vec<(PathBuf, String)> {
    std::mem::take(&mut *pending())
        .into_iter()
        .map(|f| {
            let name = f.shown_name();
            let name = if f.spoiler { format!("SPOILER_{}", name) } else { name };
            (f.path, name)
        })
        .collect()
}

pub fn has_pending() -> bool {
    !pending().is_empty()
}

/// Clears the files when switching channel, like Discord does.
pub fn clear(app_weak: &slint::Weak<crate::AppWindow>) {
    pending().clear();
    refresh_ui(app_weak, String::new());
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// "audio", "video", "image" or "file", for the icon on the card.
pub fn kind_of(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "mp3" | "ogg" | "opus" | "wav" | "m4a" | "flac" | "aac" => "audio",
        "mp4" | "webm" | "mov" | "mkv" | "avi" => "video",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => "image",
        _ => "file",
    }
}

static STATUS_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Shows an error under the bar and clears it after 10 seconds, unless
/// something else replaced it meanwhile.
pub fn show_error(app_weak: &slint::Weak<crate::AppWindow>, status: String) {
    refresh_ui(app_weak, status);
    let gen = STATUS_GEN.load(std::sync::atomic::Ordering::SeqCst);
    let w = app_weak.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(10));
        if STATUS_GEN.load(std::sync::atomic::Ordering::SeqCst) == gen {
            refresh_ui(&w, String::new());
        }
    });
}

/// Shows the pending files and a status line (empty for none).
pub fn refresh_ui(app_weak: &slint::Weak<crate::AppWindow>, status: String) {
    STATUS_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let items: Vec<(String, String, bool)> = pending()
        .iter()
        .map(|f| {
            let size = std::fs::metadata(&f.path).map(|m| format_size(m.len())).unwrap_or_default();
            (f.shown_name(), size, f.spoiler)
        })
        .collect();
    let w = app_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = w.upgrade() else { return };
        let model: Vec<crate::PendingFile> = items
            .into_iter()
            .map(|(name, size, spoiler)| crate::PendingFile {
                kind: kind_of(&name).into(),
                name: name.into(),
                size: size.into(),
                spoiler,
            })
            .collect();
        ui.set_pending_files(slint::ModelRc::new(slint::VecModel::from(model)));
        ui.set_upload_status(status.into());
    });
}

pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

pub fn sending_text(count: usize) -> String {
    format!("{} ({})", en_pt("Uploading", "Enviando"), count)
}

pub fn drop_hint() -> &'static str {
    en_pt("Drop to attach", "Solte para anexar")
}

pub fn too_large_text() -> String {
    en_pt(
        "The file is too large for Discord (10 MB without Nitro).",
        "O arquivo é grande demais para o Discord (10 MB sem Nitro).",
    )
    .to_string()
}

pub fn failed_text(err: &str) -> String {
    format!("{}: {}", en_pt("Could not send the files", "Não foi possível enviar os arquivos"), err)
}

#[cfg(target_os = "windows")]
fn open_dialog() -> Vec<PathBuf> {
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_ALLOWMULTISELECT, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST,
        OPENFILENAMEW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let mut buf = vec![0u16; 32 * 1024];
    let title: Vec<u16> = en_pt("Attach files", "Anexar arquivos").encode_utf16().chain(Some(0)).collect();
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = unsafe { GetForegroundWindow() };
    ofn.lpstrFile = buf.as_mut_ptr();
    ofn.nMaxFile = buf.len() as u32;
    ofn.lpstrTitle = title.as_ptr();
    ofn.Flags = OFN_ALLOWMULTISELECT | OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return Vec::new();
    }
    parse_multi_select(&buf)
}

/// The dialog returns either one full path, or the folder followed by the
/// file names, each ended by a NUL and the list by a second NUL.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_multi_select(buf: &[u16]) -> Vec<PathBuf> {
    let parts: Vec<String> = buf
        .split(|&c| c == 0)
        .take_while(|s| !s.is_empty())
        .map(String::from_utf16_lossy)
        .collect();
    match parts.as_slice() {
        [] => Vec::new(),
        [one] => vec![PathBuf::from(one)],
        [dir, names @ ..] => names.iter().map(|n| PathBuf::from(dir).join(n)).collect(),
    }
}

#[cfg(not(target_os = "windows"))]
fn open_dialog() -> Vec<PathBuf> {
    let title = en_pt("Attach files", "Anexar arquivos");
    // macOS: the system's own picker through AppleScript, one POSIX path per line.
    #[cfg(target_os = "macos")]
    let prompt = format!("set fs to choose file with prompt \"{}\" with multiple selections allowed", title);
    #[cfg(target_os = "macos")]
    let tries: Vec<(&str, Vec<&str>)> = vec![(
        "osascript",
        vec!["-e", prompt.as_str(), "-e", "set out to \"\"", "-e", "repeat with f in fs", "-e",
             "set out to out & POSIX path of f & linefeed", "-e", "end repeat", "-e", "return out"],
    )];
    #[cfg(not(target_os = "macos"))]
    let tries: Vec<(&str, Vec<&str>)> = vec![
        ("zenity", vec!["--file-selection", "--multiple", "--separator=\n", "--title", title]),
        ("kdialog", vec!["--getopenfilename", "--multiple", "--separate-output", ".", "--title", title]),
    ];
    for (cmd, args) in tries {
        match std::process::Command::new(cmd).args(&args).output() {
            Ok(out) => {
                return String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .map(PathBuf::from)
                    .collect();
            }
            Err(_) => continue,
        }
    }
    log::warn!("Nenhum seletor de arquivos encontrado (zenity ou kdialog)");
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn parses_one_and_many_files() {
        let one = w("C:\\a\\b.txt\0\0");
        assert_eq!(parse_multi_select(&one), vec![PathBuf::from("C:\\a\\b.txt")]);
        let many = w("C:\\dir\0x.mp4\0y.pdf\0\0");
        assert_eq!(
            parse_multi_select(&many),
            vec![PathBuf::from("C:\\dir").join("x.mp4"), PathBuf::from("C:\\dir").join("y.pdf")]
        );
        assert!(parse_multi_select(&w("\0\0")).is_empty());
    }

    #[test]
    fn kinds_and_spoiler_names() {
        assert_eq!(kind_of("a.MP3"), "audio");
        assert_eq!(kind_of("b.mp4"), "video");
        assert_eq!(kind_of("c.png"), "image");
        assert_eq!(kind_of("d.pdf"), "file");
        let dir = std::env::temp_dir().join("litecord_upload_test.txt");
        std::fs::write(&dir, b"x").unwrap();
        pending().clear();
        pending().push(Pending { path: dir.clone(), spoiler: true, name: None });
        pending().push(Pending { path: dir.clone(), spoiler: false, name: Some("novo nome.txt".into()) });
        let out = take();
        assert_eq!(out[0].1, "SPOILER_litecord_upload_test.txt");
        assert_eq!(out[1].1, "novo nome.txt");
        let _ = std::fs::remove_file(dir);
    }
}
