# Clickteam Fusion 2.5 data format notes

Working notes for `ctf_loader`. All integers are little-endian. "Verified" means it was
checked against real Fusion 2.5 builds (ANSI build 258, Unicode builds 289 and 291).

## Container

### `.exe`
1. PE image (the stub runtime). Overlay start = max(`PointerToRawData + SizeOfRawData`) over all sections.
2. Optional pack at the overlay start, magic `wwww`:
   `+8 header_size u32, data_size u32, format u32, 0 u32, 0 u32, count u32`, then from `start + header_size`
   `count` × `{ name_len u16, name[name_len] (ANSI or UTF-16), bingo u32, size u32, data[size] }`.
   It holds runtime DLLs and extensions (`mmfs2.dll`, `*.mfx`, `*.sft`). We list it but don't use it.
3. Game data header. The loader scans for `PAME`/`PAMU` followed by first chunk id `0x2223`. Verified.

### Game data header (16 bytes)
`magic[4]` (`PAME` = ANSI strings, `PAMU` = UTF-16LE strings), `runtime_version u16` (0x0302 = 2.5),
`runtime_subversion u16`, `product_version u32`, `product_build u32`.

## Chunks
`id u16, flags u16, size u32, body[size]`; a list ends at id `0x7F7F`.

| flags | body |
|---|---|
| 0 | raw |
| 1 | `decomp_size u32, comp_size u32, zlib` |
| 2 | encrypted (RC4) |
| 3 | `decomp_size u32`, then encrypted(`comp_size u32, zlib`) |

### Encryption (verified on builds 289 and 291)
- Key bytes: for each UTF-16 unit of the strings, its low byte if non-zero, then its high byte if non-zero.
  - build > 284: AppName (0x2224) + Copyright (0x223B) + EditorFilename (0x222E)
  - otherwise: EditorFilename + AppName + Copyright
  - a missing chunk counts as an empty string
- The cipher is plain RC4 (KSA with those key bytes, then a fresh PRGA for each chunk).
- build > 284: for odd chunk ids, XOR the first encrypted byte with `(id & 0xFF) ^ (id >> 8)` before decrypting.

## Application header (0x2223)
`size u32, flags u16, new_flags u16, graphics_mode u16, other_flags u16, width u16, height u16,
score u32 (stored ^0xFFFFFFFF), lives u32 (^0xFFFFFFFF), control_type u16[4], keys u16[4][8],
border_color rgba, frame_count u32, frame_rate u32, windows_menu_index u8, ...` Verified (window size, fps, frame count).

## Object definitions (0x2229 FrameItems)
`count u32`, then per object a chunk list ending in 0x7F7F:
- `0x4444` header: `handle u16, type i16, flags u16, reserved u16, ink_effect u32, ink_param u32`
- `0x4445` name (string)
- `0x4446` type-specific properties (see below)

Object header ink effect: `effect = ink & 0x0FFF` (0 none, 1 semi-transparent, 2 invert, 3 XOR, 4 AND,
5 OR, 9 add, 11 subtract, 13 shader). Flag `0x1000` means `ink_param` is an RGBA coefficient stored as bytes
B, G, R, A (alpha = opacity). Without it, effect 1 uses `ink_param` 0..128 as transparency (128 = invisible).
Otherwise `ink_param` is uninitialised garbage in older builds; ignore it. Verified.

## Object properties (0x4446)

### Quick Backdrop (type 0)
`size u32, obstacle u16, collision u16, width u32, height u32, border_size u16, border_color rgba, shape u16
(1 line, 2 rectangle, 3 ellipse), fill u16 (0 none, 1 solid, 2 gradient, 3 motif)`, then
line: `line_flags u16` (bit 0 flip X, bit 1 flip Y); solid: `color rgba`; gradient: `color1, color2, vertical u32`;
motif: `image u16` (tiled). Verified.

### Backdrop (type 1)
`size u32, obstacle u16, collision u16, width u32, height u32, image u16`. Verified.

### ObjectCommon (types ≥ 2: Active, String, Counter, extensions, ...)
`size u32`, then an offset table (offsets are relative to the start of the property block, 0 = absent).
Its order differs by build:

| off | build < 284 | build ≥ 284 |
|---|---|---|
| +4 | movements | animations |
| +6 | animations | movements |
| +8 | version | version |
| +10 | counter | ? |
| +12 | system object | extension |
| +14 | (pad) | counter |
| +16 | flags u32 | flags u32 |
| +20 | qualifiers i16×8 (−1 = none) | same |
| +36 | extension | system object |

Then, in both layouts: `+38 values u16, +40 strings u16 (unverified for ≥284), +42 new_flags u16, +44 preferences u16,
+46 identifier[4] ("SPRI", "CNTR", "TEXT", ...), +50 back_color, +54 fade_in u32, +58 fade_out u32`.
- flags: 0x10 movements, 0x20 animations, 0x100 values, 0x200 sprites, 0x800 scrolling independent, 0x10000 text
- new_flags: 0x4 collision box, **0x8 visible at start** (verified: a confirmation prompt that must start hidden lacks it)

Animations (at `animations` offset): `size u16, count u16, offsets i16[count]` (relative to this header).
Each animation: `direction_offsets i16[32]` (relative to the animation). Each direction:
`min_speed u8, max_speed u8, repeat u16 (0 = loop), back_to u16, frame_count u16, image_handles u16[frame_count]`.
Animation 0 is "Stopped"; direction 0 = right, 8 = up, 16 = left, 24 = down. Verified.

Types: 0 QuickBackdrop, 1 Backdrop, 2 Active, 3 String, 4 Q&A, 5 Score, 6 Lives, 7 Counter, 8 RTF,
9 SubApp, ≥32 extension (index = type − 32 into 0x2234). System types used in events: −1 System,
−2 Speaker, −3 Game, −4 Timer, −5 Create, −6 Keyboard, −7 Player.

## Extensions (0x2234)
`count u16, preload u16`, then each `size i16 (abs), handle i16, magic i32, version_ls i32, version_ms i32,
name cstr, subtype cstr`. Skip to `entry_start + size`.

## Frames (0x3333; one per frame, in order)
The body is a chunk list:
- `0x3334` header: `width i32, height i32, background rgba, flags u32`
- `0x3335` name
- `0x3338` instances: `count u32`, then `{ handle u16, object_info u16, x i32, y i32, parent_type i16,
  parent_handle i16, layer i16, reserved i16 }` (20 bytes). Verified: every `object_info` resolves to a 0x4444 handle.
  Entries with `parent_type != 0` (a parent object type plus `parent_handle`, usually at 0,0) are templates kept for
  "create object" actions and are not created when the frame starts. Verified: treating them as placed objects left
  stray copies on screen and blocked "no objects left" checks.
- `0x3341` layers: `count u32`, then `{ flags u32, x_coef f32, y_coef f32, n_backgrounds u32, bg_index u32, name cstr }`.
  Flags: 0x1 / 0x2 X / Y scroll coefficient in use, 0x10 visible. Instances' `layer` indexes this list. Within a layer,
  backdrop-type objects draw behind sprites, and otherwise in instance-list order.
- `0x333D` events (see "Event sheets" below)
- `0x3342` virtual rect, `0x3344` random seed, `0x3347` movement timer base, `0x333B/C` fades

`0x222B` FrameHandles is a list of `u16` mapping frame handle to frame index.

### Other ObjectCommon sub-blocks (offsets from the table above)
- movements: `count u32`, then `count` x 16-byte entries `{ name_offset u32, id u32, data_offset u32, data_size u32 }`
  (offsets relative to the block). Each movement starts with `player i16, type i16, moving_at_start u8, pad[3],
  direction_at_start u32` (32-direction bitmask). Types: 0 static, 1 mouse `{x1,x2,y1,y2}`, 2 race, 3 eight directions
  `{speed, accel, decel, bounce i16; directions u32}`, 4 ball `{speed, randomizer, angles, security, decel}`,
  5 path `{count, min_speed, max_speed i16; loop, reposition, reverse u8; pad}` + steps
  `{pad u8, size u8, speed u8, dir u8, dx i16, dy i16, cos i16, sin i16, length i16, pause i16, name}`,
  9 platform, 14 extension (name at `name_offset`). Verified: types and parameters parse plausibly in all test games.
- counter: `size u16, initial i32, min i32, max i32`. Verified.
- values / strings: `count u16` + `i32[count]` / `count u16` + strings. Unverified: none of the test games store any.
- counter display (Counter, Score, Lives; at the system-object offset): `size u32, width u32, height u32, player u16 (1-based),
  display u16, flags u16, font u16`, then by display type: 0 hidden; 1 numbers and 4 animation: `count u16` + image handles
  (digits 0-9, then `-`, `+`, `.`, `e`); 2 vertical bar, 3 horizontal bar and 5 text: a shape block (as in Quick Backdrops).
  Numbers, animation and text are anchored at their bottom-right corner (the instance position); bars at the top-left. Verified.
- String object (system-object offset): `size u32, width u32, height u32, count u32, offsets u32[count]` (relative to the
  block), each paragraph `font u16, flags u16 (0x1 h-center, 0x2 right, 0x4 v-center, 0x8 bottom), color rgba, text`. Verified.

## Event sheets (0x333D)
Marker blocks: `ER>>` (header), `ERes`, `ERev` (`size u32` + event groups), `<<ER`.

Group: `size i16` (negative), `n_conditions u8, n_actions u8, flags u16`, then 8 bytes (build < 284) or 10 bytes
(newer) of restriction data. The loader picks 14 or 16 bytes by checking that the ACE sizes add up to the group size.
Group flags: 0x1 once, 0x2 not-always, 0x4 repeat, 0x8 no-more, 0x400 OR-in-group, 0x1000 OR-logical, 0x4000 inactive.

Condition: `size u16, object_type i16, num i16, object_info u16, object_info_list i16, flags u8, other_flags u8,
n_params u8, def_type u8, identifier u16` (16 bytes). Actions are the same without `identifier` (14 bytes).
`other_flags & 1` = negated. `object_info & 0x8000` = qualifier. Object types: -1 system, -2 sound, -3 game,
-4 timer, -5 create, -6 keyboard/mouse, -7 player, >= 0 object type (>= 32 extension). Common object conditions are
negative (-1 to -43), common object actions are 1 to 81, and type-specific ones start at +/-80.

Parameter: `size u16, code i16, data`. Codes used by the runtime: 1 object `{list i16, oi u16, type i16}`, 2 time
`{ms i32, loops i32}`, 3/4/10/26/50 short, 5/25/29 int, 6/7 sample `{handle i16, flags u16, name}`, 9 create
(position + `instance u16, oi u16`), 13 every `{ms i32}`, 14/44 key (VK code), 16 position `{parent_oi u16, flags u16,
x i16, y i16, slope i16, angle i16, direction i32, parent_type i16, oi_list i16, layer i16}`, 18 shoot, 19 zone, 24 colour,
32 click `{button u8, double u8}`, 38 group `{flags u16, id u16, name}`, 39 group pointer `{pointer i32, id i16}`,
40/41 string, 42 compare-time, 15/22/23/27/28/45/46/52/53/54/59/62 expression `{comparison i16, tokens...}`.
Comparison: 0 =, 1 <>, 2 <=, 3 <, 4 >=, 5 >.

Expression tokens (infix order): `object_type i16, num i16`; `0,0` ends; otherwise `size u16` and data. Operators are
type 0 (2 +, 4 -, 6 *, 8 /, 10 mod, 12 pow, 14 and, 16 or, 18 xor). System `-1/0` long, `-1/3` string, `-1/23` double,
`-1/24` and `-1/50` global value / string (`pad u32, index i16`), `-1/-1` `(`, `-1/-2` `)`, `-1/-3` `,`. Object tokens
(type >= 2 or -7) carry `oi u16, oi_list i16`; alterable value / string tokens (16 / 19) add `index i16`.
Function arguments follow the function token and end with `)`.

Extension objects (type >= 32) number their own ACEs: condition `num` -81 is the extension's condition 0
(-82 is 1, ...), action 80 is action 0, expression 80 is expression 0. Their meaning depends on the extension
(see `src/runtime/ext/`).

Frame handles (0x222B): `u16[handle] = frame index`; "jump to frame" stores the handle. Verified.

Sources: the IDs and layouts for conditions, actions, expressions and parameters were cross-checked against the
open-source Anaconda parser's published tables (github.com/fnmwolf/Anaconda), then verified against real event
sheets (every parameter parses; usage statistics match the names).

## Extension data formats

### Ini (kcini)
Plain Windows INI text (`[group]`, `item=value`, CRLF). Without a folder, files live in `%APPDATA%\MMFApplications\<name>`
(no extension added). ACE indices as used by games: actions 0 set group, 1 set item, 2 set value, 5 set string,
6 set current file, 7 set value (item), 8 set value (group, item), 9 set string (item), 10 set string (group, item),
11-13 deletes; expressions 0 value, 1 string, 2 value (item), 3 value (group, item), 4 string (item),
5 string (group, item). 0, 6, 7 and expression 2 are verified against real games; the rest follow the object's menu order.

### Global Store X (gstorex)
ACEs (inferred from use): condition 0 compare value(index); actions 0 set, 3 add, 6 subtract, 8 set range (from, to, value),
10 clear, 29 save, 30 and 33 load; expression 0 value(index).
Save file: `u16 3`, then four sections `{u32 count, u8 flag}`; the first is followed by `count` i32 values (200 by default),
the other three are empty in all observed files. Verified: a file written by the original extension parses, and
re-encoding produces the same 822-byte layout.

## Perspective object settings
Extension data block (at ObjectCommon's extension offset): `size u32, max_size u32, version u32, id u32, private u32`,
then the object's settings: `u32 ?, width u16, height u16, effect u8 (0 panorama, 1 perspective, 2 sine wave,
3 sine offset), direction u8 (0 horizontal, 1 vertical), pad u16, zoom i32, offset i32, sine waves i32, ...`.
Action 80 sets the zoom (verified); 81 / 96 and 87 / 88 appear to select sine wave / sine offset and horizontal /
vertical (inferred from use).

## Frame transitions (0x333B / 0x333C)
`module[4]` ("STDT"), `effect[4]` ("FADE", "DOOR", "BAND", ...), `duration_ms i32, flags u32 (1 = to/from a colour),
color rgba, name_offset u32, param_offset u32, param_size u32`, transition DLL name at `name_offset`.

## Multimedia Fusion 1.5 (runtime 0x0301)
The overlay may start with extension-file chunks before the `PAME` header; the build field is not meaningful.
- Compression: `decompressed_size u32` + a deflate variant: 3-bit block type (7 stored, 5 fixed, 6 dynamic) **then**
  the final bit; stored blocks have a 2-byte length only; code-length codes in the order 18, 17, 16, 0, 1 ... 15.
- App header: no size prefix, score / lives not inverted, 6 keys per player, no frame rate (50 fps).
- Frame header: `width u16, height u16, background, flags u16`; instances 12 bytes (`x, y` i16, no layer); one layer.
- ObjectCommon: `size u16, checksum u16, movements, animations, version, counter, system` (u16), `variable u32,
  flags u16`, qualifiers, `extension, values, new_flags, preferences`, identifier, back colour, fades. A single
  movement (`player, type, moving u8, pad[3], direction u32`, then the usual data). Backdrop: `size u32, obstacle,
  collision, image`; Quick Backdrop: u16 width / height and a u16 gradient flag. Counter display: no font field.
  String paragraphs: `size u16, font u16, colour, flags u16, text` with u16 offsets.
- Events: conditions / actions use `object_type i8, num i8` (type-specific numbers +32 to match 2.x: actions >= 48,
  conditions < -48); expression tokens use `i8, i8`, adding 32 to `num >= 48` for object types > 2.
- Images: per entry `handle u32` + compressed block with a 24-byte header (`checksum u16`, no transparent colour).
- Sounds: `handle u32` + compressed `checksum u16, references, size, flags, reserved, name_len` (u32), name,
  WAVEFORMAT (16 bytes), `u16, data_size u32`, PCM. Fonts: compressed, 14-byte header, 16-bit LOGFONT.

## Image bank (0x6666)
`count u32`, then `{ handle u32, decomp u32, comp u32, zlib[comp] }`. The inflated block is:

`checksum i32, references i32, data_size u32, width u16, height u16, graphic_mode u8, flags u8, reserved u16,
hotspot i16×2, action_point i16×2, transparent rgba` (32 bytes), then `data_size` bytes of pixels.

- graphic_mode: 4 = 24-bit BGR, 6 = 15-bit 555, 7 = 16-bit 565 (all verified test games use 7), 8 = 32-bit BGRA
- colour rows are padded to an even byte count (pad in pixels = `ceil((2 - w*ps % 2) / ps)` when non-zero)
- flags: 0x01/0x02/0x04 RLE / RLEW / RLET, 0x08 LZX (data = `size u32 + zlib`), 0x10 alpha plane follows the colour data (rows padded to 4 bytes)
- RLE: `0` ends; `n > 128`: `n − 128` literal points; else `n` repeats of the next point. Verified.
- without an alpha plane, pixels matching the transparent colour become transparent
- Handles: builds ≥ 284 store bank handles 1-based, while objects reference them 0-based. Build 258 is 0-based throughout.
  The loader subtracts 1 for ≥ 284 (verified: 100% of animation/backdrop references then resolve).

## Sound / music banks (0x6668 / 0x6669)
`count u32`, then `{ handle u32, checksum i32, references u32, decomp u32, flags u32, reserved u32, name_len u32 }`,
then either `comp u32 + zlib[comp]` or, when `flags == 33`, `decomp` raw bytes. The payload is
`name[name_len chars]` followed by the audio file (sniff `RIFF`, `OggS`, `ID3`, `MThd`). Verified (WAV, MP3).

Sound handles follow the same rule as images: builds >= 284 store them 1-based (verified by matching the sample names
stored in event parameters to bank names).

## Font bank (0x6667)
`count u32`, then `{ handle u32, decomp u32, comp u32, zlib }` → `checksum, references, size` (i32) + LOGFONT
(`height, width, escapement, orientation, weight` i32, `italic, underline, strikeout, charset, out_prec,
clip_prec, quality, pitch` u8, `face_name[32]`). Handles are 1-based from build 284, like images and sounds. Only
the description is stored, never the font file.

## Not handled yet
- `0x223F` FrameItems2 and the `0x2253/0x2256/0x2257` split object chunks (newer/other exporters)
- palette-based (mode 3) images
- MFA source files (out of scope)
