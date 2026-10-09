//! Shared embedding packing + fp16 conversion for writing the `embeddings`
//! sidecar. The vision service returns f32 vectors; the on-disk sidecar format
//! is fp16 (little-endian) to halve the footprint. Readers live in
//! `media_cache::readers::read_embeddings` and do the reverse conversion.

/// IEEE-754 fp32 → fp16 conversion (round-to-nearest-even via truncation).
/// Mirror of `media_cache::readers::f16_to_f32`.
pub fn f32_to_f16(f: f32) -> u16 {
    let bits = f.to_bits();
    let sign = ((bits >> 31) & 0x1) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x7fffff;
    if exp == 0xff {
        return (sign << 15) | 0x7c00 | (mant >> 13) as u16;
    }
    let new_exp = exp - 127 + 15;
    if new_exp >= 0x1f {
        return (sign << 15) | 0x7c00;
    }
    if new_exp <= 0 {
        if new_exp < -10 {
            return sign << 15;
        }
        let mant = (mant | 0x800000) >> (1 - new_exp);
        return (sign << 15) | (mant >> 13) as u16;
    }
    (sign << 15) | ((new_exp as u16) << 10) | ((mant >> 13) as u16)
}

/// Pack a batch of f32 embedding vectors into the fp16 wire format used by the
/// `embeddings.bin` sidecar: each f32 scalar becomes a little-endian `u16`.
pub fn pack_fp16(embeddings: &[Vec<f32>]) -> Vec<u8> {
    let total = embeddings.iter().map(|v| v.len()).sum::<usize>();
    let mut packed = Vec::with_capacity(total * 2);
    for row in embeddings {
        for &f in row {
            packed.extend_from_slice(&f32_to_f16(f).to_le_bytes());
        }
    }
    packed
}

/// L2-normalise each row in place. The vision service is expected to return
/// normalised vectors already; this is a defensive re-normalisation so the
/// sidecar is always unit-length and cosine similarity works without further
/// pre-processing by the reader.
pub fn l2_normalise(embeddings: &mut [Vec<f32>]) {
    for row in embeddings {
        let norm = row.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in row.iter_mut() {
                *v /= norm;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f16_to_f32(half: u16) -> f32 {
        let sign = ((half >> 15) & 0x1) as u32;
        let exp = ((half >> 10) & 0x1f) as u32;
        let mant = (half & 0x3ff) as u32;
        let f_bits = if exp == 0 {
            if mant == 0 {
                sign << 31
            } else {
                let mut m = mant;
                let mut e = 1u32;
                while (m & 0x400) == 0 {
                    m <<= 1;
                    e = e.wrapping_sub(1);
                }
                let m = m & 0x3ff;
                (sign << 31) | ((127u32.wrapping_add(e).wrapping_sub(15)) << 23) | (m << 13)
            }
        } else if exp == 0x1f {
            (sign << 31) | (0xff << 23) | (mant << 13)
        } else {
            (sign << 31) | ((exp + (127 - 15)) << 23) | (mant << 13)
        };
        f32::from_bits(f_bits)
    }

    #[test]
    fn roundtrip_fp16_known_values() {
        for &f in &[0.0_f32, 1.0, -1.0, 0.5, -0.25, 2.5] {
            let back = f16_to_f32(f32_to_f16(f));
            assert!((back - f).abs() < 1e-3, "f16 roundtrip {f} -> {back}");
        }
    }

    #[test]
    fn fp16_inf_overflows_to_inf() {
        let v = 1e30_f32;
        let h = f32_to_f16(v);
        assert_eq!(h & 0x7fff, 0x7c00);
    }

    #[test]
    fn pack_fp16_lengths_match() {
        let embeddings = vec![vec![0.5_f32; 4], vec![-0.5_f32; 4]];
        let packed = pack_fp16(&embeddings);
        assert_eq!(packed.len(), 2 * 4 * 2);
    }

    #[test]
    fn l2_normalise_is_unit_length() {
        let mut embeddings = vec![vec![3.0_f32, 4.0_f32]];
        l2_normalise(&mut embeddings);
        let norm = embeddings[0].iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[test]
    fn l2_normalise_zero_vector_is_zero() {
        let mut embeddings = vec![vec![0.0_f32, 0.0, 0.0]];
        l2_normalise(&mut embeddings);
        assert_eq!(embeddings[0], vec![0.0, 0.0, 0.0]);
    }
}
