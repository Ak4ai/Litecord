//! Pre-shaped images. Slint's software renderer (the default) clips children to a
//! plain rectangle and ignores `border-radius` when clipping, so a square avatar inside
//! a round frame stays square. Shaping the pixels once, when the image is loaded, makes
//! it round on every renderer.

/// Makes everything outside a rounded rectangle of `radius` transparent, with a soft
/// 1 px edge. `radius` = half the side gives a circle.
pub fn round_corners(rgba: &mut [u8], width: u32, height: u32, radius: f32) {
    let (w, h) = (width as f32, height as f32);
    let r = radius.clamp(0.0, w.min(h) / 2.0);
    for y in 0..height {
        for x in 0..width {
            // Signed distance from the pixel centre to the rounded rectangle's edge.
            let px = (x as f32 + 0.5 - w / 2.0).abs() - (w / 2.0 - r);
            let py = (y as f32 + 0.5 - h / 2.0).abs() - (h / 2.0 - r);
            let outside = (px.max(0.0).powi(2) + py.max(0.0).powi(2)).sqrt() + px.max(py).min(0.0) - r;
            let coverage = (0.5 - outside).clamp(0.0, 1.0);
            if coverage < 1.0 {
                let a = &mut rgba[((y * width + x) * 4 + 3) as usize];
                *a = (*a as f32 * coverage).round() as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_at(rgba: &[u8], width: u32, x: u32, y: u32) -> u8 {
        rgba[((y * width + x) * 4 + 3) as usize]
    }

    #[test]
    fn circle_clears_the_corners_and_keeps_the_middle() {
        let (w, h) = (40, 40);
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        round_corners(&mut rgba, w, h, 20.0);
        for (x, y) in [(0, 0), (39, 0), (0, 39), (39, 39), (3, 3)] {
            assert_eq!(alpha_at(&rgba, w, x, y), 0, "corner {x},{y}");
        }
        for (x, y) in [(20, 20), (20, 1), (1, 20), (38, 20), (20, 38)] {
            assert!(alpha_at(&rgba, w, x, y) > 200, "inside {x},{y}");
        }
        // Colour channels are left alone (only alpha changes).
        assert_eq!(rgba[0], 255);
    }

    #[test]
    fn edge_pixels_are_blended_not_cut() {
        let (w, h) = (40, 40);
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        round_corners(&mut rgba, w, h, 20.0);
        let partial = (0..w * h)
            .map(|i| rgba[(i * 4 + 3) as usize])
            .filter(|&a| a > 0 && a < 255)
            .count();
        assert!(partial > 20, "expected a soft edge, got {partial} partial pixels");
    }

    #[test]
    fn rounded_square_keeps_its_straight_edges() {
        let (w, h) = (48, 48);
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        round_corners(&mut rgba, w, h, 16.0);
        assert_eq!(alpha_at(&rgba, w, 0, 0), 0);
        assert_eq!(alpha_at(&rgba, w, 24, 0), 255);
        assert_eq!(alpha_at(&rgba, w, 0, 24), 255);
    }

    #[test]
    fn existing_transparency_is_kept() {
        let (w, h) = (10, 10);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        round_corners(&mut rgba, w, h, 5.0);
        assert!(rgba.iter().all(|&b| b == 0));
    }
}
