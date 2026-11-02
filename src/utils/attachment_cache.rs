use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use log::warn;
use slint::Model;

#[derive(Clone)]
struct DecodedImage {
    buffer: slint::SharedPixelBuffer<slint::Rgba8Pixel>,
}

const MAX_FULL_CACHE_BYTES: usize = 16 * 1024 * 1024; // 16 MB budget for decoded image textures

#[derive(Clone)]
struct CacheEntry {
    image: DecodedImage,
    last_access: u64,
    size_bytes: usize,
}

#[derive(Default)]
struct MemoryFullStore {
    entries: HashMap<String, CacheEntry>,
    total_bytes: usize,
    clock: u64,
}

impl MemoryFullStore {
    fn new() -> Self {
        Self {
            entries: HashMap::with_capacity(32),
            total_bytes: 0,
            clock: 0,
        }
    }

    fn get(&mut self, key: &str) -> Option<&DecodedImage> {
        self.clock = self.clock.wrapping_add(1);
        let clock = self.clock;
        if let Some(entry) = self.entries.get_mut(key) {
            entry.last_access = clock;
            Some(&entry.image)
        } else {
            None
        }
    }

    fn insert(&mut self, key: String, image: DecodedImage) {
        self.clock = self.clock.wrapping_add(1);
        let size_bytes = (image.buffer.width() * image.buffer.height() * 4) as usize;

        // Evict least-recently-used images until we are within budget
        while self.total_bytes + size_bytes > MAX_FULL_CACHE_BYTES && !self.entries.is_empty() {
            if let Some((oldest_key, _)) = self.entries.iter().min_by_key(|(_, v)| v.last_access).map(|(k, v)| (k.clone(), v.last_access)) {
                if let Some(removed) = self.entries.remove(&oldest_key) {
                    self.total_bytes = self.total_bytes.saturating_sub(removed.size_bytes);
                }
            } else {
                break;
            }
        }

        if let Some(old) = self.entries.insert(key, CacheEntry {
            image,
            last_access: self.clock,
            size_bytes,
        }) {
            self.total_bytes = self.total_bytes.saturating_sub(old.size_bytes);
        }
        self.total_bytes += size_bytes;
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.total_bytes = 0;
    }
}

pub struct AttachmentCache {
    preview_cache: Mutex<HashMap<String, DecodedImage>>,
    full_cache: Mutex<MemoryFullStore>,
    temp_dir: PathBuf,
}

static ATTACHMENT_CACHE: OnceLock<Arc<AttachmentCache>> = OnceLock::new();

fn pending_attachment_updates() -> &'static Mutex<HashSet<String>> {
    static PENDING: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashSet::new()))
}

static ATTACHMENT_REFRESH_SCHEDULED: AtomicBool = AtomicBool::new(false);

fn downloads_in_flight() -> &'static Mutex<HashSet<String>> {
    static IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()))
}

/// One UI refresh per batch of finished downloads. Rebuilding the whole chat model
/// for every single image made image-heavy channels stutter and jump to the top.
fn queue_attachment_update(att_id: &str, app_weak: slint::Weak<crate::AppWindow>) {
    if att_id.is_empty() {
        return;
    }
    if let Ok(mut pending) = pending_attachment_updates().lock() {
        pending.insert(att_id.to_string());
    }
    if ATTACHMENT_REFRESH_SCHEDULED.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = slint::invoke_from_event_loop(move || {
        slint::Timer::single_shot(std::time::Duration::from_millis(120), move || {
            ATTACHMENT_REFRESH_SCHEDULED.store(false, Ordering::SeqCst);
            flush_attachment_updates(app_weak);
        });
    });
}

fn flush_attachment_updates(app_weak: slint::Weak<crate::AppWindow>) {
    let ids = match pending_attachment_updates().lock() {
        Ok(mut pending) => std::mem::take(&mut *pending),
        Err(_) => return,
    };
    if ids.is_empty() {
        return;
    }
    let Some(ui) = app_weak.upgrade() else {
        return;
    };
    let cache = get_attachment_cache();
    let messages = ui.get_messages();
    let row_count = messages.row_count();

    for row_idx in 0..row_count {
        if let Some(mut message) = messages.row_data(row_idx) {
            let attachments: Vec<crate::MessageAttachment> = message.attachments.iter().collect();
            let mut changed = false;
            let mut rewritten = Vec::with_capacity(attachments.len());
            for mut attachment in attachments {
                if ids.contains(attachment.id.as_str()) {
                    if let Some(image) = cache.get_full_for_ui(&attachment.id, &attachment.filename) {
                        let size = image.size();
                        attachment.full_img = image;
                        attachment.is_downloaded = true;
                        attachment.is_loading = false;
                        if attachment.width <= 0 {
                            attachment.width = size.width as i32;
                        }
                        if attachment.height <= 0 {
                            attachment.height = size.height as i32;
                        }
                        changed = true;
                    } else if let Some(preview) = cache.get_preview(&attachment.id) {
                        attachment.preview_img = preview;
                        changed = true;
                    }
                }
                rewritten.push(attachment);
            }
            if changed {
                message.attachments = slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(rewritten)));
                messages.set_row_data(row_idx, message);
            }
        }
    }
}

const UI_IMAGE_CACHE_BYTES: usize = 12 * 1024 * 1024;

#[derive(Default)]
struct UiImageCache {
    entries: std::collections::HashMap<String, (slint::Image, usize, u64)>,
    clock: u64,
    bytes: usize,
}

thread_local! {
    // Only touched from the Slint event loop: `slint::Image` is not Send.
    static UI_IMAGES: std::cell::RefCell<UiImageCache> =
        std::cell::RefCell::new(UiImageCache::default());
}

fn ui_image_get(att_id: &str) -> Option<slint::Image> {
    UI_IMAGES.with(|cell| {
        let mut cache = cell.borrow_mut();
        cache.clock = cache.clock.wrapping_add(1);
        let clock = cache.clock;
        cache.entries.get_mut(att_id).map(|(image, _, last)| {
            *last = clock;
            image.clone()
        })
    })
}

fn ui_image_put(att_id: &str, image: slint::Image) {
    let size = image.size();
    let bytes = (size.width as usize) * (size.height as usize) * 4;
    UI_IMAGES.with(|cell| {
        let mut cache = cell.borrow_mut();
        cache.clock = cache.clock.wrapping_add(1);
        let clock = cache.clock;
        if let Some((_, old_bytes, _)) = cache.entries.insert(att_id.to_string(), (image, bytes, clock)) {
            cache.bytes = cache.bytes.saturating_sub(old_bytes);
        }
        cache.bytes += bytes;
        while cache.bytes > UI_IMAGE_CACHE_BYTES && cache.entries.len() > 1 {
            let Some(oldest) = cache
                .entries
                .iter()
                .min_by_key(|(_, (_, _, last))| *last)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some((_, removed_bytes, _)) = cache.entries.remove(&oldest) {
                cache.bytes = cache.bytes.saturating_sub(removed_bytes);
            }
        }
    });
}

pub fn get_attachment_cache() -> Arc<AttachmentCache> {
    ATTACHMENT_CACHE.get_or_init(|| {
        let mut temp_dir = std::env::temp_dir();
        temp_dir.push("Litecord");
        temp_dir.push("temp_images");

        if let Err(e) = std::fs::create_dir_all(&temp_dir) {
            warn!("Não foi possível criar diretório temporário de imagens: {}", e);
        }

        Arc::new(AttachmentCache {
            preview_cache: Mutex::new(HashMap::with_capacity(64)),
            full_cache: Mutex::new(MemoryFullStore::new()),
            temp_dir,
        })
    }).clone()
}

pub fn cleanup_temp_attachments() {
    let mut temp_dir = std::env::temp_dir();
    temp_dir.push("Litecord");
    temp_dir.push("temp_images");
    if temp_dir.exists() {
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::create_dir_all(&temp_dir);
    }
}

pub fn sanitize_filename(filename: &str) -> String {
    let clean = std::path::Path::new(filename)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("attachment.bin")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .collect::<String>();
    if clean.is_empty() { "attachment.bin".to_string() } else { clean }
}

impl AttachmentCache {
    /// Check if preview is in memory or disk
    pub fn get_preview(&self, att_id: &str) -> Option<slint::Image> {
        if att_id.is_empty() {
            return None;
        }

        if let Ok(guard) = self.preview_cache.lock() {
            if let Some(dec) = guard.get(att_id) {
                return Some(slint::Image::from_rgba8(dec.buffer.clone()));
            }
        }
        None
    }

    /// Check if full image is in memory or saved on disk
    pub fn get_full(&self, att_id: &str, filename: &str) -> Option<slint::Image> {
        if att_id.is_empty() {
            return None;
        }

        // 1. In memory
        if let Ok(mut guard) = self.full_cache.lock() {
            if let Some(dec) = guard.get(att_id) {
                return Some(slint::Image::from_rgba8(dec.buffer.clone()));
            }
        }

        // 2. On disk in %TEMP%
        let safe_name = sanitize_filename(filename);
        let file_path = self.temp_dir.join(format!("{}_{}", att_id, safe_name));
        if file_path.exists() {
            if let Ok(bytes) = std::fs::read(&file_path) {
                if let Some(dec) = decode_bytes_to_rgba(&bytes) {
                    let image = slint::Image::from_rgba8(dec.buffer.clone());
                    if let Ok(mut guard) = self.full_cache.lock() {
                        guard.insert(att_id.to_string(), dec);
                    }
                    return Some(image);
                }
            }
        }

        None
    }

    /// `get_full` that reuses the Slint handle on the UI thread.
    ///
    /// The model is rebuilt every time a batch of downloads finishes; creating a
    /// new texture for each attachment on every rebuild made image-heavy channels
    /// allocate hundreds of MB of throwaway textures.
    pub fn get_full_for_ui(&self, att_id: &str, filename: &str) -> Option<slint::Image> {
        if let Some(image) = ui_image_get(att_id) {
            return Some(image);
        }
        let image = self.get_full(att_id, filename)?;
        ui_image_put(att_id, image.clone());
        Some(image)
    }

    /// Clear all cached in-memory decoded textures to reclaim RAM
    pub fn clear_memory(&self) {
        if let Ok(mut guard) = self.preview_cache.lock() {
            guard.clear();
        }
        if let Ok(mut guard) = self.full_cache.lock() {
            guard.clear();
        }
    }

    /// Fetch ultra-low-res thumbnail in background (~500 bytes) for pixel-art placeholder
    pub fn fetch_preview_async(&self, att_id: &str, url: &str, app_weak: slint::Weak<crate::AppWindow>) {
        if att_id.is_empty() || url.is_empty() {
            return;
        }

        if let Ok(guard) = self.preview_cache.lock() {
            if guard.contains_key(att_id) {
                return;
            }
        }

        let cache_arc = get_attachment_cache();
        let id_str = att_id.to_string();
        let thumb_url = if url.contains('?') {
            format!("{}&width=32&height=32", url)
        } else {
            format!("{}?width=32&height=32", url)
        };

        tokio::spawn(async move {
            let client = crate::utils::network_settings::shared_http_client();
            if let Ok(resp) = client.get(&thumb_url).send().await {
                if resp.status().is_success() {
                    if let Ok(bytes) = resp.bytes().await {
                        if let Some(dec) = decode_bytes_to_minecraft_pixel_art(&bytes) {
                            if let Ok(mut guard) = cache_arc.preview_cache.lock() {
                                if guard.len() >= 30 {
                                    guard.clear();
                                }
                                guard.insert(id_str.clone(), dec);
                            }

                            queue_attachment_update(&id_str, app_weak.clone());
                        }
                    }
                }
            }
        });
    }

    /// On-demand downloader: downloads full crisp image to %TEMP%/Litecord/temp_images
    pub fn download_full_async(&self, att_id: &str, filename: &str, url: &str, app_weak: slint::Weak<crate::AppWindow>) {
        if att_id.is_empty() || url.is_empty() {
            return;
        }

        // The chat model is rebuilt while downloads run; without this guard the same
        // picture would be requested again on every rebuild.
        match downloads_in_flight().lock() {
            Ok(mut in_flight) => {
                if !in_flight.insert(att_id.to_string()) {
                    return;
                }
            }
            Err(_) => return,
        }

        let cache_arc = get_attachment_cache();
        let id_str = att_id.to_string();
        let safe_name = sanitize_filename(filename);
        let file_path = self.temp_dir.join(format!("{}_{}", att_id, safe_name));
        let url_str = url.to_string();

        tokio::spawn(async move {
            let client = crate::utils::network_settings::shared_http_client();
            if let Ok(resp) = client.get(&url_str).send().await {
                if resp.status().is_success() {
                    if let Ok(bytes) = resp.bytes().await {
                        // Save to %TEMP%
                        let _ = std::fs::write(&file_path, &bytes);

                        if let Some(dec) = decode_bytes_to_rgba(&bytes) {
                            if let Ok(mut guard) = cache_arc.full_cache.lock() {
                                guard.insert(id_str.clone(), dec);
                            }
                            queue_attachment_update(&id_str, app_weak.clone());
                        }
                    }
                }
            }
            if let Ok(mut in_flight) = downloads_in_flight().lock() {
                in_flight.remove(&id_str);
            }
        });
    }
}

fn decode_bytes_to_rgba(bytes: &[u8]) -> Option<DecodedImage> {
    if let Ok(dyn_img) = image::load_from_memory(bytes) {
        let (w, h) = (dyn_img.width(), dyn_img.height());
        // Guard against decompression bombs / excessive dimensions causing OOM
        if w > 8192 || h > 8192 || (w as u64 * h as u64) > 25_000_000 {
            log::warn!("Anexo de imagem com dimensões excessivas rejeitado por segurança: {}x{}", w, h);
            return None;
        }
        let fitted = if w > 1024 || h > 1024 {
            dyn_img.thumbnail(1024, 1024)
        } else {
            dyn_img
        };
        let rgba = fitted.to_rgba8();
        let (width, height) = rgba.dimensions();
        let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            rgba.as_raw(),
            width,
            height,
        );
        Some(DecodedImage { buffer })
    } else {
        None
    }
}

fn decode_bytes_to_minecraft_pixel_art(bytes: &[u8]) -> Option<DecodedImage> {
    if let Ok(dyn_img) = image::load_from_memory(bytes) {
        // Downscale to chunky retro grid (28 x 16 blocks)
        let block_w = 28;
        let block_h = 16;
        let small = dyn_img.resize_exact(block_w, block_h, image::imageops::FilterType::Nearest);
        
        // Upscale using Nearest Neighbor (4x -> 112 x 64) for sharp retro pixel art with minimal RAM footprint
        let pixel_scale = 4;
        let chunky = small.resize_exact(block_w * pixel_scale, block_h * pixel_scale, image::imageops::FilterType::Nearest);
        
        let rgba = chunky.to_rgba8();
        let (width, height) = rgba.dimensions();
        let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            rgba.as_raw(),
            width,
            height,
        );
        Some(DecodedImage { buffer })
    } else {
        None
    }
}
