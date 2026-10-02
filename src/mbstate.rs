//! Per-VOP macroblock state shared by the decoder and the encoder: which
//! video packet each macroblock belongs to, how it was coded, its
//! quantiser, the DC / AC values intra prediction reads (7.4.3), and the
//! motion vectors vector prediction reads (7.6.5).

/// What intra prediction keeps of a block: its reconstructed DC
/// (`F[0][0]`) and the quantised first row and column (`QF[0][1..8]`,
/// `QF[1..8][0]`) after prediction.
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct BlockPred {
    pub dc: i16,
    pub row: [i16; 7],
    pub col: [i16; 7],
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(crate) enum MbKind {
    #[default]
    Inter,
    Intra,
    /// `not_coded` in a P-VOP: zero motion, no texture.
    Skipped,
}

/// The direction intra DC / AC prediction takes (7.4.3.1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Dir {
    /// From block A, to the left: the first column is predicted.
    Left,
    /// From block C, above: the first row is predicted.
    Up,
}

/// The predictors for one intra block.
#[derive(Clone, Copy, Debug)]
pub(crate) struct IntraPred {
    /// `F[0][0]` of the block predicted from (1024 when unavailable).
    pub dc: i32,
    pub dir: Dir,
    /// The row (from above) or column (from the left) of the block
    /// predicted from, and its quantiser; `None` when it is unavailable
    /// (outside the VOP or the video packet, or not intra).
    pub ac: Option<([i16; 7], u32)>,
}

/// Neighbours A (left), B (above left) and C (above) of each block of a
/// macroblock, as (macroblock dx, dy, block).
const NEIGH: [[(isize, isize, usize); 3]; 6] = [
    [(-1, 0, 1), (-1, -1, 3), (0, -1, 2)],
    [(0, 0, 0), (0, -1, 2), (0, -1, 3)],
    [(-1, 0, 3), (-1, 0, 1), (0, 0, 0)],
    [(0, 0, 2), (0, 0, 0), (0, 0, 1)],
    [(-1, 0, 4), (-1, -1, 4), (0, -1, 4)],
    [(-1, 0, 5), (-1, -1, 5), (0, -1, 5)],
];

pub(crate) struct MbState {
    pub mbw: usize,
    pub mbh: usize,
    /// Video packet of each macroblock: an id unique over the decoder's
    /// life, so a macroblock left over from an earlier VOP never matches.
    pub slice: Vec<u32>,
    pub kind: Vec<MbKind>,
    pub qp: Vec<u8>,
    pub pred: Vec<[BlockPred; 6]>,
    /// Motion vector of every 8x8 luminance block, `2 mbw` by `2 mbh`.
    pub mv: Vec<[i16; 2]>,
}

/// Rounding integer division of 7.4.3 (`//`): to the nearest integer,
/// halves away from zero.
#[inline]
pub(crate) fn round_div(n: i32, d: i32) -> i32 {
    if n >= 0 { (n + d / 2) / d } else { -((-n + d / 2) / d) }
}

impl MbState {
    pub fn new(mbw: usize, mbh: usize) -> MbState {
        MbState {
            mbw,
            mbh,
            slice: vec![0; mbw * mbh],
            kind: vec![MbKind::Inter; mbw * mbh],
            qp: vec![1; mbw * mbh],
            pred: vec![[BlockPred::default(); 6]; mbw * mbh],
            mv: vec![[0, 0]; 4 * mbw * mbh],
        }
    }

    /// The index of macroblock `(x, y)` when it is inside the VOP and in
    /// video packet `slice`.
    #[inline]
    pub fn available(&self, x: isize, y: isize, slice: u32) -> Option<usize> {
        if x < 0 || y < 0 || x >= self.mbw as isize || y >= self.mbh as isize {
            return None;
        }
        let i = y as usize * self.mbw + x as usize;
        (self.slice[i] == slice).then_some(i)
    }

    /// DC / AC predictors for block `k` of macroblock `(mbx, mby)` in video
    /// packet `slice`. The macroblock's own earlier blocks are always
    /// available; its quantiser must already be in `qp`.
    pub fn intra_pred(&self, mbx: usize, mby: usize, k: usize, slice: u32) -> IntraPred {
        let cur = mby * self.mbw + mbx;
        let get = |(dx, dy, b): (isize, isize, usize)| -> Option<(BlockPred, u32)> {
            let i = if dx == 0 && dy == 0 {
                cur
            } else {
                let i = self.available(mbx as isize + dx, mby as isize + dy, slice)?;
                if self.kind[i] != MbKind::Intra {
                    return None;
                }
                i
            };
            Some((self.pred[i][b], self.qp[i] as u32))
        };
        let [a, b, c] = NEIGH[k].map(get);
        let dc = |p: &Option<(BlockPred, u32)>| p.map_or(1024, |p| p.0.dc as i32);
        let (fa, fb, fc) = (dc(&a), dc(&b), dc(&c));
        if (fa - fb).abs() < (fb - fc).abs() {
            IntraPred { dc: fc, dir: Dir::Up, ac: c.map(|p| (p.0.row, p.1)) }
        } else {
            IntraPred { dc: fa, dir: Dir::Left, ac: a.map(|p| (p.0.col, p.1)) }
        }
    }

    /// Records a reconstructed intra block for later prediction.
    pub fn store_intra(&mut self, mb: usize, k: usize, dc: i32, qf: &[i16; 64]) {
        let p = &mut self.pred[mb][k];
        p.dc = dc as i16;
        for i in 0..7 {
            p.row[i] = qf[i + 1];
            p.col[i] = qf[(i + 1) * 8];
        }
    }

    /// The motion vector predictor of block `k` (0 for a one-vector
    /// macroblock) of macroblock `(mbx, mby)`: the median of the left,
    /// above and above-right candidates, with the substitutions of 7.6.5 for
    /// candidates outside the VOP or the video packet.
    pub fn mv_pred(&self, mbx: usize, mby: usize, k: usize, slice: u32) -> [i32; 2] {
        let bx = 2 * mbx as isize + (k & 1) as isize;
        let by = 2 * mby as isize + (k >> 1) as isize;
        let third = match k {
            0 => (bx + 2, by - 1),
            1 | 2 => (bx + 1, by - 1),
            _ => (bx - 1, by - 1),
        };
        let cands = [(bx - 1, by), (bx, by - 1), third];
        let mut v = [[0i32; 2]; 3];
        let mut valid = [false; 3];
        for (j, &(cx, cy)) in cands.iter().enumerate() {
            let (mx, my) = (cx.div_euclid(2), cy.div_euclid(2));
            let ok = (mx == mbx as isize && my == mby as isize)
                || (cx >= 0 && cx < 2 * self.mbw as isize && self.available(mx, my, slice).is_some());
            if ok && cy >= 0 {
                let m = self.mv[cy as usize * 2 * self.mbw + cx as usize];
                v[j] = [m[0] as i32, m[1] as i32];
                valid[j] = true;
            }
        }
        match valid.iter().filter(|&&b| b).count() {
            0 => [0, 0],
            1 => v[valid.iter().position(|&b| b).unwrap()],
            _ => {
                // One invalid candidate counts as zero.
                let med = |a: i32, b: i32, c: i32| a.max(b).min(a.min(b).max(c));
                [med(v[0][0], v[1][0], v[2][0]), med(v[0][1], v[1][1], v[2][1])]
            }
        }
    }

    /// Sets the vector of block `k` of macroblock `(mbx, mby)`.
    #[inline]
    pub fn set_mv(&mut self, mbx: usize, mby: usize, k: usize, mv: [i32; 2]) {
        let i = (2 * mby + (k >> 1)) * 2 * self.mbw + 2 * mbx + (k & 1);
        self.mv[i] = [mv[0] as i16, mv[1] as i16];
    }

    /// Sets all four vectors of a macroblock.
    pub fn set_mb_mv(&mut self, mbx: usize, mby: usize, mv: [i32; 2]) {
        for k in 0..4 {
            self.set_mv(mbx, mby, k, mv);
        }
    }

    /// The vector of block `k` of macroblock `(mbx, mby)`.
    #[inline]
    pub fn get_mv(&self, mbx: usize, mby: usize, k: usize) -> [i32; 2] {
        let m = self.mv[(2 * mby + (k >> 1)) * 2 * self.mbw + 2 * mbx + (k & 1)];
        [m[0] as i32, m[1] as i32]
    }
}

/// The predicted coefficients for AC prediction: index `i` (1..8) of the
/// first row or column, scaled from the neighbour's quantiser to `qp`.
#[inline]
pub(crate) fn ac_pred_value(v: i16, qp_n: u32, qp: u32) -> i32 {
    if qp_n == qp { v as i32 } else { round_div(v as i32 * qp_n as i32, qp as i32) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_division() {
        assert_eq!(round_div(7, 2), 4);
        assert_eq!(round_div(-7, 2), -4);
        assert_eq!(round_div(5, 3), 2);
        assert_eq!(round_div(-5, 3), -2);
        assert_eq!(round_div(1024, 8), 128);
        assert_eq!(round_div(1024, 17), 60);
    }

    #[test]
    fn dc_prediction_direction() {
        let mut s = MbState::new(2, 2);
        s.slice.fill(1);
        s.kind.fill(MbKind::Intra);
        s.qp.fill(4);
        // Block 3 of MB (1,1): A = block 2, B = block 0, C = block 1.
        let mb = 3;
        s.pred[mb][0].dc = 100;
        s.pred[mb][1].dc = 100; // C equals B: |B - C| = 0, not > |A - B|
        s.pred[mb][2].dc = 300;
        let p = s.intra_pred(1, 1, 3, 1);
        assert_eq!(p.dir, Dir::Left);
        assert_eq!(p.dc, 300);
        s.pred[mb][1].dc = 400;
        s.pred[mb][2].dc = 100; // |A - B| = 0 < |B - C| = 300: from C
        let p = s.intra_pred(1, 1, 3, 1);
        assert_eq!(p.dir, Dir::Up);
        assert_eq!(p.dc, 400);
        // Top-left block of the VOP: nothing available, 1024 everywhere.
        let p = s.intra_pred(0, 0, 0, 1);
        assert_eq!(p.dc, 1024);
        assert!(p.ac.is_none());
        // Another video packet is unavailable.
        s.slice[0] = 7;
        let p = s.intra_pred(1, 0, 0, 1);
        assert_eq!((p.dc, p.dir), (1024, Dir::Left));
    }

    #[test]
    fn mv_prediction_edges() {
        let mut s = MbState::new(3, 2);
        s.slice.fill(1);
        s.set_mb_mv(0, 0, [2, 4]);
        s.set_mb_mv(1, 0, [6, -2]);
        s.set_mb_mv(2, 0, [-4, 8]);
        s.set_mb_mv(0, 1, [10, 10]);
        // First row: only the left candidate.
        assert_eq!(s.mv_pred(1, 0, 0, 1), [2, 4]);
        assert_eq!(s.mv_pred(0, 0, 0, 1), [0, 0]);
        // Second row, middle: median of left, above, above right.
        assert_eq!(s.mv_pred(1, 1, 0, 1), [6, 8]);
        // Left column: left is zero.
        assert_eq!(s.mv_pred(0, 1, 0, 1), [2, 0]);
        // Right column: above right is zero.
        s.set_mb_mv(1, 1, [1, 1]);
        assert_eq!(s.mv_pred(2, 1, 0, 1), [0, 1]);
        // A packet boundary above: only the left one.
        s.slice[0] = 0;
        s.slice[1] = 0;
        s.slice[2] = 0;
        assert_eq!(s.mv_pred(1, 1, 0, 1), [10, 10]);
    }
}
