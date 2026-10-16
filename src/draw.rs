//! Tiny software-drawing helpers for 0x00RRGGBB buffers (softbuffer format).
//! Everything clips to the buffer, so callers can draw partly off-screen.

/// A buffer view: pixels plus width/height.
pub struct Canvas<'a> {
    pub buf: &'a mut [u32],
    pub width: usize,
    pub height: usize,
}

impl Canvas<'_> {
    /// Fills a rectangle, clipped to the canvas.
    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: u32) {
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = ((x as i64 + w as i64).max(0) as usize).min(self.width);
        let y1 = ((y as i64 + h as i64).max(0) as usize).min(self.height);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        for row in y0..y1 {
            let start = row * self.width;
            self.buf[start + x0..start + x1].fill(color);
        }
    }

    /// Draws a rectangle outline `t` px thick (inside the rectangle).
    pub fn stroke_rect(&mut self, x: i32, y: i32, w: u32, h: u32, t: u32, color: u32) {
        let t = t.min(w).min(h);
        self.fill_rect(x, y, w, t, color);
        self.fill_rect(x, y + h as i32 - t as i32, w, t, color);
        self.fill_rect(x, y, t, h, color);
        self.fill_rect(x + w as i32 - t as i32, y, t, h, color);
    }

    /// Draws `text` with the built-in 5x7 font at `scale`.
    pub fn text(&mut self, x: i32, y: i32, text: &str, scale: u32, color: u32) {
        let mut cx = x;
        for ch in text.chars() {
            if let Some(rows) = glyph(ch) {
                for (gy, bits) in rows.iter().enumerate() {
                    for gx in 0..GLYPH_W {
                        if bits & (1 << (GLYPH_W - 1 - gx)) != 0 {
                            self.fill_rect(
                                cx + (gx * scale) as i32,
                                y + (gy as u32 * scale) as i32,
                                scale,
                                scale,
                                color,
                            );
                        }
                    }
                }
            }
            cx += text_advance(scale) as i32;
        }
    }
}

pub const GLYPH_W: u32 = 5;
pub const GLYPH_H: u32 = 7;

/// Horizontal advance per character at `scale`.
pub fn text_advance(scale: u32) -> u32 {
    (GLYPH_W + 1) * scale
}

/// Width of `text` in pixels at `scale`.
pub fn text_width(text: &str, scale: u32) -> u32 {
    let n = text.chars().count() as u32;
    (n * text_advance(scale)).saturating_sub(scale)
}

/// 5x7 bitmap glyphs, one u8 per row, MSB-first in the low 5 bits.
fn glyph(ch: char) -> Option<[u8; 7]> {
    Some(match ch {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        '×' | 'x' => [0x00, 0x00, 0x11, 0x0A, 0x04, 0x0A, 0x11],
        ',' => [0x00, 0x00, 0x00, 0x00, 0x06, 0x04, 0x08],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        ' ' => [0; 7],
        _ => return None,
    })
}

/// Packs RGB into 0x00RRGGBB.
pub fn rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

/// Scales each channel by `factor`/256.
pub fn darken(c: u32, factor: u32) -> u32 {
    let channel = |shift: u32| (((c >> shift) & 0xFF) * factor / 256) << shift;
    channel(16) | channel(8) | channel(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(buf: &mut [u32], w: usize, h: usize) -> Canvas<'_> {
        Canvas {
            buf,
            width: w,
            height: h,
        }
    }

    #[test]
    fn fill_rect_clips() {
        let mut buf = vec![0u32; 16];
        canvas(&mut buf, 4, 4).fill_rect(-2, -2, 4, 4, 1);
        assert_eq!(buf.iter().filter(|&&p| p == 1).count(), 4);
        assert_eq!(buf[0], 1);
        assert_eq!(buf[5], 1);
        assert_eq!(buf[2], 0);
    }

    #[test]
    fn fill_rect_fully_outside_is_noop() {
        let mut buf = vec![0u32; 16];
        canvas(&mut buf, 4, 4).fill_rect(10, 10, 4, 4, 1);
        canvas(&mut buf, 4, 4).fill_rect(-10, 0, 4, 4, 1);
        assert!(buf.iter().all(|&p| p == 0));
    }

    #[test]
    fn stroke_rect_leaves_inside() {
        let mut buf = vec![0u32; 100];
        canvas(&mut buf, 10, 10).stroke_rect(2, 2, 6, 6, 2, 9);
        assert_eq!(buf[2 * 10 + 2], 9);
        assert_eq!(buf[7 * 10 + 7], 9);
        assert_eq!(buf[5 * 10 + 5], 0);
        assert_eq!(buf[0], 0);
    }

    #[test]
    fn text_draws_pixels() {
        let mut buf = vec![0u32; 20 * 10];
        canvas(&mut buf, 20, 10).text(0, 0, "1", 1, 7);
        // '1' top row is 0x04 -> pixel x=2
        assert_eq!(buf[2], 7);
        assert_eq!(buf[0], 0);
    }

    #[test]
    fn text_width_matches_advance() {
        assert_eq!(text_width("", 2), 0);
        assert_eq!(text_width("12", 1), 11);
        assert_eq!(text_width("994 × 645", 2), 9 * 12 - 2);
    }

    #[test]
    fn all_label_chars_have_glyphs() {
        for ch in "0123456789×x, -".chars() {
            assert!(glyph(ch).is_some(), "missing glyph {ch:?}");
        }
    }

    #[test]
    fn darken_works() {
        assert_eq!(darken(0x00FF_FFFF, 128), rgb(127, 127, 127));
        assert_eq!(darken(0, 128), 0);
    }
}
