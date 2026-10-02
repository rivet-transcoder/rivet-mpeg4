//! Decodes an MPEG-4 Visual / H.263 file (AVI or raw elementary stream)
//! and reports what happened; optionally writes frames as PNG.
//!
//! `cargo run --release --example m4vdec -- FILE [OUT_DIR FRAME...]`

#[path = "../tests/common/media.rs"]
mod media;

use mpeg4::Decoder;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: m4vdec FILE [OUT_DIR FRAME...]");
        std::process::exit(2);
    };
    let data = std::fs::read(path).expect("read");
    let sample = media::load(&data);
    let dump: Vec<usize> = args.iter().skip(2).filter_map(|s| s.parse().ok()).collect();
    let mut dec = Decoder::new();
    if !sample.dsi.is_empty()
        && let Err(e) = dec.configure(&sample.dsi)
    {
        println!("config: {e}");
    }
    let mut frames = 0usize;
    let mut concealed = 0usize;
    let mut errors = 0usize;
    let mut first_error = None;
    let mut types = String::new();
    let mut blockiness = [(0.0f64, 0usize); 4];
    // Mean |difference| from the previous frame, into I-frames and into
    // the rest: drift shows as a jump at every I-frame.
    let mut prev: Option<Vec<u8>> = None;
    let mut jump = [(0.0f64, 0usize); 2];
    let mut out = |f: mpeg4::Frame, frames: &mut usize| {
        // Mean |difference| across 8x8 block edges over mean |difference|
        // inside blocks, luma: about 1 for a clean picture.
        let (w, h) = (f.width as usize, f.height as usize);
        let y = f.plane(0);
        let (mut edge, mut ne, mut inner, mut ni) = (0.0, 0, 0.0, 0);
        for r in 0..h {
            for c in 1..w {
                let d = (y[r * w + c] as f64 - y[r * w + c - 1] as f64).abs();
                if c % 8 == 0 {
                    edge += d;
                    ne += 1;
                } else {
                    inner += d;
                    ni += 1;
                }
            }
        }
        let b = (edge / ne.max(1) as f64) / (inner / ni.max(1) as f64 + 0.5);
        if let Some(p) = &prev
            && p.len() == y.len()
        {
            let mad = p.iter().zip(y).map(|(&a, &b)| (a as f64 - b as f64).abs()).sum::<f64>() / y.len() as f64;
            let k = (f.vop_type == mpeg4::VopType::I) as usize;
            jump[k].0 += mad;
            jump[k].1 += 1;
        }
        prev = Some(y.to_vec());
        let k = f.vop_type as usize;
        blockiness[k].0 += b;
        blockiness[k].1 += 1;
        types.push(match f.vop_type {
            mpeg4::VopType::I => 'I',
            mpeg4::VopType::P => 'P',
            mpeg4::VopType::B => 'B',
            mpeg4::VopType::S => 'S',
        });
        if dump.contains(frames)
            && let Some(dir) = args.get(1)
        {
            let p = format!("{dir}/frame{:04}.png", *frames);
            std::fs::write(&p, media::png(&f)).expect("write png");
        }
        *frames += 1;
    };
    let t0 = std::time::Instant::now();
    for (i, u) in sample.units.iter().enumerate() {
        match dec.decode(u) {
            Ok(fs) => {
                for f in fs {
                    concealed += f.concealed as usize;
                    if f.concealed {
                        println!("  frame {frames} (decode index {}) concealed", f.decode_index);
                    }
                    out(f, &mut frames);
                }
            }
            Err(e) => {
                errors += 1;
                if first_error.is_none() {
                    first_error = Some(format!("unit {i}: {e}"));
                }
            }
        }
    }
    for f in dec.flush() {
        concealed += f.concealed as usize;
        out(f, &mut frames);
    }
    let el = t0.elapsed();
    println!(
        "  blockiness I {:.3} P {:.3} B {:.3} S {:.3}",
        blockiness[0].0 / blockiness[0].1.max(1) as f64,
        blockiness[1].0 / blockiness[1].1.max(1) as f64,
        blockiness[2].0 / blockiness[2].1.max(1) as f64,
        blockiness[3].0 / blockiness[3].1.max(1) as f64
    );
    println!(
        "  mean |frame difference|: into I-frames {:.2}, others {:.2}",
        jump[1].0 / jump[1].1.max(1) as f64,
        jump[0].0 / jump[0].1.max(1) as f64
    );
    if std::env::var_os("MPEG4_TYPES").is_some() {
        println!("  display order types: {types}");
    }
    let vol = dec.vol();
    println!(
        "{path}: {} units, {frames} frames, {} {}x{} object type {} qpel {} mpeg_quant {} packets {} dp {}",
        sample.units.len(),
        if vol.is_some() { "VOL" } else { "no VOL" },
        vol.map_or(0, |v| v.width),
        vol.map_or(0, |v| v.height),
        vol.map_or(0, |v| v.object_type),
        vol.is_some_and(|v| v.quarter_sample),
        vol.is_some_and(|v| v.mpeg_quant),
        vol.is_some_and(|v| !v.resync_marker_disable),
        vol.is_some_and(|v| v.data_partitioned),
    );
    println!("  stats {:?}", dec.stats());
    println!("  concealed frames {concealed}, errors {errors} {}", first_error.unwrap_or_default());
    if let Some(e) = dec.last_error() {
        println!("  last concealed error: {e}");
    }
    println!("  {:.1} ms ({:.0} fps)", el.as_secs_f64() * 1e3, frames as f64 / el.as_secs_f64());
}
