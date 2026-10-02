//! Just enough container handling to feed sample files to the decoder: the
//! video chunks of an AVI (RIFF) file and its codec extra data, or a raw
//! elementary stream cut into access units. And a PNG writer, to look at
//! decoded pictures.

#![allow(dead_code)]

use mpeg4::Frame;

/// Access units of a sample file and any decoder specific info.
pub struct Sample {
    pub dsi: Vec<u8>,
    pub units: Vec<Vec<u8>>,
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// The first video stream of an AVI file.
pub fn avi(data: &[u8]) -> Option<Sample> {
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"AVI " {
        return None;
    }
    let mut s = Sample { dsi: Vec::new(), units: Vec::new() };
    let mut video_stream: Option<u32> = None;
    let mut stream_no = 0u32;
    let mut last_was_vids = false;
    // Walk every chunk, descending into RIFF and LIST.
    let mut stack = vec![(12usize, data.len())];
    while let Some((mut pos, end)) = stack.pop() {
        while pos + 8 <= end {
            let id = &data[pos..pos + 4];
            let size = le32(&data[pos + 4..]) as usize;
            let body = pos + 8;
            let next = (body + size + (size & 1)).min(end);
            if id == b"RIFF" || id == b"LIST" {
                if body + 4 <= end {
                    stack.push((next, end));
                    stack.push((body + 4, (body + size).min(end)));
                }
                break;
            }
            let b = &data[body..(body + size).min(end)];
            match id {
                b"strh" => {
                    last_was_vids = b.len() >= 4 && &b[..4] == b"vids";
                    if last_was_vids && video_stream.is_none() {
                        video_stream = Some(stream_no);
                    }
                    stream_no += 1;
                }
                b"strf" if last_was_vids && s.dsi.is_empty() && b.len() > 40 => {
                    let header = le32(b) as usize;
                    if header >= 40 && b.len() > header {
                        s.dsi = b[header..].to_vec();
                    }
                }
                _ if (id[2] == b'd' && (id[3] == b'c' || id[3] == b'b'))
                    && id[0].is_ascii_digit()
                    && id[1].is_ascii_digit() =>
                {
                    let n = ((id[0] - b'0') * 10 + (id[1] - b'0')) as u32;
                    if Some(n) == video_stream.or(Some(0)) {
                        s.units.push(b.to_vec());
                    }
                }
                _ => {}
            }
            pos = next;
        }
    }
    Some(s)
}

/// A raw elementary stream cut before each VOP's leading headers: every
/// access unit is whatever precedes a VOP start code, the VOP included.
/// Short-header streams are cut at picture start codes.
pub fn raw(data: &[u8]) -> Sample {
    // Short header when the first start pattern is a picture start code.
    let short = (0..data.len().saturating_sub(2))
        .find(|&i| data[i] == 0 && data[i + 1] == 0 && (data[i + 2] == 1 || data[i + 2] & 0xfc == 0x80))
        .is_some_and(|i| data[i + 2] != 1);
    let mut cuts = Vec::new();
    let mut i = 0;
    while i + 3 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 {
            if short && data[i + 2] & 0xfc == 0x80 {
                cuts.push(i);
            } else if !short && data[i + 2] == 1 && data[i + 3] == 0xb6 {
                cuts.push(i);
            }
        }
        i += 1;
    }
    let mut units = Vec::new();
    let mut start = 0;
    for k in 0..cuts.len() {
        // Each unit runs to the start of the next VOP's headers: for
        // MPEG-4, back from the next VOP start code over any other units
        // that precede it (GOV, VOL, user data).
        let end = if k + 1 < cuts.len() {
            if short {
                cuts[k + 1]
            } else {
                let mut e = cuts[k + 1];
                let mut j = cuts[k] + 4;
                while j + 3 < cuts[k + 1] {
                    if data[j] == 0 && data[j + 1] == 0 && data[j + 2] == 1 {
                        e = j;
                        break;
                    }
                    j += 1;
                }
                e
            }
        } else {
            data.len()
        };
        units.push(data[start..end].to_vec());
        start = end;
    }
    Sample { dsi: Vec::new(), units }
}

/// A file's access units, by its contents.
pub fn load(data: &[u8]) -> Sample {
    avi(data).unwrap_or_else(|| raw(data))
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

/// An RGB PNG of a frame (BT.601 video-range YCbCr), stored without
/// compression.
pub fn png(f: &Frame) -> Vec<u8> {
    let (w, h) = (f.width as usize, f.height as usize);
    let (cw, _) = Frame::chroma_size(f.width, f.height);
    let (y, cb, cr) = (f.plane(0), f.plane(1), f.plane(2));
    let mut raw = Vec::with_capacity((w * 3 + 1) * h);
    for r in 0..h {
        raw.push(0);
        for c in 0..w {
            let yy = (y[r * w + c] as f64 - 16.0) * 1.164;
            let u = cb[(r / 2) * cw as usize + c / 2] as f64 - 128.0;
            let v = cr[(r / 2) * cw as usize + c / 2] as f64 - 128.0;
            for x in [yy + 1.596 * v, yy - 0.392 * u - 0.813 * v, yy + 2.017 * u] {
                raw.push(x.round().clamp(0.0, 255.0) as u8);
            }
        }
    }
    let mut z = vec![0x78, 0x01];
    for (i, chunk) in raw.chunks(65535).enumerate() {
        let last = (i + 1) * 65535 >= raw.len();
        z.push(last as u8);
        z.extend((chunk.len() as u16).to_le_bytes());
        z.extend((!(chunk.len() as u16)).to_le_bytes());
        z.extend(chunk);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend(((b << 16) | a).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |t: &[u8], d: &[u8]| {
        out.extend((d.len() as u32).to_be_bytes());
        let mut c = t.to_vec();
        c.extend(d);
        out.extend(&c);
        out.extend(crc32(&c).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend((w as u32).to_be_bytes());
    ihdr.extend((h as u32).to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    out
}
