//! `--dump-info` report.

use rekindle::ctf_loader::chunk::chunk_name;
use rekindle::ctf_loader::{GameData, ObjectKind};
use std::collections::BTreeMap;

fn count_by<K: Ord, I: IntoIterator<Item = K>>(it: I) -> BTreeMap<K, usize> {
    let mut m = BTreeMap::new();
    for k in it {
        *m.entry(k).or_insert(0) += 1;
    }
    m
}

fn opt(s: &str) -> &str {
    if s.is_empty() {
        "-"
    } else {
        s
    }
}

pub fn print_info(g: &GameData, verbose: bool) {
    let h = &g.header;
    println!("== Application");
    println!(
        "  File          {} ({:?}, game data at {:#x})",
        g.path.display(),
        g.container,
        g.data_offset
    );
    println!("  Name          {}", opt(&g.name));
    println!("  Author        {}", opt(&g.author));
    println!("  Copyright     {}", opt(&g.copyright));
    println!("  Editor file   {}", opt(&g.editor_filename));
    println!("  Target file   {}", opt(&g.target_filename));
    println!(
        "  Format        {} | runtime {:#06x}.{} | product {} build {} | encrypted chunks: {}",
        if g.unicode {
            "PAMU (Unicode)"
        } else {
            "PAME (ANSI)"
        },
        g.runtime_version,
        g.runtime_subversion,
        g.product_version,
        g.product_build,
        if g.encrypted { "yes" } else { "no" }
    );
    println!(
        "  Window        {}x{} @ {} fps, {} frames, border #{:02x}{:02x}{:02x}",
        h.window_width,
        h.window_height,
        h.frame_rate,
        h.frame_count,
        h.border_color[0],
        h.border_color[1],
        h.border_color[2]
    );
    println!(
        "  Flags         {:#06x} / new {:#06x} / other {:#06x}{}{}",
        h.flags,
        h.new_flags,
        h.other_flags,
        if h.fullscreen_at_start() { ", fullscreen at start" } else { "" },
        if h.stretch() { ", stretch to window" } else { "" }
    );
    println!(
        "  Icon          {}",
        g.icon.as_ref().map_or("none".to_string(), |i| format!("{}x{}", i.width, i.height))
    );
    println!(
        "  Start         score {}, lives {}",
        h.initial_score, h.initial_lives
    );
    if !g.pack_files.is_empty() {
        let names: Vec<_> = g.pack_files.iter().map(|p| p.name.as_str()).collect();
        println!("  Packed files  {} ({})", names.len(), names.join(", "));
    }

    println!("\n== Assets");
    let modes = count_by(g.images.images.iter().map(|i| i.graphic_mode));
    let alpha = g
        .images
        .images
        .iter()
        .filter(|i| i.flags & 0x10 != 0)
        .count();
    let unsupported = g.images.images.iter().filter(|i| !i.is_supported()).count();
    let modes: Vec<_> = modes
        .iter()
        .map(|(m, n)| format!("mode {m}: {n}"))
        .collect();
    println!(
        "  Images        {} ({}; {} with alpha; {} not decodable yet)",
        g.images.len(),
        modes.join(", "),
        alpha,
        unsupported
    );
    for (label, bank) in [("Sounds", &g.sounds), ("Music", &g.music)] {
        let fmts = count_by(bank.sounds.iter().map(|s| s.format));
        let fmts: Vec<_> = fmts.iter().map(|(f, n)| format!("{f:?}: {n}")).collect();
        let bytes: u64 = bank.sounds.iter().map(|s| s.decompressed_size as u64).sum();
        println!(
            "  {label:<13} {} ({}; {:.1} MiB)",
            bank.len(),
            fmts.join(", "),
            bytes as f64 / 1048576.0
        );
    }
    let faces: Vec<_> = g
        .fonts
        .fonts
        .iter()
        .map(|f| format!("{} {}", f.face_name, f.height.abs()))
        .collect();
    println!("  Fonts         {} ({})", faces.len(), faces.join(", "));
    println!(
        "  Globals       {} values, {} strings",
        g.global_values.len(),
        g.global_strings.len()
    );

    println!("\n== Objects: {} definitions", g.objects.len());
    let kinds = count_by(g.objects.iter().map(|o| match o.kind {
        ObjectKind::Extension(_) => "Extension objects".to_string(),
        k => k.label(),
    }));
    for (k, n) in &kinds {
        println!("  {k:<20} {n}");
    }
    if !g.extensions.is_empty() {
        let names: Vec<_> = g
            .extensions
            .iter()
            .map(|e| format!("[{}] {}", e.handle, e.name))
            .collect();
        println!("  Extensions used: {}", names.join(", "));
    }
    if verbose {
        for o in &g.objects {
            let ext = match o.kind {
                ObjectKind::Extension(t) => g
                    .extensions
                    .iter()
                    .find(|e| e.handle as i32 == t as i32 - 32)
                    .map(|e| format!(" ({})", e.name))
                    .unwrap_or_default(),
                _ => String::new(),
            };
            // Movement types (all of an object's movements, in order).
            let moves = match &o.props {
                rekindle::ctf_loader::properties::ObjectProperties::Common(c) if !c.movements.is_empty() => {
                    let names: Vec<String> = c
                        .movements
                        .iter()
                        .map(|m| {
                            let name = format!("{:?}", m.kind).split([' ', '{', '(']).next().unwrap_or("").to_string();
                            let speed = match &m.kind {
                                rekindle::ctf_loader::properties::MovementKind::EightDirections { speed, .. }
                                | rekindle::ctf_loader::properties::MovementKind::Ball { speed, .. } => format!(" speed {speed}"),
                                rekindle::ctf_loader::properties::MovementKind::Mouse { x1, x2, y1, y2 } => format!(" area x {x1}..{x2} y {y1}..{y2}"),
                                _ => String::new(),
                            };
                            format!("{name}{speed}{}{}", if m.player > 0 { format!(" p{}", m.player) } else { String::new() }, if m.moving_at_start { "" } else { " stopped" })
                        })
                        .collect();
                    format!("  [{}]", names.join(", "))
                }
                _ => String::new(),
            };
            println!(
                "    #{:<5} {:<18} {}{}{}",
                o.handle,
                o.kind.label(),
                o.name,
                ext,
                moves
            );
        }
    }

    println!("\n== Frames: {}", g.frames.len());
    println!(
        "  {:>3}  {:<32} {:>11} {:>6} {:>9} {:>10}  bg",
        "#", "name", "size", "layers", "instances", "events(B)"
    );
    let mut unresolved = 0;
    for (i, f) in g.frames.iter().enumerate() {
        unresolved += f
            .instances
            .iter()
            .filter(|inst| g.object(inst.object_info).is_none())
            .count();
        println!(
            "  {:>3}  {:<32} {:>11} {:>6} {:>9} {:>10}  #{:02x}{:02x}{:02x}",
            i,
            opt(&f.name),
            format!("{}x{}", f.width, f.height),
            f.layers.len(),
            f.instances.len(),
            f.events.as_ref().map_or(0, |e| e.len()),
            f.background_color[0],
            f.background_color[1],
            f.background_color[2]
        );
        if verbose {
            for (li, l) in f.layers.iter().enumerate() {
                let n = f
                    .instances
                    .iter()
                    .filter(|x| x.layer as usize == li)
                    .count();
                println!(
                    "        layer {li}: {:<24} scroll {:.2}/{:.2}, {n} instances",
                    opt(&l.name),
                    l.x_coefficient,
                    l.y_coefficient
                );
            }
        }
    }
    if unresolved > 0 {
        println!("  !! {unresolved} instances reference unknown object definitions");
    }

    if verbose {
        println!("\n== Top-level chunks");
        for c in &g.chunks {
            println!(
                "  {:#06x} {:<22} flags {} {:>12} bytes",
                c.id,
                chunk_name(c.id),
                c.flags,
                c.size
            );
        }
    }

    println!("\n== Warnings: {}", g.warnings.len());
    let shown = if verbose { g.warnings.len() } else { 10 };
    for w in g.warnings.iter().take(shown) {
        println!("  - {w}");
    }
    if g.warnings.len() > shown {
        println!("  ... {} more (use -v)", g.warnings.len() - shown);
    }
}

/// `--dump-events N`: print a frame's event sheet in a compact readable form.
pub fn print_events(g: &GameData, frame: usize) {
    use rekindle::ctf_loader::events::{Param, TokenValue};
    let Some(f) = g.frames.get(frame) else {
        println!("no frame {frame}");
        return;
    };
    let obj = |oi: u16| {
        g.object(oi)
            .map_or(format!("oi#{oi}"), |o| format!("\"{}\"", o.name))
    };
    let param = |p: &Param| -> String {
        match p {
            Param::Expression { comparison, tokens } => {
                let t: Vec<String> = tokens
                    .iter()
                    .map(|t| match &t.value {
                        TokenValue::Long(v) => v.to_string(),
                        TokenValue::Double(v) => v.to_string(),
                        TokenValue::String(s) => format!("{s:?}"),
                        TokenValue::Index(i) => format!("[{}/{} #{i}]", t.object_type, t.num),
                        TokenValue::None if t.object_type >= 2 => {
                            format!("{}.{}", obj(t.object_info), t.num)
                        }
                        TokenValue::None => format!("<{}/{}>", t.object_type, t.num),
                    })
                    .collect();
                format!("{comparison:?} {}", t.join(" "))
            }
            Param::Object { object_info, .. } => obj(*object_info),
            Param::Create {
                position,
                object_info,
                ..
            } => {
                format!(
                    "create {} at {},{} rel {}",
                    obj(*object_info),
                    position.x,
                    position.y,
                    position.parent_object_info
                )
            }
            other => {
                let s = format!("{other:?}");
                s.chars().take(80).collect()
            }
        }
    };
    println!(
        "== Frame {frame} \"{}\": {} events",
        f.name,
        f.event_sheet.groups.len()
    );
    for (i, gr) in f.event_sheet.groups.iter().enumerate() {
        println!("#{i} flags {:#x}", gr.flags);
        for (kind, list) in [("IF", &gr.conditions), ("DO", &gr.actions)] {
            for a in list.iter() {
                let who = if a.object_type >= 0 {
                    obj(a.object_info)
                } else {
                    format!("sys{}", a.object_type)
                };
                let ps: Vec<String> = a
                    .params
                    .iter()
                    .map(|p| format!("{}:{}", p.code, param(&p.value)))
                    .collect();
                println!(
                    "   {kind} {}{who} {}/{}  {}",
                    if a.negated() { "NOT " } else { "" },
                    a.object_type,
                    a.num,
                    ps.join(" | ")
                );
            }
        }
    }
}

/// `--audit`: look for data the runtime is likely to show differently from the original.
pub fn print_audit(g: &GameData) {
    use rekindle::ctf_loader::ObjectProperties;
    use rekindle::render::text::find_font_file;
    use std::collections::{BTreeMap, BTreeSet};

    println!("== Audit: {}", g.name);

    // 1. Colour-keyed images whose key never matches although the corners agree on a colour.
    let mut suspicious = Vec::new();
    for e in &g.images.images {
        if e.flags & 0x10 != 0 || e.width < 4 || e.height < 4 {
            continue;
        }
        let Ok(img) = e.decode_rgba() else { continue };
        let any_clear = img.pixels.chunks_exact(4).any(|p| p[3] == 0);
        if any_clear {
            continue;
        }
        let (w, h) = (img.width as usize, img.height as usize);
        let px = |x: usize, y: usize| &img.pixels[(y * w + x) * 4..(y * w + x) * 4 + 3];
        let corners = [px(0, 0), px(w - 1, 0), px(0, h - 1), px(w - 1, h - 1)];
        // A near-miss: the border is within a few levels of the key (the prologue bug's signature).
        let near = |c: &[u8]| c.iter().zip(e.transparent.iter()).all(|(a, b)| (*a as i32 - *b as i32).abs() <= 12);
        if corners.iter().all(|c| *c == corners[0]) && near(corners[0]) {
            suspicious.push((e.handle, e.width, e.height, e.transparent, corners[0].to_vec()));
        }
    }
    println!("  opaque images whose border nearly matches the transparent colour (possible missed transparency): {}", suspicious.len());
    for (h, w, hh, key, c) in suspicious.iter().take(12) {
        println!("    image {h} {w}x{hh}: key {:?}, border {:?}", &key[..3], c);
    }

    // 2. Animation frames and backdrops pointing at missing images.
    let mut missing = 0;
    for o in &g.objects {
        match &o.props {
            ObjectProperties::Common(c) => {
                for a in c.animations.iter().flatten() {
                    for d in a.directions.iter().flatten() {
                        missing += d.frames.iter().filter(|&&f| g.images.get(f as u32).is_none()).count();
                    }
                }
            }
            ObjectProperties::Backdrop(b) => missing += g.images.get(b.image as u32).is_none() as usize,
            _ => {}
        }
    }
    println!("  animation / backdrop references to missing images: {missing}");

    // 3. Ink effects drawn approximately or not at all.
    let mut inks: BTreeMap<u16, usize> = BTreeMap::new();
    for o in &g.objects {
        let e = o.ink().effect;
        if !matches!(e, 0 | 1 | 2 | 3 | 4 | 5 | 9 | 11) {
            *inks.entry(e).or_insert(0) += 1;
        }
    }
    println!("  objects with unsupported ink effects (effect: count): {inks:?}");

    // 4. Fonts that need a substitute.
    let missing_fonts: BTreeSet<String> = g
        .fonts
        .fonts
        .iter()
        .filter(|f| find_font_file(&f.face_name, false, false).is_none())
        .map(|f| f.face_name.clone())
        .collect();
    println!("  fonts not installed (drawn with a substitute): {missing_fonts:?}");

    // 5. Backdrops with a non-zero image hotspot (placed by top-left corner).
    let odd: usize = g
        .objects
        .iter()
        .filter(|o| match &o.props {
            ObjectProperties::Backdrop(b) => g.images.get(b.image as u32).is_some_and(|e| e.hotspot != (0, 0)),
            _ => false,
        })
        .count();
    println!("  backdrops with a non-zero hotspot: {odd}");
}
