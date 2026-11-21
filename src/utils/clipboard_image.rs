//! Reads an image (screenshot or copied image file) from the system clipboard.

use std::path::Path;

fn mime_for_ext(ext: &str) -> Option<(&'static str, &'static str)> {
    match ext.to_ascii_lowercase().as_str() {
        "png" => Some(("image.png", "image/png")),
        "jpg" | "jpeg" => Some(("image.jpg", "image/jpeg")),
        "webp" => Some(("image.webp", "image/webp")),
        "gif" => Some(("image.gif", "image/gif")),
        _ => None,
    }
}

fn try_read_image_path(raw: &str) -> Option<(String, String, Vec<u8>)> {
    let first_line = raw.lines().next()?.trim();
    let path_str = first_line.strip_prefix("file://").unwrap_or(first_line);
    let path = Path::new(path_str);
    if !path.is_file() {
        return None;
    }
    let ext = path.extension()?.to_str()?;
    let (_, mime) = mime_for_ext(ext)?;
    let bytes = std::fs::read(path).ok()?;
    if bytes.is_empty() || bytes.len() > 25 * 1024 * 1024 {
        return None;
    }
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image.png")
        .to_string();
    Some((filename, mime.to_string(), bytes))
}

#[cfg(target_os = "linux")]
fn read_linux_clipboard_image() -> Option<(String, String, Vec<u8>)> {
    use std::process::Command;

    if let Ok(types_out) = Command::new("wl-paste").arg("--list-types").output() {
        if types_out.status.success() {
            let types = String::from_utf8_lossy(&types_out.stdout);
            for (mime, filename) in [
                ("image/png", "screenshot.png"),
                ("image/jpeg", "image.jpg"),
                ("image/webp", "image.webp"),
                ("image/gif", "image.gif"),
            ] {
                if types.lines().any(|t| t.trim() == mime) {
                    if let Ok(img_out) = Command::new("wl-paste")
                        .args(["--no-newline", "--type", mime])
                        .output()
                    {
                        if img_out.status.success() && !img_out.stdout.is_empty() {
                            return Some((filename.to_string(), mime.to_string(), img_out.stdout));
                        }
                    }
                }
            }
            if types.lines().any(|t| t.trim() == "text/uri-list") {
                if let Ok(uri_out) = Command::new("wl-paste")
                    .args(["--no-newline", "--type", "text/uri-list"])
                    .output()
                {
                    if uri_out.status.success() {
                        if let Some(found) = try_read_image_path(&String::from_utf8_lossy(&uri_out.stdout)) {
                            return Some(found);
                        }
                    }
                }
            }
        }
    }

    if let Ok(img_out) = Command::new("xclip")
        .args(["-selection", "clipboard", "-t", "image/png", "-o"])
        .output()
    {
        if img_out.status.success() && !img_out.stdout.is_empty() {
            return Some(("screenshot.png".to_string(), "image/png".to_string(), img_out.stdout));
        }
    }

    None
}

#[cfg(target_os = "windows")]
fn read_windows_clipboard_image() -> Option<(String, String, Vec<u8>)> {
    use std::process::Command;

    let tmp_path = std::env::temp_dir().join(format!("litecord_clip_{}.png", std::process::id()));
    let tmp_str = tmp_path.to_string_lossy().replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         if ([System.Windows.Forms.Clipboard]::ContainsImage()) {{ \
           $img = [System.Windows.Forms.Clipboard]::GetImage(); \
           $img.Save('{}', [System.Drawing.Imaging.ImageFormat]::Png); \
         }}",
        tmp_str
    );
    let ok = Command::new("powershell")
        .args(["-NoProfile", "-Sta", "-NonInteractive", "-Command", &script])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok && tmp_path.is_file() {
        let bytes = std::fs::read(&tmp_path).ok();
        let _ = std::fs::remove_file(&tmp_path);
        if let Some(b) = bytes.filter(|b| !b.is_empty()) {
            return Some(("screenshot.png".to_string(), "image/png".to_string(), b));
        }
    }
    None
}

/// Reads an image from the OS clipboard (PNG/JPEG/WebP/GIF or a copied image file path).
pub fn read_clipboard_image() -> Option<(String, String, Vec<u8>)> {
    #[cfg(target_os = "linux")]
    if let Some(img) = read_linux_clipboard_image() {
        return Some(img);
    }
    #[cfg(target_os = "windows")]
    if let Some(img) = read_windows_clipboard_image() {
        return Some(img);
    }
    if let Ok(mut ctx) = copypasta::ClipboardContext::new() {
        if let Ok(text) = copypasta::ClipboardProvider::get_contents(&mut ctx) {
            if let Some(img) = try_read_image_path(&text) {
                return Some(img);
            }
        }
    }
    None
}
