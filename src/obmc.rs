//! Overlapped block motion compensation: 14496-2 7.6.6 (`obmc_disable`
//! 0) and, for the short video header, ITU-T H.263 Annex F.3 (the
//! Advanced Prediction mode), which define the same process.
//!
//! Each luminance sample of an 8x8 block is a weighted sum of three
//! predictions — with the block's own vector, with the vector of the block
//! above or below it (whichever border is nearer), and with the vector of
//! the block left or right of it — `(q H0 + r H1 + s H2 + 4) / 8`.
//! Chrominance is predicted as without OBMC.

use crate::mbstate::{MbKind, MbState};
use crate::mc;
use crate::picture::Pic;

/// Figure 7-21 / F.2: the weights of the block's own vector, raster order.
pub(crate) const H0: [u8; 64] = [
    4, 5, 5, 5, 5, 5, 5, 4, //
    5, 5, 5, 5, 5, 5, 5, 5, //
    5, 5, 6, 6, 6, 6, 5, 5, //
    5, 5, 6, 6, 6, 6, 5, 5, //
    5, 5, 6, 6, 6, 6, 5, 5, //
    5, 5, 6, 6, 6, 6, 5, 5, //
    5, 5, 5, 5, 5, 5, 5, 5, //
    4, 5, 5, 5, 5, 5, 5, 4,
];

/// Figure 7-22 / F.3: the weights of the vector above (top half) or below
/// (bottom half).
pub(crate) const H1: [u8; 64] = [
    2, 2, 2, 2, 2, 2, 2, 2, //
    1, 1, 2, 2, 2, 2, 1, 1, //
    1, 1, 1, 1, 1, 1, 1, 1, //
    1, 1, 1, 1, 1, 1, 1, 1, //
    1, 1, 1, 1, 1, 1, 1, 1, //
    1, 1, 1, 1, 1, 1, 1, 1, //
    1, 1, 2, 2, 2, 2, 1, 1, //
    2, 2, 2, 2, 2, 2, 2, 2,
];

/// Figure 7-23 / F.4: the weights of the vector left (left half) or right
/// (right half).
pub(crate) const H2: [u8; 64] = [
    2, 1, 1, 1, 1, 1, 1, 2, //
    2, 2, 1, 1, 1, 1, 2, 2, //
    2, 2, 1, 1, 1, 1, 2, 2, //
    2, 2, 1, 1, 1, 1, 2, 2, //
    2, 2, 1, 1, 1, 1, 2, 2, //
    2, 2, 1, 1, 1, 1, 2, 2, //
    2, 2, 1, 1, 1, 1, 2, 2, //
    2, 1, 1, 1, 1, 1, 1, 2,
];

/// The remote vectors of each luminance block of macroblock `(mbx, mby)`:
/// `[above, below, left, right]` per block, given its own block vectors
/// `own`. A neighbour outside the VOP, intra, or (`excluded`) across a
/// boundary OBMC does not cross gives the block's own vector; one not
/// coded gives zero; the block below a macroblock's bottom blocks is never
/// used (its macroblock is not decoded yet), the own vector standing in.
pub(crate) fn remote_vectors(
    st: &MbState,
    mbx: usize,
    mby: usize,
    own: &[[i32; 2]; 4],
    excluded: impl Fn(usize) -> bool,
) -> [[[i32; 2]; 4]; 4] {
    let (mbw, mbh) = (st.mbw as isize, st.mbh as isize);
    // The vector of block `k` of macroblock (x, y), seen from block `me`.
    let get = |x: isize, y: isize, k: usize, me: usize| -> [i32; 2] {
        if x == mbx as isize && y == mby as isize {
            return own[k];
        }
        if x < 0 || y < 0 || x >= mbw || y >= mbh {
            return own[me];
        }
        let i = y as usize * st.mbw + x as usize;
        if excluded(i) {
            return own[me];
        }
        match st.kind[i] {
            MbKind::Intra => own[me],
            MbKind::Skipped => [0, 0],
            MbKind::Inter => st.get_mv(x as usize, y as usize, k),
        }
    };
    let (x, y) = (mbx as isize, mby as isize);
    let mut out = [[[0; 2]; 4]; 4];
    for (k, o) in out.iter_mut().enumerate() {
        let (bx, by) = (k & 1, k >> 1);
        o[0] = if by == 0 {
            get(x, y - 1, k + 2, k)
        } else {
            own[k - 2]
        };
        o[1] = if by == 0 { own[k + 2] } else { own[k] };
        o[2] = if bx == 0 {
            get(x - 1, y, k + 1, k)
        } else {
            own[k - 1]
        };
        o[3] = if bx == 1 {
            get(x + 1, y, k - 1, k)
        } else {
            own[k + 1]
        };
    }
    out
}

/// The overlapped luminance prediction of macroblock `(mbx, mby)` from
/// `src` with block vectors `own` and their `remote` vectors (see
/// [`remote_vectors`]), in half or quarter samples.
#[allow(clippy::too_many_arguments)]
pub(crate) fn predict_luma(
    src: &Pic,
    mbx: usize,
    mby: usize,
    own: &[[i32; 2]; 4],
    remote: &[[[i32; 2]; 4]; 4],
    rounding: bool,
    qpel: bool,
    out: &mut [u8; 256],
) {
    let plane = src.ref_plane(0);
    let pred = |x: i32, y: i32, v: [i32; 2], o: &mut [u8; 64]| {
        if qpel {
            mc::qpel(plane, x, y, v[0], v[1], 8, 8, rounding, o, 8);
        } else {
            mc::halfpel(plane, x, y, v[0], v[1], 8, 8, rounding, o, 8);
        }
    };
    for k in 0..4 {
        let (x, y) = (
            mbx as i32 * 16 + (k & 1) as i32 * 8,
            mby as i32 * 16 + (k >> 1) as i32 * 8,
        );
        let mut q = [0u8; 64];
        let mut p = [[0u8; 64]; 4];
        pred(x, y, own[k], &mut q);
        for (d, pd) in p.iter_mut().enumerate() {
            pred(x, y, remote[k][d], pd);
        }
        for j in 0..8 {
            for i in 0..8 {
                let n = j * 8 + i;
                let r = if j < 4 { p[0][n] } else { p[1][n] };
                let s = if i < 4 { p[2][n] } else { p[3][n] };
                let v = (q[n] as u32 * H0[n] as u32
                    + r as u32 * H1[n] as u32
                    + s as u32 * H2[n] as u32
                    + 4)
                    >> 3;
                out[((k >> 1) * 8 + j) * 16 + (k & 1) * 8 + i] = v as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three weights sum to 8 at every position, so a flat prediction
    /// stays flat.
    #[test]
    fn weights_sum_to_eight() {
        for n in 0..64 {
            assert_eq!(H0[n] + H1[n] + H2[n], 8, "position {n}");
        }
        // Each matrix is symmetric left-right and top-bottom.
        for m in [&H0, &H1, &H2] {
            for j in 0..8 {
                for i in 0..8 {
                    assert_eq!(m[j * 8 + i], m[j * 8 + 7 - i]);
                    assert_eq!(m[j * 8 + i], m[(7 - j) * 8 + i]);
                }
            }
        }
    }

    /// With every remote vector equal to the block's own, OBMC is plain
    /// motion compensation.
    #[test]
    fn equal_vectors_are_plain_mc() {
        let mut pic = Pic::new(48, 48);
        for (i, v) in pic.y.iter_mut().enumerate() {
            *v = ((i * 31 + i / 48 * 7) % 253) as u8;
        }
        for mv in [[0, 0], [3, -1], [-5, 4]] {
            let own = [mv; 4];
            let remote = [[mv; 4]; 4];
            let mut a = [0u8; 256];
            predict_luma(&pic, 1, 1, &own, &remote, false, false, &mut a);
            let mut b = crate::dec::vop::MbPix::new();
            crate::dec::vop::predict_mb(&pic, 1, 1, &own, false, false, false, &mut b);
            assert_eq!(a, b.y);
        }
    }

    /// Remote vector selection: picture borders and intra neighbours give
    /// the own vector, not-coded neighbours zero, the bottom blocks never
    /// look below.
    #[test]
    fn remote_vector_rules() {
        let mut st = MbState::new(3, 3);
        for mb in 0..9 {
            st.kind[mb] = MbKind::Inter;
            st.set_mb_mv(mb % 3, mb / 3, [mb as i32 * 2, -(mb as i32)]);
        }
        st.kind[3] = MbKind::Intra; // left of the centre
        st.kind[1] = MbKind::Skipped; // above the centre
        st.set_mb_mv(1, 0, [0, 0]);
        let own = [[100, 0], [101, 0], [102, 0], [103, 0]];
        let r = remote_vectors(&st, 1, 1, &own, |_| false);
        // Block 0: above is not coded, below is block 2, left is intra,
        // right is block 1.
        assert_eq!(r[0], [[0, 0], own[2], own[0], own[1]]);
        // Block 1: right is macroblock 5's block 0.
        assert_eq!(r[1][3], [10, -5]);
        // Block 3: above is block 1, below is itself, right macroblock 5.
        assert_eq!(r[3], [own[1], own[3], own[2], [10, -5]]);
        // A corner macroblock: no left or above.
        let r = remote_vectors(&st, 0, 0, &own, |_| false);
        assert_eq!(r[0][0], own[0]);
        assert_eq!(r[0][2], own[0]);
        // An excluded neighbour (a GMC macroblock) gives the own vector.
        let r = remote_vectors(&st, 1, 1, &own, |i| i == 5);
        assert_eq!(r[1][3], own[1]);
    }
}
