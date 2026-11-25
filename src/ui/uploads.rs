//! Files picked to send with the next message. The picker is the system's own
//! dialog (Windows common dialog; zenity or kdialog on Linux), run on a short
//! lived thread; nothing here runs while no file is being picked or sent.

use std::path::PathBuf;
use std::sync::Mutex;

use slint::ComponentHandle;

use crate::utils::i18n_ui::en_pt;

/// Discord accepts at most 10 files per message.
pub const MAX_FILES: usize = 10;

static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

fn pending() -> std::sync::MutexGuard<'static, Vec<PathBuf>> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Opens the system file picker and adds the chosen files.
pub fn pick(app_weak: slint::Weak<crate::AppWindow>) {
    let _ = std::thread::Builder::new().name("file-picker".into()).spawn(move || {
        let picked = open_dialog();
        if picked.is_empty() {
            return;
        }
        {
            let mut list = pending();
            for p in picked {
                if list.len() < MAX_FILES && !list.contains(&p) {
                    list.push(p);
                }
            }
        }
        refresh_ui(&app_weak, String::new());
    });
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

/// Takes the pending files for sending (the strip empties).
pub fn take() -> Vec<PathBuf> {
    std::mem::take(&mut *pending())
}

pub fn has_pending() -> bool {
    !pending().is_empty()
}

/// Clears the files when switching channel, like Discord does.
pub fn clear(app_weak: &slint::Weak<crate::AppWindow>) {
    pending().clear();
    refresh_ui(app_weak, String::new());
}

/// Shows the pending files and a status line (empty for none).
pub fn refresh_ui(app_weak: &slint::Weak<crate::AppWindow>, status: String) {
    let items: Vec<(String, String)> = pending()
        .iter()
        .map(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let size = std::fs::metadata(p).map(|m| format_size(m.len())).unwrap_or_default();
            (name, size)
        })
        .collect();
    let w = app_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = w.upgrade() else { return };
        let model: Vec<crate::PendingFile> = items
            .into_iter()
            .map(|(name, size)| crate::PendingFile { name: name.into(), size: size.into() })
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
    let tries: [(&str, Vec<&str>); 2] = [
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
}
