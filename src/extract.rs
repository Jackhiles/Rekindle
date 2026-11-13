//! `--extract DIR`: write decoded images as PNG and sounds in their native format.

use anyhow::{Context, Result};
use rekindle::ctf_loader::{GameData, RgbaImage};
use std::fs;
use std::io::BufWriter;
use std::path::Path;

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    s.trim().chars().take(64).collect()
}

pub fn write_png(img: &RgbaImage, path: &Path) -> Result<()> {
    let w = BufWriter::new(fs::File::create(path)?);
    let mut enc = png::Encoder::new(w, img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&img.pixels)?;
    Ok(())
}

pub fn extract_all(g: &GameData, dir: &Path) -> Result<()> {
    let img_dir = dir.join("images");
    fs::create_dir_all(&img_dir).with_context(|| format!("creating {}", img_dir.display()))?;
    if let Some(icon) = &g.icon {
        let img = RgbaImage { width: icon.width, height: icon.height, pixels: icon.rgba.clone() };
        write_png(&img, &dir.join("icon.png"))?;
    }
    let (mut ok, mut failed) = (0, 0);
    for e in &g.images.images {
        if e.width == 0 || e.height == 0 {
            continue;
        }
        match e
            .decode_rgba()
            .and_then(|img| write_png(&img, &img_dir.join(format!("{}.png", e.handle))))
        {
            Ok(()) => ok += 1,
            Err(err) => {
                failed += 1;
                if failed <= 5 {
                    eprintln!("  image {}: {err:#}", e.handle);
                }
            }
        }
    }
    println!(
        "Images: {ok} written, {failed} failed -> {}",
        img_dir.display()
    );

    for (sub, bank) in [("sounds", &g.sounds), ("music", &g.music)] {
        if bank.is_empty() {
            continue;
        }
        let d = dir.join(sub);
        fs::create_dir_all(&d)?;
        let (mut ok, mut failed) = (0, 0);
        for s in &bank.sounds {
            let file = d.join(format!(
                "{}_{}.{}",
                s.handle,
                sanitize(&s.name),
                s.format.extension()
            ));
            match s
                .data()
                .and_then(|b| fs::write(&file, b).map_err(Into::into))
            {
                Ok(()) => ok += 1,
                Err(err) => {
                    failed += 1;
                    eprintln!("  {sub} {}: {err:#}", s.handle);
                }
            }
        }
        println!("{sub}: {ok} written, {failed} failed -> {}", d.display());
    }
    Ok(())
}
