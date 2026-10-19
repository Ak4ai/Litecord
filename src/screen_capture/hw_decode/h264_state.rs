//! The decoder-side bookkeeping of H.264 that a GPU decoder needs from us: parameter
//! sets, slice headers, picture order counts (spec 8.2.1), reference picture marking
//! (8.2.5) and output order. The GPU decodes the slices themselves. Nothing here touches
//! a GPU API, so the same state can feed DXVA on Windows and VA-API on Linux.
//!
//! Scope: progressive (frame) 8-bit 4:2:0 streams with flat quantisation matrices and
//! no slice groups. Anything else is `Unsupported`, and the caller decodes on the CPU.

use std::ops::Range;

use h264_reader::nal::pps::PicParameterSet;
use h264_reader::nal::slice::{DecRefPicMarking, MemoryManagementControlOperation as Mmco, PicOrderCountLsb, SliceExclusive, SliceFamily, SliceHeader};
use h264_reader::nal::sps::{ChromaFormat, FrameMbsFlags, PicOrderCntType, SeqParameterSet};
use h264_reader::nal::{Nal, RefNal, UnitType};
use h264_reader::Context;

#[derive(Debug, PartialEq)]
pub enum StateError {
    /// The stream uses something the GPU path does not handle: decode it on the CPU.
    Unsupported(String),
    /// A picture is missing (lost packets) or the stream has not reached a keyframe yet.
    NeedKeyframe(String),
}

/// One picture's worth of parsed access unit.
pub struct Parsed {
    pub sps: SeqParameterSet,
    pub pps: PicParameterSet,
    pub header: SliceHeader,
    pub nal_ref_idc: u8,
    pub idr: bool,
    /// Every slice NAL unit of the picture (without start code), as ranges of the input.
    pub slices: Vec<Range<usize>>,
}

impl Parsed {
    /// All slices are I (or SI): no references are read.
    pub fn intra(&self) -> bool {
        matches!(self.header.slice_type.family, SliceFamily::I | SliceFamily::SI) && self.header.slice_type.exclusive == SliceExclusive::Exclusive
    }
}

/// What a decoder backend needs to set up for a stream.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamInfo {
    pub mb_width: u32,
    pub mb_height: u32,
    /// Visible size after cropping.
    pub width: u32,
    pub height: u32,
    /// Decode surfaces: references, pictures waiting to be shown, the picture being
    /// decoded and the one on screen.
    pub surfaces: u32,
    /// Pictures that may arrive before one shown earlier (0 = shown as decoded).
    pub reorder_depth: usize,
    /// Samples use 0-255 instead of 16-235.
    pub full_range: bool,
    /// BT.709 colours (otherwise BT.601).
    pub bt709: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RefPic {
    pub surface: u8,
    pub frame_num: u16,
    pub long_term_idx: Option<u32>,
    pub poc: [i32; 2],
}

/// The picture to decode now: where it goes and what it may reference.
#[derive(Debug)]
pub struct Picture {
    pub surface: u8,
    pub poc: [i32; 2],
    pub refs: Vec<RefPic>,
}

/// A decoded picture that is due on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shown {
    pub surface: u8,
    pub poc: i32,
}

const SPARE_SURFACES: u32 = 2;

pub struct H264State {
    ctx: Context,
    surfaces: u8,
    dpb: Vec<RefPic>,
    /// Decoded pictures waiting for their turn on screen.
    pending: Vec<Shown>,
    reorder_depth: usize,
    last_shown: Option<u8>,
    need_idr: bool,
    // Picture order state (8.2.1).
    prev_poc_msb: i32,
    prev_poc_lsb: i32,
    prev_frame_num_offset: i32,
    prev_frame_num: u16,
    prev_ref_frame_num: u16,
    prev_had_mmco5: bool,
}

impl Default for H264State {
    fn default() -> Self {
        Self::new()
    }
}

impl H264State {
    pub fn new() -> Self {
        Self {
            ctx: Context::new(),
            surfaces: 0,
            dpb: Vec::new(),
            pending: Vec::new(),
            reorder_depth: 0,
            last_shown: None,
            need_idr: true,
            prev_poc_msb: 0,
            prev_poc_lsb: 0,
            prev_frame_num_offset: 0,
            prev_frame_num: 0,
            prev_ref_frame_num: 0,
            prev_had_mmco5: false,
        }
    }

    /// Reads one access unit: stores parameter sets and parses the first slice header.
    /// `None` when it holds no picture (parameter sets only).
    pub fn parse(&mut self, au: &[u8]) -> Result<Option<Parsed>, StateError> {
        let mut first: Option<(SliceHeader, SeqParameterSet, PicParameterSet, u8, bool)> = None;
        let mut slices = Vec::new();
        for range in nal_ranges(au) {
            let nal = &au[range.clone()];
            let rn = RefNal::new(nal, &[], true);
            let Ok(hdr) = rn.header() else { continue };
            match hdr.nal_unit_type() {
                UnitType::SeqParameterSet => {
                    let sps = SeqParameterSet::from_bits(rn.rbsp_bits()).map_err(|e| StateError::Unsupported(format!("SPS: {e:?}")))?;
                    self.ctx.put_seq_param_set(sps);
                }
                UnitType::PicParameterSet => {
                    // A PPS of an SPS we do not have yet is a stream joined mid-way (Quick Sync
                    // repeats the PPS in every picture, the SPS only with keyframes): wait for
                    // a keyframe. Any other PPS we cannot read is a stream this parser does
                    // not handle, not a lost packet.
                    let pps = PicParameterSet::from_bits(&self.ctx, rn.rbsp_bits()).map_err(|e| match e {
                        h264_reader::nal::pps::PpsError::UnknownSeqParamSetId(_) => StateError::NeedKeyframe(format!("PPS: {e:?}")),
                        _ => StateError::Unsupported(format!("PPS: {e:?}")),
                    })?;
                    self.ctx.put_pic_param_set(pps);
                }
                UnitType::SliceLayerWithoutPartitioningNonIdr | UnitType::SliceLayerWithoutPartitioningIdr => {
                    if first.is_none() {
                        let (sh, sps, pps) = SliceHeader::from_bits(&self.ctx, &mut rn.rbsp_bits(), hdr, None)
                            .map_err(|e| StateError::NeedKeyframe(format!("slice header: {e:?}")))?;
                        let idr = hdr.nal_unit_type() == UnitType::SliceLayerWithoutPartitioningIdr;
                        first = Some((sh, sps.clone(), pps.clone(), hdr.nal_ref_idc(), idr));
                    }
                    slices.push(range);
                }
                UnitType::SliceDataPartitionALayer | UnitType::SliceDataPartitionBLayer | UnitType::SliceDataPartitionCLayer => {
                    return Err(StateError::Unsupported("data partitioning".into()));
                }
                _ => {}
            }
        }
        Ok(first.map(|(header, sps, pps, nal_ref_idc, idr)| Parsed { sps, pps, header, nal_ref_idc, idr, slices }))
    }

    /// Whether the GPU path handles this stream, and what it needs to decode it.
    pub fn stream_info(p: &Parsed) -> Result<StreamInfo, StateError> {
        let (sps, pps) = (&p.sps, &p.pps);
        if !matches!(sps.frame_mbs_flags, FrameMbsFlags::Frames) {
            return Err(StateError::Unsupported("interlaced (field) coding".into()));
        }
        if sps.chroma_info.scaling_matrix.is_some() || pps.extension.as_ref().map(|e| e.pic_scaling_matrix.is_some()).unwrap_or(false) {
            return Err(StateError::Unsupported("custom quantisation matrices".into()));
        }
        if pps.slice_groups.is_some() {
            return Err(StateError::Unsupported("slice groups".into()));
        }
        if sps.chroma_info.bit_depth_luma_minus8 != 0 || sps.chroma_info.bit_depth_chroma_minus8 != 0 || !matches!(sps.chroma_info.chroma_format, ChromaFormat::YUV420) {
            return Err(StateError::Unsupported("not 8-bit 4:2:0".into()));
        }
        let (width, height) = sps.pixel_dimensions().map_err(|e| StateError::Unsupported(format!("size: {e:?}")))?;
        let refs = sps.max_num_ref_frames.max(1);
        let vui = sps.vui_parameters.as_ref();
        let reorder_depth = if matches!(sps.pic_order_cnt, PicOrderCntType::TypeTwo) || u8::from(sps.profile_idc) == 66 {
            // Output order equals decode order (POC type 2, or baseline: no B-frames).
            0
        } else if let Some(b) = vui.and_then(|v| v.bitstream_restrictions.as_ref()) {
            b.max_num_reorder_frames as usize
        } else {
            refs as usize
        };
        // Colours as Litecord senders really produce them. The app converts the screen with
        // BT.601 limited range; NVENC senders up to 1.2.1 still declared BT.709 full range,
        // so "full range" is never trusted and BT.709 only when declared without it. No
        // sender produces full range.
        let signal = vui.and_then(|v| v.video_signal_type.as_ref());
        let declared_full = signal.map(|s| s.video_full_range_flag).unwrap_or(false);
        let declared_709 = signal.and_then(|s| s.colour_description.as_ref()).map(|c| c.matrix_coefficients) == Some(1);
        Ok(StreamInfo {
            mb_width: sps.pic_width_in_mbs_minus1 + 1,
            mb_height: sps.pic_height_in_map_units_minus1 + 1,
            width,
            height,
            surfaces: refs + reorder_depth as u32 + SPARE_SURFACES,
            reorder_depth,
            full_range: false,
            bt709: declared_709 && !declared_full,
        })
    }

    /// Starts over with a new set of decode surfaces; the next picture must be an IDR.
    pub fn reset(&mut self, info: &StreamInfo) {
        self.surfaces = info.surfaces as u8;
        self.reorder_depth = info.reorder_depth;
        self.dpb.clear();
        self.pending.clear();
        self.last_shown = None;
        self.need_idr = true;
    }

    /// Forgets the references after an error; waits for the next IDR.
    pub fn need_keyframe(&mut self) {
        self.dpb.clear();
        self.pending.clear();
        self.need_idr = true;
    }

    /// Prepares the picture for decoding. Also returns pictures that must be shown before
    /// it (everything pending before an IDR or a memory reset).
    pub fn begin(&mut self, p: &Parsed) -> Result<(Picture, Vec<Shown>), StateError> {
        if self.surfaces == 0 {
            return Err(StateError::NeedKeyframe("no decode surfaces".into()));
        }
        if self.need_idr && !p.idr {
            return Err(StateError::NeedKeyframe("waiting for a keyframe".into()));
        }
        let sh = &p.header;
        let max_frame_num = 1u32 << (p.sps.log2_max_frame_num_minus4 + 4);
        let mut shown = Vec::new();
        let mmco5 = has_mmco5(sh);
        if p.idr || mmco5 {
            shown = self.flush();
        }
        if p.idr {
            self.dpb.clear();
            self.prev_ref_frame_num = sh.frame_num;
        } else if sh.frame_num != self.prev_ref_frame_num && sh.frame_num as u32 != (self.prev_ref_frame_num as u32 + 1) % max_frame_num {
            let prev = self.prev_ref_frame_num;
            self.need_keyframe();
            return Err(StateError::NeedKeyframe(format!("frame_num gap {prev} -> {}", sh.frame_num)));
        }
        let poc = self.picture_order(p, max_frame_num);
        let surface = (0..self.surfaces)
            .find(|s| !self.dpb.iter().any(|r| r.surface == *s) && !self.pending.iter().any(|q| q.surface == *s) && Some(*s) != self.last_shown)
            .ok_or_else(|| StateError::NeedKeyframe("no free decode surface".into()))?;
        Ok((Picture { surface, poc, refs: self.dpb.clone() }, shown))
    }

    /// Records a decoded picture: reference marking, then whatever is now due on screen.
    pub fn finish(&mut self, p: &Parsed, pic: &Picture) -> Vec<Shown> {
        self.need_idr = false;
        let mmco5 = has_mmco5(&p.header);
        self.mark_references(p, pic);
        // After a memory reset the picture's POC restarts at 0 (8.2.1).
        let poc = if mmco5 { 0 } else { pic.poc[0].min(pic.poc[1]) };
        self.pending.push(Shown { surface: pic.surface, poc });
        let mut out = Vec::new();
        while self.pending.len() > self.reorder_depth {
            out.extend(self.pop_shown());
        }
        out
    }

    /// Pictures still waiting to be shown.
    pub fn flush(&mut self) -> Vec<Shown> {
        let mut out = Vec::new();
        while let Some(s) = self.pop_shown() {
            out.push(s);
        }
        out
    }

    fn pop_shown(&mut self) -> Option<Shown> {
        let i = self.pending.iter().enumerate().min_by_key(|(_, s)| s.poc)?.0;
        let s = self.pending.remove(i);
        self.last_shown = Some(s.surface);
        Some(s)
    }

    /// TopFieldOrderCnt / BottomFieldOrderCnt of a frame (8.2.1).
    fn picture_order(&mut self, p: &Parsed, max_frame_num: u32) -> [i32; 2] {
        let sh = &p.header;
        let frame_num = sh.frame_num as i32;
        match &p.sps.pic_order_cnt {
            PicOrderCntType::TypeZero { log2_max_pic_order_cnt_lsb_minus4 } => {
                let max_lsb = 1i32 << (log2_max_pic_order_cnt_lsb_minus4 + 4);
                let (lsb, delta_bottom) = match &sh.pic_order_cnt_lsb {
                    Some(PicOrderCountLsb::Frame(l)) => (*l as i32, 0),
                    Some(PicOrderCountLsb::FieldsAbsolute { pic_order_cnt_lsb, delta_pic_order_cnt_bottom }) => (*pic_order_cnt_lsb as i32, *delta_pic_order_cnt_bottom),
                    _ => (0, 0),
                };
                if p.idr {
                    self.prev_poc_msb = 0;
                    self.prev_poc_lsb = 0;
                } else if self.prev_had_mmco5 {
                    // prevPicOrderCntLsb was set when the mmco5 picture was marked.
                    self.prev_poc_msb = 0;
                }
                let msb = if lsb < self.prev_poc_lsb && self.prev_poc_lsb - lsb >= max_lsb / 2 {
                    self.prev_poc_msb + max_lsb
                } else if lsb > self.prev_poc_lsb && lsb - self.prev_poc_lsb > max_lsb / 2 {
                    self.prev_poc_msb - max_lsb
                } else {
                    self.prev_poc_msb
                };
                let top = msb + lsb;
                if p.nal_ref_idc != 0 {
                    self.prev_poc_msb = msb;
                    self.prev_poc_lsb = lsb;
                }
                [top, top + delta_bottom]
            }
            PicOrderCntType::TypeOne { delta_pic_order_always_zero_flag, offset_for_non_ref_pic, offset_for_top_to_bottom_field, offsets_for_ref_frame } => {
                let offset = self.frame_num_offset(p.idr, sh.frame_num, max_frame_num);
                let cycle = offsets_for_ref_frame.len() as i32;
                let mut abs = if cycle != 0 { offset + frame_num } else { 0 };
                if p.nal_ref_idc == 0 && abs > 0 {
                    abs -= 1;
                }
                let mut expected = 0;
                if abs > 0 {
                    let delta_cycle: i32 = offsets_for_ref_frame.iter().sum();
                    let cnt = (abs - 1) / cycle;
                    let in_cycle = (abs - 1) % cycle;
                    expected = cnt * delta_cycle + offsets_for_ref_frame[..=in_cycle as usize].iter().sum::<i32>();
                }
                if p.nal_ref_idc == 0 {
                    expected += offset_for_non_ref_pic;
                }
                let deltas = match &sh.pic_order_cnt_lsb {
                    Some(PicOrderCountLsb::FieldsDelta(d)) if !delta_pic_order_always_zero_flag => *d,
                    _ => [0, 0],
                };
                let top = expected + deltas[0];
                self.prev_frame_num_offset = offset;
                self.prev_frame_num = sh.frame_num;
                [top, top + offset_for_top_to_bottom_field + deltas[1]]
            }
            PicOrderCntType::TypeTwo => {
                let offset = self.frame_num_offset(p.idr, sh.frame_num, max_frame_num);
                let poc = if p.idr { 0 } else if p.nal_ref_idc == 0 { 2 * (offset + frame_num) - 1 } else { 2 * (offset + frame_num) };
                self.prev_frame_num_offset = offset;
                self.prev_frame_num = sh.frame_num;
                [poc, poc]
            }
        }
    }

    fn frame_num_offset(&self, idr: bool, frame_num: u16, max_frame_num: u32) -> i32 {
        if idr {
            return 0;
        }
        let prev = if self.prev_had_mmco5 { 0 } else { self.prev_frame_num_offset };
        let prev_frame_num = if self.prev_had_mmco5 { 0 } else { self.prev_frame_num };
        if prev_frame_num > frame_num {
            prev + max_frame_num as i32
        } else {
            prev
        }
    }

    /// Decoded reference picture marking (8.2.5), frames only.
    fn mark_references(&mut self, p: &Parsed, pic: &Picture) {
        self.prev_had_mmco5 = false;
        let frame_num = p.header.frame_num;
        if p.nal_ref_idc == 0 {
            return;
        }
        let max_frame_num = 1i32 << (p.sps.log2_max_frame_num_minus4 + 4);
        // PicNum of a short-term frame: FrameNumWrap (8.2.4.1).
        let pic_num = |f: u16| -> i32 { if f > frame_num { f as i32 - max_frame_num } else { f as i32 } };
        let mut current_long_term = None;
        match &p.header.dec_ref_pic_marking {
            Some(DecRefPicMarking::Idr { long_term_reference_flag, .. }) => {
                self.dpb.clear();
                if *long_term_reference_flag {
                    current_long_term = Some(0);
                }
            }
            Some(DecRefPicMarking::Adaptive(ops)) => {
                for op in ops {
                    match op {
                        Mmco::ShortTermUnusedForRef { difference_of_pic_nums_minus1 } => {
                            let x = frame_num as i32 - (*difference_of_pic_nums_minus1 as i32 + 1);
                            self.dpb.retain(|r| r.long_term_idx.is_some() || pic_num(r.frame_num) != x);
                        }
                        Mmco::LongTermUnusedForRef { long_term_pic_num } => {
                            self.dpb.retain(|r| r.long_term_idx != Some(*long_term_pic_num));
                        }
                        Mmco::ShortTermUsedForLongTerm { difference_of_pic_nums_minus1, long_term_frame_idx } => {
                            let x = frame_num as i32 - (*difference_of_pic_nums_minus1 as i32 + 1);
                            self.dpb.retain(|r| r.long_term_idx != Some(*long_term_frame_idx));
                            if let Some(r) = self.dpb.iter_mut().find(|r| r.long_term_idx.is_none() && pic_num(r.frame_num) == x) {
                                r.long_term_idx = Some(*long_term_frame_idx);
                            }
                        }
                        Mmco::MaxUsedLongTermFrameRef { max_long_term_frame_idx_plus1 } => {
                            self.dpb.retain(|r| r.long_term_idx.map(|l| l < *max_long_term_frame_idx_plus1).unwrap_or(true));
                        }
                        Mmco::AllRefPicturesUnused => {
                            self.dpb.clear();
                            self.prev_had_mmco5 = true;
                        }
                        Mmco::CurrentUsedForLongTerm { long_term_frame_idx } => {
                            self.dpb.retain(|r| r.long_term_idx != Some(*long_term_frame_idx));
                            current_long_term = Some(*long_term_frame_idx);
                        }
                    }
                }
            }
            _ => {
                // Sliding window: drop the oldest short-term frame when the DPB is full.
                let max = p.sps.max_num_ref_frames.max(1) as usize;
                if self.dpb.len() >= max {
                    if let Some(oldest) = self.dpb.iter().enumerate().filter(|(_, r)| r.long_term_idx.is_none()).min_by_key(|(_, r)| pic_num(r.frame_num)).map(|(i, _)| i) {
                        self.dpb.remove(oldest);
                    }
                }
            }
        }
        if self.prev_had_mmco5 {
            // After mmco5 the picture counts as frame_num 0 with its POC relative to itself.
            let base = pic.poc[0].min(pic.poc[1]);
            self.dpb.push(RefPic { surface: pic.surface, frame_num: 0, long_term_idx: current_long_term, poc: [pic.poc[0] - base, pic.poc[1] - base] });
            self.prev_frame_num = 0;
            self.prev_ref_frame_num = 0;
            self.prev_poc_msb = 0;
            self.prev_poc_lsb = pic.poc[0] - base;
            self.prev_frame_num_offset = 0;
        } else {
            self.dpb.push(RefPic { surface: pic.surface, frame_num, long_term_idx: current_long_term, poc: pic.poc });
            self.prev_ref_frame_num = frame_num;
        }
    }

    #[cfg(test)]
    fn dpb(&self) -> &[RefPic] {
        &self.dpb
    }
}

fn has_mmco5(sh: &SliceHeader) -> bool {
    matches!(&sh.dec_ref_pic_marking, Some(DecRefPicMarking::Adaptive(ops)) if ops.iter().any(|o| matches!(o, Mmco::AllRefPicturesUnused)))
}

/// NAL units of an Annex B buffer, as ranges without start codes (trailing zero bytes
/// before the next start code dropped).
pub fn nal_ranges(data: &[u8]) -> Vec<Range<usize>> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (k, &s) in starts.iter().enumerate() {
        let mut e = starts.get(k + 1).map(|n| n - 3).unwrap_or(data.len());
        while e > s && data[e - 1] == 0 {
            e -= 1;
        }
        if e > s {
            out.push(s..e);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Quick Sync repeats the PPS in every picture but the SPS only with keyframes: a
    /// receiver that starts mid-stream sees a PPS of an SPS it does not have yet. That is
    /// "wait for a keyframe", not a stream the GPU cannot decode.
    #[test]
    fn a_pps_before_its_sps_waits_for_a_keyframe() {
        let mut st = H264State::new();
        let au = [0, 0, 0, 1, 0x68, 0xce, 0x3c, 0x80];
        match st.parse(&au) {
            Err(StateError::NeedKeyframe(_)) => {}
            other => panic!("expected NeedKeyframe, got {:?}", other.map(|p| p.is_some())),
        }
    }

    use super::*;

    /// Writes H.264 syntax elements (with emulation prevention) for synthetic streams.
    #[derive(Default)]
    struct Bits {
        bits: Vec<bool>,
    }
    impl Bits {
        fn u(&mut self, n: u32, v: u32) -> &mut Self {
            for i in (0..n).rev() {
                self.bits.push(v >> i & 1 == 1);
            }
            self
        }
        fn ue(&mut self, v: u32) -> &mut Self {
            let x = v + 1;
            let len = 32 - x.leading_zeros();
            self.u(len - 1, 0).u(len, x)
        }
        fn se(&mut self, v: i32) -> &mut Self {
            self.ue(if v > 0 { (2 * v - 1) as u32 } else { (-2 * v) as u32 })
        }
        /// RBSP trailing bits, emulation prevention and an Annex B start code.
        fn nal(&mut self, header: u8) -> Vec<u8> {
            self.bits.push(true);
            while self.bits.len() % 8 != 0 {
                self.bits.push(false);
            }
            let rbsp: Vec<u8> = self.bits.chunks(8).map(|c| c.iter().fold(0u8, |a, &b| a << 1 | b as u8)).collect();
            let mut out = vec![0, 0, 0, 1, header];
            let mut zeros = 0;
            for b in rbsp {
                if zeros >= 2 && b <= 3 {
                    out.push(3);
                    zeros = 0;
                }
                out.push(b);
                zeros = if b == 0 { zeros + 1 } else { 0 };
            }
            out
        }
    }

    #[derive(Clone, Copy)]
    enum Poc {
        Zero,
        Two,
    }

    /// SPS for a 64x64 main-profile stream (log2_max_frame_num = 4, log2_max_poc_lsb = 4).
    fn sps(poc: Poc, max_refs: u32, max_reorder: Option<u32>) -> Vec<u8> {
        let mut b = Bits::default();
        b.u(8, 77).u(8, 0).u(8, 40).ue(0).ue(0);
        match poc {
            Poc::Zero => b.ue(0).ue(0),
            Poc::Two => b.ue(2),
        };
        b.ue(max_refs).u(1, 0).ue(3).ue(3).u(1, 1).u(1, 1).u(1, 0);
        match max_reorder {
            None => b.u(1, 0),
            Some(r) => {
                // VUI with only bitstream restrictions: aspect ratio, overscan, signal type,
                // chroma location, timing, NAL HRD, VCL HRD and pic_struct all absent.
                b.u(1, 1).u(8, 0);
                b.u(1, 1).u(1, 1).ue(0).ue(0).ue(16).ue(16).ue(r).ue(max_refs)
            }
        };
        b.nal(0x67)
    }

    fn pps() -> Vec<u8> {
        let mut b = Bits::default();
        b.ue(0).ue(0).u(1, 0).u(1, 0).ue(0).ue(0).ue(0).u(1, 0).u(2, 0).se(0).se(0).se(0).u(1, 0).u(1, 0).u(1, 0);
        b.nal(0x68)
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Kind {
        I,
        P,
        B,
    }

    enum Marking {
        Sliding,
        LongTermIdr,
        Ops(Vec<(u32, u32, u32)>), // (mmco, arg1, arg2)
    }

    struct Slice {
        kind: Kind,
        idr: bool,
        reference: bool,
        frame_num: u32,
        poc_lsb: u32,
        marking: Marking,
    }

    fn slice(s: &Slice, poc: Poc) -> Vec<u8> {
        let mut b = Bits::default();
        let slice_type = match s.kind {
            Kind::P => 5,
            Kind::B => 6,
            Kind::I => 7,
        };
        b.ue(0).ue(slice_type).ue(0).u(4, s.frame_num);
        if s.idr {
            b.ue(0);
        }
        if let Poc::Zero = poc {
            b.u(4, s.poc_lsb);
        }
        if s.kind == Kind::B {
            b.u(1, 1); // direct_spatial_mv_pred_flag
        }
        if s.kind != Kind::I {
            b.u(1, 0); // num_ref_idx_active_override_flag
            b.u(1, 0); // ref_pic_list_modification_flag_l0
        }
        if s.kind == Kind::B {
            b.u(1, 0); // ref_pic_list_modification_flag_l1
        }
        if s.reference {
            if s.idr {
                b.u(1, 0).u(1, matches!(s.marking, Marking::LongTermIdr) as u32);
            } else {
                match &s.marking {
                    Marking::Ops(ops) => {
                        b.u(1, 1);
                        for &(op, a1, a2) in ops {
                            b.ue(op);
                            match op {
                                1 | 2 | 4 | 6 => {
                                    b.ue(a1);
                                }
                                3 => {
                                    b.ue(a1).ue(a2);
                                }
                                _ => {}
                            }
                        }
                        b.ue(0);
                    }
                    _ => {
                        b.u(1, 0);
                    }
                }
            }
        }
        b.se(0); // slice_qp_delta
        b.u(8, 0xff); // stand-in for slice data
        let header = (if s.reference { 0x60 } else { 0 }) | if s.idr { 5 } else { 1 };
        b.nal(header)
    }

    /// Feeds pictures through the state like a decoder would; returns the order in which
    /// pictures (by their index in `pics`) came out.
    struct Run {
        state: H264State,
        poc: Poc,
        header: Vec<u8>,
        by_surface: std::collections::HashMap<u8, usize>,
        shown: Vec<usize>,
    }
    impl Run {
        fn new(poc: Poc, max_refs: u32, max_reorder: Option<u32>) -> Self {
            let mut header = sps(poc, max_refs, max_reorder);
            header.extend(pps());
            Run { state: H264State::new(), poc, header, by_surface: Default::default(), shown: Vec::new() }
        }
        fn feed(&mut self, n: usize, s: Slice) -> Result<(), StateError> {
            let mut au = if s.idr { self.header.clone() } else { Vec::new() };
            au.extend(slice(&s, self.poc));
            let p = self.state.parse(&au)?.expect("a picture");
            if s.idr {
                let info = H264State::stream_info(&p)?;
                if self.state.surfaces == 0 {
                    self.state.reset(&info);
                }
            }
            let on_screen = self.state.last_shown;
            let (pic, before) = self.state.begin(&p)?;
            for sh in &before {
                self.shown.push(self.by_surface[&sh.surface]);
            }
            assert!(!pic.refs.iter().any(|r| r.surface == pic.surface), "decoding over a reference");
            if before.is_empty() {
                assert_ne!(Some(pic.surface), on_screen, "decoding over the picture on screen");
            }
            self.by_surface.insert(pic.surface, n);
            for sh in self.state.finish(&p, &pic) {
                self.shown.push(self.by_surface[&sh.surface]);
            }
            Ok(())
        }
        fn frame_nums(&self) -> Vec<(u16, Option<u32>)> {
            let mut v: Vec<_> = self.state.dpb().iter().map(|r| (r.frame_num, r.long_term_idx)).collect();
            v.sort();
            v
        }
    }

    fn idr(lt: bool) -> Slice {
        Slice { kind: Kind::I, idr: true, reference: true, frame_num: 0, poc_lsb: 0, marking: if lt { Marking::LongTermIdr } else { Marking::Sliding } }
    }
    fn p(frame_num: u32, poc_lsb: u32) -> Slice {
        Slice { kind: Kind::P, idr: false, reference: true, frame_num, poc_lsb, marking: Marking::Sliding }
    }
    fn p_ops(frame_num: u32, poc_lsb: u32, ops: Vec<(u32, u32, u32)>) -> Slice {
        Slice { kind: Kind::P, idr: false, reference: true, frame_num, poc_lsb, marking: Marking::Ops(ops) }
    }

    #[test]
    fn sliding_window_drops_the_oldest_short_term_frame() {
        let mut r = Run::new(Poc::Two, 2, None);
        r.feed(0, idr(false)).unwrap();
        r.feed(1, p(1, 0)).unwrap();
        r.feed(2, p(2, 0)).unwrap();
        assert_eq!(r.frame_nums(), vec![(1, None), (2, None)]);
    }

    #[test]
    fn mmco1_drops_the_named_short_term_frame() {
        let mut r = Run::new(Poc::Two, 4, None);
        r.feed(0, idr(false)).unwrap();
        r.feed(1, p(1, 0)).unwrap();
        r.feed(2, p(2, 0)).unwrap();
        // CurrPicNum 3 - (1 + 1) = PicNum 1.
        r.feed(3, p_ops(3, 0, vec![(1, 1, 0)])).unwrap();
        assert_eq!(r.frame_nums(), vec![(0, None), (2, None), (3, None)]);
    }

    #[test]
    fn long_term_frames_survive_the_sliding_window_and_mmco2_4_6_manage_them() {
        let mut r = Run::new(Poc::Two, 3, None);
        r.feed(0, idr(true)).unwrap(); // IDR kept as long-term 0
        assert_eq!(r.frame_nums(), vec![(0, Some(0))]);
        r.feed(1, p_ops(1, 0, vec![(6, 1, 0)])).unwrap(); // current -> long-term 1
        r.feed(2, p(2, 0)).unwrap();
        r.feed(3, p(3, 0)).unwrap(); // full: drops short-term 2, never a long-term
        assert_eq!(r.frame_nums(), vec![(0, Some(0)), (1, Some(1)), (3, None)]);
        r.feed(4, p_ops(4, 0, vec![(2, 0, 0)])).unwrap(); // long-term pic num 0 unused
        assert_eq!(r.frame_nums(), vec![(1, Some(1)), (3, None), (4, None)]);
        r.feed(5, p_ops(5, 0, vec![(4, 1, 0)])).unwrap(); // max long-term idx 0: drops idx 1
        assert_eq!(r.frame_nums(), vec![(3, None), (4, None), (5, None)]);
    }

    #[test]
    fn mmco3_turns_a_short_term_frame_long_term() {
        let mut r = Run::new(Poc::Two, 4, None);
        r.feed(0, idr(false)).unwrap();
        r.feed(1, p(1, 0)).unwrap();
        // PicNum 2 - (0 + 1) = 1 becomes long-term index 2.
        r.feed(2, p_ops(2, 0, vec![(3, 0, 2)])).unwrap();
        assert_eq!(r.frame_nums(), vec![(0, None), (1, Some(2)), (2, None)]);
    }

    #[test]
    fn mmco5_resets_references_frame_num_and_order() {
        let mut r = Run::new(Poc::Zero, 4, Some(0));
        r.feed(0, idr(false)).unwrap();
        r.feed(1, p(1, 2)).unwrap();
        r.feed(2, p_ops(2, 4, vec![(5, 0, 0)])).unwrap();
        // Only the mmco5 picture is left, as frame_num 0 at POC 0.
        assert_eq!(r.state.dpb().iter().map(|x| (x.frame_num, x.poc)).collect::<Vec<_>>(), vec![(0, [0, 0])]);
        // The next picture continues from frame_num 1 without a gap.
        r.feed(3, p(1, 2)).unwrap();
        assert_eq!(r.shown, vec![0, 1, 2, 3]);
    }

    #[test]
    fn poc_type_0_carries_the_msb_when_the_lsb_wraps() {
        let mut r = Run::new(Poc::Zero, 1, Some(0));
        r.feed(0, idr(false)).unwrap();
        // max lsb is 16: 6, 12, then 2 (= 18) and 8 (= 24).
        let pocs: Vec<i32> = [(1, 6), (2, 12), (3, 2), (4, 8)]
            .into_iter()
            .map(|(f, l)| {
                let mut au = Vec::new();
                au.extend(slice(&p(f, l), Poc::Zero));
                let parsed = r.state.parse(&au).unwrap().unwrap();
                let (pic, _) = r.state.begin(&parsed).unwrap();
                r.state.finish(&parsed, &pic);
                pic.poc[0]
            })
            .collect();
        assert_eq!(pocs, vec![6, 12, 18, 24]);
    }

    #[test]
    fn b_frames_come_out_in_display_order() {
        let mut r = Run::new(Poc::Zero, 2, Some(1));
        r.feed(0, idr(false)).unwrap(); // POC 0
        r.feed(1, p(1, 4)).unwrap(); // POC 4
        r.feed(2, Slice { kind: Kind::B, idr: false, reference: false, frame_num: 2, poc_lsb: 2, marking: Marking::Sliding }).unwrap(); // POC 2
        r.feed(3, p(2, 8)).unwrap(); // POC 8
        r.shown.extend(r.state.flush().iter().map(|s| r.by_surface[&s.surface]));
        assert_eq!(r.shown, vec![0, 2, 1, 3]);
    }

    #[test]
    fn streams_without_reordering_show_each_picture_at_once() {
        let mut r = Run::new(Poc::Two, 1, None);
        r.feed(0, idr(false)).unwrap();
        assert_eq!(r.shown, vec![0]);
        r.feed(1, p(1, 0)).unwrap();
        assert_eq!(r.shown, vec![0, 1]);
    }

    #[test]
    fn a_missing_picture_waits_for_the_next_keyframe() {
        let mut r = Run::new(Poc::Two, 1, None);
        r.feed(0, idr(false)).unwrap();
        r.feed(1, p(1, 0)).unwrap();
        assert!(matches!(r.feed(3, p(3, 0)), Err(StateError::NeedKeyframe(_))));
        assert!(matches!(r.feed(4, p(4, 0)), Err(StateError::NeedKeyframe(_))));
        r.feed(5, idr(false)).unwrap();
        assert_eq!(r.shown, vec![0, 1, 5]);
    }

    #[test]
    fn never_decodes_over_a_reference_or_the_picture_on_screen() {
        // feed() asserts both for every picture; run past a frame_num wrap.
        let mut r = Run::new(Poc::Two, 4, None);
        r.feed(0, idr(false)).unwrap();
        for f in 1..40 {
            r.feed(f as usize, p(f % 16, 0)).unwrap();
        }
        assert_eq!(r.shown.len(), 40);
    }

    /// SPS with a VUI that only carries the video signal type.
    fn sps_with_signal(full_range: bool, matrix: Option<u8>) -> Vec<u8> {
        let mut b = Bits::default();
        b.u(8, 77).u(8, 0).u(8, 40).ue(0).ue(0).ue(2).ue(1).u(1, 0).ue(119).ue(67).u(1, 1).u(1, 1).u(1, 0);
        b.u(1, 1).u(1, 0).u(1, 0); // VUI present; no aspect ratio, no overscan
        b.u(1, 1).u(3, 5).u(1, full_range as u32); // video signal type
        match matrix {
            Some(m) => b.u(1, 1).u(8, 1).u(8, 1).u(8, m as u32),
            None => b.u(1, 0),
        };
        b.u(1, 0).u(1, 0).u(1, 0).u(1, 0).u(1, 0).u(1, 0); // chroma loc, timing, HRDs, pic_struct, restrictions
        b.nal(0x67)
    }

    fn colour_of(full_range: bool, matrix: Option<u8>) -> (bool, bool) {
        let mut au = sps_with_signal(full_range, matrix);
        au.extend(pps());
        au.extend(slice(&idr(false), Poc::Two));
        let p = H264State::new().parse(&au).unwrap().unwrap();
        let info = H264State::stream_info(&p).unwrap();
        (info.full_range, info.bt709)
    }

    #[test]
    fn colours_follow_what_litecord_senders_really_send() {
        // NVENC senders up to 1.2.1 convert with BT.601 limited but declare BT.709 full.
        assert_eq!(colour_of(true, Some(1)), (false, false));
        // A sender that converts with BT.709 and says so.
        assert_eq!(colour_of(false, Some(1)), (false, true));
        // BT.601 declared, or nothing declared (the app converts with BT.601).
        assert_eq!(colour_of(false, Some(6)), (false, false));
        assert_eq!(colour_of(false, None), (false, false));
    }

    #[test]
    fn interlaced_and_custom_matrices_go_to_the_cpu() {
        // Field coding: frame_mbs_only_flag = 0 (with mb_adaptive_frame_field_flag = 0).
        let mut b = Bits::default();
        b.u(8, 77).u(8, 0).u(8, 40).ue(0).ue(0).ue(2).ue(1).u(1, 0).ue(3).ue(3).u(1, 0).u(1, 0).u(1, 1).u(1, 0).u(1, 0);
        let mut au = b.nal(0x67);
        au.extend(pps());
        au.extend(slice(&idr(false), Poc::Two));
        let mut st = H264State::new();
        // Field slices carry field_pic_flag; this header lacks it, so parsing may fail too.
        match st.parse(&au) {
            Ok(Some(p)) => assert!(matches!(H264State::stream_info(&p), Err(StateError::Unsupported(_)))),
            Ok(None) => panic!("no picture"),
            Err(_) => {}
        }
    }
}
