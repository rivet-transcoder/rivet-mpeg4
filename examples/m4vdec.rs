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
    let mut out = |f: mpeg4::Frame, frames: &mut usize| {
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
