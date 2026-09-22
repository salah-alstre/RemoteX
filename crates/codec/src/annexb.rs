//! Annex-B bitstream helpers.

/// Splits an Annex-B buffer into NAL unit slices (without start codes).
pub fn split_nals(data: &[u8]) -> Vec<&[u8]> {
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
    starts
        .iter()
        .enumerate()
        .map(|(n, &s)| {
            let mut e = starts.get(n + 1).map(|next| next - 3).unwrap_or(data.len());
            // Trailing zero belongs to a four-byte start code of the next unit.
            while e > s && data[e - 1] == 0 {
                e -= 1;
            }
            &data[s..e]
        })
        .collect()
}

/// NAL unit type of each unit. H.264 uses the low 5 bits, HEVC bits 1..=6.
pub fn nal_types(data: &[u8], hevc: bool) -> Vec<u8> {
    split_nals(data)
        .iter()
        .filter(|n| !n.is_empty())
        .map(|n| if hevc { (n[0] >> 1) & 0x3f } else { n[0] & 0x1f })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_three_and_four_byte_start_codes() {
        let data = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9];
        let nals = split_nals(&data);
        assert_eq!(nals.len(), 3);
        assert_eq!(nal_types(&data, false), vec![7, 8, 5]);
        assert_eq!(nals[0], &[0x67, 1, 2]);
    }

    #[test]
    fn hevc_types() {
        let data = [
            0, 0, 0, 1, 0x40, 1, 0, 0, 0, 1, 0x42, 1, 0, 0, 0, 1, 0x44, 1, 0, 0, 0, 1, 0x26, 1,
        ];
        assert_eq!(nal_types(&data, true), vec![32, 33, 34, 19]);
    }
}
