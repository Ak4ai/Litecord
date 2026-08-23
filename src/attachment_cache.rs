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

pub struct AttachmentCache {
    preview_cache: Mutex<HashMap<String, DecodedImage>>,
    full_cache: Mutex<HashMap<String, DecodedImage>>,
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
            full_cache: Mutex::new(HashMap::with_capacity(32)),
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
        if let Ok(guard) = self.full_cache.lock() {
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
        let file_path = self.temp_dir.join(format!("{}_{}", att_id, filename));
        if file_path.exists() {
            if let Ok(bytes) = std::fs::read(&file_path) {
                if let Some(dec) = decode_bytes_to_rgba(&bytes) {
                    let pixel_buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                        &dec.rgba,
                        dec.width,
                        dec.height,
                    );
                    if let Ok(mut guard) = self.full_cache.lock() {
                        if guard.len() >= 30 {
                            guard.clear();
                        }
                        guard.insert(att_id.to_string(), dec);
                    }
                    return Some(slint::Image::from_rgba8(pixel_buffer));
                }
            }
        }

        None
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
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(4))
                .build();

            if let Ok(client) = client {
                if let Ok(resp) = client.get(&thumb_url).send().await {
                    if resp.status().is_success() {
                        if let Ok(bytes) = resp.bytes().await {
                            if let Some(dec) = decode_bytes_to_minecraft_pixel_art(&bytes) {
                                if let Ok(mut guard) = cache_arc.preview_cache.lock() {
                                    if guard.len() >= 100 {
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
        let file_path = self.temp_dir.join(format!("{}_{}", att_id, filename));
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
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build();

            if let Ok(client) = client {
                if let Ok(resp) = client.get(&url_str).send().await {
                    if resp.status().is_success() {
                        if let Ok(bytes) = resp.bytes().await {
                            // Save to %TEMP%
                            let _ = std::fs::write(&file_path, &bytes);

                            if let Some(dec) = decode_bytes_to_rgba(&bytes) {
                                if let Ok(mut guard) = cache_arc.full_cache.lock() {
                                    if guard.len() >= 30 {
                                        guard.clear();
                                    }
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
        let rgba = dyn_img.to_rgba8();
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
        
        // Upscale using Nearest Neighbor (10x -> 280 x 160) so each block is a sharp solid square
        let pixel_scale = 10;
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
