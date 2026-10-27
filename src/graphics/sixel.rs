//! Sixel decoding (DCS P1;P2;P3 q ... ST) to RGBA pixels.

/// Largest image accepted, in pixels per side.
const MAX_SIDE: usize = 8192;

/// The VT340 default palette (percent RGB), used until a program defines colors.
const VT340: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (20, 20, 80),
    (80, 13, 13),
    (20, 80, 20),
    (80, 20, 80),
    (20, 80, 80),
    (80, 80, 20),
    (53, 53, 53),
    (26, 26, 26),
    (33, 33, 60),
    (60, 26, 26),
    (33, 60, 33),
    (60, 33, 60),
    (33, 60, 60),
    (60, 60, 33),
    (80, 80, 80),
];

fn pct(v: u32) -> u8 {
    ((v.min(100) * 255 + 50) / 100) as u8
}

fn hls_to_rgb(h: u32, l: u32, s: u32) -> (u8, u8, u8) {
    // Sixel hue 0 is blue; shift so 0 is red as in the usual HLS formula.
    let h = ((h % 360 + 240) % 360) as f64;
    let l = l.min(100) as f64 / 100.0;
    let s = s.min(100) as f64 / 100.0;
    if s == 0.0 {
        let v = (l * 255.0).round() as u8;
        return (v, v, v);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |t: f64| {
        let t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round() as u8
    };
    let h = h / 360.0;
    (f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
}

/// Decode sixel `data` (the bytes after 'q'). Returns (width, height, RGBA pixels).
pub fn decode(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut palette: Vec<(u8, u8, u8)> = (0..256)
        .map(|i| VT340.get(i).map(|&(r, g, b)| (pct(r as u32), pct(g as u32), pct(b as u32))).unwrap_or((0, 0, 0)))
        .collect();
    let mut color = 0usize;
    let (mut x, mut y) = (0usize, 0usize);
    let (mut width, mut height) = (0usize, 0usize);
    // Pixels as palette-resolved RGBA, grown as the image is drawn.
    let mut img: Vec<u8> = Vec::new();
    let mut stride = 0usize;

    let mut i = 0;
    let num = |i: &mut usize| -> u32 {
        let mut n: u32 = 0;
        while let Some(d) = data.get(*i).filter(|c| c.is_ascii_digit()) {
            n = n.saturating_mul(10).saturating_add((d - b'0') as u32);
            *i += 1;
        }
        n
    };

    while i < data.len() {
        let c = data[i];
        i += 1;
        match c {
            b'"' => {
                // Raster attributes: aspect numerator;denominator;width;height
                let mut p = [0u32; 4];
                for (k, slot) in p.iter_mut().enumerate() {
                    *slot = num(&mut i);
                    if k < 3 && data.get(i) == Some(&b';') {
                        i += 1;
                    } else {
                        break;
                    }
                }
                let (w, h) = (p[2] as usize, p[3] as usize);
                if w > 0 && h > 0 {
                    if !grow(&mut img, &mut stride, w, h) {
                        return None;
                    }
                    width = width.max(w);
                    height = height.max(h);
                }
            }
            b'#' => {
                let reg = num(&mut i) as usize;
                if data.get(i) == Some(&b';') {
                    let mut p = [0u32; 4];
                    for slot in p.iter_mut() {
                        if data.get(i) != Some(&b';') {
                            break;
                        }
                        i += 1;
                        *slot = num(&mut i);
                    }
                    let rgb = match p[0] {
                        1 => hls_to_rgb(p[1], p[2], p[3]),
                        _ => (pct(p[1]), pct(p[2]), pct(p[3])),
                    };
                    if let Some(slot) = palette.get_mut(reg) {
                        *slot = rgb;
                    }
                }
                color = reg.min(255);
            }
            b'!' => {
                let count = num(&mut i).max(1) as usize;
                if let Some(&s) = data.get(i).filter(|c| (b'?'..=b'~').contains(c)) {
                    i += 1;
                    if !paint(&mut img, &mut stride, &mut width, &mut height, x, y, s, count, palette[color]) {
                        return None;
                    }
                    x += count;
                }
            }
            b'$' => x = 0,
            b'-' => {
                x = 0;
                y += 6;
            }
            b'?'..=b'~' => {
                if !paint(&mut img, &mut stride, &mut width, &mut height, x, y, c, 1, palette[color]) {
                    return None;
                }
                x += 1;
            }
            _ => {}
        }
    }
    if width == 0 || height == 0 {
        return None;
    }
    // Copy out the used area.
    let mut out = vec![0u8; width * height * 4];
    for r in 0..height {
        let from = r * stride * 4;
        if from + width * 4 <= img.len() {
            out[r * width * 4..(r + 1) * width * 4].copy_from_slice(&img[from..from + width * 4]);
        }
    }
    Some((width as u32, height as u32, out))
}

/// Grow the pixel buffer to at least `w` x `h`, keeping what is drawn. False if too large.
fn grow(img: &mut Vec<u8>, stride: &mut usize, w: usize, h: usize) -> bool {
    if w > MAX_SIDE || h > MAX_SIDE {
        return false;
    }
    let rows = img.len() / 4 / (*stride).max(1);
    if w > *stride || h > rows {
        let new_stride = w.max(*stride).next_power_of_two().min(MAX_SIDE).max(w);
        let new_rows = h.max(rows).next_power_of_two().min(MAX_SIDE).max(h);
        let mut grown = vec![0u8; new_stride * new_rows * 4];
        for r in 0..rows {
            let from = r * *stride * 4;
            grown[r * new_stride * 4..r * new_stride * 4 + *stride * 4].copy_from_slice(&img[from..from + *stride * 4]);
        }
        *img = grown;
        *stride = new_stride;
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn paint(
    img: &mut Vec<u8>,
    stride: &mut usize,
    width: &mut usize,
    height: &mut usize,
    x: usize,
    y: usize,
    sixel: u8,
    count: usize,
    (r, g, b): (u8, u8, u8),
) -> bool {
    let bits = sixel - b'?';
    if bits == 0 {
        // Still extends the width, like a space.
        *width = (*width).max(x + count);
        return x + count <= MAX_SIDE;
    }
    if !grow(img, stride, x + count, y + 6) {
        return false;
    }
    *width = (*width).max(x + count);
    for bit in 0..6 {
        if bits & (1 << bit) == 0 {
            continue;
        }
        let row = y + bit;
        *height = (*height).max(row + 1);
        for col in x..x + count {
            let o = (row * *stride + col) * 4;
            img[o..o + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_colors_repeats_and_bands() {
        // Red 2x6 block, then next band a green column of 1 pixel (bit 0) at x=0.
        let data = b"\"1;1;2;7#1;2;100;0;0#1!2~-#2;2;0;100;0#2@";
        let (w, h, px) = decode(data).unwrap();
        assert_eq!((w, h), (2, 7));
        assert_eq!(&px[0..4], &[255, 0, 0, 255]);
        assert_eq!(&px[4..8], &[255, 0, 0, 255]);
        let row6 = 6 * 2 * 4;
        assert_eq!(&px[row6..row6 + 4], &[0, 255, 0, 255]);
        // Unpainted pixels stay transparent.
        assert_eq!(px[row6 + 7], 0);
    }

    #[test]
    fn rejects_empty_and_huge() {
        assert!(decode(b"").is_none());
        assert!(decode(b"\"1;1;99999;99999").is_none());
        assert!(decode(b"!99999~").is_none());
    }

    #[test]
    fn huge_hue_does_not_overflow() {
        // Found by fuzzing: HLS color with hue near u32::MAX.
        assert!(decode(b"#1;1;4294967295;50;50#1~").is_some());
    }

    #[test]
    fn hls_primaries() {
        assert_eq!(hls_to_rgb(120, 50, 100), (255, 0, 0));
        assert_eq!(hls_to_rgb(0, 50, 100), (0, 0, 255));
    }
}
