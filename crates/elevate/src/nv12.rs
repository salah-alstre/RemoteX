//! BGRA -> NV12 with scaling, for pictures the service captures where hardware duplication is not
//! available (the secure desktop). Plain integer math, BT.709 limited range like the rest of the pipeline.

/// Scales a `sw` x `sh` BGRA picture to `dw` x `dh` (both even) and returns NV12 bytes.
pub fn bgra_to_nv12(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Option<Vec<u8>> {
    if sw == 0
        || sh == 0
        || dw == 0
        || dh == 0
        || !dw.is_multiple_of(2)
        || !dh.is_multiple_of(2)
        || src.len() < sw * sh * 4
    {
        return None;
    }
    let mut out = vec![0u8; dw * dh * 3 / 2];
    let (luma, chroma) = out.split_at_mut(dw * dh);
    let x_map: Vec<usize> = (0..dw).map(|x| (x * sw / dw).min(sw - 1)).collect();
    for y in 0..dh {
        let sy = (y * sh / dh).min(sh - 1);
        let row = &src[sy * sw * 4..(sy + 1) * sw * 4];
        for x in 0..dw {
            let p = &row[x_map[x] * 4..x_map[x] * 4 + 4];
            let (b, g, r) = (p[0] as i32, p[1] as i32, p[2] as i32);
            luma[y * dw + x] = (((47 * r + 157 * g + 16 * b + 128) >> 8) + 16).clamp(16, 235) as u8;
            if y % 2 == 0 && x % 2 == 0 {
                let u = (((-26 * r - 87 * g + 112 * b + 128) >> 8) + 128).clamp(16, 240) as u8;
                let v = (((112 * r - 102 * g - 10 * b + 128) >> 8) + 128).clamp(16, 240) as u8;
                let ci = (y / 2) * dw + x;
                chroma[ci] = u;
                chroma[ci + 1] = v;
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_and_white_map_to_limited_range() {
        let black = bgra_to_nv12(&[0, 0, 0, 255].repeat(16), 4, 4, 4, 4).unwrap();
        assert!(black[..16].iter().all(|&y| y == 16));
        assert!(black[16..].iter().all(|&c| c == 128));
        let white = bgra_to_nv12(&[255, 255, 255, 255].repeat(16), 4, 4, 4, 4).unwrap();
        assert!(white[..16].iter().all(|&y| y >= 234));
    }

    #[test]
    fn scales_down_and_checks_sizes() {
        let src = vec![90u8; 8 * 8 * 4];
        let out = bgra_to_nv12(&src, 8, 8, 4, 4).unwrap();
        assert_eq!(out.len(), 4 * 4 * 3 / 2);
        assert!(bgra_to_nv12(&src, 8, 8, 5, 4).is_none(), "odd width");
        assert!(bgra_to_nv12(&src[..10], 8, 8, 4, 4).is_none(), "short input");
    }
}
