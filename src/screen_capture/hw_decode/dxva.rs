//! Windows backend: the D3D11 video decoder (DXVA), fed in the "short slice" format so the
//! driver parses each slice and we only describe the picture and its references.
//!
//! It runs on the shared device (see device.rs), so the windows can show its pictures.

use std::sync::Arc;

use slint::{Rgba8Pixel, SharedPixelBuffer};
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::device::{shared_device, SharedDevice};
use super::h264_state::{H264State, Parsed, Picture, StateError, StreamInfo};
use super::HwError;
use crate::screen_capture::video_pipeline::{nv12_to_rgba_scaled, Colour};

// DXVA structures from dxva.h (packed), not in the windows crate.
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct PicParamsH264 {
    w_frame_width_in_mbs_minus1: u16,
    w_frame_height_in_mbs_minus1: u16,
    curr_pic: u8,
    num_ref_frames: u8,
    w_bit_fields: u16,
    bit_depth_luma_minus8: u8,
    bit_depth_chroma_minus8: u8,
    reserved16_bits: u16,
    status_report_feedback_number: u32,
    ref_frame_list: [u8; 16],
    curr_field_order_cnt: [i32; 2],
    field_order_cnt_list: [[i32; 2]; 16],
    pic_init_qs_minus26: i8,
    chroma_qp_index_offset: i8,
    second_chroma_qp_index_offset: i8,
    continuation_flag: u8,
    pic_init_qp_minus26: i8,
    num_ref_idx_l0_active_minus1: u8,
    num_ref_idx_l1_active_minus1: u8,
    reserved8_bits_a: u8,
    frame_num_list: [u16; 16],
    used_for_reference_flags: u32,
    non_existing_frame_flags: u16,
    frame_num: u16,
    log2_max_frame_num_minus4: u8,
    pic_order_cnt_type: u8,
    log2_max_pic_order_cnt_lsb_minus4: u8,
    delta_pic_order_always_zero_flag: u8,
    direct_8x8_inference_flag: u8,
    entropy_coding_mode_flag: u8,
    pic_order_present_flag: u8,
    num_slice_groups_minus1: u8,
    slice_group_map_type: u8,
    deblocking_filter_control_present_flag: u8,
    redundant_pic_cnt_present_flag: u8,
    reserved8_bits_b: u8,
    slice_group_change_rate_minus1: u16,
    slice_group_map: [u8; 810],
}
const _: () = assert!(std::mem::size_of::<PicParamsH264>() == 1040);

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct QmatrixH264 {
    lists4x4: [[u8; 16]; 6],
    lists8x8: [[u8; 64]; 2],
}
const _: () = assert!(std::mem::size_of::<QmatrixH264>() == 224);

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct SliceH264Short {
    bs_nal_unit_data_location: u32,
    slice_bytes_in_buffer: u32,
    w_bad_slice_chopping: u16,
}
const _: () = assert!(std::mem::size_of::<SliceH264Short>() == 10);

const E_PENDING: i32 = 0x8000000Au32 as i32;

struct Session {
    info: StreamInfo,
    decoder: ID3D11VideoDecoder,
    textures: ID3D11Texture2D,
    views: Vec<ID3D11VideoDecoderOutputView>,
}

/// A decoded picture: slice `index` of `texture` (NV12).
pub struct Frame {
    pub texture: ID3D11Texture2D,
    pub index: u32,
    pub info: StreamInfo,
}

pub struct DxvaDecoder {
    shared: Arc<SharedDevice>,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    vdev: ID3D11VideoDevice,
    vctx: ID3D11VideoContext,
    state: H264State,
    session: Option<Session>,
    feedback: u32,
    bitstream: Vec<u8>,
    slices: Vec<SliceH264Short>,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
}

// The D3D11 objects are used from the decoder thread only.
unsafe impl Send for DxvaDecoder {}

impl DxvaDecoder {
    pub fn new() -> Result<Self, HwError> {
        unsafe {
            let shared = shared_device()?;
            let (device, context) = (shared.device.clone(), shared.context.clone());
            let vdev: ID3D11VideoDevice = device.cast()?;
            let vctx: ID3D11VideoContext = context.cast()?;
            let has_h264 = (0..vdev.GetVideoDecoderProfileCount()).filter_map(|i| vdev.GetVideoDecoderProfile(i).ok()).any(|g| g == D3D11_DECODER_PROFILE_H264_VLD_NOFGT);
            if !has_h264 || !vdev.CheckVideoDecoderFormat(&D3D11_DECODER_PROFILE_H264_VLD_NOFGT, DXGI_FORMAT_NV12)?.as_bool() {
                return Err(HwError::Unsupported("the GPU has no H.264 decoder".into()));
            }
            Ok(Self { shared, device, context, vdev, vctx, state: H264State::new(), session: None, feedback: 0, bitstream: Vec::new(), slices: Vec::new(), staging: None })
        }
    }

    pub fn shared(&self) -> &SharedDevice {
        &self.shared
    }

    /// Decodes one access unit (Annex B). Returns the pictures now due on screen.
    pub fn decode(&mut self, au: &[u8]) -> Result<Vec<Frame>, HwError> {
        let Some(p) = self.state.parse(au)? else { return Ok(Vec::new()) };
        let info = H264State::stream_info(&p)?;
        if self.session.as_ref().map(|s| s.info != info).unwrap_or(true) {
            if !p.idr {
                return Err(HwError::NeedKeyframe("waiting for a keyframe".into()));
            }
            self.session = None;
            self.session = Some(self.create_session(&info)?);
            self.state.reset(&info);
        }
        let (pic, mut shown) = self.state.begin(&p)?;
        if let Err(e) = self.submit(au, &p, &pic) {
            self.state.need_keyframe();
            return Err(e);
        }
        shown.extend(self.state.finish(&p, &pic));
        let s = self.session.as_ref().unwrap();
        Ok(shown.into_iter().map(|sh| Frame { texture: s.textures.clone(), index: sh.surface as u32, info: s.info }).collect())
    }

    /// Copies a decoded picture back and converts it to RGBA at `out_w` x `out_h`.
    pub fn read_rgba(&mut self, f: &Frame, out_w: u32, out_h: u32) -> Result<SharedPixelBuffer<Rgba8Pixel>, HwError> {
        let (tw, th) = (f.info.mb_width * 16, f.info.mb_height * 16);
        unsafe {
            if self.staging.as_ref().map(|s| (s.1, s.2) != (tw, th)).unwrap_or(true) {
                let desc = D3D11_TEXTURE2D_DESC { Width: tw, Height: th, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_NV12, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_STAGING, BindFlags: 0, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32, MiscFlags: 0 };
                let mut t = None;
                self.device.CreateTexture2D(&desc, None, Some(&mut t))?;
                self.staging = Some((t.unwrap(), tw, th));
            }
            let staging = &self.staging.as_ref().unwrap().0;
            self.context.CopySubresourceRegion(staging, 0, 0, 0, 0, &f.texture, f.index, None);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            let pitch = m.RowPitch as usize;
            let y = std::slice::from_raw_parts(m.pData as *const u8, pitch * th as usize);
            let uv = std::slice::from_raw_parts((m.pData as *const u8).add(pitch * th as usize), pitch * th as usize / 2);
            let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(out_w, out_h);
            let colour = Colour { full_range: f.info.full_range, bt709: f.info.bt709 };
            nv12_to_rgba_scaled(y, uv, pitch, pitch, f.info.width as usize, f.info.height as usize, buf.make_mut_bytes(), out_w as usize, out_h as usize, colour);
            self.context.Unmap(staging, 0);
            Ok(buf)
        }
    }

    fn create_session(&self, info: &StreamInfo) -> Result<Session, HwError> {
        unsafe {
            let desc = D3D11_VIDEO_DECODER_DESC { Guid: D3D11_DECODER_PROFILE_H264_VLD_NOFGT, SampleWidth: info.mb_width * 16, SampleHeight: info.mb_height * 16, OutputFormat: DXGI_FORMAT_NV12 };
            let mut chosen = None;
            for i in 0..self.vdev.GetVideoDecoderConfigCount(&desc)? {
                let mut cfg = D3D11_VIDEO_DECODER_CONFIG::default();
                self.vdev.GetVideoDecoderConfig(&desc, i, &mut cfg)?;
                if cfg.ConfigBitstreamRaw == 2 {
                    chosen = Some(cfg);
                    break;
                }
            }
            let cfg = chosen.ok_or(HwError::Unsupported("the driver lacks the short slice format".into()))?;
            let decoder = self.vdev.CreateVideoDecoder(&desc, &cfg)?;
            let tdesc = D3D11_TEXTURE2D_DESC {
                Width: info.mb_width * 16,
                Height: info.mb_height * 16,
                MipLevels: 1,
                ArraySize: info.surfaces,
                Format: DXGI_FORMAT_NV12,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_DECODER.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut tex = None;
            self.device.CreateTexture2D(&tdesc, None, Some(&mut tex))?;
            let textures = tex.unwrap();
            let mut views = Vec::with_capacity(info.surfaces as usize);
            for i in 0..info.surfaces {
                let mut vd = D3D11_VIDEO_DECODER_OUTPUT_VIEW_DESC { DecodeProfile: D3D11_DECODER_PROFILE_H264_VLD_NOFGT, ViewDimension: D3D11_VDOV_DIMENSION_TEXTURE2D, ..Default::default() };
                vd.Anonymous.Texture2D = D3D11_TEX2D_VDOV { ArraySlice: i };
                let mut v = None;
                self.vdev.CreateVideoDecoderOutputView(&textures, &vd, Some(&mut v))?;
                views.push(v.unwrap());
            }
            Ok(Session { info: *info, decoder, textures, views })
        }
    }

    fn submit(&mut self, au: &[u8], p: &Parsed, pic: &Picture) -> Result<(), HwError> {
        let session = self.session.as_ref().unwrap();
        let (sps, pps, sh) = (&p.sps, &p.pps, &p.header);
        let ext = pps.extension.as_ref();

        let mut pp: PicParamsH264 = unsafe { std::mem::zeroed() };
        pp.w_frame_width_in_mbs_minus1 = sps.pic_width_in_mbs_minus1 as u16;
        pp.w_frame_height_in_mbs_minus1 = sps.pic_height_in_map_units_minus1 as u16;
        pp.curr_pic = pic.surface;
        pp.num_ref_frames = sps.max_num_ref_frames as u8;
        pp.w_bit_fields = (sh.sp_for_switch_flag.unwrap_or(false) as u16) << 3
            | 1 << 4 // chroma_format_idc = 4:2:0
            | ((p.nal_ref_idc != 0) as u16) << 6
            | (pps.constrained_intra_pred_flag as u16) << 7
            | (pps.weighted_pred_flag as u16) << 8
            | (pps.weighted_bipred_idc as u16 & 3) << 9
            | 1 << 11 // MbsConsecutiveFlag
            | 1 << 12 // frame_mbs_only_flag
            | (ext.map(|e| e.transform_8x8_mode_flag).unwrap_or(false) as u16) << 13
            | ((sps.level_idc >= 31) as u16) << 14 // MinLumaBipredSize8x8Flag
            | (p.intra() as u16) << 15;
        pp.reserved16_bits = 3;
        self.feedback = self.feedback.wrapping_add(1).max(1);
        pp.status_report_feedback_number = self.feedback;
        pp.ref_frame_list = [0xff; 16];
        for (i, r) in pic.refs.iter().enumerate().take(16) {
            pp.ref_frame_list[i] = r.surface | if r.long_term_idx.is_some() { 0x80 } else { 0 };
            pp.field_order_cnt_list[i] = r.poc;
            pp.frame_num_list[i] = r.long_term_idx.map(|l| l as u16).unwrap_or(r.frame_num);
            pp.used_for_reference_flags |= 3 << (2 * i);
        }
        pp.curr_field_order_cnt = pic.poc;
        pp.pic_init_qs_minus26 = pps.pic_init_qs_minus26 as i8;
        pp.chroma_qp_index_offset = pps.chroma_qp_index_offset as i8;
        pp.second_chroma_qp_index_offset = ext.map(|e| e.second_chroma_qp_index_offset).unwrap_or(pps.chroma_qp_index_offset) as i8;
        pp.continuation_flag = 1;
        pp.pic_init_qp_minus26 = pps.pic_init_qp_minus26 as i8;
        pp.num_ref_idx_l0_active_minus1 = pps.num_ref_idx_l0_default_active_minus1 as u8;
        pp.num_ref_idx_l1_active_minus1 = pps.num_ref_idx_l1_default_active_minus1 as u8;
        pp.frame_num = sh.frame_num;
        pp.log2_max_frame_num_minus4 = sps.log2_max_frame_num_minus4;
        use h264_reader::nal::sps::PicOrderCntType;
        match &sps.pic_order_cnt {
            PicOrderCntType::TypeZero { log2_max_pic_order_cnt_lsb_minus4 } => {
                pp.pic_order_cnt_type = 0;
                pp.log2_max_pic_order_cnt_lsb_minus4 = *log2_max_pic_order_cnt_lsb_minus4;
            }
            PicOrderCntType::TypeOne { delta_pic_order_always_zero_flag, .. } => {
                pp.pic_order_cnt_type = 1;
                pp.delta_pic_order_always_zero_flag = *delta_pic_order_always_zero_flag as u8;
            }
            PicOrderCntType::TypeTwo => pp.pic_order_cnt_type = 2,
        }
        pp.direct_8x8_inference_flag = sps.direct_8x8_inference_flag as u8;
        pp.entropy_coding_mode_flag = pps.entropy_coding_mode_flag as u8;
        pp.pic_order_present_flag = pps.bottom_field_pic_order_in_frame_present_flag as u8;
        pp.deblocking_filter_control_present_flag = pps.deblocking_filter_control_present_flag as u8;
        pp.redundant_pic_cnt_present_flag = pps.redundant_pic_cnt_present_flag as u8;

        let qm = QmatrixH264 { lists4x4: [[16; 16]; 6], lists8x8: [[16; 64]; 2] };

        self.bitstream.clear();
        self.slices.clear();
        for r in &p.slices {
            let at = self.bitstream.len() as u32;
            self.bitstream.extend_from_slice(&[0, 0, 1]);
            self.bitstream.extend_from_slice(&au[r.clone()]);
            self.slices.push(SliceH264Short { bs_nal_unit_data_location: at, slice_bytes_in_buffer: (r.len() + 3) as u32, w_bad_slice_chopping: 0 });
        }
        // Padded to 128 bytes like other DXVA clients; the last slice owns the padding.
        while self.bitstream.len() % 128 != 0 {
            self.bitstream.push(0);
        }
        if let Some(last) = self.slices.last_mut() {
            last.slice_bytes_in_buffer = self.bitstream.len() as u32 - last.bs_nal_unit_data_location;
        }

        unsafe {
            let view = &session.views[pic.surface as usize];
            // The driver may still be busy with the surface; a GPU that stays busy failed.
            let since = std::time::Instant::now();
            loop {
                match self.vctx.DecoderBeginFrame(&session.decoder, view, 0, None) {
                    Err(e) if e.code().0 == E_PENDING && since.elapsed() < std::time::Duration::from_millis(100) => std::thread::yield_now(),
                    r => break r?,
                }
            }
            let result = (|| -> Result<(), HwError> {
                let mbs = session.info.mb_width * session.info.mb_height;
                let as_bytes = |ptr: *const u8, n: usize| std::slice::from_raw_parts(ptr, n);
                let descs = [
                    fill(&self.vctx, &session.decoder, D3D11_VIDEO_DECODER_BUFFER_PICTURE_PARAMETERS, as_bytes(&pp as *const _ as *const u8, std::mem::size_of_val(&pp)), 0)?,
                    fill(&self.vctx, &session.decoder, D3D11_VIDEO_DECODER_BUFFER_INVERSE_QUANTIZATION_MATRIX, as_bytes(&qm as *const _ as *const u8, std::mem::size_of_val(&qm)), 0)?,
                    fill(&self.vctx, &session.decoder, D3D11_VIDEO_DECODER_BUFFER_SLICE_CONTROL, as_bytes(self.slices.as_ptr() as *const u8, self.slices.len() * std::mem::size_of::<SliceH264Short>()), mbs)?,
                    fill(&self.vctx, &session.decoder, D3D11_VIDEO_DECODER_BUFFER_BITSTREAM, &self.bitstream, mbs)?,
                ];
                self.vctx.SubmitDecoderBuffers(&session.decoder, &descs)?;
                Ok(())
            })();
            self.vctx.DecoderEndFrame(&session.decoder)?;
            result
        }
    }
}

unsafe fn fill(vctx: &ID3D11VideoContext, decoder: &ID3D11VideoDecoder, kind: D3D11_VIDEO_DECODER_BUFFER_TYPE, data: &[u8], mbs: u32) -> Result<D3D11_VIDEO_DECODER_BUFFER_DESC, HwError> {
    let mut size = 0u32;
    let mut ptr = std::ptr::null_mut();
    vctx.GetDecoderBuffer(decoder, kind, &mut size, &mut ptr)?;
    if (size as usize) < data.len() {
        let _ = vctx.ReleaseDecoderBuffer(decoder, kind);
        return Err(HwError::NeedKeyframe(format!("decoder buffer too small ({size} < {})", data.len())));
    }
    std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
    vctx.ReleaseDecoderBuffer(decoder, kind)?;
    Ok(D3D11_VIDEO_DECODER_BUFFER_DESC { BufferType: kind, DataSize: data.len() as u32, NumMBsInBuffer: mbs, ..Default::default() })
}

impl From<StateError> for HwError {
    fn from(e: StateError) -> Self {
        match e {
            StateError::Unsupported(s) => HwError::Unsupported(s),
            StateError::NeedKeyframe(s) => HwError::NeedKeyframe(s),
        }
    }
}

impl From<windows::core::Error> for HwError {
    fn from(e: windows::core::Error) -> Self {
        HwError::Device(e.to_string())
    }
}
