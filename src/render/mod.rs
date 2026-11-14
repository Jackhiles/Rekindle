//! Macroquad frame viewer (milestone 2): draws a frame's layers, backdrops and the
//! placed objects at their starting positions. No game logic runs yet.

pub mod audio_out;
mod hud;
pub mod scene;
pub mod gamepad;
pub mod settings;
pub mod text;

use crate::ctf_loader::properties::{Fill, ShapeKind};
use crate::ctf_loader::GameData;
use crate::runtime::{Input, Runtime};
use macroquad::miniquad::{BlendFactor, BlendState, BlendValue, Equation};
use macroquad::prelude::*;
use scene::{SceneItem, Visual};
use std::collections::HashMap;
use std::path::PathBuf;

/// Layer flag: layer is visible.
const LAYER_VISIBLE: u32 = 0x10;

#[derive(Debug, Clone, Default)]
pub struct ViewerOptions {
    pub start_frame: usize,
    /// Render one image to this PNG path and exit.
    pub screenshot: Option<PathBuf>,
    /// Start zoomed out to show the whole frame.
    pub fit: bool,
    /// Draw objects even if they start hidden.
    pub show_hidden: bool,
    /// Layer indices to hide in the first frame shown (debug aid for covering overlays).
    pub hidden_layers: Vec<usize>,
    /// Run the event sheets (milestone 3). Off = static start-of-frame view.
    pub run_events: bool,
    /// With `screenshot`: game ticks to simulate before capturing.
    pub screenshot_ticks: u32,
    /// With `screenshot` (a folder): scripted input and the ticks to capture at.
    pub script: Option<crate::runtime::script::InputScript>,
    /// Disable sound output.
    pub mute: bool,
    /// Where game save files go (None = default per-game folder).
    pub save_dir: Option<std::path::PathBuf>,
    /// The file the player opened (an .exe even if its data is a .dat); play time is logged under it.
    pub game_path: Option<PathBuf>,
    /// The game's player settings (fullscreen, scaling, filter, HUD).
    pub settings: settings::Settings,
    /// Force fullscreen (true) or windowed (false); None = saved choice, else the game's setting.
    pub fullscreen: Option<bool>,
}

/// Window configuration derived from the application header. Very large games are
/// scaled down so the window fits on a typical desktop.
pub fn window_conf(g: &GameData, s: &settings::Settings) -> Conf {
    let (w, h) = app_size(g);
    let scale = match s.window_scale {
        0 => (1600.0 / w).min(900.0 / h).min(1.0),
        k => k as f32,
    };
    Conf {
        window_title: if g.name.is_empty() {
            "Rekindle".into()
        } else {
            format!("{} - Rekindle", g.name)
        },
        window_width: (w * scale) as i32,
        window_height: (h * scale) as i32,
        window_resizable: true,
        high_dpi: false,
        icon: g
            .icon
            .as_ref()
            .map(|i| {
                let mut icon = macroquad::miniquad::conf::Icon::miniquad_logo();
                icon.small.copy_from_slice(&i.resized(16));
                icon.medium.copy_from_slice(&i.resized(32));
                icon.big.copy_from_slice(&i.resized(64));
                icon
            })
            .or_else(crate::launcher::window_icon),
        ..Default::default()
    }
}

fn app_size(g: &GameData) -> (f32, f32) {
    let w = if g.header.window_width > 0 {
        g.header.window_width
    } else {
        640
    };
    let h = if g.header.window_height > 0 {
        g.header.window_height
    } else {
        480
    };
    (w as f32, h as f32)
}

fn rgba(c: [u8; 4]) -> Color {
    Color::from_rgba(c[0], c[1], c[2], c[3])
}

fn opaque(c: [u8; 4]) -> Color {
    Color::from_rgba(c[0], c[1], c[2], 255)
}

const SPRITE_VS: &str = r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;
varying lowp vec2 uv;
varying lowp vec4 color;
uniform mat4 Model;
uniform mat4 Projection;
void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    color = color0 / 255.0;
    uv = texcoord;
}"#;

const SPRITE_FS: &str = r#"#version 100
varying lowp vec4 color;
varying lowp vec2 uv;
uniform sampler2D Texture;
void main() {
    gl_FragColor = color * texture2D(Texture, uv);
}"#;

/// Whether render-target textures come out upside down when drawn back with this camera setup.
const OFFSCREEN_FLIP_Y: bool = true;

const EFFECT_VS: &str = r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;
varying mediump vec2 uv;
varying lowp vec4 color;
uniform mat4 Model;
uniform mat4 Projection;
void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    color = color0 / 255.0;
    uv = texcoord;
}"#;

/// Perspective-object distortions. `src` is the source rectangle in texture coordinates,
/// `params` = (effect, vertical, zoom, waves).
const EFFECT_FS: &str = r#"#version 100
precision mediump float;
varying mediump vec2 uv;
varying lowp vec4 color;
uniform sampler2D Texture;
uniform vec4 src;
uniform vec4 params;
void main() {
    vec2 size = src.zw - src.xy;
    vec2 p = (uv - src.xy) / size;
    bool vertical = params.y > 0.5;
    if (vertical) { p = p.yx; }
    float k = params.z / 1000.0;
    vec2 q = p;
    if (params.x < 0.5) {
        // Panorama: magnify towards the edges like a cylinder.
        float f = (p.x - 0.5) * 2.0;
        q.y = 0.5 + (p.y - 0.5) / (1.0 + k * f * f);
    } else if (params.x < 1.5) {
        // Perspective: magnification grows across the width.
        q.y = 0.5 + (p.y - 0.5) / (1.0 + k * p.x);
    } else if (params.x < 2.5) {
        // Sine wave.
        q.y = p.y + sin(p.x * params.w * 6.2831853) * k * 0.05;
    } else {
        // Sine offset.
        q.x = p.x + sin(p.y * params.w * 6.2831853) * k * 0.05;
    }
    if (vertical) { q = q.yx; }
    q = clamp(q, 0.0, 1.0);
    gl_FragColor = vec4(texture2D(Texture, src.xy + q * size).rgb, 1.0);
}"#;

/// Copies a texture as fully opaque, without blending (render-target alpha is meaningless).
const BLIT_FS: &str = r#"#version 100
precision mediump float;
varying mediump vec2 uv;
varying lowp vec4 color;
uniform sampler2D Texture;
void main() {
    gl_FragColor = vec4(texture2D(Texture, uv).rgb, 1.0);
}"#;

/// Catmull-Rom bicubic upscaling in 9 bilinear taps (needs a linearly filtered texture).
const BICUBIC_FS: &str = r#"#version 100
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
varying lowp vec4 color;
varying lowp vec2 uv;
uniform sampler2D Texture;
uniform vec2 tex_size;
void main() {
    vec2 p = uv * tex_size;
    vec2 t1 = floor(p - 0.5) + 0.5;
    vec2 f = p - t1;
    vec2 w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    vec2 w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    vec2 w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    vec2 w3 = f * f * (-0.5 + 0.5 * f);
    vec2 w12 = w1 + w2;
    vec2 t0 = (t1 - 1.0) / tex_size;
    vec2 t3 = (t1 + 2.0) / tex_size;
    vec2 t12 = (t1 + w2 / w12) / tex_size;
    vec3 c = vec3(0.0);
    c += texture2D(Texture, vec2(t0.x, t0.y)).rgb * w0.x * w0.y;
    c += texture2D(Texture, vec2(t12.x, t0.y)).rgb * w12.x * w0.y;
    c += texture2D(Texture, vec2(t3.x, t0.y)).rgb * w3.x * w0.y;
    c += texture2D(Texture, vec2(t0.x, t12.y)).rgb * w0.x * w12.y;
    c += texture2D(Texture, vec2(t12.x, t12.y)).rgb * w12.x * w12.y;
    c += texture2D(Texture, vec2(t3.x, t12.y)).rgb * w3.x * w12.y;
    c += texture2D(Texture, vec2(t0.x, t3.y)).rgb * w0.x * w3.y;
    c += texture2D(Texture, vec2(t12.x, t3.y)).rgb * w12.x * w3.y;
    c += texture2D(Texture, vec2(t3.x, t3.y)).rgb * w3.x * w3.y;
    gl_FragColor = vec4(clamp(c, 0.0, 1.0), 1.0);
}"#;

/// Off-screen targets used when a frame contains distortion effects.
struct Offscreen {
    size: (u32, u32),
    main: RenderTarget,
    snapshot: RenderTarget,
    material: Option<Material>,
    blit: Option<Material>,
    /// Bicubic upscaling for the "enhanced" filter.
    bicubic: Option<Material>,
}

impl Offscreen {
    fn new(w: u32, h: u32) -> Self {
        let make = || {
            let rt = render_target(w, h);
            rt.texture.set_filter(FilterMode::Nearest);
            rt
        };
        let material = load_material(
            ShaderSource::Glsl {
                vertex: EFFECT_VS,
                fragment: EFFECT_FS,
            },
            MaterialParams {
                uniforms: vec![
                    UniformDesc::new("src", UniformType::Float4),
                    UniformDesc::new("params", UniformType::Float4),
                ],
                ..Default::default()
            },
        )
        .map_err(|e| eprintln!("effect shader unavailable: {e:?}"))
        .ok();
        let blit = load_material(
            ShaderSource::Glsl {
                vertex: EFFECT_VS,
                fragment: BLIT_FS,
            },
            MaterialParams::default(),
        )
        .map_err(|e| eprintln!("blit shader unavailable: {e:?}"))
        .ok();
        let main = make();
        // The finished frame is scaled to the window; filtering keeps that smooth.
        main.texture.set_filter(FilterMode::Linear);
        let bicubic = load_material(
            ShaderSource::Glsl {
                vertex: EFFECT_VS,
                fragment: BICUBIC_FS,
            },
            MaterialParams {
                uniforms: vec![UniformDesc::new("tex_size", UniformType::Float2)],
                ..Default::default()
            },
        )
        .map_err(|e| eprintln!("bicubic shader unavailable: {e:?}"))
        .ok();
        Self {
            bicubic,
            size: (w, h),
            main,
            snapshot: make(),
            material,
            blit,
        }
    }

    /// Draw a render target's texture opaquely.
    fn blit(&self, from: &RenderTarget, x: f32, y: f32, size: Vec2) {
        if let Some(m) = &self.blit {
            gl_use_material(m);
        }
        let params = DrawTextureParams {
            dest_size: Some(size),
            flip_y: OFFSCREEN_FLIP_Y,
            ..Default::default()
        };
        draw_texture_ex(&from.texture, x, y, WHITE, params);
        gl_use_default_material();
    }

    /// Draw the finished frame to the window with the chosen filter.
    fn present(&self, from: &RenderTarget, at: Vec2, size: Vec2, filter: settings::Filter) {
        let tex = &from.texture;
        tex.set_filter(if filter == settings::Filter::Sharp { FilterMode::Nearest } else { FilterMode::Linear });
        let material = match (filter, &self.bicubic) {
            (settings::Filter::Enhanced, Some(b)) => {
                b.set_uniform("tex_size", vec2(self.size.0 as f32, self.size.1 as f32));
                Some(b)
            }
            _ => self.blit.as_ref(),
        };
        if let Some(m) = material {
            gl_use_material(m);
        }
        let params = DrawTextureParams {
            dest_size: Some(size),
            flip_y: OFFSCREEN_FLIP_Y,
            ..Default::default()
        };
        draw_texture_ex(tex, at.x, at.y, WHITE, params);
        gl_use_default_material();
        tex.set_filter(FilterMode::Linear);
    }

    fn camera(&self, target: &RenderTarget) -> Camera2D {
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let mut cam = Camera2D::from_display_rect(Rect::new(0.0, 0.0, w, h));
        cam.render_target = Some(target.clone());
        cam
    }
}

/// Ink effect ids that need a non-default blend mode.
const INK_ADD: u16 = 9;
const INK_SUBTRACT: u16 = 11;

/// Fragment shaders for ink effects. `color` carries the object's tint and opacity.
const INK_PREMUL_FS: &str = r#"#version 100
precision mediump float;
varying lowp vec4 color;
varying lowp vec2 uv;
uniform sampler2D Texture;
void main() {
    vec4 t = color * texture2D(Texture, uv);
    gl_FragColor = vec4(t.rgb * t.a, t.a);
}"#;

const INK_AND_FS: &str = r#"#version 100
precision mediump float;
varying lowp vec4 color;
varying lowp vec2 uv;
uniform sampler2D Texture;
void main() {
    vec4 t = color * texture2D(Texture, uv);
    gl_FragColor = vec4(mix(vec3(1.0), t.rgb, t.a), 1.0);
}"#;

const INK_INVERT_FS: &str = r#"#version 100
precision mediump float;
varying lowp vec4 color;
varying lowp vec2 uv;
uniform sampler2D Texture;
void main() {
    vec4 t = texture2D(Texture, uv);
    gl_FragColor = vec4(1.0 - t.rgb, t.a) * color;
}"#;

/// Ink effect ids.
const INK_INVERT: u16 = 2;
const INK_XOR: u16 = 3;
const INK_AND: u16 = 4;
const INK_OR: u16 = 5;

/// Blend-mode materials for Fusion ink effects. The bitwise effects (AND / OR / XOR)
/// are approximated with multiply / screen / difference-style blending.
struct InkMaterials {
    add: Option<Material>,
    subtract: Option<Material>,
    invert: Option<Material>,
    and: Option<Material>,
    or: Option<Material>,
    xor: Option<Material>,
}

impl InkMaterials {
    fn new() -> Self {
        use BlendFactor::{One, OneMinusValue, Value, Zero};
        let make = |fs: &str, blend: BlendState| {
            load_material(
                ShaderSource::Glsl { vertex: SPRITE_VS, fragment: fs },
                MaterialParams {
                    pipeline_params: PipelineParams { color_blend: Some(blend), ..Default::default() },
                    ..Default::default()
                },
            )
            .map_err(|e| eprintln!("ink material unavailable: {e:?}"))
            .ok()
        };
        let alpha = BlendState::new(Equation::Add, Value(BlendValue::SourceAlpha), OneMinusValue(BlendValue::SourceAlpha));
        Self {
            add: make(SPRITE_FS, BlendState::new(Equation::Add, Value(BlendValue::SourceAlpha), One)),
            subtract: make(SPRITE_FS, BlendState::new(Equation::ReverseSubtract, Value(BlendValue::SourceAlpha), One)),
            invert: make(INK_INVERT_FS, alpha),
            and: make(INK_AND_FS, BlendState::new(Equation::Add, Value(BlendValue::DestinationColor), Zero)),
            or: make(INK_PREMUL_FS, BlendState::new(Equation::Add, One, OneMinusValue(BlendValue::SourceColor))),
            xor: make(
                INK_PREMUL_FS,
                BlendState::new(Equation::Add, OneMinusValue(BlendValue::DestinationColor), OneMinusValue(BlendValue::SourceColor)),
            ),
        }
    }

    fn for_effect(&self, effect: u16) -> Option<&Material> {
        match effect {
            INK_ADD => self.add.as_ref(),
            INK_SUBTRACT => self.subtract.as_ref(),
            INK_INVERT => self.invert.as_ref(),
            INK_AND => self.and.as_ref(),
            INK_OR => self.or.as_ref(),
            INK_XOR => self.xor.as_ref(),
            _ => None,
        }
    }
}

/// Lazily decoded GPU textures keyed by image handle, kept within a memory budget:
/// the least recently drawn ones are dropped first (never those of the current frame).
#[derive(Default)]
struct Textures {
    /// handle -> (sprite, bytes of its own texture (0 for atlas sprites), draw frame it was
    /// last used in, atlas page if packed)
    map: HashMap<u32, (Option<Sprite>, usize, u64, Option<usize>)>,
    /// Atlas pages that small images are packed into, so consecutive draws share a texture
    /// and the GPU batches them (hundreds of separate textures mean hundreds of draw calls).
    pages: Vec<Option<AtlasPage>>,
    bytes: usize,
    /// Counts rendered frames (for least-recently-used eviction).
    clock: u64,
    /// Replacement images from mods.
    mods: std::sync::Arc<crate::mods::Mods>,
}

/// A drawable image: a texture and the part of it holding the image.
#[derive(Clone)]
struct Sprite {
    tex: Texture2D,
    src: Rect,
}

impl Sprite {
    fn size(&self) -> Vec2 {
        vec2(self.src.w, self.src.h)
    }

    /// Source rectangle for a sub-area of the image.
    fn sub(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::new(self.src.x + x, self.src.y + y, w, h)
    }
}

const ATLAS_SIZE: u32 = 2048;
/// Images up to this size (each side) are packed into atlas pages.
const ATLAS_MAX: u32 = 256;
const PAGE_BYTES: usize = (ATLAS_SIZE * ATLAS_SIZE * 4) as usize;

struct AtlasPage {
    tex: Texture2D,
    /// Shelf packing: current shelf's top, its height, and the next free x on it.
    shelf_y: u32,
    shelf_h: u32,
    x: u32,
    last_used: u64,
}

impl AtlasPage {
    fn new() -> Self {
        let tex = Texture2D::from_rgba8(ATLAS_SIZE as u16, ATLAS_SIZE as u16, &vec![0u8; PAGE_BYTES]);
        tex.set_filter(FilterMode::Nearest);
        Self { tex, shelf_y: 0, shelf_h: 0, x: 0, last_used: 0 }
    }

    /// Room for w x h, plus a 1-pixel transparent border so neighbours never bleed in.
    fn place(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let (pw, ph) = (w + 2, h + 2);
        if self.x + pw > ATLAS_SIZE {
            self.shelf_y += self.shelf_h;
            self.shelf_h = 0;
            self.x = 0;
        }
        if self.shelf_y + ph > ATLAS_SIZE {
            return None;
        }
        let at = (self.x + 1, self.shelf_y + 1);
        self.x += pw;
        self.shelf_h = self.shelf_h.max(ph);
        Some(at)
    }
}

/// Textures kept on the GPU at most (roughly; `REKINDLE_TEXTURE_MB` overrides it).
fn texture_budget() -> usize {
    static B: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *B.get_or_init(|| std::env::var("REKINDLE_TEXTURE_MB").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(192) << 20)
}

impl Textures {
    fn get(&mut self, g: &GameData, handle: u32) -> Option<Sprite> {
        let clock = self.clock;
        if let Some(e) = self.map.get_mut(&handle) {
            e.2 = clock;
            if let Some(Some(p)) = e.3.and_then(|k| self.pages.get_mut(k)) {
                p.last_used = clock;
            }
            return e.0.clone();
        }
        let original = g.images.get(handle);
        let replaced = self.mods.image(handle);
        let resized = match (&replaced, original) {
            (Some(r), Some(o)) => r.width != o.width as u32 || r.height != o.height as u32,
            _ => false,
        };
        let img = replaced.or_else(|| original.and_then(|e| e.decode_rgba().ok()));
        let Some(img) = img.filter(|i| i.width > 0 && i.height > 0) else {
            self.map.insert(handle, (None, 0, clock, None));
            return None;
        };
        // Small images go into an atlas page (not resized replacements: those need smooth
        // filtering, which atlas pages don't use).
        if !resized && img.width <= ATLAS_MAX && img.height <= ATLAS_MAX {
            if let Some(sprite) = self.pack(handle, &img) {
                return Some(sprite);
            }
        }
        let t = Texture2D::from_rgba8(img.width as u16, img.height as u16, &img.pixels);
        // A replacement at another resolution is scaled to the original's size.
        t.set_filter(if resized { FilterMode::Linear } else { FilterMode::Nearest });
        let sprite = Sprite { src: Rect::new(0.0, 0.0, img.width as f32, img.height as f32), tex: t };
        self.bytes += img.pixels.len();
        self.map.insert(handle, (Some(sprite.clone()), img.pixels.len(), clock, None));
        Some(sprite)
    }

    fn pack(&mut self, handle: u32, img: &crate::ctf_loader::RgbaImage) -> Option<Sprite> {
        let (w, h) = (img.width, img.height);
        // The most recent pages first (older ones are usually full).
        let mut found = None;
        for (k, page) in self.pages.iter_mut().enumerate().rev() {
            if let Some(p) = page {
                if let Some(at) = p.place(w, h) {
                    found = Some((k, at));
                    break;
                }
            }
        }
        let (k, (x, y)) = match found {
            Some(f) => f,
            None => {
                let mut page = AtlasPage::new();
                let at = page.place(w, h)?;
                self.bytes += PAGE_BYTES;
                let k = match self.pages.iter().position(|p| p.is_none()) {
                    Some(free) => {
                        self.pages[free] = Some(page);
                        free
                    }
                    None => {
                        self.pages.push(Some(page));
                        self.pages.len() - 1
                    }
                };
                (k, at)
            }
        };
        let page = self.pages[k].as_mut()?;
        page.last_used = self.clock;
        let image = Image { bytes: img.pixels.clone(), width: w as u16, height: h as u16 };
        page.tex.update_part(&image, x as i32, y as i32, w as i32, h as i32);
        let sprite = Sprite { tex: page.tex.clone(), src: Rect::new(x as f32, y as f32, w as f32, h as f32) };
        self.map.insert(handle, (Some(sprite.clone()), 0, self.clock, Some(k)));
        Some(sprite)
    }

    /// Call once per rendered frame: advance the clock and release old textures (or whole
    /// atlas pages) if the cache is over budget.
    fn end_frame(&mut self) {
        if self.bytes > texture_budget() {
            // (last used, own texture handle or atlas page, bytes)
            let mut old: Vec<(u64, Result<u32, usize>, usize)> = self
                .map
                .iter()
                .filter(|(_, e)| e.2 < self.clock && e.3.is_none() && e.1 > 0)
                .map(|(h, e)| (e.2, Ok(*h), e.1))
                .collect();
            old.extend(self.pages.iter().enumerate().filter_map(|(k, p)| {
                p.as_ref().filter(|p| p.last_used < self.clock).map(|p| (p.last_used, Err(k), PAGE_BYTES))
            }));
            old.sort_unstable_by_key(|e| e.0);
            for (_, what, size) in old {
                if self.bytes <= texture_budget() * 3 / 4 {
                    break;
                }
                match what {
                    Ok(h) => {
                        self.map.remove(&h);
                    }
                    Err(k) => {
                        self.pages[k] = None;
                        self.map.retain(|_, e| e.3 != Some(k));
                    }
                }
                self.bytes -= size;
            }
        }
        self.clock += 1;
    }
}

/// World-to-screen mapping for one layer.
#[derive(Clone, Copy)]
struct Xform {
    origin: Vec2,
    scroll: Vec2,
    scale: f32,
}

impl Xform {
    fn pt(&self, x: f32, y: f32) -> Vec2 {
        self.origin + (vec2(x, y) - self.scroll) * self.scale
    }
}

struct Viewer {
    /// Pointer position in the game view last time the frame position was updated.
    last_window_mouse: Option<Vec2>,
    rt: Runtime,
    textures: Textures,
    inks: InkMaterials,
    fonts: text::Fonts,
    audio: audio_out::AudioOut,
    offscreen: Option<Offscreen>,
    /// Number of frame entries (frame_log length) the textures/layer toggles were set up for.
    shown_frame: usize,
    scroll: Vec2,
    fit: bool,
    show_hidden: bool,
    debug_boxes: bool,
    hud: hud::HudMode,
    /// F4: show details of the object under the mouse.
    inspector: bool,
    /// Index into hud::SPEEDS.
    speed_index: usize,
    stats: hud::HudStats,
    hud_font: Option<Font>,
    /// F12 pressed: save a screenshot after this frame is drawn.
    screenshot_requested: bool,
    running: bool,
    tick_acc: f64,
    input: Input,
    /// Per-layer visibility override for the current frame (debug toggles).
    layer_off: Vec<bool>,
    drag_from: Option<(Vec2, Vec2)>,
    settings: settings::Settings,
    fullscreen: bool,
    log: crate::log::Log,
    /// Unsupported features already written to the log.
    logged_unimplemented: usize,
    /// An error message shown on screen until the given time.
    notice: Option<(String, f64)>,
    /// Consecutive ticks that failed; the game pauses if this keeps growing.
    failed_ticks: u32,
    /// Controller mapped to keyboard + mouse.
    pad: gamepad::Gamepad,
}

/// Consecutive failing ticks before the game is paused.
const MAX_FAILED_TICKS: u32 = 30;

/// Windows virtual-key code for a Macroquad key (what Fusion events compare against).
fn vk_code(k: KeyCode) -> Option<u16> {
    use KeyCode::*;
    let code = k as u16;
    Some(match k {
        A | B | C | D | E | F | G | H | I | J | K | L | M | N | O | P | Q | R | S | T | U | V
        | W | X | Y | Z => 0x41 + (code - A as u16),
        Key0 | Key1 | Key2 | Key3 | Key4 | Key5 | Key6 | Key7 | Key8 | Key9 => {
            0x30 + (code - Key0 as u16)
        }
        Kp0 | Kp1 | Kp2 | Kp3 | Kp4 | Kp5 | Kp6 | Kp7 | Kp8 | Kp9 => 0x60 + (code - Kp0 as u16),
        F1 | F2 | F3 | F4 | F5 | F6 | F7 | F8 | F9 | F10 | F11 | F12 => 0x70 + (code - F1 as u16),
        Space => 0x20,
        Enter | KpEnter => 0x0D,
        Escape => 0x1B,
        Backspace => 0x08,
        Tab => 0x09,
        LeftShift | RightShift => 0x10,
        LeftControl | RightControl => 0x11,
        LeftAlt | RightAlt => 0x12,
        Left => 0x25,
        Up => 0x26,
        Right => 0x27,
        Down => 0x28,
        Insert => 0x2D,
        Delete => 0x2E,
        Home => 0x24,
        End => 0x23,
        PageUp => 0x21,
        PageDown => 0x22,
        _ => return None,
    })
}

impl Viewer {
    fn load_frame(&mut self, index: usize) {
        let n = self.rt.game.frames.len();
        if n == 0 {
            return;
        }
        self.rt.load_frame(index % n);
        self.on_frame_changed();
    }

    fn on_frame_changed(&mut self) {
        self.shown_frame = self.rt.frame_log.len();
        self.scroll = Vec2::ZERO;
        self.layer_off = vec![false; self.rt.frame().layers.len().max(1)];
        let line = format!("frame {} \"{}\"", self.rt.frame_index, self.rt.frame().name);
        self.log.line(&line);
    }

    fn view_size(&self) -> Vec2 {
        let (w, h) = app_size(&self.rt.game);
        if self.fit {
            let f = self.rt.frame();
            vec2((f.width as f32).max(w), (f.height as f32).max(h))
        } else {
            vec2(w, h)
        }
    }

    fn frame_size(&self) -> Vec2 {
        let f = self.rt.frame();
        vec2(f.width as f32, f.height as f32)
    }

    fn clamp_scroll(&mut self) {
        let max = (self.frame_size() - self.view_size()).max(Vec2::ZERO);
        self.scroll = self.scroll.clamp(Vec2::ZERO, max);
    }

    /// Where the game view appears on screen (top-left and size), per the scaling setting.
    fn present(&self) -> (Vec2, Vec2) {
        let view = self.view_size();
        let screen = vec2(screen_width(), screen_height());
        let fit = (screen.x / view.x).min(screen.y / view.y);
        let scale = match self.settings.scaling {
            _ if self.fit => fit,
            settings::Scaling::Stretch => return (Vec2::ZERO, screen),
            settings::Scaling::Integer if fit >= 1.0 => fit.floor(),
            _ => fit,
        };
        let size = view * scale;
        ((screen - size) * 0.5, size)
    }

    /// Screen position and uniform scale of the game view (stretching is done by the
    /// final blit, so the scene itself is always drawn with one scale).
    /// F12: the window as a PNG in `<data dir>/screenshots/`.
    fn save_screenshot(&mut self) {
        let dir = crate::runtime::files::data_root().join("screenshots");
        let _ = std::fs::create_dir_all(&dir);
        let stem: String = self
            .rt
            .game
            .name
            .chars()
            .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' })
            .collect();
        let stem = if stem.trim().is_empty() { "game".to_string() } else { stem.trim().to_string() };
        let path = dir.join(format!("{stem} {}.png", chrono::Local::now().format("%Y-%m-%d %H-%M-%S")));
        get_screen_data().export_png(&path.to_string_lossy());
        self.log.line(&format!("screenshot {}", path.display()));
        self.notice = Some((format!("Screenshot saved to {}", path.display()), get_time() + 3.0));
    }

    fn viewport(&self) -> (Vec2, f32) {
        let (origin, size) = self.present();
        (origin, size.x / self.view_size().x)
    }

    fn current_scroll(&self) -> Vec2 {
        if self.fit {
            Vec2::ZERO
        } else if self.running {
            vec2(self.rt.scroll.0 as f32, self.rt.scroll.1 as f32)
        } else {
            self.scroll
        }
    }

    /// Collect game input for the next tick (keys accumulate until a tick consumes them).
    fn gather_input(&mut self) {
        while let Some(c) = get_char_pressed() {
            self.input.typed.push(c);
        }
        for k in get_keys_pressed() {
            if let Some(v) = vk_code(k) {
                self.input.pressed.insert(v);
            }
        }
        self.input.down = get_keys_down().into_iter().filter_map(vk_code).collect();
        for (i, b) in [MouseButton::Left, MouseButton::Middle, MouseButton::Right]
            .into_iter()
            .enumerate()
        {
            if is_mouse_button_pressed(b) {
                self.input.clicked[i] = true;
            }
            if is_mouse_button_down(b) {
                self.input.down.insert([1u16, 4, 2][i]);
            }
        }
        let (origin, size) = self.present();
        // Like the original runtime, the pointer's frame position is updated when the
        // pointer moves (with the scroll at that moment), not every tick: an object that
        // follows the mouse and that the camera follows then pans as the mouse moves and
        // stops when it stops.
        let win = (Vec2::from(mouse_position()) - origin) / (size / self.view_size());
        if self.last_window_mouse != Some(win) || self.shown_frame != self.rt.frame_log.len() {
            let m = win + self.current_scroll();
            self.input.mouse = (m.x as f64, m.y as f64);
            self.last_window_mouse = Some(win);
        }
        self.input.wheel += mouse_wheel().1;
        let (origin, size) = self.present();
        let view = self.view_size();
        let scroll = self.current_scroll();
        self.pad.apply(&mut self.input, view, |p| (p - origin) / (size / view), scroll);
    }

    /// Debug keys. Function keys and Ctrl combinations are reserved for the viewer;
    /// everything else goes to the game.
    fn handle_debug_keys(&mut self) {
        let n = self.rt.game.frames.len();
        let ctrl = is_key_down(KeyCode::LeftControl) || is_key_down(KeyCode::RightControl);
        if is_key_pressed(KeyCode::PageDown) && ctrl {
            self.load_frame(self.rt.frame_index + 1);
        }
        if is_key_pressed(KeyCode::PageUp) && ctrl {
            self.load_frame(self.rt.frame_index + n - 1);
        }
        if is_key_pressed(KeyCode::F1) {
            self.hud = self.hud.next();
        }
        if is_key_pressed(KeyCode::F4) {
            self.inspector = !self.inspector;
        }
        if is_key_pressed(KeyCode::F12) {
            self.screenshot_requested = true;
        }
        if ctrl && is_key_pressed(KeyCode::M) {
            let m = !self.audio.muted();
            self.audio.set_muted(m);
            self.notice = Some((if m { "Sound off".into() } else { "Sound on".into() }, get_time() + 2.0));
        }
        if ctrl && (is_key_pressed(KeyCode::Minus) || is_key_pressed(KeyCode::KpSubtract)) {
            self.speed_index = self.speed_index.saturating_sub(1);
            self.notice = Some((format!("Game speed x{}", hud::SPEEDS[self.speed_index]), get_time() + 2.0));
        }
        if ctrl && (is_key_pressed(KeyCode::Equal) || is_key_pressed(KeyCode::KpAdd)) {
            self.speed_index = (self.speed_index + 1).min(hud::SPEEDS.len() - 1);
            self.notice = Some((format!("Game speed x{}", hud::SPEEDS[self.speed_index]), get_time() + 2.0));
        }
        let alt = is_key_down(KeyCode::LeftAlt) || is_key_down(KeyCode::RightAlt);
        if is_key_pressed(KeyCode::F11) || (alt && is_key_pressed(KeyCode::Enter)) {
            self.set_fullscreen(!self.fullscreen);
            self.settings.fullscreen = Some(self.fullscreen);
            self.settings.save();
        }
        if is_key_pressed(KeyCode::F9) {
            use settings::Filter::*;
            self.settings.filter = match self.settings.filter {
                Sharp => Smooth,
                Smooth => Enhanced,
                Enhanced => Sharp,
            };
            self.settings.save();
            self.notice = Some((format!("Filter: {}", self.settings.filter.name()), get_time() + 2.0));
        }
        if is_key_pressed(KeyCode::F10) {
            use settings::Scaling::*;
            self.settings.scaling = match self.settings.scaling {
                Fit => Integer,
                Integer => Stretch,
                Stretch => Fit,
            };
            self.settings.save();
            self.notice = Some((format!("Scaling: {}", self.settings.scaling.name()), get_time() + 2.0));
        }
        if is_key_pressed(KeyCode::F2) {
            self.running = !self.running;
            self.scroll = vec2(self.rt.scroll.0 as f32, self.rt.scroll.1 as f32);
        }
        if is_key_pressed(KeyCode::F3) && !self.running {
            self.step();
        }
        if is_key_pressed(KeyCode::F5) {
            self.load_frame(self.rt.frame_index);
        }
        if is_key_pressed(KeyCode::F6) {
            self.debug_boxes = !self.debug_boxes;
        }
        if is_key_pressed(KeyCode::F7) {
            self.show_hidden = !self.show_hidden;
        }
        if is_key_pressed(KeyCode::F8) {
            self.fit = !self.fit;
        }
        if ctrl {
            const DIGITS: [KeyCode; 10] = [
                KeyCode::Key1,
                KeyCode::Key2,
                KeyCode::Key3,
                KeyCode::Key4,
                KeyCode::Key5,
                KeyCode::Key6,
                KeyCode::Key7,
                KeyCode::Key8,
                KeyCode::Key9,
                KeyCode::Key0,
            ];
            for (i, k) in DIGITS.iter().enumerate() {
                if is_key_pressed(*k) {
                    if let Some(off) = self.layer_off.get_mut(i) {
                        *off = !*off;
                    }
                }
            }
        }

        // Free camera only while paused; while running the game owns the view.
        if !self.running {
            let speed = if is_key_down(KeyCode::LeftShift) {
                2400.0
            } else {
                800.0
            } * get_frame_time();
            let mut d = Vec2::ZERO;
            if is_key_down(KeyCode::Left) {
                d.x -= speed;
            }
            if is_key_down(KeyCode::Right) {
                d.x += speed;
            }
            if is_key_down(KeyCode::Up) {
                d.y -= speed;
            }
            if is_key_down(KeyCode::Down) {
                d.y += speed;
            }
            self.scroll += d;
            let mouse = Vec2::from(mouse_position());
            if is_mouse_button_pressed(MouseButton::Left) {
                self.drag_from = Some((mouse, self.scroll));
            }
            if is_mouse_button_down(MouseButton::Left) {
                if let Some((start, scroll)) = self.drag_from {
                    let (_, scale) = self.viewport();
                    self.scroll = scroll - (mouse - start) / scale;
                }
            } else {
                self.drag_from = None;
            }
            self.clamp_scroll();
        }
    }

    /// Run as many fixed-rate game ticks as real time allows.
    fn advance(&mut self) {
        if !self.running {
            return;
        }
        let dt = 1.0 / self.rt.frame_rate as f64;
        let speed = hud::SPEEDS[self.speed_index] as f64;
        self.tick_acc = (self.tick_acc + get_frame_time() as f64 * speed).min(dt * 4.0 * speed.max(1.0));
        while self.tick_acc >= dt && self.running {
            self.tick_acc -= dt;
            self.step();
        }
        self.log_unimplemented();
    }

    /// One game tick with the gathered input. A failure inside the game logic is logged
    /// and reported on screen instead of closing the program.
    fn step(&mut self) {
        let input = std::mem::take(&mut self.input);
        let rt = &mut self.rt;
        let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rt.tick(&input))).is_ok();
        self.input = input;
        self.input.consume_edges();
        if ok {
            self.failed_ticks = 0;
        } else {
            self.failed_ticks += 1;
            let msg = crate::log::take_panic().unwrap_or_else(|| "unknown error".into());
            let line = format!(
                "error in frame {} \"{}\" at tick {}: {msg}",
                self.rt.frame_index,
                self.rt.frame().name,
                self.rt.tick
            );
            self.log.line(&line);
            let mut text = format!("Game logic error (logged to {})", self.log.path.display());
            if self.failed_ticks >= MAX_FAILED_TICKS {
                self.running = false;
                text += " - paused after repeated errors (F2 resumes)";
                self.log.line("paused after repeated errors");
            }
            self.notice = Some((text, get_time() + 6.0));
        }
        if self.rt.frame_log.len() != self.shown_frame {
            self.on_frame_changed();
        }
    }

    /// Write newly hit unsupported features to the log file.
    fn log_unimplemented(&mut self) {
        let un = self.rt.unimplemented.borrow();
        if un.len() == self.logged_unimplemented {
            return;
        }
        let names: Vec<String> = un.keys().cloned().collect();
        drop(un);
        let line = format!(
            "frame {}: {} unsupported feature(s) so far: {}",
            self.rt.frame_index,
            names.len(),
            names.join("; ")
        );
        self.log.line(&line);
        self.logged_unimplemented = names.len();
    }

    fn set_fullscreen(&mut self, on: bool) {
        if on == self.fullscreen {
            return;
        }
        set_fullscreen(on);
        if !on {
            // Back to the normal window size.
            let conf = window_conf(&self.rt.game, &self.settings);
            request_new_screen_size(conf.window_width as f32, conf.window_height as f32);
        }
        self.fullscreen = on;
    }

    fn draw(&mut self) {
        let g_border = self.rt.game.header.border_color;
        clear_background(opaque(g_border));
        let (screen_origin, screen_scale) = self.viewport();
        let (_, present_size) = self.present();
        let view = self.view_size();
        let scene = self.rt.draw_items();
        // Distortion effects need what's behind them as a texture: draw the game view
        // into an off-screen target at native resolution, then scale it to the window.
        let has_effect = scene
            .iter()
            .any(|i| matches!(i.visual, Visual::Effect { .. }) && (i.visible || self.show_hidden));
        let scaled = (present_size - view).abs().max_element() > 0.5;
        let stretched = self.settings.scaling == settings::Scaling::Stretch;
        let use_offscreen = !self.fit
            && (has_effect || stretched || (scaled && self.settings.filter != settings::Filter::Sharp));
        if use_offscreen {
            let want = (view.x.max(1.0) as u32, view.y.max(1.0) as u32);
            if self.offscreen.as_ref().is_none_or(|o| o.size != want) {
                self.offscreen = Some(Offscreen::new(want.0, want.1));
            }
            let off = self.offscreen.as_ref().expect("created");
            set_camera(&off.camera(&off.main));
            clear_background(opaque(g_border));
        }
        let (origin, scale) = if use_offscreen {
            (Vec2::ZERO, 1.0)
        } else {
            (screen_origin, screen_scale)
        };
        let scroll = self.current_scroll();
        let frame = self.rt.frame();
        let (fw, fh) = (frame.width as f32, frame.height as f32);

        // Frame background, clipped to both the frame and the view.
        let base = Xform {
            origin,
            scroll,
            scale,
        };
        let tl = base.pt(0.0, 0.0).max(origin);
        let br = base.pt(fw, fh).min(origin + view * scale);
        if br.x > tl.x && br.y > tl.y {
            draw_rectangle(
                tl.x,
                tl.y,
                br.x - tl.x,
                br.y - tl.y,
                opaque(self.rt.background_color),
            );
        }

        let layer_count = frame.layers.len().max(1);
        let layer_info: Vec<(bool, Vec2)> = (0..layer_count)
            .map(|i| match frame.layers.get(i) {
                Some(l) => {
                    let (cx, cy) = l.scroll_coefficients();
                    (l.flags & LAYER_VISIBLE != 0, vec2(cx, cy))
                }
                None => (true, Vec2::ONE),
            })
            .collect();

        for item in &scene {
            let li = item.layer.min(layer_count - 1);
            let (layer_visible, coef) = layer_info[li];
            // While the game runs, partially scrolling layers have already moved their
            // objects with the camera (see Runtime::center_display); the static viewer
            // applies the layer's coefficient instead.
            // While the game runs, objects that don't follow the frame and partially
            // scrolling layers have already moved with the camera (Runtime::center_display);
            // the static viewer applies the layer's coefficient instead.
            let coef = if self.running {
                Vec2::ONE
            } else if item.fixed {
                Vec2::ZERO
            } else {
                coef
            };
            if self.layer_off.get(li).copied().unwrap_or(false)
                || (!layer_visible && !self.show_hidden)
            {
                continue;
            }
            let xf = Xform {
                origin,
                scroll: scroll * coef,
                scale,
            };
            let shown = item.visible || self.show_hidden;
            if shown {
                self.draw_item(item, xf, view);
            }
            if self.debug_boxes {
                self.draw_debug_box(item, xf);
            }
        }

        if use_offscreen {
            set_default_camera();
            let off = self.offscreen.as_ref().expect("created");
            off.present(&off.main, screen_origin, present_size, self.settings.filter);
        }
        let (origin, scale) = (screen_origin, screen_scale);

        // Frame transition.
        if let Some(t) = &self.rt.transition {
            draw_transition(t, origin, present_size);
        }

        // Letterbox bars hide anything drawn outside the game view.
        let sw = screen_width();
        let sh = screen_height();
        let border = opaque(g_border);
        let end = origin + present_size;
        draw_rectangle(0.0, 0.0, sw, origin.y, border);
        draw_rectangle(0.0, end.y, sw, sh - end.y, border);
        draw_rectangle(0.0, 0.0, origin.x, sh, border);
        draw_rectangle(end.x, 0.0, sw - end.x, sh, border);

        // Custom mouse pointer (Cursor extension).
        if self.running {
            if let Some(h) = self.rt.cursor_image() {
                if let (Some(sp), Some(e)) = (self.textures.get(&self.rt.game, h), self.rt.game.images.get(h)) {
                    let m = Vec2::from(mouse_position());
                    let hot = vec2(e.hotspot.0 as f32, e.hotspot.1 as f32) * scale;
                    let params = DrawTextureParams {
                        dest_size: Some(vec2(e.width as f32, e.height as f32) * scale),
                        source: Some(sp.src),
                        ..Default::default()
                    };
                    draw_texture_ex(&sp.tex, m.x - hot.x, m.y - hot.y, WHITE, params);
                }
            }
        }

        if self.notice.as_ref().is_some_and(|n| get_time() >= n.1) {
            self.notice = None;
        }
        if let Some((msg, _)) = &self.notice {
            let w = measure_text(msg, None, 20, 1.0).width;
            draw_rectangle(8.0, screen_height() - 36.0, w + 16.0, 28.0, Color::new(0.5, 0.0, 0.0, 0.85));
            draw_text(msg, 16.0, screen_height() - 16.0, 20.0, WHITE);
        }
        if let Some(c) = self.pad.cursor {
            let (o, size) = self.present();
            self.pad.draw_cursor(o + c * (size / self.view_size()));
        }
        self.draw_hud();
        self.textures.end_frame();
    }

    fn item_bounds(&self, item: &SceneItem) -> Rect {
        match &item.visual {
            Visual::Widget(w) => Rect::new(item.x, item.y, w.w, w.h),
            Visual::Image { handle } => match self.rt.game.images.get(*handle) {
                Some(e) => {
                    let (sx, sy) = (item.scale.0.abs(), item.scale.1.abs());
                    Rect::new(
                        item.x - e.hotspot.0 as f32 * sx,
                        item.y - e.hotspot.1 as f32 * sy,
                        e.width as f32 * sx,
                        e.height as f32 * sy,
                    )
                }
                None => Rect::new(item.x, item.y, 16.0, 16.0),
            },
            Visual::Shape { w, h, .. }
            | Visual::Placeholder { w, h }
            | Visual::Text { w, h, .. }
            | Visual::Effect { w, h, .. } => Rect::new(item.x, item.y, *w, *h),
            Visual::Glyphs { handles } => {
                let (mut tw, mut th) = (0.0f32, 0.0f32);
                for e in handles.iter().filter_map(|h| self.rt.game.images.get(*h)) {
                    tw += e.width as f32;
                    th = th.max(e.height as f32);
                }
                Rect::new(item.x - tw, item.y - th, tw, th)
            }
        }
    }

    fn draw_item(&mut self, item: &SceneItem, xf: Xform, view: Vec2) {
        let b = self.item_bounds(item);
        // Cull against the visible part of the world for this layer.
        let visible_world = Rect::new(xf.scroll.x, xf.scroll.y, view.x, view.y);
        if !visible_world.overlaps(&b) {
            return;
        }
        DRAWN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = xf.pt(b.x, b.y);
        let size = vec2(b.w, b.h) * xf.scale;
        match &item.visual {
            Visual::Image { handle } => {
                if let Some(sp) = self.textures.get(&self.rt.game, *handle) {
                    let (sx, sy) = item.scale;
                    let params = DrawTextureParams {
                        dest_size: Some(size),
                        source: Some(sp.src),
                        flip_x: sx < 0.0,
                        flip_y: sy < 0.0,
                        rotation: -item.angle.to_radians(),
                        pivot: Some(xf.pt(item.x, item.y)),
                        ..Default::default()
                    };
                    let material = self.inks.for_effect(item.ink_effect);
                    if let Some(m) = material {
                        gl_use_material(m);
                    }
                    draw_texture_ex(&sp.tex, p.x, p.y, rgba(item.tint), params);
                    if material.is_some() {
                        gl_use_default_material();
                    }
                }
            }
            Visual::Shape { shape, .. } => {
                let border = shape.border_size as f32 * xf.scale;
                let bcol = opaque(shape.border_color);
                match shape.kind {
                    ShapeKind::Line => {
                        let (mut a, mut c) = (p, p + size);
                        if shape.line_flags & 1 != 0 {
                            std::mem::swap(&mut a.x, &mut c.x);
                        }
                        if shape.line_flags & 2 != 0 {
                            std::mem::swap(&mut a.y, &mut c.y);
                        }
                        draw_line(a.x, a.y, c.x, c.y, border.max(1.0), bcol);
                    }
                    ShapeKind::Ellipse => {
                        let c = p + size * 0.5;
                        match shape.fill {
                            Fill::Solid(col) => {
                                draw_ellipse(c.x, c.y, size.x / 2.0, size.y / 2.0, 0.0, opaque(col))
                            }
                            Fill::Gradient { from, .. } => draw_ellipse(
                                c.x,
                                c.y,
                                size.x / 2.0,
                                size.y / 2.0,
                                0.0,
                                opaque(from),
                            ),
                            _ => {}
                        }
                        if shape.border_size > 0 {
                            draw_ellipse_lines(
                                c.x,
                                c.y,
                                size.x / 2.0,
                                size.y / 2.0,
                                0.0,
                                border,
                                bcol,
                            );
                        }
                    }
                    _ => {
                        match shape.fill {
                            Fill::Solid(col) => {
                                draw_rectangle(p.x, p.y, size.x, size.y, opaque(col))
                            }
                            Fill::Gradient { from, to, vertical } => {
                                draw_gradient(p, size, from, to, vertical)
                            }
                            Fill::Motif(img) => {
                                self.draw_tiled(img as u32, p, size, xf.scale, rgba(item.tint))
                            }
                            Fill::None => {}
                        }
                        if shape.border_size > 0 {
                            draw_rectangle_lines(p.x, p.y, size.x, size.y, border * 2.0, bcol);
                        }
                    }
                }
            }
            Visual::Text {
                text,
                font,
                color,
                flags,
                size: size_override,
                ..
            } => {
                let (f, px) = self.fonts.get(&self.rt.game, *font);
                let px = size_override.unwrap_or(px);
                let mut c = rgba(*color);
                c.a = item.tint[3] as f32 / 255.0;
                text::draw_text_box(
                    text,
                    f.as_ref(),
                    px * xf.scale,
                    c,
                    *flags,
                    p.x,
                    p.y,
                    size.x,
                    size.y,
                );
            }
            Visual::Glyphs { handles } => {
                // Laid out right-to-left from the bottom-right corner.
                let mut right = p.x + size.x;
                let bottom = p.y + size.y;
                for h in handles.iter().rev() {
                    let Some(sp) = self.textures.get(&self.rt.game, *h) else {
                        continue;
                    };
                    let s = self
                        .rt
                        .game
                        .images
                        .get(*h)
                        .map_or(sp.size(), |e| vec2(e.width as f32, e.height as f32))
                        * xf.scale;
                    right -= s.x;
                    let params = DrawTextureParams {
                        dest_size: Some(s),
                        source: Some(sp.src),
                        ..Default::default()
                    };
                    draw_texture_ex(&sp.tex, right, bottom - s.y, rgba(item.tint), params);
                }
            }
            Visual::Effect { params, .. } => {
                static NO_EFFECTS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
                if *NO_EFFECTS.get_or_init(|| std::env::var_os("REKINDLE_NO_EFFECTS").is_some()) {
                    return;
                }
                let Some(off) = &self.offscreen else { return };
                let Some(material) = &off.material else {
                    return;
                };
                // Snapshot what has been drawn so far, then redraw this region distorted.
                let (tw, th) = (off.size.0 as f32, off.size.1 as f32);
                set_camera(&off.camera(&off.snapshot));
                off.blit(&off.main, 0.0, 0.0, vec2(tw, th));
                set_camera(&off.camera(&off.main));
                let src = Rect::new(p.x, p.y, size.x, size.y);
                let flip = OFFSCREEN_FLIP_Y;
                let (v0, v1) = if flip {
                    (1.0 - (p.y + size.y) / th, 1.0 - p.y / th)
                } else {
                    (p.y / th, (p.y + size.y) / th)
                };
                material.set_uniform("src", vec4(p.x / tw, v0, (p.x + size.x) / tw, v1));
                material.set_uniform(
                    "params",
                    vec4(
                        params.effect as f32,
                        params.vertical as u8 as f32,
                        params.zoom,
                        params.waves,
                    ),
                );
                gl_use_material(material);
                let region = DrawTextureParams {
                    dest_size: Some(size),
                    source: Some(src),
                    flip_y: flip,
                    ..Default::default()
                };
                draw_texture_ex(&off.snapshot.texture, p.x, p.y, WHITE, region);
                gl_use_default_material();
            }
            Visual::Widget(w) => {
                let font = self.fonts.by_face(&w.font_face, w.bold);
                draw_widget(w, font.as_ref(), p, xf.scale);
            }
            Visual::Placeholder { .. } => {}
        }
    }

    fn draw_tiled(&mut self, handle: u32, p: Vec2, size: Vec2, scale: f32, tint: Color) {
        let Some(sp) = self.textures.get(&self.rt.game, handle) else {
            return;
        };
        // Game-pixel size of the tile (a replacement image may have more pixels).
        let orig = self
            .rt
            .game
            .images
            .get(handle)
            .map_or(sp.size(), |e| vec2(e.width as f32, e.height as f32));
        let k = sp.size() / orig.max(Vec2::ONE);
        let tile = orig * scale;
        if tile.x < 0.5 || tile.y < 0.5 {
            return;
        }
        let mut y = 0.0;
        while y < size.y {
            let th = tile.y.min(size.y - y);
            let mut x = 0.0;
            while x < size.x {
                let tw = tile.x.min(size.x - x);
                let src = sp.sub(0.0, 0.0, tw / scale * k.x, th / scale * k.y);
                let params = DrawTextureParams {
                    dest_size: Some(vec2(tw, th)),
                    source: Some(src),
                    ..Default::default()
                };
                draw_texture_ex(&sp.tex, p.x + x, p.y + y, tint, params);
                x += tile.x;
            }
            y += tile.y;
        }
    }

    fn draw_debug_box(&self, item: &SceneItem, xf: Xform) {
        let b = self.item_bounds(item);
        let p = xf.pt(b.x, b.y);
        let col = match (&item.visual, item.visible) {
            (Visual::Placeholder { .. }, _) => ORANGE,
            (_, false) => GRAY,
            (_, true) if item.background => SKYBLUE,
            _ => LIME,
        };
        draw_rectangle_lines(p.x, p.y, b.w * xf.scale, b.h * xf.scale, 1.0, col);
        draw_text(&item.label, p.x + 2.0, p.y + 12.0, 14.0, col);
    }
}

/// Draw a transition as coverage of the view with its colour. Wipe effects cover the
/// screen geometrically; anything we don't model is drawn as a fade.
fn draw_transition(t: &crate::runtime::TransitionState, origin: Vec2, size: Vec2) {
    let k = t.coverage();
    let c = opaque(t.color);
    let (x, y, w, h) = (origin.x, origin.y, size.x, size.y);
    match t.effect.as_str() {
        // Two doors closing from the left and right edges.
        "DOOR" => {
            let half = w / 2.0 * k;
            draw_rectangle(x, y, half, h, c);
            draw_rectangle(x + w - half, y, half, h, c);
        }
        // Horizontal bands growing until they meet.
        "BAND" => {
            const BANDS: usize = 8;
            let bh = h / BANDS as f32;
            for i in 0..BANDS {
                draw_rectangle(x, y + bh * i as f32, w, bh * k, c);
            }
        }
        // A rectangle growing out of the centre.
        "SQUA" | "ZOOM" | "OPEN" => {
            let (cw, ch) = (w * k, h * k);
            draw_rectangle(x + (w - cw) / 2.0, y + (h - ch) / 2.0, cw, ch, c);
        }
        // Covering from the left.
        "SCRL" | "SCRO" | "PUSH" => draw_rectangle(x, y, w * k, h, c),
        _ => {
            let mut c = c;
            c.a = k;
            draw_rectangle(x, y, w, h, c);
        }
    }
}

fn draw_gradient(p: Vec2, size: Vec2, from: [u8; 4], to: [u8; 4], vertical: bool) {
    let (a, b) = (opaque(from), opaque(to));
    let (c_tl, c_tr, c_br, c_bl) = if vertical { (a, a, b, b) } else { (a, b, b, a) };
    let v = |x: f32, y: f32, c: Color| Vertex::new(x, y, 0.0, 0.0, 0.0, c);
    let mesh = Mesh {
        vertices: vec![
            v(p.x, p.y, c_tl),
            v(p.x + size.x, p.y, c_tr),
            v(p.x + size.x, p.y + size.y, c_br),
            v(p.x, p.y + size.y, c_bl),
        ],
        indices: vec![0, 1, 2, 0, 2, 3],
        texture: None,
    };
    draw_mesh(&mesh);
}

/// Run the game (or take a screenshot). Call from inside a Macroquad window.
/// Make sure only one copy of a game runs at a time: takes a lock for `game_path` that
/// is held until the process exits. Returns false when another copy already holds it.
pub fn claim_single_instance(game_path: &std::path::Path) -> bool {
    let canon = std::fs::canonicalize(game_path).unwrap_or_else(|_| game_path.to_path_buf());
    let key = canon.to_string_lossy();
    let key = if cfg!(windows) { key.to_lowercase() } else { key.into_owned() };
    let mut crc = flate2::Crc::new();
    crc.update(key.as_bytes());
    let dir = crate::runtime::files::data_root().join("running");
    let _ = std::fs::create_dir_all(&dir);
    let Ok(file) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(format!("{:08x}.lock", crc.sum())))
    else {
        return true; // can't tell; don't stop the game over it
    };
    match file.try_lock() {
        Ok(()) => {
            // Released by the OS when the process ends.
            std::mem::forget(file);
            true
        }
        Err(std::fs::TryLockError::WouldBlock) => false,
        Err(_) => true,
    }
}

/// Items drawn (after culling), for REKINDLE_PROFILE.
static DRAWN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Per-part frame timing for REKINDLE_PROFILE.
#[derive(Default)]
struct FrameProfile {
    sum: [std::time::Duration; 4],
    frames: u32,
    since: Option<std::time::Instant>,
}

impl FrameProfile {
    /// Returns true when it printed a report.
    fn add(&mut self, parts: [std::time::Duration; 4]) -> bool {
        let since = *self.since.get_or_insert_with(std::time::Instant::now);
        for (s, p) in self.sum.iter_mut().zip(parts) {
            *s += p;
        }
        self.frames += 1;
        if since.elapsed().as_secs_f32() >= 3.0 {
            let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0 / self.frames as f64;
            eprintln!(
                "profile: {:.0} fps, {} items drawn | logic {:.2} ms, audio {:.2} ms, draw {:.2} ms, present {:.2} ms",
                self.frames as f32 / since.elapsed().as_secs_f32(),
                DRAWN.swap(0, std::sync::atomic::Ordering::Relaxed) / self.frames.max(1) as usize,
                ms(self.sum[0]),
                ms(self.sum[1]),
                ms(self.sum[2]),
                ms(self.sum[3])
            );
            *self = Self { since: Some(std::time::Instant::now()), ..Default::default() };
            return true;
        }
        false
    }
}

pub async fn run(game: GameData, opts: ViewerOptions) {
    if game.frames.is_empty() {
        eprintln!("game has no frames to display");
        return;
    }
    let rt = Runtime::with_save_root(game, opts.start_frame, opts.save_dir.clone());
    let save_root = rt.files.save_root.clone();
    let mut v = Viewer {
        last_window_mouse: None,
        shown_frame: rt.frame_log.len(),
        rt,
        textures: Textures::default(),
        inks: InkMaterials::new(),
        fonts: text::Fonts::default(),
        audio: audio_out::AudioOut::new(!opts.mute && opts.screenshot.is_none()),
        offscreen: None,
        scroll: Vec2::ZERO,
        fit: opts.fit,
        show_hidden: opts.show_hidden,
        debug_boxes: false,
        // REKINDLE_HUD=compact|full|inspect overrides (also for screenshots).
        hud: match std::env::var("REKINDLE_HUD").as_deref() {
            Ok("compact") => hud::HudMode::Compact,
            Ok("full") | Ok("inspect") => hud::HudMode::Full,
            _ if opts.screenshot.is_none() && opts.settings.hud => hud::HudMode::Full,
            _ => hud::HudMode::Off,
        },
        inspector: std::env::var("REKINDLE_HUD").as_deref() == Ok("inspect"),
        speed_index: hud::NORMAL_SPEED,
        stats: hud::HudStats::default(),
        hud_font: load_ttf_font_from_bytes(crate::launcher::FONT_REGULAR).ok(),
        screenshot_requested: false,
        running: opts.run_events,
        tick_acc: 0.0,
        input: Input::default(),
        layer_off: Vec::new(),
        drag_from: None,
        settings: opts.settings.clone(),
        fullscreen: false,
        log: crate::log::Log::open(&save_root),
        logged_unimplemented: 0,
        notice: None,
        failed_ticks: 0,
        pad: gamepad::Gamepad::new(&opts.settings),
    };
    // Replacement packs for this game.
    let mods = std::sync::Arc::new(crate::mods::Mods::load(opts.game_path.as_deref().unwrap_or(&v.rt.game.path)));
    if !mods.is_empty() {
        v.log.line(&format!(
            "mods: {} images, {} sounds from {}",
            mods.images.len(),
            mods.sounds.len(),
            mods.packs.join(", ")
        ));
        eprintln!("Mods: {} images, {} sounds ({})", mods.images.len(), mods.sounds.len(), mods.packs.join(", "));
    }
    v.textures.mods = mods.clone();
    v.audio.mods = mods;
    v.on_frame_changed();
    if opts.screenshot.is_none() {
        let g = &v.rt.game;
        let line = format!("started \"{}\" (runtime {:#x}, build {})", g.name, g.runtime_version, g.product_build);
        v.log.line(&line);
        let want = opts.fullscreen.or(v.settings.fullscreen).unwrap_or(g.header.fullscreen_at_start());
        v.set_fullscreen(want);
    }
    for &l in &opts.hidden_layers {
        if let Some(off) = v.layer_off.get_mut(l) {
            *off = true;
        }
    }

    if let (Some(dir), Some(script)) = (&opts.screenshot, &opts.script) {
        let _ = std::fs::create_dir_all(dir);
        let mut input = Input::default();
        for t in 0..=script.last_tick() {
            script.apply(t, &mut input, v.rt.scroll);
            v.rt.tick(&input);
            input.consume_edges();
            if v.rt.frame_log.len() != v.shown_frame {
                v.on_frame_changed();
            }
            for (_, name) in script.shots.iter().filter(|s| s.0 == t) {
                for _ in 0..2 {
                    v.draw();
                    next_frame().await;
                }
                v.draw();
                let path = dir.join(name);
                get_screen_data().export_png(&path.to_string_lossy());
                eprintln!("tick {t}: frame {} \"{}\" -> {}", v.rt.frame_index, v.rt.frame().name, path.display());
            }
            if v.rt.finished {
                eprintln!("tick {t}: game ended");
                break;
            }
        }
        return;
    }

    if let Some(path) = opts.screenshot {
        if opts.run_events {
            for _ in 0..opts.screenshot_ticks {
                v.rt.tick(&Input::default());
                if v.rt.frame_log.len() != v.shown_frame {
                    v.on_frame_changed();
                }
            }
        }
        // A couple of frames so the window has its final size before capturing.
        for _ in 0..3 {
            v.draw();
            next_frame().await;
        }
        v.draw();
        get_screen_data().export_png(&path.to_string_lossy());
        eprintln!(
            "Saved screenshot of frame {} (tick {}) to {}",
            v.rt.frame_index,
            v.rt.tick,
            path.display()
        );
        return;
    }

    // Closing the window ends the loop (instead of exiting at once) so the session is logged.
    prevent_quit();
    let started = chrono::Local::now();
    let clock = std::time::Instant::now();
    // Benchmarking: REKINDLE_WALK_FRAMES=<ms> shows every frame for that long, then exits.
    let walk = std::env::var("REKINDLE_WALK_FRAMES").ok().and_then(|v| v.parse::<f64>().ok());
    let mut walk_next = get_time();
    let mut walk_frame = 0usize;
    // REKINDLE_PROFILE: average time per frame in each part of the loop, every few seconds.
    let mut frame_profile = std::env::var_os("REKINDLE_PROFILE").map(|_| FrameProfile::default());
    loop {
        if let Some(ms) = walk {
            if get_time() >= walk_next {
                if walk_frame >= v.rt.game.frames.len() {
                    break;
                }
                eprintln!("walk: frame {walk_frame}, textures {} MB", v.textures.bytes >> 20);
                v.load_frame(walk_frame);
                walk_frame += 1;
                walk_next = get_time() + ms / 1000.0;
            }
        }
        let ctrl = is_key_down(KeyCode::LeftControl) || is_key_down(KeyCode::RightControl);
        if (ctrl && is_key_pressed(KeyCode::Q)) || is_quit_requested() {
            break;
        }
        // The game's own "end application" closes the window, as in the original runtime.
        if v.rt.finished && v.running {
            break;
        }
        let t0 = std::time::Instant::now();
        v.handle_debug_keys();
        v.gather_input();
        v.advance();
        let t1 = std::time::Instant::now();
        v.audio.process(&mut v.rt).await;
        show_mouse((v.rt.cursor_visible && v.rt.cursor_object.is_none()) || !v.running);
        let t2 = std::time::Instant::now();
        v.draw();
        let t3 = std::time::Instant::now();
        v.stats.record(get_frame_time() * 1000.0, (t1 - t0).as_secs_f32() * 1000.0, (t3 - t2).as_secs_f32() * 1000.0);
        if v.screenshot_requested {
            v.screenshot_requested = false;
            v.save_screenshot();
        }
        next_frame().await;
        if let Some(p) = &mut frame_profile {
            if p.add([t1 - t0, t2 - t1, t3 - t2, t3.elapsed()]) {
                if let Some(ev) = &v.rt.profile {
                    let mut ev = ev.borrow_mut();
                    let mut top: Vec<_> = ev.iter().map(|(k, d)| (*k, *d)).collect();
                    top.sort_by_key(|e| std::cmp::Reverse(e.1 .0));
                    for ((f, g), (d, n)) in top.into_iter().take(5) {
                        eprintln!("  frame {f} event #{g}: {d:.2?} / {n} runs");
                    }
                    ev.clear();
                }
            }
        }
    }
    let secs = clock.elapsed().as_secs();
    let path = opts.game_path.clone().unwrap_or_else(|| v.rt.game.path.clone());
    let title = if v.rt.game.name.trim().is_empty() {
        path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    } else {
        v.rt.game.name.trim().to_string()
    };
    if walk.is_none() {
        crate::launcher::playlog::record(&path, &title, started, secs);
    }
    v.log.line(&format!("session ended after {}", crate::launcher::playlog::duration(secs)));
}

/// Draw an edit box / list / button / combo box at screen position `p`.
fn draw_widget(w: &crate::runtime::widgets::Widget, font: Option<&Font>, p: Vec2, scale: f32) {
    use crate::runtime::widgets::WidgetKind;
    let (ww, wh) = (w.w * scale, w.h * scale);
    let px = w.font_px * scale;
    let size = px.round().clamp(4.0, 200.0) as u16;
    let fg = rgba(w.fg);
    let fg = if w.enabled { fg } else { Color::new(0.5, 0.5, 0.5, 1.0) };
    let text_at = |s: &str, x: f32, y: f32, c: Color| {
        let params = TextParams { font, font_size: size, font_scale: 1.0, color: c, ..Default::default() };
        draw_text_ex(s, x, y, params);
    };
    let inset = 3.0 * scale;
    let ascent = px * 0.85;
    match w.kind {
        WidgetKind::Button => {
            draw_rectangle(p.x, p.y, ww, wh, rgba(w.bg));
            draw_rectangle_lines(p.x, p.y, ww, wh, 1.0, Color::new(0.4, 0.4, 0.4, 1.0));
            let label = w.text.replace('&', "");
            let tw = measure_text(&label, font, size, 1.0).width;
            text_at(&label, p.x + (ww - tw) / 2.0, p.y + (wh - px) / 2.0 + ascent, fg);
        }
        WidgetKind::Edit => {
            draw_rectangle(p.x, p.y, ww, wh, rgba(w.bg));
            draw_rectangle_lines(p.x, p.y, ww, wh, 1.0, Color::new(0.45, 0.45, 0.45, 1.0));
            let lh = px * 1.15;
            let max_lines = ((wh - inset) / lh).floor().max(1.0) as usize;
            let all: Vec<&str> = w.text.split(['\n']).map(|l| l.trim_end_matches('\r')).collect();
            let first = all.len().saturating_sub(max_lines);
            let mut caret = (p.x + inset, p.y + inset);
            for (i, line) in all[first..].iter().enumerate() {
                let y = p.y + inset + lh * i as f32;
                text_at(line, p.x + inset, y + ascent, fg);
                caret = (p.x + inset + measure_text(line, font, size, 1.0).width, y);
            }
            // Blinking caret in the focused box.
            if w.focused && (get_time() * 2.0) as i64 % 2 == 0 {
                draw_line(caret.0 + 1.0, caret.1, caret.0 + 1.0, caret.1 + px, 1.0, fg);
            }
        }
        WidgetKind::List | WidgetKind::Combo => {
            draw_rectangle(p.x, p.y, ww, wh, rgba(w.bg));
            draw_rectangle_lines(p.x, p.y, ww, wh, 1.0, Color::new(0.45, 0.45, 0.45, 1.0));
            let lh = w.line_height() * scale;
            let mut y = p.y + 1.0;
            if w.kind == WidgetKind::Combo {
                text_at(&w.text, p.x + inset, y + ascent, fg);
                y += lh;
            }
            for (k, line) in w.lines.iter().enumerate().skip(w.top) {
                if y + lh > p.y + wh + 0.5 {
                    break;
                }
                if k + 1 == w.current {
                    draw_rectangle(p.x + 1.0, y, ww - 2.0, lh, Color::new(0.2, 0.4, 0.8, 1.0));
                    text_at(line, p.x + inset, y + ascent, WHITE);
                } else {
                    text_at(line, p.x + inset, y + ascent, fg);
                }
                y += lh;
            }
        }
    }
}
