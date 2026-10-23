//! HDR transfer functions shared by decode, preview and export (BT.2100, BT.2408).

/// An HDR transfer function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hdr {
    /// SMPTE ST 2084.
    Pq,
    /// ARIB STD-B67 (BT.2100 HLG).
    Hlg,
}

impl Hdr {
    /// From an FFmpeg transfer name; `None` for SDR ones.
    pub fn from_transfer(name: &str) -> Option<Self> {
        match name {
            "smpte2084" => Some(Self::Pq),
            "arib-std-b67" => Some(Self::Hlg),
            _ => None,
        }
    }
}

/// BT.2408 HDR reference white: SDR 100% maps here, and back.
pub const REFERENCE_WHITE: f32 = 203.0;

const M1: f32 = 0.159_301_76;
const M2: f32 = 78.843_75;
const C1: f32 = 0.835_937_5;
const C2: f32 = 18.851_563;
const C3: f32 = 18.687_5;

/// PQ signal (0–1) to cd/m² (0–10000).
pub fn pq_to_nits(e: f32) -> f32 {
    let p = e.clamp(0.0, 1.0).powf(1.0 / M2);
    10_000.0 * ((p - C1).max(0.0) / (C2 - C3 * p)).powf(1.0 / M1)
}

/// cd/m² to PQ signal.
pub fn nits_to_pq(nits: f32) -> f32 {
    let y = (nits / 10_000.0).clamp(0.0, 1.0).powf(M1);
    ((C1 + C2 * y) / (1.0 + C3 * y)).powf(M2)
}

/// Linear BT.709 RGB to linear BT.2020 RGB (BT.2087).
pub const BT709_TO_BT2020: [[f32; 3]; 3] = [
    [0.6274, 0.3293, 0.0433],
    [0.0691, 0.9195, 0.0114],
    [0.0164, 0.0880, 0.8956],
];

/// Linear BT.2020 RGB to linear BT.709 RGB, the inverse of [`BT709_TO_BT2020`].
pub const BT2020_TO_BT709: [[f32; 3]; 3] = [
    [1.6605, -0.5876, -0.0728],
    [-0.1246, 1.1329, -0.0083],
    [-0.0182, -0.1006, 1.1187],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pq_round_trips_at_the_reference_points() {
        // BT.2408 table: 203 cd/m² is a 58% PQ signal, 1000 cd/m² 75%.
        assert!((nits_to_pq(REFERENCE_WHITE) - 0.58).abs() < 0.005);
        assert!((nits_to_pq(1000.0) - 0.7518).abs() < 0.001);
        for nits in [0.01, 1.0, 26.0, 203.0, 1000.0, 10_000.0] {
            let back = pq_to_nits(nits_to_pq(nits));
            assert!((back - nits).abs() / nits < 1e-3, "{nits} -> {back}");
        }
        assert_eq!(pq_to_nits(0.0), 0.0);
    }

    #[test]
    fn gamut_matrices_invert() {
        for (i, row) in BT2020_TO_BT709.iter().enumerate() {
            for j in 0..3 {
                let v: f32 = row.iter().zip(BT709_TO_BT2020).map(|(r, m)| r * m[j]).sum();
                assert!((v - f32::from(i == j)).abs() < 2e-3, "{i},{j}: {v}");
            }
        }
    }
}
