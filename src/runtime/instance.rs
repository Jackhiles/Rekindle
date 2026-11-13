//! Live object instances: position, visibility, values, animation and movement state.

use super::input::Input;
use super::value::Value;
use crate::ctf_loader::header::PlayerControls;
use crate::ctf_loader::properties::{
    anim, Animation, AnimationDirection, CounterDisplayKind, MovementKind, ObjectCommon,
};
use crate::ctf_loader::{GameData, ObjectInfo, ObjectKind, ObjectProperties};
use crate::render::scene::Visual;

/// Fusion movement speeds are 0..100 "units"; this converts to pixels per tick.
pub const SPEED_TO_PIXELS: f64 = 1.0 / 8.0;

#[derive(Debug, Clone, Default)]
pub struct AnimState {
    pub anim: usize,
    pub forced_anim: Option<usize>,
    /// 0..32, 0 = right, 8 = up.
    pub dir: usize,
    pub forced_dir: Option<usize>,
    pub frame: usize,
    pub forced_frame: Option<usize>,
    pub counter: i32,
    pub loops: u16,
    pub stopped: bool,
    /// Set on the tick an animation reaches its end.
    pub finished_anim: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct MoveState {
    pub index: usize,
    pub speed: f64,
    pub forced_speed: Option<f64>,
    /// "Set maximum speed": caps the movement's speed.
    pub max_speed: Option<f64>,
    pub stopped: bool,
    pub reversed: bool,
    // Path movement.
    pub step: usize,
    pub step_progress: f64,
    pub pause_ticks: u32,
    pub path_finished: bool,
    pub start: (f64, f64),
    /// Subpixel remainder so slow speeds still move.
    pub frac: (f64, f64),
    /// Position before this tick's movement ("stop" moves back here out of obstacles).
    pub prev: (f64, f64),
    // Platform movement.
    pub vx: f64,
    pub vy: f64,
    pub on_ground: bool,
    /// Animation the movement wants (walking, jumping, falling), if the object has it.
    pub anim_hint: Option<usize>,
    /// Mouse movement: the pointer's position in the window last tick.
    pub last_mouse: Option<(f64, f64)>,
}

#[derive(Debug, Clone)]
pub struct Instance {
    pub id: u32,
    pub oi: u16,
    pub kind: ObjectKind,
    pub x: f64,
    pub y: f64,
    pub layer: usize,
    pub z: i64,
    pub visible: bool,
    pub background: bool,
    pub destroyed: bool,
    pub values: Vec<Value>,
    pub strings: Vec<String>,
    pub flags: u32,
    pub anim: AnimState,
    pub movement: MoveState,
    /// RGB tint and alpha (0..=255).
    pub tint: [u8; 4],
    pub ink_effect: u16,
    pub counter: Value,
    pub counter_min: f64,
    pub counter_max: f64,
    pub text: String,
    pub visual: Visual,
    pub scale_x: f64,
    pub scale_y: f64,
    /// Degrees, counter-clockwise.
    pub angle: f64,
    /// Obstacle type for backdrops: 0 none, 1 solid, 2 platform, 3 ladder.
    pub obstacle: u16,
    /// A copy pasted into the background (not selectable by events any more).
    pub pasted: bool,
    pub text_color: Option<[u8; 4]>,
    pub font_size: Option<f32>,
    /// "Follow the frame" is off: the position is in screen coordinates and ignores scrolling.
    pub fixed: bool,
    /// Windows-control extensions (edit box, list, button, combo box).
    pub widget: Option<Box<super::widgets::Widget>>,
}

impl Instance {
    /// Move the object from an action. The movement's origin (where a path restarts with
    /// "reposition at end") moves along with it.
    pub fn set_position(&mut self, x: f64, y: f64) {
        self.movement.start.0 += x - self.x;
        self.movement.start.1 += y - self.y;
        self.x = x;
        self.y = y;
    }

    /// Settings of a distortion-effect object, if this is one.
    pub fn effect_params(&self) -> Option<crate::render::scene::EffectParams> {
        match &self.visual {
            Visual::Effect { params, .. } => Some(*params),
            _ => None,
        }
    }
}

pub fn dir_to_angle(dir: usize) -> f64 {
    (dir % 32) as f64 * std::f64::consts::TAU / 32.0
}

/// Direction 0..32 pointing from (0,0) towards (dx, dy) in screen coordinates.
pub fn angle_to_dir(dx: f64, dy: f64) -> usize {
    let a = (-dy).atan2(dx).rem_euclid(std::f64::consts::TAU);
    ((a / std::f64::consts::TAU * 32.0).round() as usize) % 32
}

/// Pick a direction from a 32-bit direction mask (the lowest set bit; 0 if empty).
pub fn dir_from_mask(mask: u32, rnd: u32) -> usize {
    let set: Vec<usize> = (0..32).filter(|b| mask & (1 << b) != 0).collect();
    if set.is_empty() {
        0
    } else {
        set[rnd as usize % set.len()]
    }
}

impl Instance {
    pub fn new(id: u32, obj: &ObjectInfo, x: f64, y: f64, layer: usize, z: i64) -> Self {
        let ink = obj.ink();
        let mut inst = Instance {
            id,
            oi: obj.handle,
            kind: obj.kind,
            x,
            y,
            layer,
            z,
            visible: true,
            background: false,
            destroyed: false,
            values: Vec::new(),
            strings: Vec::new(),
            flags: 0,
            anim: AnimState::default(),
            movement: MoveState {
                start: (x, y),
                ..Default::default()
            },
            tint: ink.rgba,
            ink_effect: ink.effect,
            counter: Value::Int(0),
            counter_min: f64::MIN,
            counter_max: f64::MAX,
            text: String::new(),
            visual: Visual::Placeholder { w: 32.0, h: 32.0 },
            scale_x: 1.0,
            scale_y: 1.0,
            angle: 0.0,
            obstacle: 0,
            pasted: false,
            text_color: None,
            font_size: None,
            fixed: false,
            widget: None,
        };
        match &obj.props {
            ObjectProperties::Backdrop(b) => {
                inst.background = true;
                inst.obstacle = b.obstacle;
                inst.visual = Visual::Image {
                    handle: b.image as u32,
                };
            }
            ObjectProperties::QuickBackdrop(q) => {
                inst.background = true;
                inst.obstacle = q.obstacle;
                inst.visual = Visual::Shape {
                    w: q.width as f32,
                    h: q.height as f32,
                    shape: q.shape.clone(),
                };
            }
            ObjectProperties::Common(c) => {
                inst.visible = c.visible_at_start();
                inst.fixed = c.flags & crate::ctf_loader::properties::oc_flags::SCROLLING_INDEPENDENT != 0;
                inst.values = c.alterable_values.iter().map(|&v| Value::Int(v)).collect();
                inst.strings = c.alterable_strings.clone();
                if let Some(cs) = &c.counter {
                    inst.counter = Value::Int(cs.initial);
                    inst.counter_min = cs.minimum as f64;
                    inst.counter_max = cs.maximum as f64;
                }
                if let Some(m) = c.movements.first() {
                    inst.movement.stopped = !m.moving_at_start;
                    inst.anim.dir = dir_from_mask(m.direction_at_start, 0);
                    inst.movement.speed = initial_speed(&m.kind);
                }
                if let Some(p) = c.text.as_ref().and_then(|t| t.paragraphs.first()) {
                    inst.text = p.text.clone();
                }
                if obj.kind == ObjectKind::Active {
                    inst.visual = Visual::Image { handle: 0 };
                    inst.anim.anim = default_anim(c);
                } else if !matches!(
                    obj.kind,
                    ObjectKind::Counter | ObjectKind::Text | ObjectKind::Score | ObjectKind::Lives
                ) {
                    // Extensions etc. have no visuals yet.
                    inst.visible = false;
                }
            }
            ObjectProperties::None => inst.visible = false,
        }
        inst
    }

    pub fn value(&self, i: usize) -> Value {
        self.values.get(i).cloned().unwrap_or_default()
    }

    pub fn set_value(&mut self, i: usize, v: Value) {
        if i >= self.values.len() {
            if i > 4096 {
                return;
            }
            self.values.resize(i + 1, Value::Int(0));
        }
        self.values[i] = v;
    }

    pub fn string(&self, i: usize) -> String {
        self.strings.get(i).cloned().unwrap_or_default()
    }

    pub fn set_string(&mut self, i: usize, s: String) {
        if i >= self.strings.len() {
            if i > 4096 {
                return;
            }
            self.strings.resize(i + 1, String::new());
        }
        self.strings[i] = s;
    }

    pub fn set_counter(&mut self, v: Value) {
        let f = v.as_f64().clamp(self.counter_min, self.counter_max);
        self.counter = if matches!(v, Value::Float(_)) {
            Value::Float(f)
        } else {
            Value::Int(f as i32)
        };
    }

    pub fn direction(&self) -> usize {
        self.anim.forced_dir.unwrap_or(self.anim.dir)
    }

    pub fn current_speed(&self) -> f64 {
        if self.movement.stopped {
            0.0
        } else {
            let s = self.movement.forced_speed.unwrap_or(self.movement.speed);
            self.movement.max_speed.map_or(s, |m| s.min(m))
        }
    }

    pub fn current_anim_id(&self) -> usize {
        self.anim.forced_anim.unwrap_or(self.anim.anim)
    }

    fn current_direction_data<'a>(
        &self,
        common: &'a ObjectCommon,
    ) -> Option<&'a AnimationDirection> {
        let anims = &common.animations;
        let a: &Animation = anims
            .get(self.current_anim_id())
            .and_then(|a| a.as_ref())
            .or_else(|| anims.iter().flatten().next())?;
        a.nearest_direction(self.direction())
    }

    /// The image handle to draw for an Active.
    pub fn current_image(&self, obj: &ObjectInfo) -> Option<u32> {
        let ObjectProperties::Common(c) = &obj.props else {
            return None;
        };
        let d = self.current_direction_data(c)?;
        let f = self
            .anim
            .forced_frame
            .unwrap_or(self.anim.frame)
            .min(d.frames.len().saturating_sub(1));
        d.frames.get(f).map(|&h| h as u32)
    }

    /// Set a (forced) animation, restarting it if it changed.
    pub fn force_anim(&mut self, a: usize) {
        if self.anim.forced_anim != Some(a) {
            self.anim.forced_anim = Some(a);
            self.restart_anim();
        }
        self.anim.stopped = false;
    }

    pub fn restart_anim(&mut self) {
        self.anim.frame = 0;
        self.anim.counter = 0;
        self.anim.loops = 0;
        self.anim.stopped = false;
    }

    /// Advance the animation by one tick.
    pub fn update_animation(&mut self, obj: &ObjectInfo) {
        self.anim.finished_anim = None;
        let ObjectProperties::Common(c) = &obj.props else {
            return;
        };
        if c.animations.is_empty() {
            return;
        }
        // Automatic animation follows the movement when not forced.
        if self.anim.forced_anim.is_none() {
            let want = match self.movement.anim_hint {
                Some(h) if has_anim(c, h) => h,
                _ if self.current_speed() > 0.0 && has_anim(c, anim::WALKING) => anim::WALKING,
                _ => default_anim(c),
            };
            if want != self.anim.anim {
                self.anim.anim = want;
                self.restart_anim();
            }
        }
        if self.anim.stopped || self.anim.forced_frame.is_some() {
            return;
        }
        let Some(d) = self.current_direction_data(c) else {
            return;
        };
        let len = d.frames.len();
        if len == 0 || (len == 1 && d.repeat == 0) {
            return;
        }
        let (min, max) = (d.min_speed as f64, d.max_speed as f64);
        let speed = if max <= min {
            max
        } else {
            min + (max - min) * (self.current_speed().min(100.0) / 100.0)
        };
        let (repeat, back_to) = (d.repeat, d.back_to as usize);
        self.anim.counter += speed as i32;
        while self.anim.counter >= 100 {
            self.anim.counter -= 100;
            self.anim.frame += 1;
            if self.anim.frame >= len {
                self.anim.loops += 1;
                if repeat == 0 || self.anim.loops < repeat {
                    self.anim.frame = back_to.min(len - 1);
                } else {
                    self.anim.frame = len - 1;
                    self.anim.stopped = true;
                    self.anim.finished_anim = Some(self.current_anim_id());
                    break;
                }
            }
        }
    }

    /// Move by (dx, dy) pixels, keeping subpixel precision.
    fn shift(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
    }

    /// Advance the object's movement by one tick.
    /// `screen_mouse`: the mouse position in the window (game pixels), for mouse movement.
    /// `controls`: the four players' controls (a movement follows its own player's).
    pub fn update_movement(&mut self, obj: &ObjectInfo, input: &Input, controls: &[PlayerControls; 4], screen_mouse: (f64, f64)) {
        let ObjectProperties::Common(c) = &obj.props else {
            return;
        };
        let Some(m) = c.movements.get(self.movement.index) else {
            return;
        };
        match &m.kind {
            MovementKind::Ball { .. } => {
                let s = self.current_speed() * SPEED_TO_PIXELS;
                if s > 0.0 {
                    let a = dir_to_angle(self.direction());
                    self.shift(a.cos() * s, -a.sin() * s);
                }
            }
            MovementKind::EightDirections {
                speed, directions, ..
            } => {
                if self.movement.stopped {
                    return;
                }
                // Movements are numbered by player 1-4 (0 in older files means player 1).
                let k = &controls[(m.player.max(1) as usize - 1).min(3)].keys;
                let dx = input.down.contains(&k[3]) as i32 - input.down.contains(&k[2]) as i32;
                let dy = input.down.contains(&k[1]) as i32 - input.down.contains(&k[0]) as i32;
                if dx == 0 && dy == 0 {
                    self.movement.speed = 0.0;
                    return;
                }
                let dir = angle_to_dir(dx as f64, dy as f64);
                if *directions != 0 && directions & (1 << dir) == 0 {
                    self.movement.speed = 0.0;
                    return;
                }
                self.anim.dir = dir;
                self.movement.speed = *speed as f64;
                let s = self.current_speed() * SPEED_TO_PIXELS;
                let a = dir_to_angle(dir);
                self.shift(a.cos() * s, -a.sin() * s);
            }
            // Moves by as much as the mouse moved in the window, within its area around
            // where it started (so an office that pans with the mouse is centred while the
            // pointer is, and following the pointer's frame position can't run away when
            // the camera follows the object).
            MovementKind::Mouse { x1, x2, y1, y2 } => {
                if let Some((px, py)) = self.movement.last_mouse {
                    let (sx, sy) = self.movement.start;
                    let (dx, dy) = (screen_mouse.0 - px, screen_mouse.1 - py);
                    self.x = (self.x + dx).clamp(sx + *x1 as f64, sx + *x2 as f64);
                    self.y = (self.y + dy).clamp(sy + *y1 as f64, sy + *y2 as f64);
                }
                self.movement.last_mouse = Some(screen_mouse);
            }
            MovementKind::Path {
                steps,
                looping,
                reposition_at_end,
                reverse_at_end,
                ..
            } => {
                if self.movement.stopped || steps.is_empty() || self.movement.path_finished {
                    return;
                }
                if self.movement.pause_ticks > 0 {
                    self.movement.pause_ticks -= 1;
                    return;
                }
                let idx = self.movement.step.min(steps.len() - 1);
                let step = &steps[idx];
                let (mut dx, mut dy) = (step.dx as f64, step.dy as f64);
                if self.movement.reversed {
                    dx = -dx;
                    dy = -dy;
                }
                let len = (dx * dx + dy * dy).sqrt().max(1.0);
                let speed = self.movement.forced_speed.unwrap_or(step.speed as f64);
                self.movement.speed = speed;
                let travel = (speed * SPEED_TO_PIXELS).max(0.0);
                let remaining = len - self.movement.step_progress;
                let d = travel.min(remaining);
                self.shift(dx / len * d, dy / len * d);
                self.anim.dir = angle_to_dir(dx, dy);
                self.movement.step_progress += d;
                if self.movement.step_progress >= len - 1e-6 {
                    self.movement.step_progress = 0.0;
                    self.movement.pause_ticks = (step.pause.max(0) as u32) * 60 / 1000;
                    let n = steps.len();
                    let last = if self.movement.reversed {
                        self.movement.step == 0
                    } else {
                        self.movement.step + 1 >= n
                    };
                    if !last {
                        if self.movement.reversed {
                            self.movement.step -= 1;
                        } else {
                            self.movement.step += 1;
                        }
                    } else if *reverse_at_end {
                        self.movement.reversed = !self.movement.reversed;
                    } else if *looping {
                        if *reposition_at_end {
                            (self.x, self.y) = self.movement.start;
                        }
                        self.movement.step = 0;
                    } else {
                        self.movement.path_finished = true;
                        self.movement.speed = 0.0;
                    }
                }
            }
            _ => {}
        }
    }
}

fn has_anim(c: &ObjectCommon, a: usize) -> bool {
    c.animations.get(a).is_some_and(|x| x.is_some())
}

fn default_anim(c: &ObjectCommon) -> usize {
    if has_anim(c, anim::STOPPED) {
        anim::STOPPED
    } else {
        c.animations.iter().position(|a| a.is_some()).unwrap_or(0)
    }
}

fn initial_speed(k: &MovementKind) -> f64 {
    match k {
        MovementKind::Ball { speed, .. } => *speed as f64,
        MovementKind::Path { max_speed, .. } => *max_speed as f64,
        _ => 0.0,
    }
}

/// Bounding box (x, y, w, h) of an instance in frame coordinates.
pub fn bounds(g: &GameData, inst: &Instance, obj: &ObjectInfo) -> (f64, f64, f64, f64) {
    if let ObjectProperties::Common(c) = &obj.props {
        if let Some(t) = &c.text {
            return (inst.x, inst.y, t.width as f64, t.height as f64);
        }
        if let Some(d) = &c.counter_display {
            let (w, h) = (d.width as f64, d.height as f64);
            return match d.kind {
                CounterDisplayKind::HorizontalBar(_) | CounterDisplayKind::VerticalBar(_) => {
                    (inst.x, inst.y, w, h)
                }
                _ => (inst.x - w, inst.y - h, w, h),
            };
        }
    }
    match &inst.visual {
        Visual::Widget(w) => (inst.x, inst.y, w.w as f64, w.h as f64),
        Visual::Image { .. } => {
            let handle = if inst.kind == ObjectKind::Active && !inst.pasted {
                inst.current_image(obj)
            } else if let Visual::Image { handle } = inst.visual {
                Some(handle)
            } else {
                None
            };
            match handle.and_then(|h| g.images.get(h)) {
                Some(e) => {
                    let (sx, sy) = (inst.scale_x, inst.scale_y);
                    // Backdrops are placed by their top-left corner; only sprites use the hotspot.
                    let (hx, hy) = if inst.background { (0.0, 0.0) } else { (e.hotspot.0 as f64, e.hotspot.1 as f64) };
                    let (w, h) = (e.width as f64, e.height as f64);
                    if inst.angle == 0.0 {
                        let (x0, x1) = ((0.0 - hx) * sx, (w - hx) * sx);
                        let (y0, y1) = ((0.0 - hy) * sy, (h - hy) * sy);
                        (
                            inst.x + x0.min(x1),
                            inst.y + y0.min(y1),
                            (x1 - x0).abs(),
                            (y1 - y0).abs(),
                        )
                    } else {
                        // Axis-aligned box around the rotated corners (counter-clockwise on screen).
                        let (s, c) = inst.angle.to_radians().sin_cos();
                        let corners = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].map(|(lx, ly)| {
                            let (ox, oy) = ((lx - hx) * sx, (ly - hy) * sy);
                            (ox * c + oy * s, -ox * s + oy * c)
                        });
                        let minx = corners.iter().map(|p| p.0).fold(f64::MAX, f64::min);
                        let maxx = corners.iter().map(|p| p.0).fold(f64::MIN, f64::max);
                        let miny = corners.iter().map(|p| p.1).fold(f64::MAX, f64::min);
                        let maxy = corners.iter().map(|p| p.1).fold(f64::MIN, f64::max);
                        (inst.x + minx, inst.y + miny, maxx - minx, maxy - miny)
                    }
                }
                None => (inst.x, inst.y, 1.0, 1.0),
            }
        }
        Visual::Shape { w, h, .. }
        | Visual::Placeholder { w, h }
        | Visual::Text { w, h, .. }
        | Visual::Effect { w, h, .. } => (inst.x, inst.y, *w as f64, *h as f64),
        Visual::Glyphs { .. } => (inst.x, inst.y, 1.0, 1.0),
    }
}
