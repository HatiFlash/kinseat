//! Pixel coverage for the notice ring and the preview dot (independent of Win32).
use std::f64::consts::{FRAC_PI_2, TAU};

fn disc(x: f64, y: f64, cx: f64, cy: f64, radius: f64) -> f64 {
    (radius + 0.5 - (x - cx).hypot(y - cy)).clamp(0.0, 1.0)
}

/// Coverage of a round-capped, clockwise arc starting at 12 o'clock.
pub fn arc(x: f64, y: f64, cx: f64, cy: f64, radius: f64, width: f64, fraction: f64) -> f64 {
    if fraction <= 0.0 {
        return 0.0;
    }
    let distance = (x - cx).hypot(y - cy);
    let radial = (width / 2.0 + 0.5 - (distance - radius).abs()).clamp(0.0, 1.0);
    if fraction >= 1.0 {
        return radial;
    }
    let sweep = fraction * TAU;
    let angle = (y - cy).atan2(x - cx) + FRAC_PI_2;
    let angle = angle.rem_euclid(TAU);
    if angle <= sweep {
        return radial;
    }
    let start = disc(x, y, cx, cy - radius, width / 2.0);
    let end = disc(
        x,
        y,
        cx + radius * (sweep - FRAC_PI_2).cos(),
        cy + radius * (sweep - FRAC_PI_2).sin(),
        width / 2.0,
    );
    start.max(end)
}

/// Opaque BGR pixels for a normal (constant-opacity layered) GDI window.
/// Coverage is blended into the panel background, not stored as non-premultiplied alpha.
#[allow(clippy::too_many_arguments)]
pub fn raster(
    width: usize,
    height: usize,
    cx: f64,
    cy: f64,
    radius: f64,
    stroke: f64,
    fraction: f64,
    track: [u8; 3],
    active: [u8; 3],
    background: [u8; 3],
) -> Vec<u8> {
    let mut pixels = vec![0; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let px = x as f64 + 0.5;
            let py = y as f64 + 0.5;
            let base =
                (stroke / 2.0 + 0.5 - ((px - cx).hypot(py - cy) - radius).abs()).clamp(0.0, 1.0);
            let progress = arc(px, py, cx, cy, radius, stroke, fraction);
            let i = (y * width + x) * 4;
            for c in 0..3 {
                let value = background[c] as f64 * (1.0 - base) + track[c] as f64 * base;
                pixels[i + c] =
                    (value * (1.0 - progress) + active[c] as f64 * progress).round() as u8;
            }
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ring_coverage_and_progress() {
        assert!(arc(10.5, 1.5, 10.5, 10.5, 9.0, 3.0, 1.0) > 0.99);
        assert_eq!(arc(10.5, 10.5, 10.5, 10.5, 9.0, 3.0, 1.0), 0.0);
        assert_eq!(arc(30.0, 10.5, 10.5, 10.5, 9.0, 3.0, 1.0), 0.0);
        assert_eq!(arc(10.5, 1.5, 10.5, 10.5, 9.0, 3.0, 0.0), 0.0);
        assert!(arc(19.5, 10.5, 10.5, 10.5, 9.0, 3.0, 0.5) > 0.99);
        assert_eq!(arc(1.5, 10.5, 10.5, 10.5, 9.0, 3.0, 0.25), 0.0);
        assert_eq!(
            arc(1.5, 10.5, 10.5, 10.5, 9.0, 3.0, 1.0),
            arc(19.5, 10.5, 10.5, 10.5, 9.0, 3.0, 1.0)
        );
        assert!(arc(10.5, 0.0, 10.5, 10.5, 9.0, 3.0, 1.0) > 0.0);
    }
    #[test]
    fn raster_has_smooth_edges() {
        let pixels = raster(25, 25, 12.5, 12.5, 9.0, 3.0, 1.0, [30; 3], [255; 3], [0; 3]);
        let at = |x: usize, y: usize| pixels[(y * 25 + x) * 4];
        assert_eq!(at(12, 12), 0);
        assert_eq!(at(0, 0), 0);
        assert_eq!(at(12, 3), 255);
        assert_eq!(at(3, 12), at(21, 12));
        assert!(pixels
            .chunks_exact(4)
            .any(|pixel| pixel[0] > 0 && pixel[0] < 255));
    }
}
