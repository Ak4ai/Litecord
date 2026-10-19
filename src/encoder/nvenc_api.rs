//! NVENC API layout (API 12.1), generated: do not edit by hand.
//!
//! Every size, offset and bit below was printed by `tools/nvenc_layout/probe.c`, compiled
//! against the official header from nv-codec-headers n12.1.14.0
//! (include/ffnvcodec/nvEncodeAPI.h, MIT). To regenerate:
//!
//! ```text
//! gcc -std=c11 -I <nv-codec-headers>/include/ffnvcodec -o probe tools/nvenc_layout/probe.c
//! ./probe > src/encoder/nvenc_api.rs   (then put this header back)
//! ```
//!
//! Bit constants are (byte offset, bit) of a bitfield inside its struct.
#![allow(dead_code)]

pub const NVENCAPI_VERSION: u32 = 0x0100000c;
pub const FUNCTION_LIST_VER: u32 = 0x7102000c;
pub const OPEN_SESSION_EX_VER: u32 = 0x7101000c;
pub const PRESET_CONFIG_VER: u32 = 0xf104000c;
pub const CONFIG_VER: u32 = 0xf108000c;
pub const RC_PARAMS_VER: u32 = 0x7101000c;
pub const INITIALIZE_PARAMS_VER: u32 = 0xf106000c;
pub const CREATE_BITSTREAM_BUFFER_VER: u32 = 0x7101000c;
pub const REGISTER_RESOURCE_VER: u32 = 0x7104000c;
pub const MAP_INPUT_RESOURCE_VER: u32 = 0x7104000c;
pub const PIC_PARAMS_VER: u32 = 0xf106000c;
pub const LOCK_BITSTREAM_VER: u32 = 0xf101000c;
pub const RECONFIGURE_PARAMS_VER: u32 = 0xf101000c;
pub const CODEC_H264_GUID: [u8; 16] = [0x62, 0x27, 0xc8, 0x6b, 0x63, 0x4e, 0xa4, 0x4c, 0xaa, 0x85, 0x1e, 0x50, 0xf3, 0x21, 0xf6, 0xbf];
pub const PRESET_P1_GUID: [u8; 16] = [0x3e, 0x8d, 0x0a, 0xfc, 0xf8, 0x45, 0xf8, 0x4c, 0x80, 0xc7, 0x29, 0x88, 0x71, 0x59, 0x0e, 0xbf];
pub const H264_PROFILE_MAIN_GUID: [u8; 16] = [0xd4, 0xc1, 0xb5, 0x60, 0xfe, 0x67, 0x90, 0x47, 0x94, 0xd5, 0xc4, 0x72, 0x6d, 0x7b, 0x6e, 0x6d];
pub const SIZE_NV_ENCODE_API_FUNCTION_LIST: usize = 2552;
pub const FN_OPEN_SESSION_EX: usize = 240;
pub const FN_GET_PRESET_CONFIG_EX: usize = 320;
pub const FN_INITIALIZE: usize = 96;
pub const FN_CREATE_BITSTREAM: usize = 120;
pub const FN_DESTROY_BITSTREAM: usize = 128;
pub const FN_ENCODE_PICTURE: usize = 136;
pub const FN_LOCK_BITSTREAM: usize = 144;
pub const FN_UNLOCK_BITSTREAM: usize = 152;
pub const FN_MAP_INPUT: usize = 208;
pub const FN_UNMAP_INPUT: usize = 216;
pub const FN_DESTROY_ENCODER: usize = 224;
pub const FN_REGISTER_RESOURCE: usize = 248;
pub const FN_UNREGISTER_RESOURCE: usize = 256;
pub const FN_RECONFIGURE: usize = 264;
pub const FN_GET_LAST_ERROR: usize = 304;
pub const SIZE_NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS: usize = 1552;
pub const OPEN_DEVICE_TYPE: usize = 4;
pub const OPEN_DEVICE: usize = 8;
pub const OPEN_API_VERSION: usize = 24;
pub const SIZE_NV_ENC_PRESET_CONFIG: usize = 5128;
pub const PRESET_CFG: usize = 8;
pub const SIZE_NV_ENC_CONFIG: usize = 3584;
pub const CFG_PROFILE_GUID: usize = 4;
pub const CFG_GOP_LENGTH: usize = 20;
pub const CFG_FRAME_INTERVAL_P: usize = 24;
pub const CFG_RC: usize = 40;
pub const CFG_CODEC: usize = 168;
pub const RC_VERSION: usize = 0;
pub const RC_MODE: usize = 4;
pub const RC_AVG_BITRATE: usize = 20;
pub const RC_MAX_BITRATE: usize = 24;
pub const RC_VBV_BUFFER: usize = 28;
pub const RC_VBV_INITIAL_DELAY: usize = 32;
pub const H264_IDR_PERIOD: usize = 8;
pub const H264_VUI: usize = 72;
pub const H264_REPEAT_SPSPPS_BIT: (usize, u32) = (1, 4);
pub const H264_OUTPUT_AUD_BIT: (usize, u32) = (0, 6);
pub const VUI_SIGNAL_TYPE_PRESENT: usize = 8;
pub const VUI_VIDEO_FORMAT: usize = 12;
pub const VUI_FULL_RANGE: usize = 16;
pub const VUI_COLOUR_DESCRIPTION_PRESENT: usize = 20;
pub const VUI_COLOUR_PRIMARIES: usize = 24;
pub const VUI_TRANSFER: usize = 28;
pub const VUI_MATRIX: usize = 32;
pub const SIZE_NV_ENC_INITIALIZE_PARAMS: usize = 1808;
pub const INIT_ENCODE_GUID: usize = 4;
pub const INIT_PRESET_GUID: usize = 20;
pub const INIT_WIDTH: usize = 36;
pub const INIT_HEIGHT: usize = 40;
pub const INIT_DAR_WIDTH: usize = 44;
pub const INIT_DAR_HEIGHT: usize = 48;
pub const INIT_FRAME_RATE_NUM: usize = 52;
pub const INIT_FRAME_RATE_DEN: usize = 56;
pub const INIT_ENCODE_ASYNC: usize = 60;
pub const INIT_ENABLE_PTD: usize = 64;
pub const INIT_ENCODE_CONFIG: usize = 88;
pub const INIT_MAX_WIDTH: usize = 96;
pub const INIT_MAX_HEIGHT: usize = 100;
pub const INIT_TUNING_INFO: usize = 136;
pub const SIZE_NV_ENC_CREATE_BITSTREAM_BUFFER: usize = 776;
pub const CREATE_BS_BUFFER: usize = 16;
pub const SIZE_NV_ENC_REGISTER_RESOURCE: usize = 1536;
pub const REG_RESOURCE_TYPE: usize = 4;
pub const REG_WIDTH: usize = 8;
pub const REG_HEIGHT: usize = 12;
pub const REG_PITCH: usize = 16;
pub const REG_SUBRESOURCE: usize = 20;
pub const REG_RESOURCE: usize = 24;
pub const REG_REGISTERED: usize = 32;
pub const REG_BUFFER_FORMAT: usize = 40;
pub const REG_BUFFER_USAGE: usize = 44;
pub const SIZE_NV_ENC_MAP_INPUT_RESOURCE: usize = 1544;
pub const MAP_REGISTERED: usize = 16;
pub const MAP_MAPPED: usize = 24;
pub const MAP_BUFFER_FORMAT: usize = 32;
pub const SIZE_NV_ENC_PIC_PARAMS: usize = 3360;
pub const PIC_INPUT_WIDTH: usize = 4;
pub const PIC_INPUT_HEIGHT: usize = 8;
pub const PIC_INPUT_PITCH: usize = 12;
pub const PIC_FLAGS: usize = 16;
pub const PIC_FRAME_IDX: usize = 20;
pub const PIC_TIMESTAMP: usize = 24;
pub const PIC_INPUT_BUFFER: usize = 40;
pub const PIC_OUTPUT_BITSTREAM: usize = 48;
pub const PIC_BUFFER_FORMAT: usize = 64;
pub const PIC_STRUCT: usize = 68;
pub const SIZE_NV_ENC_LOCK_BITSTREAM: usize = 1552;
pub const LOCK_OUTPUT_BITSTREAM: usize = 8;
pub const LOCK_SIZE: usize = 36;
pub const LOCK_PTR: usize = 56;
pub const LOCK_PICTURE_TYPE: usize = 64;
pub const SIZE_NV_ENC_RECONFIGURE_PARAMS: usize = 1824;
pub const RECONF_INIT_PARAMS: usize = 8;
pub const RECONF_RESET_BIT: (usize, u32) = (1816, 0);
pub const RECONF_FORCE_IDR_BIT: (usize, u32) = (1816, 1);
pub const PARAMS_RC_CBR: u32 = 2;
pub const TUNING_ULTRA_LOW_LATENCY: u32 = 3;
pub const DEVICE_TYPE_DIRECTX: u32 = 0;
pub const INPUT_RESOURCE_TYPE_DIRECTX: u32 = 0;
pub const BUFFER_FORMAT_NV12: u32 = 1;
pub const INPUT_IMAGE: u32 = 0;
pub const PIC_STRUCT_FRAME: u32 = 1;
pub const PIC_FLAG_FORCEIDR: u32 = 2;
pub const PIC_FLAG_OUTPUT_SPSPPS: u32 = 4;
pub const PIC_TYPE_IDR: u32 = 3;
pub const ERR_ENCODER_NEEDS_MORE_INPUT: u32 = 17;
