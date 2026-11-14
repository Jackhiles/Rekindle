//! Object runtime and event interpreter (milestone 3).
//!
//! `Runtime` owns the loaded game, the live object instances of the current frame
//! and all interpreter state. It has no rendering or windowing dependencies: feed it
//! an [`Input`] snapshot and call [`Runtime::tick`] at the game's frame rate.

pub mod audio;
pub mod events;
pub mod expr;
pub mod ext;
pub mod files;
pub mod input;
pub mod instance;
pub mod scenario;
pub mod script;
pub mod value;
pub mod widgets;

use crate::ctf_loader::events::Events;
use crate::ctf_loader::{Frame, GameData, ObjectKind, ObjectProperties};
use crate::render::scene::{SceneItem, Visual};
use events::EventState;
pub use input::Input;

use instance::{dir_to_angle, Instance, SPEED_TO_PIXELS};
use std::cell::{Cell, RefCell};
use crate::fxhash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::collections::BTreeMap;
use std::rc::Rc;
use value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameChange {
    Jump(usize),
    Restart,
    End,
}

/// A running frame fade. Game logic is paused while it plays, as in Fusion.
#[derive(Debug, Clone)]
pub struct TransitionState {
    pub fade_in: bool,
    pub elapsed_ms: f64,
    pub duration_ms: f64,
    /// Colour faded to / from (black for frame-to-frame effects we approximate as fades).
    pub color: [u8; 4],
    /// Effect id from the transition DLL ("FADE", "DOOR", "BAND", ...).
    pub effect: String,
    /// Frame change to perform when a fade-out ends.
    then: Option<FrameChange>,
}

impl TransitionState {
    /// How much of the screen is covered, 0..=1 (rises during a fade-out, falls during a fade-in).
    pub fn coverage(&self) -> f32 {
        let t = (self.elapsed_ms / self.duration_ms.max(1.0)).clamp(0.0, 1.0) as f32;
        if self.fade_in {
            1.0 - t
        } else {
            t
        }
    }

    fn new(
        t: &crate::ctf_loader::frame::Transition,
        fade_in: bool,
        then: Option<FrameChange>,
    ) -> Option<Self> {
        if t.duration_ms == 0 {
            return None;
        }
        let color = if t.flags & crate::ctf_loader::frame::Transition::FLAG_COLOR != 0 {
            t.color
        } else {
            [0, 0, 0, 255]
        };
        Some(Self {
            fade_in,
            elapsed_ms: 0.0,
            duration_ms: t.duration_ms as f64,
            color,
            effect: t.name.trim().to_ascii_uppercase(),
            then,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct LoopState {
    pub index: i32,
    pub stop: bool,
}

/// 1-bit collision mask of an image (set = opaque pixel).
#[derive(Debug)]
pub struct Mask {
    pub width: usize,
    pub height: usize,
    bits: Vec<u64>,
}

impl Mask {
    fn from_rgba(img: &crate::ctf_loader::RgbaImage) -> Self {
        let (w, h) = (img.width as usize, img.height as usize);
        let mut bits = vec![0u64; (w * h).div_ceil(64)];
        for (i, px) in img.pixels.chunks_exact(4).enumerate() {
            if px[3] > 0 {
                bits[i / 64] |= 1 << (i % 64);
            }
        }
        Self {
            width: w,
            height: h,
            bits,
        }
    }

    /// `n` (1..=64) bits starting at bit `start` of the mask, lowest bit first.
    #[inline]
    fn bits_at(&self, start: usize, n: usize) -> u64 {
        let (w, off) = (start / 64, start % 64);
        let mut v = self.bits.get(w).copied().unwrap_or(0) >> off;
        if off > 0 {
            v |= self.bits.get(w + 1).copied().unwrap_or(0) << (64 - off);
        }
        if n < 64 {
            v &= (1u64 << n) - 1;
        }
        v
    }

    /// Solid pixels of row `y` in columns `x0..x1` (both inside the mask), 64 at a time:
    /// calls `f(chunk_start, bits)` until it returns true.
    #[inline]
    pub(crate) fn row_chunks(&self, y: usize, x0: usize, x1: usize, mut f: impl FnMut(usize, u64) -> bool) -> bool {
        let base = y * self.width;
        let mut x = x0;
        while x < x1 {
            let n = (x1 - x).min(64);
            if f(x - x0, self.bits_at(base + x, n)) {
                return true;
            }
            x += n;
        }
        false
    }

    #[inline]
    pub fn solid(&self, x: usize, y: usize) -> bool {
        let i = y * self.width + x;
        x < self.width && y < self.height && self.bits[i / 64] & (1 << (i % 64)) != 0
    }
}

/// Time spent and run count per (frame, event).
pub type EventProfile = HashMap<(usize, usize), (std::time::Duration, u64)>;

type ObstacleCache = ((u64, usize), Vec<(usize, u16, (f64, f64, f64, f64))>);

/// What a global object keeps between frames.
#[derive(Debug, Clone)]
struct GlobalObjectState {
    values: Vec<Value>,
    strings: Vec<String>,
    counter: Value,
    text: String,
}

pub struct Runtime {
    pub game: GameData,
    pub frame_index: usize,
    pub instances: Vec<Instance>,
    next_id: u32,
    next_z: i64,
    min_z: i64,
    /// Ticks since the frame started.
    pub tick: u64,
    /// Frame timer in milliseconds.
    pub timer_ms: f64,
    prev_timer_ms: f64,
    pub frame_rate: u32,
    pub global_values: Vec<Value>,
    pub global_strings: Vec<String>,
    pub score: [i32; 4],
    pub lives: [i32; 4],
    events: Rc<Events>,
    event_state: HashMap<usize, EventState>,
    groups_disabled: HashSet<u16>,
    group_just_activated: HashSet<u32>,
    loops: HashMap<String, LoopState>,
    pending: Option<FrameChange>,
    /// Top-left of the visible area in frame coordinates.
    pub scroll: (f64, f64),
    pub view_size: (f64, f64),
    pub background_color: [u8; 4],
    pub cursor_visible: bool,
    /// Set when the game asked to quit.
    pub finished: bool,
    /// Sound playback model; its queued commands are executed by the audio backend.
    pub audio: audio::AudioState,
    pub input_mouse: (f64, f64),
    rng: Cell<u64>,
    qualifiers: HashMap<u16, Vec<u16>>,
    bullets: HashSet<u32>,
    /// Instances currently using platform movement (moved with collision handling).
    platformers: HashSet<u32>,
    /// Conditions/actions/expressions encountered but not implemented, with hit counts.
    pub unimplemented: RefCell<BTreeMap<String, u64>>,
    /// Frames entered, in order (for diagnostics).
    pub frame_log: Vec<usize>,
    /// Sandboxed file access for saves.
    pub files: files::SaveFs,
    /// Native extension implementations, one per extension type (they persist across frames).
    /// Native extension state per object (two Ini objects keep separate files).
    exts: HashMap<u16, Box<dyn ext::Extension>>,
    ext_shared: ext::Shared,
    /// State of "global" objects, carried into the next frame that has them.
    globals: HashMap<u16, GlobalObjectState>,
    /// True while running the "end of frame" events before a frame change.
    end_of_frame_pass: bool,
    /// Running only triggered input events ("upon pressing a key", "user clicks").
    triggered_pass: events::Trigger,
    /// Frame fade currently playing.
    pub transition: Option<TransitionState>,
    /// Name of an object whose image replaces the mouse pointer (Cursor extension).
    pub cursor_object: Option<String>,
    /// Collision masks by image handle (None = image could not be decoded).
    masks: RefCell<HashMap<u32, Option<Rc<Mask>>>>,
    /// Per-event time spent (enabled by the REKINDLE_PROFILE environment variable).
    pub profile: Option<RefCell<EventProfile>>,
    /// Obstacle instances with their bounding boxes, keyed by (tick, instance count).
    obstacle_cache: RefCell<Option<ObstacleCache>>,
    /// Instance indices per object handle; None = needs rebuilding.
    oi_index: RefCell<Option<HashMap<u16, Vec<usize>>>>,
}

impl Runtime {
    pub fn new(game: GameData, start_frame: usize) -> Self {
        Self::with_save_root(game, start_frame, None)
    }

    /// `save_root`: where the game's files are written (default: per-game folder in AppData).
    pub fn with_save_root(
        game: GameData,
        start_frame: usize,
        save_root: Option<std::path::PathBuf>,
    ) -> Self {
        let title = if game.name.is_empty() {
            game.path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            game.name.clone()
        };
        let files = files::SaveFs::new(
            &game.path,
            save_root.unwrap_or_else(|| files::default_save_root(&title)),
        );
        let mut qualifiers: HashMap<u16, Vec<u16>> = HashMap::default();
        for o in &game.objects {
            if let ObjectProperties::Common(c) = &o.props {
                for &q in &c.qualifiers {
                    qualifiers.entry(q as u16).or_default().push(o.handle);
                }
            }
        }
        let view_size = (
            game.header.window_width.max(1) as f64,
            game.header.window_height.max(1) as f64,
        );
        let mut rt = Runtime {
            frame_rate: game.header.frame_rate.clamp(1, 1000),
            global_values: game.global_values.iter().map(|&v| Value::Int(v)).collect(),
            global_strings: game.global_strings.clone(),
            score: [game.header.initial_score as i32; 4],
            lives: [game.header.initial_lives as i32; 4],
            game,
            frame_index: 0,
            instances: Vec::new(),
            next_id: 1,
            next_z: 0,
            min_z: 0,
            tick: 0,
            timer_ms: 0.0,
            prev_timer_ms: 0.0,
            events: Rc::new(Events::default()),
            event_state: HashMap::default(),
            groups_disabled: HashSet::default(),
            group_just_activated: HashSet::default(),
            loops: HashMap::default(),
            pending: None,
            scroll: (0.0, 0.0),
            view_size,
            background_color: [0, 0, 0, 255],
            cursor_visible: true,
            finished: false,
            audio: audio::AudioState::default(),
            input_mouse: (0.0, 0.0),
            rng: Cell::new(0x2545_F491_4F6C_DD1D),
            qualifiers,
            bullets: HashSet::default(),
            platformers: HashSet::default(),
            unimplemented: RefCell::new(BTreeMap::new()),
            frame_log: Vec::new(),
            files,
            exts: HashMap::default(),
            ext_shared: ext::Shared::default(),
            globals: HashMap::default(),
            end_of_frame_pass: false,
            triggered_pass: events::Trigger::None,
            transition: None,
            cursor_object: None,
            masks: RefCell::new(HashMap::default()),
            oi_index: RefCell::new(None),
            obstacle_cache: RefCell::new(None),
            profile: std::env::var_os("REKINDLE_PROFILE").map(|_| RefCell::new(HashMap::default())),
        };
        rt.load_frame(start_frame);
        rt
    }

    pub fn frame(&self) -> &Frame {
        &self.game.frames[self.frame_index]
    }

    /// (Re)start a frame: create its initial instances and reset event state.
    pub fn load_frame(&mut self, index: usize) {
        if self.game.frames.is_empty() {
            return;
        }
        self.frame_index = index.min(self.game.frames.len() - 1);
        self.frame_log.push(self.frame_index);
        self.remember_globals();
        self.instances.clear();
        self.invalidate_index();
        self.bullets.clear();
        self.next_z = 0;
        self.min_z = 0;
        self.tick = 0;
        self.timer_ms = 0.0;
        self.prev_timer_ms = 0.0;
        self.scroll = (0.0, 0.0);
        self.event_state.clear();
        self.loops.clear();
        self.pending = None;
        self.group_just_activated.clear();
        let frame = &self.game.frames[self.frame_index];
        self.transition = frame
            .fade_in
            .as_ref()
            .and_then(|t| TransitionState::new(t, true, None));
        self.background_color = frame.background_color;
        self.events = Rc::new(frame.event_sheet.clone());
        self.groups_disabled = self
            .events
            .groups
            .iter()
            .filter_map(|g| g.conditions.first())
            .filter_map(|c| match c.params.first().map(|p| &p.value) {
                Some(crate::ctf_loader::events::Param::Group { flags, id, .. })
                    if c.num == -10 && flags & 1 != 0 =>
                {
                    Some(*id)
                }
                _ => None,
            })
            .collect();
        // Entries with a parent are templates kept for "create object" actions, not
        // objects placed in the frame: they don't exist until an event creates them.
        let placed: Vec<_> = frame
            .instances
            .iter()
            .filter(|i| i.parent_type == 0)
            .map(|i| {
                (
                    i.object_info,
                    i.x as f64,
                    i.y as f64,
                    i.layer.max(0) as usize,
                )
            })
            .collect();
        for (oi, x, y, layer) in placed {
            if let Some(i) = self.create_instance(oi, x, y, layer) {
                if let Some(g) = self.globals.get(&oi) {
                    let inst = &mut self.instances[i];
                    inst.values = g.values.clone();
                    inst.strings = g.strings.clone();
                    inst.counter = g.counter.clone();
                    inst.text = g.text.clone();
                }
            }
        }
    }

    /// Does object `oi` create widget instances (edit box, list, button, combo)?
    pub(crate) fn oi_has_widget(&self, oi: u16) -> bool {
        let Some(ObjectKind::Extension(t)) = self.game.object(oi).map(|o| o.kind) else {
            return false;
        };
        self.game
            .extensions
            .iter()
            .find(|e| e.handle as i32 == t as i32 - 32)
            .is_some_and(|e| widgets::WidgetKind::from_ext_name(&e.name).is_some())
    }

    /// Route clicks and typing to edit boxes, lists and buttons.
    fn widget_input(&mut self, input: &Input) {
        if !self.instances.iter().any(|i| i.widget.is_some()) {
            return;
        }
        let click = input.clicked[0] || input.double_clicked[0];
        if click {
            let (mx, my) = input.mouse;
            // Topmost visible widget under the mouse.
            let hit = (0..self.instances.len())
                .filter(|&i| {
                    let inst = &self.instances[i];
                    inst.widget.as_ref().is_some_and(|w| w.visible) && !inst.destroyed && self.mouse_over(i, mx, my)
                })
                .max_by_key(|&i| (self.instances[i].layer, self.instances[i].z));
            for (i, inst) in self.instances.iter_mut().enumerate() {
                if let Some(w) = inst.widget.as_mut() {
                    if Some(i) != hit && w.kind == widgets::WidgetKind::Edit {
                        w.focused = false;
                    }
                }
            }
            if let Some(i) = hit {
                let (bx, by, _, _) = self.bounds_of(i);
                let (lx, ly) = self.layer_mouse(i, mx, my);
                if let Some(w) = self.instances[i].widget.as_mut() {
                    w.click((lx - bx) as f32, (ly - by) as f32, input.double_clicked[0]);
                }
            }
        }
        let backspace = usize::from(input.pressed.contains(&8));
        if !input.typed.is_empty() || backspace > 0 {
            for w in self.instances.iter_mut().filter_map(|i| i.widget.as_mut()) {
                w.type_text(&input.typed, backspace);
            }
        }
    }

    /// Keep the state of objects marked "global" for the next frame (first instance of each).
    fn remember_globals(&mut self) {
        let mut seen = HashSet::default();
        for inst in self.instances.iter().filter(|i| !i.destroyed && !i.pasted) {
            let global = self
                .game
                .object(inst.oi)
                .is_some_and(|o| o.flags & crate::ctf_loader::objects::object_flags::GLOBAL != 0);
            if global && seen.insert(inst.oi) {
                self.globals.insert(
                    inst.oi,
                    GlobalObjectState {
                        values: inst.values.clone(),
                        strings: inst.strings.clone(),
                        counter: inst.counter.clone(),
                        text: inst.text.clone(),
                    },
                );
            }
        }
    }

    /// Create a new instance of object `oi`; returns its index.
    pub fn create_instance(&mut self, oi: u16, x: f64, y: f64, layer: usize) -> Option<usize> {
        let obj = self.game.object(oi)?;
        if let ObjectKind::Extension(t) = obj.kind {
            if !self.exts.contains_key(&oi) {
                let ext = self
                    .game
                    .extensions
                    .iter()
                    .find(|e| e.handle as i32 == t as i32 - 32);
                let data = match &obj.props {
                    ObjectProperties::Common(c) => c.extension_data.as_deref(),
                    _ => None,
                };
                if let Some(imp) = ext.and_then(|e| ext::create(&e.name, data, &self.ext_shared)) {
                    self.exts.insert(oi, imp);
                }
            }
        }
        let max_layer = self.frame().layers.len().saturating_sub(1);
        self.next_z += 1;
        let mut inst = Instance::new(self.next_id, obj, x, y, layer.min(max_layer), self.next_z);
        if let (ObjectKind::Extension(t), ObjectProperties::Common(c)) = (obj.kind, &obj.props) {
            let kind = self
                .game
                .extensions
                .iter()
                .find(|e| e.handle as i32 == t as i32 - 32)
                .and_then(|e| widgets::WidgetKind::from_ext_name(&e.name));
            if let Some(kind) = kind {
                let w = widgets::Widget::from_settings(kind, c.extension_data.as_deref().unwrap_or(&[]));
                inst.visual = Visual::Placeholder { w: w.w, h: w.h };
                inst.visible = w.visible;
                inst.widget = Some(Box::new(w));
            }
        }
        self.next_id += 1;
        // The Perspective extension is drawn natively as a distortion effect.
        if let (ObjectKind::Extension(t), ObjectProperties::Common(c)) = (obj.kind, &obj.props) {
            let is_perspective = self.game.extensions.iter().any(|e| {
                e.handle as i32 == t as i32 - 32 && e.name.eq_ignore_ascii_case("perspective")
            });
            if is_perspective {
                if let Some((params, w, h)) = c
                    .extension_data
                    .as_deref()
                    .and_then(crate::render::scene::EffectParams::from_perspective_data)
                {
                    inst.visual = Visual::Effect { w, h, params };
                    inst.visible = c.visible_at_start();
                }
            }
        }
        self.instances.push(inst);
        self.note_pushed();
        Some(self.instances.len() - 1)
    }

    /// Advance the game by one tick.
    pub fn tick(&mut self, input: &Input) {
        if self.finished || self.game.frames.is_empty() {
            return;
        }
        self.input_mouse = input.mouse;
        {
            let mut keys = self.ext_shared.keys.borrow_mut();
            keys.clear();
            keys.extend(input.down.iter().copied());
        }
        let dt = 1000.0 / self.frame_rate as f64;
        self.audio.update(dt);
        if let Some(t) = &mut self.transition {
            t.elapsed_ms += dt;
            if t.elapsed_ms >= t.duration_ms {
                let done = self.transition.take().expect("checked");
                if let Some(change) = done.then {
                    self.apply_frame_change(change);
                }
            }
            return;
        }
        let all_controls = self.game.header.controls.clone();
        let controls = all_controls[0].clone();
        self.update_platforms(input, &controls);
        let game = &self.game;
        for inst in self.instances.iter_mut().filter(|i| !i.destroyed) {
            let Some(obj) = game.object(inst.oi) else {
                continue;
            };
            inst.movement.prev = (inst.x, inst.y);
            if self.platformers.contains(&inst.id) {
                // Moved by update_platforms.
            } else if self.bullets.contains(&inst.id) {
                let s = inst.current_speed() * SPEED_TO_PIXELS;
                let a = dir_to_angle(inst.direction());
                inst.x += a.cos() * s;
                inst.y -= a.sin() * s;
            } else {
                inst.update_movement(obj, input, &all_controls);
            }
            inst.update_animation(obj);
        }

        // Animations that just ended trigger their "animation is over" events.
        if self.instances.iter().any(|i| i.anim.finished_anim.is_some() && !i.destroyed) {
            self.triggered_pass = events::Trigger::AnimationEnd;
            self.run_events(input, None);
            self.triggered_pass = events::Trigger::None;
        }
        // Triggered input events run as the input arrives, i.e. before the main pass.
        let any_input = !input.pressed.is_empty()
            || input.clicked.iter().any(|&c| c)
            || input.double_clicked.iter().any(|&c| c);
        self.widget_input(input);
        if self.tick == 0 {
            self.triggered_pass = events::Trigger::StartOfFrame;
            self.run_events(input, None);
        }
        if any_input && self.pending.is_none() {
            self.triggered_pass = events::Trigger::Input;
            self.run_events(input, None);
        }
        self.triggered_pass = events::Trigger::None;
        self.run_events(input, None);
        self.group_just_activated.clear();
        for w in self.instances.iter_mut().filter_map(|i| i.widget.as_mut()) {
            w.clear_events();
        }

        self.tick += 1;
        self.prev_timer_ms = self.timer_ms;
        self.timer_ms += 1000.0 / self.frame_rate as f64;
        if self.instances.iter().any(|i| i.destroyed) {
            self.instances.retain(|i| !i.destroyed);
            self.invalidate_index();
        }
        if self.pending.is_some() {
            // "End of frame" events run before leaving (games save their progress here).
            let change = self.pending.take();
            self.end_of_frame_pass = true;
            self.run_events(input, None);
            self.end_of_frame_pass = false;
            self.pending = change;
        }
        if let Some(change) = self.pending.take() {
            let fade = self
                .frame()
                .fade_out
                .as_ref()
                .and_then(|t| TransitionState::new(t, false, Some(change)));
            match fade {
                Some(f) => self.transition = Some(f),
                None => self.apply_frame_change(change),
            }
        }
    }

    fn apply_frame_change(&mut self, change: FrameChange) {
        match change {
            FrameChange::Jump(f) if f < self.game.frames.len() => self.load_frame(f),
            FrameChange::Jump(_) | FrameChange::End => self.finished = true,
            FrameChange::Restart => self.load_frame(self.frame_index),
        }
    }

    /// Index of the first obstacle (solid, or also platform when `platforms`) the instance overlaps.
    pub(crate) fn obstacle_hit(&self, idx: usize, platforms: bool) -> Option<usize> {
        // Obstacles (and their boxes) are gathered once per tick; they rarely move.
        let key = (self.tick, self.instances.len());
        let mut cache = self.obstacle_cache.borrow_mut();
        if cache.as_ref().is_none_or(|(k, _)| *k != key) {
            let list = self
                .instances
                .iter()
                .enumerate()
                .filter(|(_, i)| !i.destroyed && i.background && i.obstacle != 0)
                .map(|(o, i)| (o, i.obstacle, self.bounds_of(o)))
                .collect();
            *cache = Some((key, list));
        }
        let (ax, ay, aw, ah) = self.bounds_of(idx);
        let list = &cache.as_ref().expect("filled").1;
        list.iter().find_map(|&(o, kind, (bx, by, bw, bh))| {
            let kind_ok = kind == 1 || (platforms && kind == 2);
            let boxes = ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah;
            (o != idx && kind_ok && boxes && self.overlaps_pub(idx, o)).then_some(o)
        })
    }

    fn with_offset<T>(&mut self, idx: usize, dx: f64, dy: f64, f: impl FnOnce(&Self) -> T) -> T {
        self.instances[idx].x += dx;
        self.instances[idx].y += dy;
        let r = f(self);
        self.instances[idx].x -= dx;
        self.instances[idx].y -= dy;
        r
    }

    /// Fusion's platform movement: walking with acceleration, gravity, jumping, solid
    /// obstacles, jump-through platforms and small slopes.
    fn update_platforms(
        &mut self,
        input: &Input,
        controls: &crate::ctf_loader::header::PlayerControls,
    ) {
        use crate::ctf_loader::properties::{anim, MovementKind};
        self.platformers.clear();
        for idx in 0..self.instances.len() {
            let inst = &self.instances[idx];
            if inst.destroyed || inst.pasted {
                continue;
            }
            let Some(obj) = self.game.object(inst.oi) else {
                continue;
            };
            let ObjectProperties::Common(c) = &obj.props else {
                continue;
            };
            let Some(m) = c.movements.get(inst.movement.index) else {
                continue;
            };
            let MovementKind::Platform {
                speed,
                acceleration,
                deceleration,
                control,
                gravity,
                jump_strength,
            } = m.kind
            else {
                continue;
            };
            self.platformers.insert(inst.id);
            let stopped = inst.movement.stopped;
            let k = &controls.keys;
            let (left, right) = (input.down.contains(&k[2]), input.down.contains(&k[3]));
            let jump_key = match control {
                1 => Some(k[0]),
                2 => Some(k[4]),
                3 => Some(k[5]),
                _ => None,
            };
            let jump = !stopped
                && jump_key.is_some_and(|j| input.pressed.contains(&j) || input.down.contains(&j));

            let max = inst.movement.forced_speed.unwrap_or(speed as f64) * SPEED_TO_PIXELS;
            let dir = if stopped {
                0.0
            } else {
                (right as i32 - left as i32) as f64
            };
            let mut vx = inst.movement.vx;
            if dir != 0.0 {
                vx += dir * max * (acceleration.max(1) as f64 / 100.0);
                vx = vx.clamp(-max, max);
            } else {
                let dec = max * (deceleration.max(1) as f64 / 100.0);
                vx = if vx.abs() <= dec {
                    0.0
                } else {
                    vx - vx.signum() * dec
                };
            }
            let prev_vy = inst.movement.vy;
            let mut vy = prev_vy + gravity as f64 * 0.05;
            vy = vy.min(16.0);
            let on_ground =
                self.with_offset(idx, 0.0, 1.0, |rt| rt.obstacle_hit(idx, true).is_some());
            if on_ground && jump && prev_vy >= 0.0 {
                vy = -(jump_strength as f64) * 0.25;
            }

            // Horizontal: pixel steps, climbing up to 4px slopes.
            let steps = vx.abs().round() as i32;
            for _ in 0..steps {
                let sx = vx.signum();
                let blocked =
                    self.with_offset(idx, sx, 0.0, |rt| rt.obstacle_hit(idx, false).is_some());
                if !blocked {
                    self.instances[idx].x += sx;
                    continue;
                }
                let climb = (1..=4).find(|&up| {
                    !self.with_offset(idx, sx, -(up as f64), |rt| {
                        rt.obstacle_hit(idx, false).is_some()
                    })
                });
                match climb {
                    Some(up) if on_ground => {
                        self.instances[idx].x += sx;
                        self.instances[idx].y -= up as f64;
                    }
                    _ => {
                        vx = 0.0;
                        break;
                    }
                }
            }
            // Vertical: platforms only stop downward motion when we start above them.
            let mut landed = false;
            let steps = vy.abs().round() as i32;
            for _ in 0..steps {
                let sy = vy.signum();
                let hit = if sy > 0.0 {
                    let was_inside = self.obstacle_hit(idx, true).is_some();
                    !was_inside
                        && self.with_offset(idx, 0.0, sy, |rt| rt.obstacle_hit(idx, true).is_some())
                } else {
                    self.with_offset(idx, 0.0, sy, |rt| rt.obstacle_hit(idx, false).is_some())
                };
                if hit {
                    landed = sy > 0.0;
                    vy = 0.0;
                    break;
                }
                self.instances[idx].y += sy;
            }
            let inst = &mut self.instances[idx];
            inst.movement.vx = vx;
            inst.movement.vy = vy;
            inst.movement.on_ground = landed || (on_ground && vy >= 0.0);
            inst.movement.speed = if max > 0.0 {
                vx.abs() / max * speed as f64
            } else {
                0.0
            };
            if dir != 0.0 {
                inst.anim.dir = if dir > 0.0 { 0 } else { 16 };
            }
            inst.movement.anim_hint = Some(if !inst.movement.on_ground {
                if vy < 0.0 {
                    anim::JUMPING
                } else {
                    anim::FALLING
                }
            } else if vx != 0.0 {
                anim::WALKING
            } else {
                anim::STOPPED
            });
        }
    }

    /// Copy an instance's current look into the background ("paste into background" / "add backdrop").
    pub(crate) fn paste_into_background(&mut self, idx: usize, obstacle: u16) {
        let Some(obj) = self.game.object(self.instances[idx].oi) else {
            return;
        };
        let Some(handle) = self.instances[idx].current_image(obj) else {
            return;
        };
        let mut copy = self.instances[idx].clone();
        copy.id = self.next_id;
        self.next_id += 1;
        copy.visual = Visual::Image { handle };
        copy.background = true;
        copy.pasted = true;
        copy.obstacle = obstacle;
        copy.visible = true;
        self.instances.push(copy);
        self.note_pushed();
    }

    /// Play a bank sample (estimating its length if the backend hasn't reported one yet).
    pub fn play_sample(&mut self, handle: u32, channel: Option<usize>, loops: u32) {
        if self.game.sounds.get(handle).is_none() {
            return;
        }
        if !self.audio.durations.contains_key(&handle) {
            let ms = self
                .game
                .sounds
                .get(handle)
                .and_then(|s| s.data().ok())
                .and_then(|d| crate::audio::estimate_duration_ms(&d))
                .unwrap_or(1000.0);
            self.audio.durations.insert(handle, ms);
        }
        self.audio.play(handle, channel, loops);
    }

    fn invalidate_index(&self) {
        *self.oi_index.borrow_mut() = None;
    }

    /// Keep the index in step with an instance appended at the end.
    fn note_pushed(&self) {
        let idx = self.instances.len() - 1;
        if let Some(map) = self.oi_index.borrow_mut().as_mut() {
            map.entry(self.instances[idx].oi).or_default().push(idx);
        }
    }

    /// Indices of all instances (live or not) of an object handle.
    pub(crate) fn indices_of(&self, oi: u16) -> Vec<usize> {
        let mut guard = self.oi_index.borrow_mut();
        let map = guard.get_or_insert_with(|| {
            let mut m: HashMap<u16, Vec<usize>> = HashMap::default();
            for (k, i) in self.instances.iter().enumerate() {
                m.entry(i.oi).or_default().push(k);
            }
            m
        });
        map.get(&oi).cloned().unwrap_or_default()
    }

    pub fn random(&self, n: u32) -> u32 {
        // xorshift64*
        let mut x = self.rng.get();
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng.set(x);
        let r = (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as u32;
        if n == 0 {
            0
        } else {
            r % n
        }
    }

    pub fn global_value(&self, i: usize) -> Value {
        self.global_values.get(i).cloned().unwrap_or_default()
    }

    pub fn set_global_value(&mut self, i: usize, v: Value) {
        if i < 4096 {
            if i >= self.global_values.len() {
                self.global_values.resize(i + 1, Value::Int(0));
            }
            self.global_values[i] = v;
        }
    }

    pub fn global_string(&self, i: usize) -> String {
        self.global_strings.get(i).cloned().unwrap_or_default()
    }

    pub fn set_global_string(&mut self, i: usize, s: String) {
        if i < 4096 {
            if i >= self.global_strings.len() {
                self.global_strings.resize(i + 1, String::new());
            }
            self.global_strings[i] = s;
        }
    }

    pub fn loop_index(&self, name: &str) -> i32 {
        self.loops.get(&name.to_lowercase()).map_or(0, |l| l.index)
    }

    fn group_enabled(&self, id: u16) -> bool {
        !self.groups_disabled.contains(&id)
    }

    fn set_group_enabled(&mut self, id: u16, on: bool) {
        if on {
            if self.groups_disabled.remove(&id) {
                // Find the group header event so "on group activation" can fire.
                for (gi, g) in self.events.groups.iter().enumerate() {
                    let is_header = g.conditions.first().is_some_and(|c| {
                        c.num == -10
                            && matches!(c.params.first().map(|p| &p.value),
                                Some(crate::ctf_loader::events::Param::Group { id: gid, .. }) if *gid == id)
                    });
                    if is_header {
                        // The first event inside the group carries the activation trigger.
                        self.group_just_activated.insert(gi as u32 + 1);
                    }
                }
            }
        } else {
            self.groups_disabled.insert(id);
        }
    }

    fn center_display(&mut self, x: Option<f64>, y: Option<f64>) {
        let (fw, fh) = (self.frame().width as f64, self.frame().height as f64);
        let (vw, vh) = self.view_size;
        let old = self.scroll;
        if let Some(x) = x {
            self.scroll.0 = (x - vw / 2.0).clamp(0.0, (fw - vw).max(0.0));
        }
        if let Some(y) = y {
            self.scroll.1 = (y - vh / 2.0).clamp(0.0, (fh - vh).max(0.0));
        }
        self.shift_partial_layers(self.scroll.0 - old.0, self.scroll.1 - old.1);
    }

    /// Objects keep frame coordinates everywhere. Objects that don't follow the frame move
    /// by each scroll step, and a layer that scrolls at a fraction c of the camera moves
    /// its objects by (1 - c) of it, as the original runtime does: a HUD stays on screen
    /// while its coordinates follow the camera, so positions relative to other objects
    /// and "set Y to the camera position" land where the game expects.
    fn shift_partial_layers(&mut self, dx: f64, dy: f64) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        let coefs: Vec<(f64, f64)> = self
            .frame()
            .layers
            .iter()
            .map(|l| {
                let (cx, cy) = l.scroll_coefficients();
                (1.0 - cx as f64, 1.0 - cy as f64)
            })
            .collect();
        for inst in self.instances.iter_mut().filter(|i| !i.destroyed) {
            let Some(&(kx, ky)) = (if inst.fixed { Some(&(1.0, 1.0)) } else { coefs.get(inst.layer) }) else { continue };
            if kx != 0.0 || ky != 0.0 {
                inst.x += dx * kx;
                inst.y += dy * ky;
            }
        }
    }

    /// Collision mask of an image, decoded on first use.
    pub(crate) fn mask(&self, handle: u32) -> Option<Rc<Mask>> {
        self.masks
            .borrow_mut()
            .entry(handle)
            .or_insert_with(|| {
                let img = self.game.images.get(handle)?.decode_rgba().ok()?;
                Some(Rc::new(Mask::from_rgba(&img)))
            })
            .clone()
    }

    pub(crate) fn ext_expression(&self, oi: u16, idx: usize, args: &[Value]) -> Option<Value> {
        self.exts.get(&oi)?.expression(idx, args, &self.files)
    }

    /// Image (handle) of the object used as the mouse pointer, if any.
    /// A visible object moved by the mouse over most of the screen is on screen: the game
    /// draws its own pointer with it, so the system pointer is hidden.
    pub fn has_mouse_cursor_object(&self) -> bool {
        use crate::ctf_loader::properties::MovementKind;
        self.instances.iter().any(|i| {
            !i.destroyed
                && i.visible
                && self.game.object(i.oi).is_some_and(|o| match &o.props {
                    ObjectProperties::Common(c) => {
                        // A pointer roams most of the screen; small areas are wobble effects.
                        matches!(c.movements.get(i.movement.index).map(|m| &m.kind),
                            Some(MovementKind::Mouse { x1, x2, y1, y2 }) if x2 - x1 >= 300 && y2 - y1 >= 200)
                    }
                    _ => false,
                })
        })
    }

    pub fn cursor_image(&self) -> Option<u32> {
        let name = self.cursor_object.as_ref()?;
        self.instances.iter().filter(|i| !i.destroyed).find_map(|i| {
            let obj = self.game.object(i.oi)?;
            if !obj.name.eq_ignore_ascii_case(name) {
                return None;
            }
            match &i.visual {
                Visual::Image { .. } if i.kind == ObjectKind::Active => i.current_image(obj),
                Visual::Image { handle } => Some(*handle),
                _ => None,
            }
        })
    }

    /// Readable owner of an ACE for diagnostics, e.g. "Active", "System", "ext:kcini".
    pub fn ace_owner_label(&self, object_type: i16) -> String {
        match object_type {
            -1 => "System".into(),
            -2 => "Sound".into(),
            -3 => "Game".into(),
            -4 => "Timer".into(),
            -5 => "Create".into(),
            -6 => "Mouse/Keyboard".into(),
            -7 => "Player".into(),
            t if t >= 32 => {
                let ext = self
                    .game
                    .extensions
                    .iter()
                    .find(|e| e.handle as i32 == t as i32 - 32);
                format!("ext:{}", ext.map_or("?", |e| e.name.as_str()))
            }
            t => ObjectKind::from_type_id(t).label(),
        }
    }

    /// Record an unimplemented feature and return `ret`.
    pub fn note_unimplemented<T>(&self, what: String, ret: T) -> T {
        *self.unimplemented.borrow_mut().entry(what).or_insert(0) += 1;
        ret
    }

    /// The value a Counter / Score / Lives instance shows.
    pub fn displayed_value(&self, inst: &Instance, player: u16) -> Value {
        let p = (player.max(1) as usize - 1).min(3);
        match inst.kind {
            ObjectKind::Score => Value::Int(self.score[p]),
            ObjectKind::Lives => Value::Int(self.lives[p]),
            _ => inst.counter.clone(),
        }
    }

    /// Visual for String / Counter / Score / Lives objects, plus an offset to its draw position.
    fn hud_visual(
        &self,
        inst: &Instance,
        obj: &crate::ctf_loader::ObjectInfo,
    ) -> Option<(Visual, f32, f32)> {
        use crate::ctf_loader::properties::{paragraph_flags, CounterDisplayKind, Fill};
        let ObjectProperties::Common(c) = &obj.props else {
            return None;
        };
        if let Some(t) = &c.text {
            let p = t.paragraphs.first();
            return Some((
                Visual::Text {
                    w: t.width as f32,
                    h: t.height as f32,
                    text: inst.text.clone(),
                    font: p.map(|p| p.font as u32),
                    color: inst
                        .text_color
                        .unwrap_or(p.map_or([0, 0, 0, 255], |p| p.color)),
                    flags: p.map_or(0, |p| p.flags),
                    size: inst.font_size,
                },
                0.0,
                0.0,
            ));
        }
        let d = c.counter_display.as_ref()?;
        let value = self.displayed_value(inst, d.player);
        let (w, h) = (d.width as f32, d.height as f32);
        let (min, max) = (inst.counter_min, inst.counter_max);
        let fraction = if max > min && max < 1e9 && min > -1e9 {
            ((value.as_f64() - min) / (max - min)).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        Some(match &d.kind {
            CounterDisplayKind::Hidden | CounterDisplayKind::Unknown(_) => return None,
            CounterDisplayKind::Numbers(frames) => {
                let handles = value
                    .to_string()
                    .chars()
                    .filter_map(|ch| {
                        let i = match ch {
                            '0'..='9' => ch as usize - '0' as usize,
                            '-' => 10,
                            '+' => 11,
                            '.' => 12,
                            'e' | 'E' => 13,
                            _ => return None,
                        };
                        frames.get(i).map(|&h| h as u32)
                    })
                    .collect();
                (Visual::Glyphs { handles }, 0.0, 0.0)
            }
            CounterDisplayKind::Animation(frames) => {
                if frames.is_empty() {
                    return None;
                }
                if inst.kind == ObjectKind::Lives {
                    // One icon per life.
                    let handles = vec![frames[0] as u32; value.as_i32().clamp(0, 64) as usize];
                    (Visual::Glyphs { handles }, 0.0, 0.0)
                } else {
                    // A single frame, placed by its hotspot like an Active's image.
                    let i = (fraction * (frames.len() - 1) as f32).round() as usize;
                    (Visual::Image { handle: frames[i.min(frames.len() - 1)] as u32 }, 0.0, 0.0)
                }
            }
            CounterDisplayKind::HorizontalBar(shape) => (
                Visual::Shape {
                    w: w * fraction,
                    h,
                    shape: shape.clone(),
                },
                0.0,
                0.0,
            ),
            CounterDisplayKind::VerticalBar(shape) => {
                let bh = h * fraction;
                (
                    Visual::Shape {
                        w,
                        h: bh,
                        shape: shape.clone(),
                    },
                    0.0,
                    h - bh,
                )
            }
            CounterDisplayKind::Text(shape) => {
                let color = match shape.fill {
                    Fill::Solid(c) | Fill::Gradient { from: c, .. } => c,
                    _ => [0, 0, 0, 255],
                };
                (
                    Visual::Text {
                        w,
                        h,
                        text: value.to_string(),
                        font: Some(d.font as u32),
                        color: inst.text_color.unwrap_or(color),
                        flags: paragraph_flags::RIGHT | paragraph_flags::BOTTOM,
                        size: inst.font_size,
                    },
                    -w,
                    -h,
                )
            }
        })
    }

    /// Everything to draw this frame, in back-to-front order.
    pub fn draw_items(&self) -> Vec<SceneItem> {
        let last_layer = self.frame().layers.len().saturating_sub(1);
        let mut v: Vec<(i64, SceneItem)> = Vec::with_capacity(self.instances.len());
        for inst in self.instances.iter().filter(|i| !i.destroyed) {
            let Some(obj) = self.game.object(inst.oi) else {
                continue;
            };
            let (mut x, mut y) = (inst.x as f32, inst.y as f32);
            let visual = match (&inst.visual, inst.kind) {
                _ if inst.widget.is_some() => Visual::Widget(inst.widget.clone().expect("checked")),
                (Visual::Image { .. }, ObjectKind::Active) if !inst.pasted => {
                    match inst.current_image(obj) {
                        Some(h) => Visual::Image { handle: h },
                        None => continue,
                    }
                }
                (
                    _,
                    ObjectKind::Text | ObjectKind::Counter | ObjectKind::Score | ObjectKind::Lives,
                ) => match self.hud_visual(inst, obj) {
                    Some((v, dx, dy)) => {
                        x += dx;
                        y += dy;
                        v
                    }
                    None => continue,
                },
                (v, _) => {
                    if let (Visual::Image { handle }, true) = (v, inst.background) {
                        if let Some(e) = self.game.images.get(*handle) {
                            x += e.hotspot.0 as f32;
                            y += e.hotspot.1 as f32;
                        }
                    }
                    v.clone()
                }
            };
            let label = match inst.kind {
                ObjectKind::Counter => format!("{} = {}", obj.name, inst.counter),
                ObjectKind::Text => format!("{}: {}", obj.name, inst.text),
                _ => obj.name.clone(),
            };
            v.push((
                inst.z,
                SceneItem {
                    layer: inst.layer.min(last_layer),
                    x,
                    y,
                    visual,
                    tint: inst.tint,
                    ink_effect: inst.ink_effect,
                    visible: inst.visible,
                    background: inst.background,
                    label,
                    scale: (inst.scale_x as f32, inst.scale_y as f32),
                    angle: inst.angle as f32,
                    fixed: inst.fixed,
                },
            ));
        }
        v.sort_by_key(|(z, i)| (i.layer, !i.background, *z));
        v.into_iter().map(|(_, i)| i).collect()
    }
}
