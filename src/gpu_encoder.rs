// ============================================================================
// 🚀 LITECORD GPU VIDEO ENCODER - MODULAR PROXY RE-EXPORT
// ============================================================================
// For full modular implementations, see:
// - `src/encoder/nvenc.rs` (NVIDIA NVENC Hardware Video Engine)
// - `src/encoder/amd_amf.rs` (AMD AMF Native Zero-Copy Engine)
// - `src/encoder/wmf.rs` (Direct3D 11 / Windows Media Foundation Universal GPU)
// - `src/encoder/software.rs` (Cisco OpenH264 SIMD Multi-threading)
// - `src/encoder/mod.rs` (VideoEncoder trait, GPU Hardware detection & factory)
// ============================================================================

pub use crate::encoder::*;
