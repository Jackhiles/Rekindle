//! fusion-rt: a clean-room, open-source runtime for Clickteam Fusion 2.5 games.
//!
//! This binary contains no game data. Point it at a game you own:
//!     cargo run --release -- path/to/game.exe              (open the frame viewer)
//!     cargo run --release -- path/to/game.exe --dump-info  (print a summary)

use fusion_rt::{ctf_loader, render, runtime};
mod dump;
mod extract;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Clean-room Clickteam Fusion 2.5 runtime (bring your own game files)"
)]
struct Cli {
    /// Path to a Fusion game: a built .exe or a .ccn data file.
    #[arg(required_unless_present = "scenario")]
    game: Option<PathBuf>,

    /// Print a summary of the parsed application, frames, objects and assets (no window).
    #[arg(long)]
    dump_info: bool,

    /// With --dump-info: also list every object, frame layer, chunk and warning.
    #[arg(short, long)]
    verbose: bool,

    /// Extract decoded images (PNG) and sounds into this directory (no window).
    #[arg(long, value_name = "DIR")]
    extract: Option<PathBuf>,

    /// Frame index to open first in the viewer.
    #[arg(long, default_value_t = 0)]
    frame: usize,

    /// Start zoomed out so the whole frame is visible.
    #[arg(long)]
    fit: bool,

    /// Also draw objects that start hidden.
    #[arg(long)]
    show_hidden: bool,

    /// Hide this layer index (0-based) of the first frame; repeatable.
    #[arg(long, value_name = "N")]
    hide_layer: Vec<usize>,

    /// Render the chosen frame to this PNG file and exit.
    #[arg(long, value_name = "PNG")]
    screenshot: Option<PathBuf>,

    /// With --screenshot: run this many game ticks first.
    #[arg(long, value_name = "N", default_value_t = 0)]
    ticks: u32,

    /// Don't run events: show frames in their static start-of-frame state.
    #[arg(long = "static")]
    static_view: bool,

    /// Folder for the game's save files (default: %APPDATA%\\fusion-rt\\<game>).
    #[arg(long, value_name = "DIR")]
    save_dir: Option<PathBuf>,

    /// Disable sound.
    #[arg(long)]
    mute: bool,

    /// Report data the runtime may show differently from the original, then exit.
    #[arg(long)]
    audit: bool,

    /// Print the event sheet of frame N and exit.
    #[arg(long, value_name = "N")]
    dump_events: Option<usize>,

    /// Run N game ticks without a window, then print frame changes and unimplemented features.
    #[arg(long, value_name = "N")]
    headless: Option<u64>,

    /// Run a scenario script (or every *.scn in a folder) and report pass / fail.
    #[arg(long, value_name = "FILE|DIR")]
    scenario: Option<PathBuf>,

    /// Run every frame for N ticks (Enter pressed now and then) and report unsupported features.
    #[arg(long, value_name = "N")]
    sweep: Option<u64>,

    /// With --headless: press a key (Windows VK code) at a tick, e.g. 100:13 for Enter; repeatable.
    #[arg(long, value_name = "TICK:VK")]
    press: Vec<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(target) = &cli.scenario {
        return run_scenarios(target);
    }
    let game_path = cli.game.clone().expect("required by clap");
    let t = Instant::now();
    let game = ctf_loader::GameData::load(&game_path)?;
    eprintln!("Loaded {} in {:.2?}", game_path.display(), t.elapsed());

    if cli.dump_info {
        dump::print_info(&game, cli.verbose);
    }
    if let Some(dir) = &cli.extract {
        extract::extract_all(&game, dir)?;
    }
    if cli.dump_info || cli.extract.is_some() {
        return Ok(());
    }

    if cli.audit {
        dump::print_audit(&game);
        return Ok(());
    }
    if let Some(f) = cli.dump_events {
        dump::print_events(&game, f);
        return Ok(());
    }
    if let Some(n) = cli.sweep {
        run_sweep(game, n, cli.save_dir.clone());
        return Ok(());
    }
    if let Some(n) = cli.headless {
        let presses: Vec<(u64, u16)> = cli
            .press
            .iter()
            .filter_map(|s| {
                let (t, k) = s.split_once(':')?;
                Some((t.parse().ok()?, k.parse().ok()?))
            })
            .collect();
        run_headless(game, cli.frame, n, &presses, cli.save_dir.clone());
        return Ok(());
    }

    let opts = render::ViewerOptions {
        start_frame: cli.frame,
        screenshot: cli.screenshot,
        fit: cli.fit,
        show_hidden: cli.show_hidden,
        hidden_layers: cli.hide_layer,
        run_events: !cli.static_view,
        screenshot_ticks: cli.ticks,
        mute: cli.mute,
        save_dir: cli.save_dir,
    };
    let conf = render::window_conf(&game);
    macroquad::Window::from_config(conf, render::run(game, opts));
    Ok(())
}

/// Simulate the game with no input and report what happened.
fn run_headless(
    game: ctf_loader::GameData,
    start: usize,
    ticks: u64,
    presses: &[(u64, u16)],
    save_dir: Option<PathBuf>,
) {
    let mut rt = runtime::Runtime::with_save_root(game, start, save_dir);
    println!("saves: {}", rt.files.save_root.display());
    let mut input = runtime::Input::default();
    let t = Instant::now();
    let mut last = rt.frame_log.len();
    for n in 0..ticks {
        input.down.clear();
        for &(t, k) in presses {
            if t == n {
                input.pressed.insert(k);
                println!("tick {n:6}: press VK {k}");
            }
            if (t..t + 3).contains(&n) {
                input.down.insert(k);
            }
        }
        rt.tick(&input);
        input.consume_edges();
        if rt.frame_log.len() != last {
            println!(
                "tick {n:6}: -> frame {} \"{}\"",
                rt.frame_index,
                rt.frame().name
            );
            last = rt.frame_log.len();
        }
        if rt.finished {
            println!("tick {n:6}: game ended");
            break;
        }
    }
    println!(
        "ran {ticks} ticks in {:.2?}; now on frame {} \"{}\" with {} objects, timer {:.1}s",
        t.elapsed(),
        rt.frame_index,
        rt.frame().name,
        rt.instances.len(),
        rt.timer_ms / 1000.0
    );
    if let Some(p) = &rt.profile {
        let p = p.borrow();
        let mut v: Vec<_> = p.iter().collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.1 .0));
        println!("slowest events (frame, event): total time / runs");
        for ((f, g), (d, n)) in v.iter().take(12) {
            println!("  frame {f} event #{g}: {d:.2?} / {n}");
        }
    }
    let un = rt.unimplemented.borrow();
    let mut v: Vec<_> = un.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1));
    println!("unimplemented features hit ({}):", v.len());
    for (k, n) in v.iter().take(25) {
        println!("  {n:8}  {k}");
    }
}

/// Compatibility sweep: every frame, N ticks each, then the unsupported features hit overall.
fn run_sweep(game: ctf_loader::GameData, ticks: u64, save_dir: Option<PathBuf>) {
    let frames = game.frames.len();
    let save_dir = save_dir.unwrap_or_else(|| std::env::temp_dir().join("fusion-rt-sweep"));
    let mut rt = runtime::Runtime::with_save_root(game, 0, Some(save_dir));
    let t = Instant::now();
    for f in 0..frames {
        rt.load_frame(f);
        let mut input = runtime::Input::default();
        for n in 0..ticks {
            if n % 90 == 30 {
                input.pressed.insert(13);
                input.clicked[0] = true;
            }
            rt.tick(&input);
            input.consume_edges();
            if rt.finished {
                rt.finished = false;
            }
        }
    }
    println!(
        "swept {frames} frames x {ticks} ticks in {:.2?}",
        t.elapsed()
    );
    let un = rt.unimplemented.borrow();
    let mut v: Vec<_> = un.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1));
    println!("unimplemented features hit ({}):", v.len());
    for (k, n) in v.iter().take(30) {
        println!("  {n:8}  {k}");
    }
}

fn run_scenarios(target: &std::path::Path) -> Result<()> {
    use fusion_rt::runtime::scenario::{find_scenarios, run_scenario};
    let files = if target.is_dir() { find_scenarios(target) } else { vec![target.to_path_buf()] };
    let mut failed = 0;
    for f in &files {
        let t = Instant::now();
        match run_scenario(f) {
            Ok(r) => {
                let status = if r.passed() { "PASS" } else { "FAIL" };
                println!("{status} {} ({:.1?})", r.name, t.elapsed());
                for l in &r.log {
                    println!("     {l}");
                }
                for e in &r.failures {
                    println!("  !! {e}");
                }
                failed += (!r.passed()) as usize;
            }
            Err(e) => {
                println!("ERROR {}: {e:#}", f.display());
                failed += 1;
            }
        }
    }
    println!("{} passed, {failed} failed", files.len() - failed);
    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}
