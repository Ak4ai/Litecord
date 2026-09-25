use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};
use slint::Model;
use tokio::sync::Semaphore;

use crate::{AppWindow, ChatMessage};

const AVATAR_SIZE: u32 = 40;
const AVATAR_BYTES: usize = (AVATAR_SIZE * AVATAR_SIZE * 4) as usize;
const MAX_DOWNLOAD_BYTES: usize = 512 * 1024;
// Decoded thumbnails use about 1.6 MiB in RAM and 25 MiB on disk at these limits.
const MAX_MEMORY_AVATARS: usize = 256;
const MAX_DISK_AVATARS: usize = 4096;
const RETRY_AFTER: Duration = Duration::from_secs(30);

struct MemoryAvatar {
    rgba: Vec<u8>,
    last_used: u64,
}

#[derive(Default)]
struct MemoryStore {
    entries: HashMap<String, MemoryAvatar>,
    clock: u64,
}

impl MemoryStore {
    fn get(&mut self, url: &str) -> Option<slint::Image> {
        self.clock = self.clock.wrapping_add(1);
        let avatar = self.entries.get_mut(url)?;
        avatar.last_used = self.clock;
        let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            &avatar.rgba,
            AVATAR_SIZE,
            AVATAR_SIZE,
        );
        Some(slint::Image::from_rgba8(buffer))
    }

    fn insert(&mut self, url: String, rgba: Vec<u8>) {
        self.clock = self.clock.wrapping_add(1);
        if !self.entries.contains_key(&url) && self.entries.len() >= MAX_MEMORY_AVATARS {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, avatar)| avatar.last_used)
                .map(|(key, _)| key.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(
            url,
            MemoryAvatar {
                rgba,
                last_used: self.clock,
            },
        );
    }
}

pub struct AvatarCache {
    images: Mutex<MemoryStore>,
    pending: Mutex<HashSet<String>>,
    failed_until: Mutex<HashMap<String, Instant>>,
    requests: Arc<Semaphore>,
    disk_dir: PathBuf,
}

static AVATAR_CACHE: OnceLock<Arc<AvatarCache>> = OnceLock::new();

pub fn get_avatar_cache() -> Arc<AvatarCache> {
    AVATAR_CACHE
        .get_or_init(|| {
            Arc::new(AvatarCache {
                images: Mutex::new(MemoryStore::default()),
                pending: Mutex::new(HashSet::new()),
                failed_until: Mutex::new(HashMap::new()),
                requests: Arc::new(Semaphore::new(4)),
                disk_dir: avatar_disk_dir(),
            })
        })
        .clone()
}

fn avatar_disk_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("Litecord").join("cache").join("avatars")
}

fn avatar_disk_path(dir: &Path, url: &str) -> PathBuf {
    let key = format!("{:x}", Sha256::digest(url.as_bytes()));
    dir.join(format!("{key}.rgba"))
}

fn load_disk_avatar(path: &Path) -> Option<Vec<u8>> {
    let rgba = std::fs::read(path).ok()?;
    if rgba.len() != AVATAR_BYTES {
        let _ = std::fs::remove_file(path);
        return None;
    }
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = file.set_times(std::fs::FileTimes::new().set_modified(SystemTime::now()));
    }
    Some(rgba)
}

fn save_disk_avatar(dir: &Path, url: &str, rgba: &[u8]) {
    if rgba.len() != AVATAR_BYTES || std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let path = avatar_disk_path(dir, url);
    let temporary = path.with_extension(format!("rgba.{}.tmp", std::process::id()));
    if std::fs::write(&temporary, rgba).is_err() {
        let _ = std::fs::remove_file(&temporary);
        return;
    }
    if std::fs::rename(&temporary, &path).is_ok() {
        prune_disk_avatars(dir, MAX_DISK_AVATARS);
    } else {
        let _ = std::fs::remove_file(&temporary);
    }
}

fn prune_disk_avatars(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            let hash = name.strip_suffix(".rgba")?;
            if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            Some((
                entry.path(),
                metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            ))
        })
        .collect();
    if files.len() > keep {
        files.sort_by_key(|(_, modified)| *modified);
        let excess = files.len() - keep;
        for (path, _) in files.into_iter().take(excess) {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn decode_avatar(bytes: &[u8]) -> Option<Vec<u8>> {
    let decoded = image::load_from_memory(bytes).ok()?;
    if decoded.width() > 1024 || decoded.height() > 1024 {
        return None;
    }
    Some(
        decoded
            .resize_to_fill(
                AVATAR_SIZE,
                AVATAR_SIZE,
                image::imageops::FilterType::Triangle,
            )
            .to_rgba8()
            .into_raw(),
    )
}

impl AvatarCache {
    pub fn hydrate(&self, messages: &mut [ChatMessage], app: &slint::Weak<AppWindow>) {
        for message in messages {
            if !message.show_header
                || message.author_id.is_empty()
                || message.avatar_url.is_empty()
                || message.avatar_img.size().width > 0
            {
                continue;
            }
            let url = message.avatar_url.as_str();
            if let Some(image) = self.get(url) {
                message.avatar_img = image;
            } else {
                self.fetch(url, app.clone());
            }
        }
    }

    fn get(&self, url: &str) -> Option<slint::Image> {
        self.images.lock().ok()?.get(url)
    }

    fn fetch(&self, url: &str, app: slint::Weak<AppWindow>) {
        if self
            .failed_until
            .lock()
            .is_ok_and(|failed| failed.get(url).is_some_and(|until| *until > Instant::now()))
        {
            return;
        }
        if let Ok(mut pending) = self.pending.lock() {
            if !pending.insert(url.to_owned()) {
                return;
            }
        } else {
            return;
        }

        let url = url.to_owned();
        let cache = get_avatar_cache();
        tokio::spawn(async move {
            let _permit = match cache.requests.acquire().await {
                Ok(permit) => permit,
                Err(_) => return,
            };
            let disk_path = avatar_disk_path(&cache.disk_dir, &url);
            let mut pixels = tokio::task::spawn_blocking(move || load_disk_avatar(&disk_path))
                .await
                .ok()
                .flatten();
            let mut downloaded = false;

            if pixels.is_none() {
                let client = crate::utils::apply_proxy_to_builder(
                    reqwest::Client::builder().timeout(Duration::from_secs(6)),
                )
                .build();
                if let Ok(client) = client {
                    if let Ok(mut response) = client.get(&url).send().await {
                        if response.status().is_success()
                            && response.content_length().unwrap_or(0) <= MAX_DOWNLOAD_BYTES as u64
                        {
                            let mut bytes = Vec::new();
                            let mut valid = true;
                            loop {
                                match response.chunk().await {
                                    Ok(Some(chunk))
                                        if bytes.len() + chunk.len() <= MAX_DOWNLOAD_BYTES =>
                                    {
                                        bytes.extend_from_slice(&chunk);
                                    }
                                    Ok(None) => break,
                                    _ => {
                                        valid = false;
                                        break;
                                    }
                                }
                            }
                            if valid {
                                pixels = tokio::task::spawn_blocking(move || decode_avatar(&bytes))
                                    .await
                                    .ok()
                                    .flatten();
                                downloaded = pixels.is_some();
                            }
                        }
                    }
                }
            }

            if let Some(rgba) = pixels {
                let disk_copy = if downloaded { Some(rgba.clone()) } else { None };
                if let Ok(mut images) = cache.images.lock() {
                    images.insert(url.clone(), rgba);
                }
                let ui_cache = cache.clone();
                let ui_url = url.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let (Some(ui), Some(image)) = (app.upgrade(), ui_cache.get(&ui_url)) {
                        let messages = ui.get_messages();
                        for index in 0..messages.row_count() {
                            if let Some(mut message) = messages.row_data(index) {
                                if message.show_header
                                    && message.avatar_url == ui_url.as_str()
                                    && message.avatar_img.size().width == 0
                                {
                                    message.avatar_img = image.clone();
                                    messages.set_row_data(index, message);
                                }
                            }
                        }
                    }
                });
                if let Some(disk_copy) = disk_copy {
                    let dir = cache.disk_dir.clone();
                    let save_url = url.clone();
                    tokio::task::spawn_blocking(move || {
                        save_disk_avatar(&dir, &save_url, &disk_copy);
                    });
                }
                if let Ok(mut failed) = cache.failed_until.lock() {
                    failed.remove(&url);
                }
            } else if let Ok(mut failed) = cache.failed_until.lock() {
                failed.retain(|_, until| *until > Instant::now());
                if failed.len() >= 256 {
                    failed.clear();
                }
                failed.insert(url.clone(), Instant::now() + RETRY_AFTER);
            }
            if let Ok(mut pending) = cache.pending.lock() {
                pending.remove(&url);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_names_are_stable_and_do_not_embed_urls() {
        let dir = Path::new("avatars");
        let url = "https://cdn.discordapp.com/avatars/42/abc.webp?size=64";
        let path = avatar_disk_path(dir, url);
        assert_eq!(path.parent(), Some(dir));
        assert_eq!(path.file_name().unwrap().to_string_lossy().len(), 69);
        assert_eq!(path, avatar_disk_path(dir, url));
    }

    #[test]
    fn disk_cache_round_trips_a_thumbnail() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "litecord-avatar-cache-test-{}-{unique}",
            std::process::id()
        ));
        let url = "https://cdn.discordapp.com/avatars/42/abc.webp?size=64";
        let rgba = vec![123; AVATAR_BYTES];
        save_disk_avatar(&dir, url, &rgba);
        assert_eq!(load_disk_avatar(&avatar_disk_path(&dir, url)), Some(rgba));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn disk_limit_removes_only_avatar_cache_files() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "litecord-avatar-prune-test-{}-{unique}",
            std::process::id()
        ));
        for index in 0..3 {
            save_disk_avatar(&dir, &format!("avatar-{index}"), &vec![0; AVATAR_BYTES]);
        }
        std::fs::write(dir.join("unrelated.txt"), b"keep").unwrap();
        prune_disk_avatars(&dir, 2);
        let avatars = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("rgba"))
            .count();
        assert_eq!(avatars, 2);
        assert!(dir.join("unrelated.txt").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn memory_cache_evicts_only_the_least_recently_used_avatar() {
        let mut store = MemoryStore::default();
        for index in 0..MAX_MEMORY_AVATARS {
            store.insert(index.to_string(), vec![0; AVATAR_BYTES]);
        }
        store.get("0");
        store.insert("new".into(), vec![0; AVATAR_BYTES]);
        assert!(store.entries.contains_key("0"));
        assert!(!store.entries.contains_key("1"));
        assert_eq!(store.entries.len(), MAX_MEMORY_AVATARS);
    }
}
