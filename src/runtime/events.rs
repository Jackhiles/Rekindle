//! Event interpreter: evaluates conditions with Fusion's object selection rules and
//! runs actions on the selected instances.
//!
//! Selection: each event starts with nothing selected. A condition on an object type
//! filters that type's instances (all of them the first time); the event fires if every
//! condition holds, and its actions apply to the instances that survived the filters.
//! Object types not mentioned by any condition act on all their instances.

use super::audio;
use super::expr::Eval;
use super::input::Input;
use super::instance::{angle_to_dir, dir_from_mask, dir_to_angle};
use super::value::Value;
use super::{FrameChange, Runtime};
use crate::ctf_loader::events::{position_flags, Ace, Comparison, EventGroup, Param, Position};
use crate::ctf_loader::{ObjectKind, ObjectProperties};
use crate::render::scene::Visual;
use std::collections::{HashMap, HashSet};

/// Selected instance indices per object-info handle (or qualifier).
pub type Sel = HashMap<u16, Vec<usize>>;

/// Collision geometry: an axis-aligned bounding box, optionally refined by the
/// image's pixel mask and/or its rotation and scale.
struct CollisionShape {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    local: Option<LocalShape>,
}

struct LocalShape {
    /// Hotspot position in the frame.
    x: f64,
    y: f64,
    hot: (f64, f64),
    scale: (f64, f64),
    sin: f64,
    cos: f64,
    size: (f64, f64),
    mask: Option<std::rc::Rc<super::Mask>>,
}

impl CollisionShape {
    fn boxed(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self {
            x,
            y,
            w,
            h,
            local: None,
        }
    }

    /// Whether the frame point (px, py) is solid.
    fn contains(&self, px: f64, py: f64) -> bool {
        let Some(l) = &self.local else {
            return px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h;
        };
        // Undo the rotation (counter-clockwise on screen) and scale around the hotspot.
        let (dx, dy) = (px - l.x, py - l.y);
        let rx = dx * l.cos - dy * l.sin;
        let ry = dx * l.sin + dy * l.cos;
        let lx = rx / if l.scale.0 == 0.0 { 1.0 } else { l.scale.0 } + l.hot.0;
        let ly = ry / if l.scale.1 == 0.0 { 1.0 } else { l.scale.1 } + l.hot.1;
        if lx < 0.0 || ly < 0.0 || lx >= l.size.0 || ly >= l.size.1 {
            return false;
        }
        l.mask
            .as_ref()
            .is_none_or(|m| m.solid(lx as usize, ly as usize))
    }
}

/// Per-event runtime bookkeeping.
#[derive(Debug, Clone, Default)]
pub struct EventState {
    pub last_true_tick: Option<u64>,
    pub once_fired: bool,
    pub repeat_count: u32,
    pub restrict_until: f64,
    /// Next fire time for "every" conditions, keyed by condition index.
    pub every_next: HashMap<usize, f64>,
    /// Instance-id pairs that were colliding last time, keyed by condition index.
    pub collisions: HashMap<usize, HashSet<(u32, u32)>>,
}

const SYS: i16 = -1;
const SOUND: i16 = -2;
const GAME: i16 = -3;
const TIMER: i16 = -4;
const CREATE: i16 = -5;
const MOUSE: i16 = -6;
const PLAYER: i16 = -7;

fn cond_is(c: &Ace, t: i16, n: i16) -> bool {
    c.object_type == t && c.num == n
}

impl Runtime {
    // ---------- selection helpers ----------

    /// All live instances of an object-info handle; handles with bit 15 set are qualifiers.
    pub fn instances_of(&self, oi: u16) -> Vec<usize> {
        let members: Option<&Vec<u16>> = if oi & 0x8000 != 0 {
            self.qualifiers.get(&(oi & 0x7FFF))
        } else {
            None
        };
        let mut found: Vec<usize> = match members {
            Some(m) => m.iter().flat_map(|&o| self.indices_of(o)).collect(),
            None => self.indices_of(oi),
        };
        if members.is_some() {
            found.sort_unstable();
        }
        found
            .into_iter()
            .filter(|&k| !self.instances[k].destroyed && !self.instances[k].pasted)
            .collect()
    }

    pub fn candidates(&self, oi: u16, sel: &Sel) -> Vec<usize> {
        match sel.get(&oi) {
            Some(v) => v
                .iter()
                .copied()
                .filter(|&i| !self.instances[i].destroyed)
                .collect(),
            None => self.instances_of(oi),
        }
    }

    /// The instance an expression about `oi` refers to.
    pub fn pick(&self, oi: u16, sel: &Sel, cur: Option<(u16, usize)>) -> Option<usize> {
        if let Some((coi, idx)) = cur {
            if coi == oi || self.instances.get(idx).is_some_and(|i| i.oi == oi) {
                return Some(idx);
            }
        }
        self.candidates(oi, sel).first().copied()
    }

    /// Filter `oi`'s candidates by `pred` (inverted when `neg`), store the selection and
    /// report whether any instance remains.
    fn filter(
        &self,
        oi: u16,
        sel: &mut Sel,
        neg: bool,
        pred: impl Fn(&Self, &Sel, usize) -> bool,
    ) -> bool {
        let kept: Vec<usize> = self
            .candidates(oi, sel)
            .into_iter()
            .filter(|&i| pred(self, sel, i) != neg)
            .collect();
        let any = !kept.is_empty();
        sel.insert(oi, kept);
        any
    }

    pub fn bounds_of(&self, idx: usize) -> (f64, f64, f64, f64) {
        let inst = &self.instances[idx];
        match self.game.object(inst.oi) {
            Some(obj) => super::instance::bounds(&self.game, inst, obj),
            None => (inst.x, inst.y, 1.0, 1.0),
        }
    }

    /// How an instance occupies space for collision purposes.
    fn collision_shape(&self, idx: usize) -> CollisionShape {
        use crate::ctf_loader::properties::oc_new_flags;
        let inst = &self.instances[idx];
        let (bx, by, bw, bh) = self.bounds_of(idx);
        let mut shape = CollisionShape::boxed(bx, by, bw, bh);
        let Some(obj) = self.game.object(inst.oi) else {
            return shape;
        };
        let (handle, fine) = match (&obj.props, &inst.visual) {
            (ObjectProperties::Common(c), _) if inst.kind == ObjectKind::Active && !inst.pasted => {
                match inst.current_image(obj) {
                    Some(h) => (h, c.new_flags & oc_new_flags::COLLISION_BOX == 0),
                    None => return shape,
                }
            }
            // Backdrops: collision mode 0 = fine (pixel), 1 = box.
            (ObjectProperties::Backdrop(b), Visual::Image { handle }) => {
                (*handle, b.collision == 0)
            }
            (_, Visual::Image { handle }) if inst.pasted => (*handle, true),
            _ => return shape,
        };
        let Some(e) = self.game.images.get(handle) else {
            return shape;
        };
        let transformed = inst.scale_x != 1.0 || inst.scale_y != 1.0 || inst.angle != 0.0;
        if !fine && !transformed {
            return shape;
        }
        let (s, c) = inst.angle.to_radians().sin_cos();
        shape.local = Some(LocalShape {
            x: inst.x,
            y: inst.y,
            hot: if inst.background { (0.0, 0.0) } else { (e.hotspot.0 as f64, e.hotspot.1 as f64) },
            scale: (inst.scale_x, inst.scale_y),
            sin: s,
            cos: c,
            size: (e.width as f64, e.height as f64),
            mask: if fine { self.mask(handle) } else { None },
        });
        shape
    }

    pub(crate) fn overlaps_pub(&self, a: usize, b: usize) -> bool {
        self.overlaps(a, b)
    }

    fn overlaps(&self, a: usize, b: usize) -> bool {
        if a == b {
            return false;
        }
        // Cheap bounding-box rejection before building pixel shapes.
        let (ax, ay, aw, ah) = self.bounds_of(a);
        let (bx, by, bw, bh) = self.bounds_of(b);
        if !(ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah) {
            return false;
        }
        let (sa, sb) = (self.collision_shape(a), self.collision_shape(b));
        let (x0, y0) = (sa.x.max(sb.x), sa.y.max(sb.y));
        let (x1, y1) = (
            (sa.x + sa.w).min(sb.x + sb.w),
            (sa.y + sa.h).min(sb.y + sb.h),
        );
        if x0 >= x1 || y0 >= y1 {
            return false;
        }
        if sa.local.is_none() && sb.local.is_none() {
            return true;
        }
        // Sample pixel centres over the intersection of the two bounding boxes.
        for y in y0.floor() as i64..y1.ceil() as i64 {
            for x in x0.floor() as i64..x1.ceil() as i64 {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                if sa.contains(px, py) && sb.contains(px, py) {
                    return true;
                }
            }
        }
        false
    }

    /// Mouse hit test. `x, y` are frame coordinates at full scroll; objects on layers that
    /// scroll less (e.g. a fixed HUD layer) are tested in their own layer's coordinates.
    fn point_in(&self, idx: usize, x: f64, y: f64) -> bool {
        let (cx, cy) = self
            .frame()
            .layers
            .get(self.instances[idx].layer)
            .map_or((1.0, 1.0), |l| l.scroll_coefficients());
        let x = x - self.scroll.0 * (1.0 - cx as f64);
        let y = y - self.scroll.1 * (1.0 - cy as f64);
        let (bx, by, bw, bh) = self.bounds_of(idx);
        x >= bx && x < bx + bw && y >= by && y < by + bh
    }

    // ---------- parameter helpers ----------

    pub fn eval_param(&self, a: &Ace, i: usize, sel: &Sel, cur: Option<(u16, usize)>) -> Value {
        match a.params.get(i).map(|p| &p.value) {
            Some(Param::Expression { tokens, .. }) => Eval::new(self, sel, cur, tokens).run(),
            Some(Param::Short(v)) => Value::Int(*v as i32),
            Some(Param::Int(v)) => Value::Int(*v),
            Some(Param::String(s)) => Value::Str(s.clone()),
            Some(Param::Key(k)) => Value::Int(*k as i32),
            Some(Param::Time { ms, .. }) | Some(Param::Every { ms }) => Value::Int(*ms),
            _ => Value::Int(0),
        }
    }

    fn param_cmp(
        &self,
        a: &Ace,
        i: usize,
        sel: &Sel,
        cur: Option<(u16, usize)>,
    ) -> (Comparison, Value) {
        let cmp = match a.params.get(i).map(|p| &p.value) {
            Some(Param::Expression { comparison, .. }) => *comparison,
            Some(Param::CompareTime { comparison, .. }) => *comparison,
            _ => Comparison::Equal,
        };
        (cmp, self.eval_param(a, i, sel, cur))
    }

    fn param_i(&self, a: &Ace, i: usize, sel: &Sel, cur: Option<(u16, usize)>) -> i32 {
        self.eval_param(a, i, sel, cur).as_i32()
    }

    /// Index parameter for alterable/global values: shorts are 0-based.
    fn param_index(&self, a: &Ace, i: usize, sel: &Sel, cur: Option<(u16, usize)>) -> usize {
        self.param_i(a, i, sel, cur).max(0) as usize
    }

    fn param_colour(a: &Ace, i: usize) -> Option<[u8; 4]> {
        match a.params.get(i).map(|p| &p.value) {
            Some(Param::Colour(c)) => Some(*c),
            _ => None,
        }
    }

    /// A colour given as an expression (0xBBGGRR, as RGB() produces).
    fn colour_value(&self, a: &Ace, i: usize, sel: &Sel, cur: Option<(u16, usize)>) -> [u8; 4] {
        let v = self.param_i(a, i, sel, cur);
        [
            (v & 255) as u8,
            ((v >> 8) & 255) as u8,
            ((v >> 16) & 255) as u8,
            255,
        ]
    }

    fn param_sample(a: &Ace, i: usize) -> Option<u32> {
        match a.params.get(i).map(|p| &p.value) {
            Some(Param::Sample { handle, .. }) => Some(*handle as u32),
            _ => None,
        }
    }

    fn param_object(a: &Ace, i: usize) -> Option<u16> {
        match a.params.get(i).map(|p| &p.value) {
            Some(Param::Object { object_info, .. }) => Some(*object_info),
            _ => None,
        }
    }

    // ---------- event loop ----------

    /// Run the event sheet once. With `loop_name`, only "On loop" events for that loop run.
    pub(super) fn run_events(&mut self, input: &Input, loop_name: Option<&str>) {
        let events = self.events.clone();
        let mut stack: Vec<bool> = Vec::new();
        for (gi, group) in events.groups.iter().enumerate() {
            if let Some(first) = group.conditions.first() {
                if cond_is(first, SYS, -10) {
                    let enabled = match first.params.first().map(|p| &p.value) {
                        Some(Param::Group { id, .. }) => self.group_enabled(*id),
                        _ => true,
                    };
                    let parent = stack.last().copied().unwrap_or(true);
                    stack.push(parent && enabled);
                    continue;
                }
                if cond_is(first, SYS, -11) {
                    stack.pop();
                    continue;
                }
            }
            if !stack.last().copied().unwrap_or(true) {
                continue;
            }
            let end_of_frame = group.conditions.iter().any(|c| cond_is(c, GAME, -2));
            if end_of_frame != self.end_of_frame_pass {
                continue;
            }
            let on_loop = group.conditions.iter().find(|c| cond_is(c, SYS, -16));
            match (loop_name, on_loop) {
                (None, Some(_)) => continue,
                (Some(_), None) => continue,
                (Some(name), Some(c)) => {
                    let n = self.eval_param(c, 0, &Sel::new(), None).as_string();
                    if !n.eq_ignore_ascii_case(name) {
                        continue;
                    }
                }
                (None, None) => {}
            }
            let started = self.profile.as_ref().map(|_| std::time::Instant::now());
            self.run_group(gi, group, input);
            if let (Some(p), Some(t)) = (&self.profile, started) {
                let mut map = p.borrow_mut();
                let e = map.entry((self.frame_index, gi)).or_default();
                e.0 += t.elapsed();
                e.1 += 1;
            }
            if self.pending.is_some() {
                break;
            }
        }
    }

    fn run_group(&mut self, gi: usize, group: &EventGroup, input: &Input) {
        // Split on OR conditions; the first block that holds wins.
        let mut blocks: Vec<Vec<(usize, &Ace)>> = vec![Vec::new()];
        for (ci, c) in group.conditions.iter().enumerate() {
            if cond_is(c, SYS, -24) || cond_is(c, SYS, -25) {
                blocks.push(Vec::new());
            } else {
                blocks.last_mut().expect("non-empty").push((ci, c));
            }
        }
        let mut chosen: Option<Sel> = None;
        for block in &blocks {
            let mut sel = Sel::new();
            if block
                .iter()
                .all(|&(ci, c)| self.eval_condition(gi, ci, c, &mut sel, input))
            {
                chosen = Some(sel);
                break;
            }
        }
        let Some(mut sel) = chosen else { return };

        // Event-level gates that depend on history.
        let tick = self.tick;
        let timer = self.timer_ms;
        let st = self.event_state.entry(gi).or_default();
        let prev = st.last_true_tick.replace(tick);
        for c in &group.conditions {
            if c.object_type != SYS {
                continue;
            }
            match c.num {
                -7 if prev.is_some_and(|p| p + 1 >= tick) => return,
                -6 => {
                    if st.once_fired {
                        return;
                    }
                    st.once_fired = true;
                }
                // "Repeat N times": counted here, compared against N below.
                -5 => st.repeat_count += 1,
                // "Restrict actions for": skip while the restriction window is open.
                -4 if timer < st.restrict_until => return,
                _ => {}
            }
        }
        for c in &group.conditions {
            if cond_is(c, SYS, -5) {
                let n = self.param_i(c, 0, &sel, None).max(0) as u32;
                if self.event_state.get(&gi).map_or(0, |s| s.repeat_count) > n {
                    return;
                }
            }
            if cond_is(c, SYS, -4) {
                let ms = self.eval_param(c, 0, &sel, None).as_f64();
                self.event_state.entry(gi).or_default().restrict_until = timer + ms;
            }
        }

        for a in &group.actions {
            self.exec_action(a, &mut sel, input);
            if self.pending.is_some() {
                break;
            }
        }
    }

    // ---------- conditions ----------

    fn eval_condition(
        &mut self,
        gi: usize,
        ci: usize,
        c: &Ace,
        sel: &mut Sel,
        input: &Input,
    ) -> bool {
        let neg = c.negated();
        if c.object_type >= 0 {
            return self.object_condition(gi, ci, c, sel, input, neg);
        }
        let r = match (c.object_type, c.num) {
            (SYS, -1) => true,
            (SYS, -2) => false,
            (SYS, -3) => {
                let lhs = self.eval_param(c, 0, sel, None);
                let (cmp, rhs) = self.param_cmp(c, 1, sel, None);
                lhs.compare(cmp, &rhs)
            }
            // History-dependent gates are applied in run_group.
            (SYS, -4) | (SYS, -5) | (SYS, -6) | (SYS, -7) | (SYS, -9) | (SYS, -16) => true,
            (SYS, -8) | (SYS, -39..=-28) => {
                let idx = self.param_index(c, 0, sel, None);
                let (cmp, rhs) = self.param_cmp(c, 1, sel, None);
                self.global_value(idx).compare(cmp, &rhs)
            }
            (SYS, -12) => match c.params.first().map(|p| &p.value) {
                Some(Param::GroupPointer { id }) => self.group_enabled(*id as u16),
                _ => true,
            },
            (SYS, -20) => {
                let idx = self.param_index(c, 0, sel, None);
                let (cmp, rhs) = self.param_cmp(c, 1, sel, None);
                Value::Str(self.global_string(idx)).compare(cmp, &rhs)
            }
            (SYS, -23) => self.group_just_activated.contains(&(gi as u32)),
            (SYS, -26) => {
                let chance = self.param_i(c, 0, sel, None).max(0) as u32;
                let out_of = self.param_i(c, 1, sel, None).max(1) as u32;
                self.random(out_of) < chance
            }
            (GAME, -1) => self.tick == 0,
            (GAME, -2) => self.end_of_frame_pass,
            (GAME, -4) => false,
            (TIMER, -1) => self.timer_ms > self.eval_param(c, 0, sel, None).as_f64(),
            (TIMER, -2) => self.timer_ms < self.eval_param(c, 0, sel, None).as_f64(),
            (TIMER, -3) | (TIMER, -7) => {
                let t = self.eval_param(c, 0, sel, None).as_f64();
                self.prev_timer_ms < t && t <= self.timer_ms || (self.tick == 0 && t <= 0.0)
            }
            (TIMER, -4) | (TIMER, -8) => {
                let ms = self.eval_param(c, 0, sel, None).as_f64().max(1.0);
                let now = self.timer_ms;
                let st = self.event_state.entry(gi).or_default();
                let next = st.every_next.entry(ci).or_insert(ms);
                if now >= *next {
                    *next += ms;
                    if *next <= now {
                        *next = now + ms;
                    }
                    true
                } else {
                    false
                }
            }
            (MOUSE, -1) => {
                matches!(c.params.first().map(|p| &p.value), Some(Param::Key(k)) if input.pressed.contains(k))
            }
            (MOUSE, -2) | (MOUSE, -8) => {
                matches!(c.params.first().map(|p| &p.value), Some(Param::Key(k)) if input.down.contains(k))
            }
            (MOUSE, -9) => !input.pressed.is_empty(),
            (MOUSE, -5) | (MOUSE, -6) => {
                let (button, double) = match c.params.first().map(|p| &p.value) {
                    Some(Param::Click { button, double }) => (*button as usize, *double),
                    _ => (0, false),
                };
                let clicked = if double {
                    input.double_clicked
                } else {
                    input.clicked
                };
                let hit = clicked.get(button).copied().unwrap_or(false);
                hit && (c.num == -5 || self.mouse_in_zone(c, 1, input))
            }
            (MOUSE, -3) => self.mouse_in_zone(c, 0, input),
            // Like collisions, mouse tests also hit hidden objects (games use invisible hover zones).
            (MOUSE, -4) => {
                let Some(oi) = Self::param_object(c, 0) else {
                    return false;
                };
                let (mx, my) = input.mouse;
                return self.filter(oi, sel, neg, |rt, _, i| {
                    rt.point_in(i, mx, my)
                });
            }
            (MOUSE, -7) => {
                let (button, double) = match c.params.first().map(|p| &p.value) {
                    Some(Param::Click { button, double }) => (*button as usize, *double),
                    _ => (0, false),
                };
                let clicked = if double {
                    input.double_clicked
                } else {
                    input.clicked
                };
                if !clicked.get(button).copied().unwrap_or(false) {
                    return neg;
                }
                let Some(oi) = Self::param_object(c, 1) else {
                    return false;
                };
                let (mx, my) = input.mouse;
                return self.filter(oi, sel, neg, |rt, _, i| {
                    rt.point_in(i, mx, my)
                });
            }
            (MOUSE, -10) => self.cursor_visible,
            (CREATE, -3) | (CREATE, -15) => {
                let (cmp, rhs) = self.param_cmp(c, 0, sel, None);
                Value::Int(self.instances.iter().filter(|i| !i.destroyed).count() as i32)
                    .compare(cmp, &rhs)
            }
            (PLAYER, -4) | (PLAYER, -6) => {
                // Joystick-direction mask (up, down, left, right, fire1, fire2, ...) on player 1's keys.
                let mask = self.param_i(c, 0, sel, None) as u32;
                let keys = self.game.header.controls[0].keys;
                let set = if c.num == -4 {
                    &input.pressed
                } else {
                    &input.down
                };
                (0..8)
                    .filter(|b| mask & (1 << b) != 0)
                    .all(|b| set.contains(&keys[b]))
                    && mask != 0
            }
            (PLAYER, -2) => {
                let (cmp, rhs) = self.param_cmp(c, 0, sel, None);
                Value::Int(self.score[0]).compare(cmp, &rhs)
            }
            (PLAYER, -3) => {
                let (cmp, rhs) = self.param_cmp(c, 0, sel, None);
                Value::Int(self.lives[0]).compare(cmp, &rhs)
            }
            (SOUND, -1) => {
                Self::param_sample(c, 0).is_none_or(|h| !self.audio.is_sample_playing(h))
            }
            (SOUND, -3) => !self.audio.any_playing(),
            (SOUND, -6) => Self::param_sample(c, 0).is_some_and(|h| self.audio.is_sample_paused(h)),
            (SOUND, -8) => !self
                .audio
                .is_channel_playing(self.param_i(c, 0, sel, None).max(0) as usize),
            (SOUND, -9) => self
                .audio
                .is_channel_paused(self.param_i(c, 0, sel, None).max(0) as usize),
            // Music bank (MIDI etc.) is not played: nothing is ever playing.
            (SOUND, -2) | (SOUND, -4) => true,
            (SOUND, -5) | (SOUND, -7) => false,
            (t, n) => {
                self.note_unimplemented(format!("cond {}/{}", self.ace_owner_label(t), n), ());
                false
            }
        };
        r != neg
    }

    fn mouse_in_zone(&self, c: &Ace, i: usize, input: &Input) -> bool {
        match c.params.get(i).map(|p| &p.value) {
            Some(Param::Zone { x1, y1, x2, y2 }) => {
                let (mx, my) = input.mouse;
                mx >= *x1 as f64 && mx < *x2 as f64 && my >= *y1 as f64 && my < *y2 as f64
            }
            _ => false,
        }
    }

    /// Evaluate all parameters of an extension condition/action, plus its comparison operator.
    fn ext_args(&self, a: &Ace, sel: &Sel, cur: Option<(u16, usize)>) -> (Vec<Value>, Comparison) {
        let args = (0..a.params.len())
            .map(|i| self.eval_param(a, i, sel, cur))
            .collect();
        let cmp = a
            .params
            .iter()
            .find_map(|p| match &p.value {
                Param::Expression { comparison, .. } if matches!(p.code, 23 | 46) => {
                    Some(*comparison)
                }
                _ => None,
            })
            .unwrap_or(Comparison::Equal);
        (args, cmp)
    }

    fn object_condition(
        &mut self,
        gi: usize,
        ci: usize,
        c: &Ace,
        sel: &mut Sel,
        input: &Input,
        neg: bool,
    ) -> bool {
        let oi = c.object_info;
        if c.object_type >= 32 && c.num <= -81 {
            let idx = (-(c.num as i32) - 81) as usize;
            let (args, cmp) = self.ext_args(c, sel, None);
            let r = self
                .exts
                .get(&c.object_type)
                .and_then(|e| e.condition(idx, &args, cmp, &self.files));
            let Some(r) = r else {
                self.note_unimplemented(
                    format!("cond {}/{}", self.ace_owner_label(c.object_type), c.num),
                    (),
                );
                return false;
            };
            return self.filter(oi, sel, neg, |_, _, _| r);
        }
        let cur = |i: usize| Some((oi, i));
        match c.num {
            -1 => self.filter(oi, sel, neg, |rt, s, i| {
                let (cmp, v) = rt.param_cmp(c, 0, s, cur(i));
                let inst = &rt.instances[i];
                Value::Int(inst.anim.forced_frame.unwrap_or(inst.anim.frame) as i32)
                    .compare(cmp, &v)
            }),
            -2 => {
                let a = self.param_i(c, 0, sel, None).max(0) as usize;
                self.filter(oi, sel, neg, |rt, _, i| {
                    rt.instances[i].anim.finished_anim == Some(a)
                })
            }
            -3 => {
                let a = self.param_i(c, 0, sel, None).max(0) as usize;
                self.filter(oi, sel, neg, |rt, _, i| {
                    rt.instances[i].current_anim_id() == a
                })
            }
            -4 | -14 => {
                let Some(other) = Self::param_object(c, 0) else {
                    return false;
                };
                let others = self.candidates(other, sel);
                let edge = c.num == -14;
                let mut pairs = HashSet::new();
                for i in self.candidates(oi, sel) {
                    for &j in &others {
                        if self.overlaps(i, j) {
                            pairs.insert((self.instances[i].id, self.instances[j].id));
                        }
                    }
                }
                let prev = if edge {
                    let st = self.event_state.entry(gi).or_default();
                    st.collisions.insert(ci, pairs.clone()).unwrap_or_default()
                } else {
                    HashSet::new()
                };
                let fresh: HashSet<(u32, u32)> = pairs.difference(&prev).copied().collect();
                let hit_a: HashSet<u32> = fresh.iter().map(|p| p.0).collect();
                let hit_b: HashSet<u32> = fresh.iter().map(|p| p.1).collect();
                let r = self.filter(oi, sel, neg, |rt, _, i| hit_a.contains(&rt.instances[i].id));
                if !neg {
                    let kept: Vec<usize> = others
                        .into_iter()
                        .filter(|&j| hit_b.contains(&self.instances[j].id))
                        .collect();
                    sel.insert(other, kept);
                }
                r
            }
            -7 => self.filter(oi, sel, neg, |rt, _, i| {
                rt.instances[i].current_speed() == 0.0
            }),
            -8 => {
                let mask = self.param_i(c, 0, sel, None) as u32;
                self.filter(oi, sel, neg, |rt, _, i| {
                    mask & (1 << rt.instances[i].direction()) != 0
                })
            }
            -9 | -10 => {
                let (fw, fh) = (self.frame().width as f64, self.frame().height as f64);
                let inside = c.num == -9;
                self.filter(oi, sel, neg, |rt, _, i| {
                    let (x, y, w, h) = rt.bounds_of(i);
                    let within = x + w > 0.0 && y + h > 0.0 && x < fw && y < fh;
                    within == inside
                })
            }
            -11 | -12 => {
                // Entering / leaving the playfield on the borders in a mask (1 left, 2 right, 4 top, 8 bottom).
                let mask = match c.params.first().map(|p| &p.value) {
                    Some(Param::Short(m)) => *m as u16,
                    _ => 15,
                };
                let (fw, fh) = (self.frame().width as f64, self.frame().height as f64);
                self.filter(oi, sel, neg, |rt, _, i| {
                    let (x, y, w, h) = rt.bounds_of(i);
                    (mask & 1 != 0 && x < 0.0)
                        || (mask & 2 != 0 && x + w > fw)
                        || (mask & 4 != 0 && y < 0.0)
                        || (mask & 8 != 0 && y + h > fh)
                })
            }
            -13 | -23 => self.filter(oi, sel, neg, |rt, _, i| rt.obstacle_hit(i, true).is_some()),
            -15 => self.filter(oi, sel, neg, |rt, s, i| {
                let (cmp, v) = rt.param_cmp(c, 0, s, cur(i));
                Value::Int(rt.instances[i].current_speed() as i32).compare(cmp, &v)
            }),
            -16 | -17 => self.filter(oi, sel, neg, |rt, s, i| {
                let (cmp, v) = rt.param_cmp(c, 0, s, cur(i));
                let inst = &rt.instances[i];
                let p = if c.num == -16 { inst.y } else { inst.x };
                Value::Int(p as i32).compare(cmp, &v)
            }),
            -20 => self.filter(oi, sel, neg, |_, _, _| false),
            -22 => {
                let d = self.eval_param(c, 0, sel, None).as_f64();
                let (sx, sy) = self.scroll;
                let (vw, vh) = self.view_size;
                self.filter(oi, sel, neg, |rt, _, i| {
                    let (x, y, w, h) = rt.bounds_of(i);
                    x < sx + d || y < sy + d || x + w > sx + vw - d || y + h > sy + vh - d
                })
            }
            -21 => self.filter(oi, sel, neg, |rt, _, i| {
                rt.instances[i].movement.path_finished
            }),
            -24 | -25 => self.filter(oi, sel, neg, |rt, s, i| {
                let bit = rt.param_i(c, 0, s, cur(i)) & 31;
                let on = rt.instances[i].flags & (1 << bit) != 0;
                on == (c.num == -25)
            }),
            -26 => self.filter(oi, sel, neg, |rt, s, i| {
                let (cmp, v) = rt.param_cmp(c, 0, s, cur(i));
                Value::Int(rt.instances[i].id as i32).compare(cmp, &v)
            }),
            -27 | -42 | -43 => self.filter(oi, sel, neg, |rt, s, i| {
                let idx = rt.param_index(c, 0, s, cur(i));
                let (cmp, v) = rt.param_cmp(c, 1, s, cur(i));
                rt.instances[i].value(idx).compare(cmp, &v)
            }),
            -28 => self.filter(oi, sel, neg, |rt, _, i| !rt.instances[i].visible),
            -29 => self.filter(oi, sel, neg, |rt, _, i| rt.instances[i].visible),
            -30 | -31 => {
                let Some(Param::Zone { x1, y1, x2, y2 }) = c.params.first().map(|p| &p.value)
                else {
                    return false;
                };
                let (x1, y1, x2, y2) = (*x1 as f64, *y1 as f64, *x2 as f64, *y2 as f64);
                let n = self
                    .candidates(oi, sel)
                    .into_iter()
                    .filter(|&i| {
                        let p = &self.instances[i];
                        p.x >= x1 && p.x < x2 && p.y >= y1 && p.y < y2
                    })
                    .count();
                let r = if c.num == -31 {
                    n == 0
                } else {
                    let (cmp, v) = self.param_cmp(c, 1, sel, None);
                    Value::Int(n as i32).compare(cmp, &v)
                };
                r != neg
            }
            -32 => {
                let n = self.candidates(oi, sel).len();
                let (cmp, v) = self.param_cmp(c, 0, sel, None);
                Value::Int(n as i32).compare(cmp, &v) != neg
            }
            -33 => self.instances_of(oi).is_empty() != neg,
            -34 => {
                let all = self.candidates(oi, sel);
                if all.is_empty() {
                    return neg;
                }
                let pick = all[self.random(all.len() as u32) as usize];
                sel.insert(oi, vec![pick]);
                !neg
            }
            -36 => self.filter(oi, sel, neg, |rt, s, i| {
                let idx = rt.param_index(c, 0, s, cur(i));
                let (cmp, v) = rt.param_cmp(c, 1, s, cur(i));
                Value::Str(rt.instances[i].string(idx)).compare(cmp, &v)
            }),
            -81 if c.object_type == 7 => self.filter(oi, sel, neg, |rt, s, i| {
                let (cmp, v) = rt.param_cmp(c, 0, s, cur(i));
                rt.instances[i].counter.compare(cmp, &v)
            }),
            -81 if c.object_type == 2 => {
                // "Object clicked" on an Active.
                let button = match c.params.first().map(|p| &p.value) {
                    Some(Param::Click { button, .. }) => *button as usize,
                    _ => 0,
                };
                if !input.clicked.get(button).copied().unwrap_or(false) {
                    return neg;
                }
                let (mx, my) = input.mouse;
                self.filter(oi, sel, neg, |rt, _, i| {
                    rt.instances[i].visible && rt.point_in(i, mx, my)
                })
            }
            n => {
                self.note_unimplemented(
                    format!("cond {}/{}", self.ace_owner_label(c.object_type), n),
                    (),
                );
                false
            }
        }
    }

    // ---------- actions ----------

    fn exec_action(&mut self, a: &Ace, sel: &mut Sel, input: &Input) {
        if a.object_type >= 0 {
            let targets = self.candidates(a.object_info, sel);
            for idx in targets {
                if !self.instances[idx].destroyed {
                    self.object_action(a, idx, sel);
                }
            }
            return;
        }
        let _ = input;
        match (a.object_type, a.num) {
            (SYS, 0) => {}
            (SYS, 3) | (SYS, 27..=30) => {
                let i = self.param_index(a, 0, sel, None);
                let v = self.eval_param(a, 1, sel, None);
                self.set_global_value(i, v);
            }
            (SYS, 4) | (SYS, 35..=38) | (SYS, 5) | (SYS, 31..=34) => {
                let i = self.param_index(a, 0, sel, None);
                let v = self.eval_param(a, 1, sel, None);
                let old = self.global_value(i);
                let sub = a.num == 4 || (35..=38).contains(&a.num);
                self.set_global_value(i, if sub { old.arith(4, &v) } else { old.add(&v) });
            }
            (SYS, 6) | (SYS, 7) => {
                if let Some(Param::GroupPointer { id }) = a.params.first().map(|p| &p.value) {
                    self.set_group_enabled(*id as u16, a.num == 6);
                }
            }
            (SYS, 14) => {
                let name = self.eval_param(a, 0, sel, None).as_string();
                let times = self.eval_param(a, 1, sel, None).as_i32();
                self.run_fast_loop(&name, times, input);
            }
            (SYS, 15) => {
                let name = self.eval_param(a, 0, sel, None).as_string().to_lowercase();
                if let Some(l) = self.loops.get_mut(&name) {
                    l.stop = true;
                }
            }
            (SYS, 16) => {
                let name = self.eval_param(a, 0, sel, None).as_string().to_lowercase();
                let v = self.eval_param(a, 1, sel, None).as_i32();
                self.loops.entry(name).or_default().index = v;
            }
            (SYS, 17) => {
                let seed = self.eval_param(a, 0, sel, None).as_i32() as u64;
                self.rng.set(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            }
            (SYS, 19) => {
                let i = self.param_index(a, 0, sel, None);
                let v = self.eval_param(a, 1, sel, None).as_string();
                self.set_global_string(i, v);
            }
            (SOUND, n) => self.sound_action(a, n, sel),
            (GAME, 0) => self.pending = Some(FrameChange::Jump(self.frame_index + 1)),
            (GAME, 1) => self.pending = Some(FrameChange::Jump(self.frame_index.saturating_sub(1))),
            (GAME, 2) => {
                let target = match a.params.first().map(|p| &p.value) {
                    Some(Param::Short(h)) => self
                        .game
                        .frame_handles
                        .get(*h as usize)
                        .map(|&f| f as usize)
                        .unwrap_or(*h as usize),
                    _ => (self.eval_param(a, 0, sel, None).as_i32() - 1).max(0) as usize,
                };
                self.pending = Some(FrameChange::Jump(target));
            }
            (GAME, 4) => self.pending = Some(FrameChange::End),
            (GAME, 5) => self.pending = Some(FrameChange::Jump(0)),
            (GAME, 6) => self.pending = Some(FrameChange::Restart),
            (GAME, 7) => {
                if let Some(Param::Position(p)) = a.params.first().map(|p| &p.value) {
                    if let Some((x, y, _, _)) = self.resolve_position(p, sel, None) {
                        self.center_display(Some(x), Some(y));
                    }
                }
            }
            (GAME, 8) => {
                let x = self.eval_param(a, 0, sel, None).as_f64();
                self.center_display(Some(x), None);
            }
            (GAME, 9) => {
                let y = self.eval_param(a, 0, sel, None).as_f64();
                self.center_display(None, Some(y));
            }
            (GAME, 16) => {
                self.frame_rate = self.eval_param(a, 0, sel, None).as_i32().clamp(1, 1000) as u32
            }
            (GAME, 23) => {
                if let Some(Param::Colour(c)) = a.params.first().map(|p| &p.value) {
                    self.background_color = *c;
                }
            }
            (TIMER, 0) => self.timer_ms = self.eval_param(a, 0, sel, None).as_f64(),
            (CREATE, 0) => {
                if let Some(Param::Create {
                    position,
                    object_info,
                    ..
                }) = a.params.first().map(|p| &p.value)
                {
                    if let Some((x, y, layer, dir)) = self.resolve_position(position, sel, None) {
                        if let Some(idx) = self.create_instance(*object_info, x, y, layer) {
                            if let Some(d) = dir {
                                self.instances[idx].anim.dir = d;
                            }
                            sel.insert(*object_info, vec![idx]);
                        }
                    }
                }
            }
            (MOUSE, 0) => self.cursor_visible = false,
            (MOUSE, 1) => self.cursor_visible = true,
            (PLAYER, n @ (0 | 4 | 6)) => {
                let v = self.eval_param(a, 0, sel, None).as_i32();
                self.score[0] = match n {
                    0 => v,
                    4 => self.score[0] + v,
                    _ => self.score[0] - v,
                };
            }
            (PLAYER, n @ (1 | 5 | 7)) => {
                let v = self.eval_param(a, 0, sel, None).as_i32();
                self.lives[0] = match n {
                    1 => v,
                    5 => self.lives[0] + v,
                    _ => self.lives[0] - v,
                };
            }
            (PLAYER, 2) | (PLAYER, 3) => {}
            (t, n) => self.note_unimplemented(format!("act {}/{}", self.ace_owner_label(t), n), ()),
        }
    }

    fn sound_action(&mut self, a: &Ace, n: i16, sel: &Sel) {
        let sample = Self::param_sample(a, 0);
        let int = |rt: &Self, i: usize| rt.param_i(a, i, sel, None);
        match n {
            0 => {
                if let Some(h) = sample {
                    self.play_sample(h, None, 1);
                }
            }
            4 => {
                if let Some(h) = sample {
                    let loops = int(self, 1).max(0) as u32;
                    self.play_sample(h, None, loops);
                }
            }
            11 | 12 => {
                if let Some(h) = sample {
                    let ch = int(self, 1).clamp(1, audio::CHANNELS as i32) as usize;
                    let loops = if n == 12 {
                        int(self, 2).max(0) as u32
                    } else {
                        1
                    };
                    self.play_sample(h, Some(ch), loops);
                }
            }
            1 => self.audio.stop_all(),
            6 => {
                if let Some(h) = sample {
                    self.audio.stop_sample(h);
                }
            }
            7 | 8 => {
                if let Some(h) = sample {
                    self.audio.set_paused(|s, _| s == h, n == 7);
                }
            }
            13 | 14 => {
                let ch = int(self, 0).max(0) as usize;
                self.audio.set_paused(|_, c| c == Some(ch), n == 13);
            }
            15 => {
                let ch = int(self, 0).max(0) as usize;
                self.audio.stop_channel(ch);
            }
            17 => {
                let ch = int(self, 0).clamp(0, audio::CHANNELS as i32) as usize;
                self.audio.channel_volume[ch] =
                    self.eval_param(a, 1, sel, None).as_f64().clamp(0.0, 100.0);
                self.audio.refresh_volumes();
            }
            20 => {
                self.audio.main_volume =
                    self.eval_param(a, 0, sel, None).as_f64().clamp(0.0, 100.0);
                self.audio.refresh_volumes();
            }
            21 => {
                if let Some(h) = sample {
                    let v = self.eval_param(a, 1, sel, None).as_f64().clamp(0.0, 100.0);
                    self.audio.sample_volume.insert(h, v);
                    self.audio.refresh_volumes();
                }
            }
            24 | 25 => self.audio.set_paused(|_, _| true, n == 24),
            // Music bank (MIDI) and playback frequency are not supported.
            2 | 3 | 5 | 9 | 10 | 32 | 33 => {}
            16 => {
                let ch = int(self, 0).max(0) as usize;
                let ms = self.eval_param(a, 1, sel, None).as_f64();
                self.audio.set_position(|_, c| c == Some(ch), ms);
            }
            19 => {
                if let Some(h) = sample {
                    let ms = self.eval_param(a, 1, sel, None).as_f64();
                    self.audio.set_position(|s, _| s == h, ms);
                }
            }
            18 => {
                let ch = int(self, 0).clamp(0, audio::CHANNELS as i32) as usize;
                self.audio.channel_pan[ch] = self.eval_param(a, 1, sel, None).as_f64().clamp(-100.0, 100.0);
                self.audio.refresh_pans(|_, c| c == Some(ch));
            }
            22 => {
                self.audio.main_pan = self.eval_param(a, 0, sel, None).as_f64().clamp(-100.0, 100.0);
                self.audio.refresh_pans(|_, _| true);
            }
            23 => {
                if let Some(h) = sample {
                    let v = self.eval_param(a, 1, sel, None).as_f64().clamp(-100.0, 100.0);
                    self.audio.sample_pan.insert(h, v);
                    self.audio.refresh_pans(|s, _| s == h);
                }
            }
            n => self.note_unimplemented(format!("act Sound/{n}"), ()),
        }
    }

    fn run_fast_loop(&mut self, name: &str, times: i32, input: &Input) {
        let key = name.to_lowercase();
        let saved = self.loops.remove(&key);
        self.loops.insert(key.clone(), Default::default());
        let mut i = 0;
        while times < 0 || i < times {
            if let Some(l) = self.loops.get_mut(&key) {
                l.index = i;
            }
            self.run_events(input, Some(name));
            let l = self.loops.get(&key).cloned().unwrap_or_default();
            if l.stop || self.pending.is_some() || i > 1_000_000 {
                break;
            }
            i = l.index + 1;
        }
        match saved {
            Some(s) => {
                self.loops.insert(key, s);
            }
            None => {
                self.loops.remove(&key);
            }
        }
    }

    fn object_action(&mut self, a: &Ace, idx: usize, sel: &mut Sel) {
        let oi = self.instances[idx].oi;
        let cur = Some((a.object_info, idx));
        let kind = self.instances[idx].kind;
        if a.num >= 80 && matches!(self.instances[idx].visual, Visual::Effect { .. }) {
            // Perspective object. 80 = set zoom (verified). The parameterless pairs are
            // inferred from use: 81 / 96 pick the sine-wave / sine-offset effect,
            // 87 / 88 pick horizontal / vertical.
            let zoom = (a.num == 80).then(|| self.eval_param(a, 0, sel, cur).as_f64() as f32);
            let mut handled = true;
            if let Visual::Effect { params, .. } = &mut self.instances[idx].visual {
                match a.num {
                    80 => params.zoom = zoom.unwrap_or(params.zoom),
                    81 => params.effect = 2,
                    96 => params.effect = 3,
                    87 => params.vertical = false,
                    88 => params.vertical = true,
                    _ => handled = false,
                }
            }
            if !handled {
                self.note_unimplemented(format!("act {}/{}", self.ace_owner_label(a.object_type), a.num), ());
            }
            return;
        }
        if let ObjectKind::Extension(t) = kind {
            let ext_name = self.ace_owner_label(t);
            if ext_name.eq_ignore_ascii_case("ext:kccursor") && a.num == 104 {
                // Cursor object: use an Active object's image as the mouse pointer.
                let name = self.eval_param(a, 0, sel, cur).as_string();
                self.cursor_object = if name.is_empty() { None } else { Some(name) };
                return;
            }
        }
        if let ObjectKind::Extension(t) = kind {
            if a.num >= 80 {
                let (args, _) = self.ext_args(a, sel, cur);
                let handled = match self.exts.get_mut(&t) {
                    Some(e) => e.action((a.num - 80) as usize, &args, &self.files),
                    None => false,
                };
                if !handled {
                    self.note_unimplemented(
                        format!("act {}/{}", self.ace_owner_label(a.object_type), a.num),
                        (),
                    );
                }
                return;
            }
        }
        match (kind, a.num) {
            (_, 1) => {
                if let Some(Param::Position(p)) = a.params.first().map(|p| &p.value) {
                    if let Some((x, y, _, _)) = self.resolve_position(p, sel, cur) {
                        let inst = &mut self.instances[idx];
                        inst.x = x;
                        inst.y = y;
                    }
                }
            }
            (_, 2) => self.instances[idx].x = self.eval_param(a, 0, sel, cur).as_f64(),
            (_, 3) => self.instances[idx].y = self.eval_param(a, 0, sel, cur).as_f64(),
            (_, 4) => {
                // "Stop" in a collision event moves the object back out of the obstacle
                // (only relevant if it moved this tick).
                let inst = &self.instances[idx];
                let moved = (inst.x, inst.y) != inst.movement.prev;
                if moved && self.obstacle_hit(idx, true).is_some() {
                    let (px, py) = self.instances[idx].movement.prev;
                    let inst = &mut self.instances[idx];
                    inst.x = px;
                    inst.y = py;
                    inst.movement.vx = 0.0;
                }
                let m = &mut self.instances[idx].movement;
                m.stopped = true;
                m.speed = 0.0;
            }
            (_, 5) => self.instances[idx].movement.stopped = false,
            (_, 6) => self.instances[idx].movement.speed = self.eval_param(a, 0, sel, cur).as_f64(),
            (_, 9) | (_, 10) => {
                let inst = &mut self.instances[idx];
                inst.anim.dir = (inst.direction() + 16) % 32;
                inst.anim.forced_dir = None;
                inst.movement.reversed = !inst.movement.reversed;
            }
            (_, 14) => {
                if let Some(Param::Position(p)) = a.params.first().map(|p| &p.value) {
                    if let Some((x, y, _, _)) = self.resolve_position(p, sel, cur) {
                        let inst = &mut self.instances[idx];
                        inst.anim.dir = angle_to_dir(x - inst.x, y - inst.y);
                    }
                }
            }
            (_, 15) => self.instances[idx].anim.stopped = true,
            (_, 16) => self.instances[idx].anim.stopped = false,
            (_, 17) => {
                let n = self.param_i(a, 0, sel, cur).max(0) as usize;
                self.instances[idx].force_anim(n);
            }
            (_, 18) => {
                let d = self.direction_param(a, 0, sel, cur);
                self.instances[idx].anim.forced_dir = Some(d);
            }
            (_, 19) => {
                self.instances[idx].movement.forced_speed =
                    Some(self.eval_param(a, 0, sel, cur).as_f64())
            }
            (_, 20) => {
                let inst = &mut self.instances[idx];
                inst.anim.forced_anim = None;
                inst.restart_anim();
            }
            (_, 21) => self.instances[idx].anim.forced_dir = None,
            (_, 22) => self.instances[idx].movement.forced_speed = None,
            (_, 23) => {
                let d = self.direction_param(a, 0, sel, cur);
                self.instances[idx].anim.dir = d;
            }
            (_, 24) => self.instances[idx].destroyed = true,
            (_, 26) => self.instances[idx].visible = false,
            (_, 27) => self.instances[idx].visible = true,
            (_, 28) => {}
            (_, 29) | (_, 30) => {
                if let Some(Param::Shoot {
                    position,
                    object_info,
                    speed,
                    ..
                }) = a.params.first().map(|p| &p.value)
                {
                    if let Some((x, y, layer, dir)) = self.resolve_position(position, sel, cur) {
                        let mut d = dir.unwrap_or(self.instances[idx].direction());
                        if a.num == 30 {
                            if let Some(Param::Position(t)) = a.params.get(1).map(|p| &p.value) {
                                if let Some((tx, ty, _, _)) = self.resolve_position(t, sel, cur) {
                                    d = angle_to_dir(tx - x, ty - y);
                                }
                            }
                        }
                        if let Some(n) = self.create_instance(*object_info, x, y, layer) {
                            let b = &mut self.instances[n];
                            b.anim.dir = d;
                            b.movement.speed = *speed as f64;
                            b.movement.stopped = false;
                            b.movement.index = usize::MAX; // shot objects use a straight bullet path
                            self.bullets.insert(b.id);
                            sel.insert(*object_info, vec![n]);
                        }
                    }
                }
            }
            (_, 31) | (_, 32) | (_, 33) => {
                let i = self.param_index(a, 0, sel, cur);
                let v = self.eval_param(a, 1, sel, cur);
                let inst = &mut self.instances[idx];
                let nv = match a.num {
                    31 => v,
                    32 => inst.value(i).add(&v),
                    _ => inst.value(i).arith(4, &v),
                };
                inst.set_value(i, nv);
            }
            (_, 35) | (_, 36) | (_, 37) => {
                let bit = self.param_i(a, 0, sel, cur) & 31;
                let f = &mut self.instances[idx].flags;
                match a.num {
                    35 => *f |= 1 << bit,
                    36 => *f &= !(1 << bit),
                    _ => *f ^= 1 << bit,
                }
            }
            (_, 38) => {
                // Set ink effect: (effect, parameter).
                if let Some(Param::TwoShorts(effect, param)) = a.params.first().map(|p| &p.value) {
                    let inst = &mut self.instances[idx];
                    inst.ink_effect = *effect as u16;
                    if *effect == 1 {
                        let t = (*param).clamp(0, 128) as u32;
                        inst.tint[3] = (255 * (128 - t) / 128) as u8;
                    }
                }
            }
            (_, 39) => {
                let t = self.eval_param(a, 0, sel, cur).as_f64().clamp(0.0, 128.0);
                self.instances[idx].tint[3] = (255.0 * (128.0 - t) / 128.0) as u8;
            }
            (_, 40) => {
                let f = self.param_i(a, 0, sel, cur).max(0) as usize;
                self.instances[idx].anim.forced_frame = Some(f);
            }
            (_, 41) => self.instances[idx].anim.forced_frame = None,
            (_, 49) => {
                let i = self.param_index(a, 0, sel, cur);
                let v = self.eval_param(a, 1, sel, cur).as_string();
                self.instances[idx].set_string(i, v);
            }
            (_, 57) => {
                self.next_z += 1;
                self.instances[idx].z = self.next_z;
            }
            (_, 58) => {
                self.min_z -= 1;
                self.instances[idx].z = self.min_z;
            }
            (_, 61) => {
                let l = (self.param_i(a, 0, sel, cur) - 1).max(0) as usize;
                let max = self.frame().layers.len().saturating_sub(1);
                self.instances[idx].layer = l.min(max);
            }
            (_, 63) | (_, 64) => {} // shaders: not supported
            (_, 11) | (_, 12) | (_, 13) => {
                let count =
                    self.game
                        .object(self.instances[idx].oi)
                        .map_or(0, |o| match &o.props {
                            crate::ctf_loader::ObjectProperties::Common(c) => c.movements.len(),
                            _ => 0,
                        });
                if count > 0 {
                    let m = &mut self.instances[idx].movement;
                    let target = match a.num {
                        11 => (m.index + 1).min(count - 1),
                        12 => m.index.saturating_sub(1),
                        _ => (self.param_i(a, 0, sel, cur).max(0) as usize).min(count - 1),
                    };
                    let m = &mut self.instances[idx].movement;
                    if target != m.index {
                        m.index = target;
                        m.step = 0;
                        m.step_progress = 0.0;
                        m.path_finished = false;
                    }
                }
            }
            (_, 51) => {
                self.instances[idx].font_size =
                    Some(self.eval_param(a, 0, sel, cur).as_f64().abs().max(1.0) as f32)
            }
            (_, 56) => {
                if let Some(c) =
                    Self::param_colour(a, 0).or_else(|| Some(self.colour_value(a, 0, sel, cur)))
                {
                    self.instances[idx].text_color = Some(c);
                }
            }
            (_, 65) => {
                let v = self.eval_param(a, 0, sel, cur).as_f64().clamp(0.0, 255.0);
                self.instances[idx].tint[3] = 255 - v as u8;
            }
            (_, 66) => {
                let v = self.eval_param(a, 0, sel, cur).as_i32();
                let t = &mut self.instances[idx].tint;
                t[0] = (v & 255) as u8;
                t[1] = ((v >> 8) & 255) as u8;
                t[2] = ((v >> 16) & 255) as u8;
            }
            (ObjectKind::Active, 85) => {
                let v = self.eval_param(a, 0, sel, cur).as_f64();
                let inst = &mut self.instances[idx];
                inst.scale_x = v;
                inst.scale_y = v;
            }
            (ObjectKind::Active, 86) => {
                self.instances[idx].scale_x = self.eval_param(a, 0, sel, cur).as_f64()
            }
            (ObjectKind::Active, 87) => {
                self.instances[idx].scale_y = self.eval_param(a, 0, sel, cur).as_f64()
            }
            (ObjectKind::Active, 88) => {
                self.instances[idx].angle =
                    self.eval_param(a, 0, sel, cur).as_f64().rem_euclid(360.0)
            }
            (ObjectKind::Active, 81) => {
                self.next_z += 1;
                self.instances[idx].z = self.next_z;
            }
            (ObjectKind::Active, 82) => {
                self.min_z -= 1;
                self.instances[idx].z = self.min_z;
            }
            (ObjectKind::Active, 80) | (ObjectKind::Active, 83) => {
                let obstacle = self.param_i(a, 0, sel, cur).max(0) as u16;
                self.paste_into_background(idx, obstacle);
            }
            (ObjectKind::Counter, 85) | (ObjectKind::Counter, 86) | (ObjectKind::Text, 83) => {
                let c =
                    Self::param_colour(a, 0).unwrap_or_else(|| self.colour_value(a, 0, sel, cur));
                self.instances[idx].text_color = Some(c);
            }
            (ObjectKind::Counter, n @ 80..=84) => {
                let v = self.eval_param(a, 0, sel, cur);
                let inst = &mut self.instances[idx];
                match n {
                    80 => inst.set_counter(v),
                    81 => inst.set_counter(inst.counter.add(&v)),
                    82 => inst.set_counter(inst.counter.arith(4, &v)),
                    83 => {
                        inst.counter_min = v.as_f64();
                        inst.set_counter(inst.counter.clone());
                    }
                    _ => {
                        inst.counter_max = v.as_f64();
                        inst.set_counter(inst.counter.clone());
                    }
                }
            }
            (ObjectKind::Text, 80) => self.instances[idx].text.clear(),
            (ObjectKind::Text, 88) => {
                self.instances[idx].text = self.eval_param(a, 0, sel, cur).as_string()
            }
            (ObjectKind::Text, 87) => {
                let i = self.param_index(a, 0, sel, cur);
                self.instances[idx].text = self.instances[idx].string(i);
            }
            (_, n) => {
                let _ = oi;
                self.note_unimplemented(
                    format!("act {}/{}", self.ace_owner_label(a.object_type), n),
                    (),
                );
            }
        }
    }

    /// Direction parameter: a 32-bit mask (code 29) or an expression giving 0..31.
    fn direction_param(&self, a: &Ace, i: usize, sel: &Sel, cur: Option<(u16, usize)>) -> usize {
        match a.params.get(i).map(|p| &p.value) {
            Some(Param::Int(mask)) => dir_from_mask(*mask as u32, self.random(32)),
            _ => self.eval_param(a, i, sel, cur).as_i32().rem_euclid(32) as usize,
        }
    }

    /// Resolve a position parameter to (x, y, layer, direction).
    pub(super) fn resolve_position(
        &self,
        p: &Position,
        sel: &Sel,
        cur: Option<(u16, usize)>,
    ) -> Option<(f64, f64, usize, Option<usize>)> {
        let max_layer = self.frame().layers.len().saturating_sub(1);
        if p.parent_object_info == 0xFFFF || p.parent_type == 0 {
            let layer = if p.layer < 0 {
                0
            } else {
                (p.layer as usize).min(max_layer)
            };
            let dir = if p.direction >= 0 && p.flags & position_flags::DEFAULT_DIRECTION == 0 {
                Some(p.direction as usize % 32)
            } else {
                None
            };
            return Some((p.x as f64, p.y as f64, layer, dir));
        }
        let parent = self.pick(p.parent_object_info, sel, cur)?;
        let pi = &self.instances[parent];
        let (mut dx, mut dy) = (p.x as f64, p.y as f64);
        if p.flags & position_flags::DIRECTION != 0 {
            let a = dir_to_angle(pi.direction());
            let (s, c) = a.sin_cos();
            (dx, dy) = (dx * c + dy * s, -dx * s + dy * c);
        }
        let (mut ox, mut oy) = (pi.x, pi.y);
        if p.flags & position_flags::ACTION_POINT != 0 {
            if let Some(obj) = self.game.object(pi.oi) {
                if let Some(e) = pi.current_image(obj).and_then(|h| self.game.images.get(h)) {
                    ox += (e.action_point.0 - e.hotspot.0) as f64;
                    oy += (e.action_point.1 - e.hotspot.1) as f64;
                }
            }
        }
        // Relative to an object: Fusion uses the parent's layer, not the parameter's.
        let layer = pi.layer.min(max_layer);
        let dir = if p.flags & position_flags::INITIAL_DIRECTION != 0 {
            Some(pi.direction())
        } else {
            None
        };
        Some((ox + dx, oy + dy, layer, dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rotated_square(angle_deg: f64) -> CollisionShape {
        let (s, c) = angle_deg.to_radians().sin_cos();
        CollisionShape {
            x: -100.0,
            y: -100.0,
            w: 200.0,
            h: 200.0,
            local: Some(LocalShape {
                x: 0.0,
                y: 0.0,
                hot: (0.0, 0.0),
                scale: (1.0, 1.0),
                sin: s,
                cos: c,
                size: (10.0, 4.0),
                mask: None,
            }),
        }
    }

    #[test]
    fn unrotated_shape_covers_its_rectangle() {
        let s = rotated_square(0.0);
        assert!(s.contains(9.5, 3.5));
        assert!(!s.contains(10.5, 1.0));
        assert!(!s.contains(1.0, -0.5));
    }

    #[test]
    fn rotation_is_counter_clockwise_on_screen() {
        // A 10x4 bar pointing right, rotated 90 degrees, points up (negative y).
        let s = rotated_square(90.0);
        assert!(s.contains(1.0, -8.0));
        assert!(!s.contains(8.0, 1.0));
    }

    #[test]
    fn scaling_grows_the_shape() {
        let mut s = rotated_square(0.0);
        if let Some(l) = &mut s.local {
            l.scale = (2.0, 2.0);
        }
        assert!(s.contains(19.0, 7.0));
        assert!(!s.contains(21.0, 1.0));
    }
}
