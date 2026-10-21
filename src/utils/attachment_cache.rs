use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use log::warn;
use slint::Model;

#[derive(Clone)]
struct DecodedImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

const MAX_FULL_CACHE_BYTES: usize = 36 * 1024 * 1024; // 36 MB budget for decoded image textures

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
        let size_bytes = image.rgba.len();

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
                let pixel_buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                    &dec.rgba,
                    dec.width,
                    dec.height,
                );
                return Some(slint::Image::from_rgba8(pixel_buffer));
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
                let pixel_buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                    &dec.rgba,
                    dec.width,
                    dec.height,
                );
                return Some(slint::Image::from_rgba8(pixel_buffer));
            }
        }

        // 2. On disk in %TEMP%
        let safe_name = sanitize_filename(filename);
        let file_path = self.temp_dir.join(format!("{}_{}", att_id, safe_name));
        if file_path.exists() {
            if let Ok(bytes) = std::fs::read(&file_path) {
                if let Some(dec) = decode_bytes_to_rgba(&bytes) {
                    let pixel_buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                        &dec.rgba,
                        dec.width,
                        dec.height,
                    );
                    if let Ok(mut guard) = self.full_cache.lock() {
                        guard.insert(att_id.to_string(), dec);
                    }
                    return Some(slint::Image::from_rgba8(pixel_buffer));
                }
            }
        }

        None
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
            let builder = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(4));
            let client = crate::utils::apply_proxy_to_builder(builder).build();

            if let Ok(client) = client {
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

                                let id_clone = id_str.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(ui) = app_weak.upgrade() {
                                        let current_msgs: Vec<crate::ChatMessage> = ui.get_messages().iter().collect();
                                        let mut updated_msgs = Vec::with_capacity(current_msgs.len());
                                        let mut changed = false;

                                        for mut msg in current_msgs {
                                            let current_atts: Vec<crate::MessageAttachment> = msg.attachments.iter().collect();
                                            let mut updated_atts = Vec::with_capacity(current_atts.len());
                                            for mut att in current_atts {
                                                if att.id == id_clone.as_str() && !att.is_downloaded {
                                                    if let Some(img) = cache_arc.get_preview(&id_clone) {
                                                        att.preview_img = img;
                                                        changed = true;
                                                    }
                                                }
                                                updated_atts.push(att);
                                            }
                                            if changed {
                                                msg.attachments = slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(updated_atts)));
                                            }
                                            updated_msgs.push(msg);
                                        }

                                        if changed {
                                            ui.set_messages(slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(updated_msgs))));
                                        }
                                    }
                                });
                            }
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

        let cache_arc = get_attachment_cache();
        let id_str = att_id.to_string();
        let filename_str = filename.to_string();
        let safe_name = sanitize_filename(filename);
        let file_path = self.temp_dir.join(format!("{}_{}", att_id, safe_name));
        let url_str = url.to_string();

        // Mark loading state in UI
        let id_loading = id_str.clone();
        let app_w_load = app_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = app_w_load.upgrade() {
                let current_msgs: Vec<crate::ChatMessage> = ui.get_messages().iter().collect();
                let mut updated_msgs = Vec::with_capacity(current_msgs.len());
                for mut msg in current_msgs {
                    let current_atts: Vec<crate::MessageAttachment> = msg.attachments.iter().collect();
                    let mut updated_atts = Vec::with_capacity(current_atts.len());
                    for mut att in current_atts {
                        if att.id == id_loading.as_str() {
                            att.is_loading = true;
                        }
                        updated_atts.push(att);
                    }
                    msg.attachments = slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(updated_atts)));
                    updated_msgs.push(msg);
                }
                ui.set_messages(slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(updated_msgs))));
            }
        });

        tokio::spawn(async move {
            let builder = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30));
            let client = crate::utils::apply_proxy_to_builder(builder).build();

            if let Ok(client) = client {
                if let Ok(resp) = client.get(&url_str).send().await {
                    if resp.status().is_success() {
                        if let Ok(bytes) = resp.bytes().await {
                            // Save to %TEMP%
                            let _ = std::fs::write(&file_path, &bytes);

                            if let Some(dec) = decode_bytes_to_rgba(&bytes) {
                                let img_w = dec.width as i32;
                                let img_h = dec.height as i32;

                                if let Ok(mut guard) = cache_arc.full_cache.lock() {
                                    guard.insert(id_str.clone(), dec);
                                }

                                let id_done = id_str.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(ui) = app_weak.upgrade() {
                                        let current_msgs: Vec<crate::ChatMessage> = ui.get_messages().iter().collect();
                                        let mut updated_msgs = Vec::with_capacity(current_msgs.len());
                                        for mut msg in current_msgs {
                                            let current_atts: Vec<crate::MessageAttachment> = msg.attachments.iter().collect();
                                            let mut updated_atts = Vec::with_capacity(current_atts.len());
                                            for mut att in current_atts {
                                                if att.id == id_done.as_str() {
                                                    att.is_loading = false;
                                                    att.is_downloaded = true;
                                                    if att.width <= 0 || att.height <= 0 {
                                                        att.width = img_w;
                                                        att.height = img_h;
                                                    }
                                                    if let Some(img) = cache_arc.get_full(&id_done, &filename_str) {
                                                        att.full_img = img;
                                                    }
                                                }
                                                updated_atts.push(att);
                                            }
                                            msg.attachments = slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(updated_atts)));
                                            updated_msgs.push(msg);
                                        }
                                        ui.set_messages(slint::ModelRc::from(std::rc::Rc::new(slint::VecModel::from(updated_msgs))));
                                    }
                                });
                            }
                        }
                    }
                }
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
        let fitted = if w > 1280 || h > 1280 {
            dyn_img.thumbnail(1280, 1280)
        } else {
            dyn_img
        };
        let rgba = fitted.to_rgba8();
        let (width, height) = rgba.dimensions();
        Some(DecodedImage {
            width,
            height,
            rgba: rgba.into_raw(),
        })
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
        Some(DecodedImage {
            width,
            height,
            rgba: rgba.into_raw(),
        })
    } else {
        None
    }
}
