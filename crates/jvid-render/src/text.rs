//! Text/title layers: shaped and rasterized with `cosmic-text` into a straight RGBA image the
//! size of the frame, so the compositor's fit fills the frame and transforms, crop and
//! opacity apply as to any clip.

use std::collections::BTreeMap;
use std::io;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight};
use jvid_core::model::{Align, Text};
use wgpu::util::DeviceExt;

/// A style and frame size.
type Key = (Text, (u32, u32));

/// Glyph coverage and the outline-grown shape, one byte per pixel, and the shape's bounds.
struct Masks {
    fill: Vec<u8>,
    shape: Vec<u8>,
    bounds: Option<[u32; 4]>,
}

struct Fonts {
    // ponytail: retain loaded project faces until exit for concurrent and queued exports.
    project_sources: BTreeMap<String, Vec<FontSource>>,
    project_content: BTreeMap<String, Vec<FontContent>>,
    system: FontSystem,
    swash: SwashCache,
    /// Recent rasters, newest last.
    // shortcut: a linear scan over 8 entries; key by hash if many text layers show at once.
    recent: Vec<(Key, Arc<Vec<u8>>)>,
    /// Recent masks keyed by colorless styles, so animated colors only re-tint.
    masks: Vec<(Key, Arc<Masks>)>,
}

/// Faces loaded after the system fonts, set by [`set_faces`].
static FACES: OnceLock<Vec<&'static [u8]>> = OnceLock::new();

/// Adds `faces` after the system fonts, so an installed face wins. The app passes the suite's
/// Noto Sans CJK faces, which draw translated title presets on systems without a CJK font. Call
/// it before the fonts load ([`warm`]); later calls are ignored.
pub fn set_faces(faces: Vec<&'static [u8]>) {
    let _ = FACES.set(faces);
}

fn sources(faces: &[&'static [u8]]) -> impl Iterator<Item = cosmic_text::fontdb::Source> {
    faces
        .iter()
        .map(|&b| cosmic_text::fontdb::Source::Binary(Arc::new(b)))
}

fn fonts() -> &'static Mutex<Fonts> {
    static FONTS: OnceLock<Mutex<Fonts>> = OnceLock::new();
    FONTS.get_or_init(|| {
        Mutex::new(Fonts::new(FontSystem::new_with_fonts(sources(
            FACES.get().map_or(&[], |f| f),
        ))))
    })
}

/// A font file whose owner explicitly allowed project bundling. `family` is a
/// content-derived private alias, so an installed face cannot override collected bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontSource {
    pub family: String,
    pub original_family: String,
    pub path: PathBuf,
}

/// The immutable bytes and selected faces behind a private alias. Paths may move;
/// content, face order and provenance cannot change while queued exports use it.
#[derive(PartialEq)]
struct FontContent {
    original_family: String,
    bytes: Arc<Vec<u8>>,
    indices: Vec<u32>,
}

fn read_font_faces(
    source: &FontSource,
) -> io::Result<(Arc<Vec<u8>>, Vec<cosmic_text::fontdb::FaceInfo>)> {
    let bytes = Arc::new(std::fs::read(&source.path)?);
    let mut database = cosmic_text::fontdb::Database::new();
    let ids = database.load_font_source(cosmic_text::fontdb::Source::Binary(bytes.clone()));
    if ids.is_empty() {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    let faces = ids
        .into_iter()
        .filter_map(|id| database.face(id))
        .filter(|face| {
            source.original_family.is_empty()
                || face
                    .families
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case(&source.original_family))
        })
        .cloned()
        .collect();
    Ok((bytes, faces))
}

fn alias_faces(
    database: &mut cosmic_text::fontdb::Database,
    faces: Vec<cosmic_text::fontdb::FaceInfo>,
    alias: &str,
) {
    for mut face in faces {
        for (name, _) in &mut face.families {
            *name = alias.to_owned();
        }
        database.push_face_info(face);
    }
}

impl Fonts {
    fn new(system: FontSystem) -> Self {
        Self {
            project_sources: BTreeMap::new(),
            project_content: BTreeMap::new(),
            system,
            swash: SwashCache::new(),
            recent: Vec::new(),
            masks: Vec::new(),
        }
    }
}

/// Proves that cold reopening with only permitted faces preserves actual shaping.
/// Unsupported fallback/face unions remain explicitly unresolved.
pub fn project_font_coverage(
    sources: &[FontSource],
    texts: &[Text],
    size: (u32, u32),
) -> io::Result<bool> {
    let mut fonts = fonts().lock().unwrap_or_else(|e| e.into_inner());
    project_font_coverage_with(&mut fonts, sources, texts, size)
}

fn project_font_coverage_with(
    fonts: &mut Fonts,
    sources: &[FontSource],
    texts: &[Text],
    size: (u32, u32),
) -> io::Result<bool> {
    let alias = "jvid-collection-cold-coverage";
    let mut database = cosmic_text::fontdb::Database::new();
    for source in sources {
        let (_, faces) = read_font_faces(source)?;
        // A valid permitted fallback file may have a different family from the UI's
        // requested family. Report coverage, rather than a generic invalid-file error.
        if faces.is_empty() {
            return Ok(false);
        }
        alias_faces(&mut database, faces, alias);
    }
    let mut packaged = Fonts::new(FontSystem::new_with_locale_and_db(
        fonts.system.locale().to_owned(),
        database,
    ));
    for text in texts {
        if text.content.trim().is_empty() {
            continue;
        }
        let (original, wrap, offset) = layout(fonts, text, size);
        let collected = Text {
            family: alias.into(),
            ..text.clone()
        };
        let (cold, cold_wrap, cold_offset) = layout(&mut packaged, &collected, size);
        let Some(primary) = packaged.system.db().query(&cosmic_text::fontdb::Query {
            families: &[Family::Name(alias)],
            weight: Weight(text.weight),
            ..Default::default()
        }) else {
            return Ok(false);
        };
        // cosmic-text tries installed script/common fallbacks before secondary alias
        // faces, and requires an exact weight for its named-family first choice. Those
        // paths cannot be made portable with the current single-family representation.
        if packaged
            .system
            .db()
            .face(primary)
            .is_none_or(|face| face.weight.0 != text.weight)
            || cold
                .layout_runs()
                .flat_map(|run| run.glyphs)
                .any(|glyph| glyph.font_id != primary)
        {
            return Ok(false);
        }
        if wrap != cold_wrap
            || offset != cold_offset
            || !same_shaping(&fonts.system, &original, &packaged.system, &cold)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn same_shaping(original: &FontSystem, a: &Buffer, packaged: &FontSystem, b: &Buffer) -> bool {
    let left: Vec<_> = a.layout_runs().collect();
    let right: Vec<_> = b.layout_runs().collect();
    if left.is_empty() || left.len() != right.len() || left.iter().all(|run| run.glyphs.is_empty())
    {
        return false;
    }
    let mut faces = BTreeMap::new();
    left.iter().zip(&right).all(|(a, b)| {
        (
            a.line_i,
            a.rtl,
            [a.line_y, a.line_top, a.line_height, a.line_w],
        ) == (
            b.line_i,
            b.rtl,
            [b.line_y, b.line_top, b.line_height, b.line_w],
        ) && a.glyphs.len() == b.glyphs.len()
            && a.glyphs.iter().zip(b.glyphs).all(|(a, b)| {
                a.glyph_id != 0
                    && (
                        a.start,
                        a.end,
                        a.glyph_id,
                        a.font_size,
                        a.font_weight,
                        a.line_height_opt,
                        a.level,
                        [a.x, a.y, a.w, a.x_offset, a.y_offset],
                        a.cache_key_flags,
                    ) == (
                        b.start,
                        b.end,
                        b.glyph_id,
                        b.font_size,
                        b.font_weight,
                        b.line_height_opt,
                        b.level,
                        [b.x, b.y, b.w, b.x_offset, b.y_offset],
                        b.cache_key_flags,
                    )
                    && *faces.entry((a.font_id, b.font_id)).or_insert_with(|| {
                        original
                            .db()
                            .with_face_data(a.font_id, |data, index| {
                                packaged
                                    .db()
                                    .with_face_data(b.font_id, |cold_data, cold_index| {
                                        index == cold_index && data == cold_data
                                    })
                                    .unwrap_or(false)
                            })
                            .unwrap_or(false)
                    })
            })
    })
}

/// Validates every alias before changing renderer state; retains loaded resources for
/// queued exports. Moving identical bytes is allowed; reusing an alias for different
/// bytes, selected faces or provenance is rejected even with a warm cache.
pub fn load_project_fonts(sources: &[FontSource]) -> io::Result<()> {
    let mut parsed = Vec::new();
    let mut identities: BTreeMap<String, Vec<FontContent>> = BTreeMap::new();
    for source in sources {
        if !source.family.starts_with("jvid-collection-") {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        let (bytes, faces) = read_font_faces(source)?;
        if faces.is_empty() {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        identities
            .entry(source.family.clone())
            .or_default()
            .push(FontContent {
                original_family: source.original_family.clone(),
                bytes,
                indices: faces.iter().map(|face| face.index).collect(),
            });
        parsed.push((source.clone(), faces));
    }
    if parsed.is_empty() {
        return Ok(());
    }
    let mut fonts = fonts().lock().unwrap_or_else(|e| e.into_inner());
    for (alias, identity) in &identities {
        if fonts
            .project_content
            .get(alias)
            .is_some_and(|loaded| loaded != identity)
        {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
    }
    let mut changed = false;
    let mut current: BTreeMap<String, Vec<FontSource>> = BTreeMap::new();
    for (source, faces) in parsed {
        if !fonts.project_sources.contains_key(&source.family) {
            alias_faces(fonts.system.db_mut(), faces, &source.family);
            changed = true;
        }
        current
            .entry(source.family.clone())
            .or_default()
            .push(source);
    }
    fonts.project_sources.extend(current);
    fonts.project_content.extend(identities);
    if changed {
        fonts.recent.clear();
        fonts.masks.clear();
        fonts.swash = SwashCache::new();
    }
    Ok(())
}

/// Display a collected family's original name while retaining its private render key.
pub fn display_family(family: &str) -> String {
    if !family.starts_with("jvid-collection-") {
        return family.to_owned();
    }
    let fonts = fonts().lock().unwrap_or_else(|e| e.into_inner());
    fonts
        .project_sources
        .get(family)
        .and_then(|s| s.first())
        .map_or_else(|| family.to_owned(), |s| s.original_family.clone())
}

/// Permission/provenance for private aliases used in a project, retained by Save As.
pub fn project_font_sources(families: &[String]) -> Vec<FontSource> {
    if !families.iter().any(|f| f.starts_with("jvid-collection-")) {
        return Vec::new();
    }
    let fonts = fonts().lock().unwrap_or_else(|e| e.into_inner());
    families
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter_map(|family| fonts.project_sources.get(family))
        .flatten()
        .cloned()
        .collect()
}

/// Loads the system fonts, which takes seconds on some systems; call it off the UI thread
/// before the first text frame.
pub fn warm() {
    drop(fonts().lock());
}

/// The font families the renderer can draw, sorted and deduplicated; loads the fonts on first
/// call, like [`warm`].
pub fn families() -> &'static [String] {
    static LIST: OnceLock<Vec<String>> = OnceLock::new();
    LIST.get_or_init(|| {
        let f = fonts().lock().unwrap_or_else(|e| e.into_inner());
        let mut v: Vec<String> = f
            .system
            .db()
            .faces()
            .filter_map(|face| face.families.first().map(|(n, _)| n.clone()))
            .filter(|name| !name.starts_with("jvid-collection-"))
            .collect();
        v.sort_by_key(|n| n.to_lowercase());
        v.dedup();
        v
    })
}

/// Pushes `v` under `k`, keeping the newest 8.
fn remember<T>(list: &mut Vec<(Key, T)>, k: Key, v: T) {
    if list.len() >= 8 {
        list.remove(0);
    }
    list.push((k, v));
}

/// `text` shaped for a `size` frame, the wrap width, and the offset from buffer to frame pixels.
fn layout(f: &mut Fonts, text: &Text, size: (u32, u32)) -> (Buffer, f32, [i32; 2]) {
    let (w, h) = size;
    let metrics = Metrics::new(text.size.max(1.0), text.size.max(1.0) * 1.2);
    let mut buf = Buffer::new(&mut f.system, metrics);
    let attrs = Attrs::new()
        .weight(Weight(text.weight))
        .family(if text.family.is_empty() {
            Family::SansSerif
        } else {
            Family::Name(&text.family)
        });
    let align = match text.align {
        Align::Left => cosmic_text::Align::Left,
        Align::Center => cosmic_text::Align::Center,
        Align::Right => cosmic_text::Align::Right,
    };
    // Wrap so a block centered on the anchor stays in frame; lines align within the widest.
    let ax = text.anchor[0].clamp(0.0, 1.0);
    let wrap = (2.0 * ax.min(1.0 - ax) * w as f32).max(text.size);
    buf.set_size(&mut f.system, Some(wrap), None);
    buf.set_text(
        &mut f.system,
        &text.content,
        &attrs,
        Shaping::Advanced,
        Some(align),
    );
    buf.shape_until_scroll(&mut f.system, false);
    let runs: Vec<_> = buf
        .layout_runs()
        .map(|r| (r.line_w, r.line_top + r.line_height))
        .collect();
    let block_w = runs.iter().map(|r| r.0).fold(0.0, f32::max);
    let block_h = runs.last().map_or(0.0, |r| r.1);
    // Alignment places lines across the full wrap width; shift the widest to the anchor.
    let left = match text.align {
        Align::Left => 0.0,
        Align::Center => (wrap - block_w) / 2.0,
        Align::Right => wrap - block_w,
    };
    let dx = (text.anchor[0] * w as f32 - block_w / 2.0 - left).round() as i32;
    let dy = (text.anchor[1] * h as f32 - block_h / 2.0).round() as i32;
    (buf, wrap, [dx, dy])
}

/// Glyph coverage of `text` laid out on a `size` frame: one byte per pixel.
fn coverage(f: &mut Fonts, text: &Text, size: (u32, u32)) -> Vec<u8> {
    let (w, h) = size;
    let (buf, _, [dx, dy]) = layout(f, text, size);
    let mut out = vec![0u8; (w * h) as usize];
    let Fonts { system, swash, .. } = f;
    buf.draw(
        system,
        swash,
        Color::rgb(255, 255, 255),
        |x, y, gw, gh, c| {
            for (yy, xx) in (0..gh as i32).flat_map(|j| (0..gw as i32).map(move |i| (j, i))) {
                let (px, py) = (x + xx + dx, y + yy + dy);
                if (0..w as i32).contains(&px) && (0..h as i32).contains(&py) {
                    let o = &mut out[(py as u32 * w + px as u32) as usize];
                    *o = (*o).max(c.a());
                }
            }
        },
    );
    out
}

/// `mask` grown by `r` pixels: the max over a disc.
fn dilate(mask: &[u8], (w, h): (u32, u32), r: f32) -> Vec<u8> {
    let ri = r.ceil() as i32;
    let disc: Vec<_> = (-ri..=ri)
        .flat_map(|j| (-ri..=ri).map(move |i| (i, j)))
        .filter(|(i, j)| ((i * i + j * j) as f32) <= r * r)
        .collect();
    let mut out = mask.to_vec();
    for (k, &m) in mask.iter().enumerate().filter(|(_, m)| **m > 0) {
        let (x, y) = ((k as u32 % w) as i32, (k as u32 / w) as i32);
        for (i, j) in &disc {
            let (px, py) = (x + i, y + j);
            if (0..w as i32).contains(&px) && (0..h as i32).contains(&py) {
                let o = &mut out[(py * w as i32 + px) as usize];
                *o = (*o).max(m);
            }
        }
    }
    out
}

/// Bounding box `[x0, y0, x1, y1)` of nonzero coverage.
fn bounds(mask: &[u8], w: u32) -> Option<[u32; 4]> {
    let mut b: Option<[u32; 4]> = None;
    for (k, _) in mask.iter().enumerate().filter(|(_, m)| **m > 0) {
        let (x, y) = (k as u32 % w, k as u32 / w);
        let e = b.get_or_insert([x, y, x + 1, y + 1]);
        *e = [e[0].min(x), e[1].min(y), e[2].max(x + 1), e[3].max(y + 1)];
    }
    b
}

/// The masks of `text` on a `size` frame, cached by its colorless style.
fn masks(f: &mut Fonts, text: &Text, size: (u32, u32)) -> Arc<Masks> {
    let key = (
        Text {
            color: [0.0; 4],
            outline_color: [0.0; 4],
            shadow_color: [0.0; 4],
            box_color: [0.0; 4],
            ..text.clone()
        },
        size,
    );
    if let Some((_, m)) = f.masks.iter().find(|(k, _)| *k == key) {
        return m.clone();
    }
    let fill = coverage(f, text, size);
    let shape = if text.outline > 0.0 {
        dilate(&fill, size, text.outline)
    } else {
        fill.clone()
    };
    let bounds = bounds(&shape, size.0);
    let m = Arc::new(Masks {
        fill,
        shape,
        bounds,
    });
    remember(&mut f.masks, key, m.clone());
    m
}

/// `text` on a transparent `size` frame, straight RGBA8. `fill` overrides `text.color`, so a
/// clip's `color` keyframes animate it.
pub fn rasterize(text: &Text, fill: Option<[f64; 4]>, size: (u32, u32)) -> Arc<Vec<u8>> {
    let text = Text {
        color: fill.unwrap_or(text.color),
        ..text.clone()
    };
    let key = (text, size);
    let mut f = fonts().lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, img)) = f.recent.iter().find(|(k, _)| *k == key) {
        return img.clone();
    }
    let (text, (w, h)) = &key;
    let (w, h) = (*w as i64, *h as i64);
    let m = masks(&mut f, text, size);
    let mut img = vec![0u8; (w * h * 4) as usize];
    let Some([x0, y0, x1, y1]) = m.bounds.map(|b| b.map(i64::from)) else {
        let img = Arc::new(img);
        remember(&mut f.recent, key, img.clone());
        return img;
    };
    let [sx, sy] = text.shadow.map(|v| v.round() as i64);
    let p = padding(text);
    let (rx, ry) = reach(text, [x0, y0, x1, y1], (w, h));
    let at = |m: &[u8], x: i64, y: i64| {
        if (0..w).contains(&x) && (0..h).contains(&y) {
            m[(y * w + x) as usize] as f32 / 255.0
        } else {
            0.0
        }
    };
    for y in ry {
        for x in rx.clone() {
            // Premultiplied layers, bottom first: box, shadow, outline, fill.
            let mut px = [0f32; 4];
            let mut over = |a: f32, c: [f64; 4]| {
                let a = a * c[3] as f32;
                if a > 0.0 {
                    for i in 0..3 {
                        px[i] = c[i] as f32 * a + px[i] * (1.0 - a);
                    }
                    px[3] = a + px[3] * (1.0 - a);
                }
            };
            let inside = (x0 - p..x1 + p).contains(&x) && (y0 - p..y1 + p).contains(&y);
            over(inside as u8 as f32, text.box_color);
            if text.shadow != [0.0, 0.0] {
                over(at(&m.shape, x - sx, y - sy), text.shadow_color);
            }
            if text.outline > 0.0 {
                over(at(&m.shape, x, y), text.outline_color);
            }
            over(at(&m.fill, x, y), text.color);
            let a = px[3];
            let un = |v: f32| if a > 0.0 { v / a } else { 0.0 };
            let rgba = [un(px[0]), un(px[1]), un(px[2]), a];
            let o = ((y * w + x) * 4) as usize;
            for (d, v) in img[o..o + 4].iter_mut().zip(rgba) {
                *d = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
    let img = Arc::new(img);
    remember(&mut f.recent, key, img.clone());
    img
}

/// The box padding drawn around the glyphs, 0 without a box.
fn padding(text: &Text) -> i64 {
    if text.box_color[3] > 0.0 {
        text.box_padding.max(0.0) as i64
    } else {
        0
    }
}

/// The pixels the box, shadow and glyphs within `[x0, y0, x1, y1)` can reach on a `w`x`h` frame.
fn reach(text: &Text, [x0, y0, x1, y1]: [i64; 4], (w, h): (i64, i64)) -> (Range<i64>, Range<i64>) {
    let [sx, sy] = text.shadow.map(|v| v.round() as i64);
    let p = padding(text);
    let rx = (x0.min(x0 + sx) - p).max(0)..(x1.max(x1 + sx) + p).min(w);
    let ry = (y0.min(y0 + sy) - p).max(0)..(y1.max(y1 + sy) + p).min(h);
    (rx, ry)
}

/// The pixel rect `[x0, y0, x1, y1)` [`rasterize`] draws `text` into on a `size` frame;
/// `None` when nothing shows.
pub fn drawn_bounds(text: &Text, size: (u32, u32)) -> Option<[u32; 4]> {
    let mut f = fonts().lock().unwrap_or_else(|e| e.into_inner());
    let b = masks(&mut f, text, size).bounds?.map(i64::from);
    let (rx, ry) = reach(text, b, (size.0 as i64, size.1 as i64));
    (!rx.is_empty() && !ry.is_empty())
        .then(|| [rx.start, ry.start, rx.end, ry.end].map(|v| v as u32))
}

/// Byte offsets where each line (paragraph) of `s` starts.
fn line_starts(s: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(s.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

/// Where a caret before each char of `text.content`, and after the last, sits on a `size`
/// frame: `[x, top, bottom]` in frame pixels, one more than there are chars.
// shortcut: left-to-right only; a caret in right-to-left text sits mirrored.
pub fn carets(text: &Text, size: (u32, u32)) -> Vec<[f32; 3]> {
    let s = &text.content;
    let starts = line_starts(s);
    let mut f = fonts().lock().unwrap_or_else(|e| e.into_inner());
    let (buf, wrap, [dx, dy]) = layout(&mut f, text, size);
    let empty = match text.align {
        Align::Left => 0.0,
        Align::Center => wrap / 2.0,
        Align::Right => wrap,
    };
    let runs: Vec<_> = buf.layout_runs().collect();
    let bytes = s.char_indices().map(|(i, _)| i).chain([s.len()]);
    bytes
        .map(|byte| {
            let line = starts.partition_point(|&b| b <= byte) - 1;
            let index = byte - starts[line];
            let mut out = [empty, 0.0, text.size];
            for run in runs.iter().filter(|r| r.line_i == line) {
                let end = run.glyphs.last().map_or(empty, |g| g.x + g.w);
                out = [end, run.line_top, run.line_top + run.line_height];
                // The first glyph ending past the caret starts at it; none means a later run.
                if let Some(g) = run.glyphs.iter().find(|g| g.end > index) {
                    out[0] = g.x;
                    break;
                }
            }
            [out[0] + dx as f32, out[1] + dy as f32, out[2] + dy as f32]
        })
        .collect()
}

/// The char of `text.content` nearest frame pixel `[x, y]` on a `size` frame, as a caret index.
pub fn hit(text: &Text, size: (u32, u32), [x, y]: [f32; 2]) -> usize {
    let mut f = fonts().lock().unwrap_or_else(|e| e.into_inner());
    let (buf, _, [dx, dy]) = layout(&mut f, text, size);
    let Some(c) = buf.hit(x - dx as f32, y - dy as f32) else {
        return 0;
    };
    let s = &text.content;
    let byte = line_starts(s).get(c.line).map_or(s.len(), |b| b + c.index);
    s[..byte.min(s.len())].chars().count()
}

/// A texture of [`rasterize`]'s image, in [`crate::composite::FORMAT`].
pub fn texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: (u32, u32),
    rgba: &[u8],
) -> wgpu::Texture {
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("text"),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: crate::composite::FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        Default::default(),
        rgba,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIMARY: &str = "Jvid Fixture Primary";
    const FALLBACK: &str = "Jvid Fixture Fallback";

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/fonts")
            .join(name)
    }

    fn fixture_fonts() -> Fonts {
        let mut db = cosmic_text::fontdb::Database::new();
        db.load_font_file(fixture("primary.ttf")).unwrap();
        db.load_font_file(fixture("fallback.ttf")).unwrap();
        Fonts::new(FontSystem::new_with_locale_and_db("en-US".into(), db))
    }

    fn fixture_source(name: &str, original: &str) -> FontSource {
        FontSource {
            family: PRIMARY.into(),
            original_family: original.into(),
            path: fixture(name),
        }
    }

    #[test]
    fn portable_primary_preserves_cold_pixels_with_different_installed_fonts() {
        let mut original = fixture_fonts();
        let text = Text {
            content: "AB\n\nA".into(),
            family: PRIMARY.into(),
            weight: 400,
            ..Text::default()
        };
        let source = fixture_source("primary.ttf", PRIMARY);
        assert!(
            project_font_coverage_with(
                &mut original,
                std::slice::from_ref(&source),
                std::slice::from_ref(&text),
                (640, 360)
            )
            .unwrap()
        );
        let expected = masks(&mut original, &text, (640, 360));
        assert!(
            expected.fill.iter().any(|alpha| *alpha > 0),
            "fixture must rasterize visible glyphs"
        );
        let mut db = cosmic_text::fontdb::Database::new();
        // A cold renderer has a conflicting installed primary face and no original fallback.
        db.load_font_file(fixture("alternate.ttf")).unwrap();
        let alias = "jvid-collection-cold-pixels-fixture";
        let (_, faces) = read_font_faces(&source).unwrap();
        alias_faces(&mut db, faces, alias);
        let mut cold = Fonts::new(FontSystem::new_with_locale_and_db("ja-JP".into(), db));
        let actual = masks(
            &mut cold,
            &Text {
                family: alias.into(),
                ..text
            },
            (640, 360),
        );
        assert_eq!(expected.fill, actual.fill);
        assert_eq!(expected.bounds, actual.bounds);
    }

    #[test]
    fn portable_ttc_coverage_rejects_changed_face_order() {
        let mut db = cosmic_text::fontdb::Database::new();
        db.load_font_file(fixture("faces.ttc")).unwrap();
        let mut original = Fonts::new(FontSystem::new_with_locale_and_db("en-US".into(), db));
        let text = Text {
            content: "AΩ".into(),
            family: PRIMARY.into(),
            weight: 400,
            ..Text::default()
        };
        let sources = [
            fixture_source("faces.ttc", FALLBACK),
            fixture_source("faces.ttc", PRIMARY),
        ];
        let (_, primary) = read_font_faces(&sources[1]).unwrap();
        let (_, fallback) = read_font_faces(&sources[0]).unwrap();
        assert_eq!(primary[0].index, 0);
        assert_eq!(fallback[0].index, 1);
        assert!(!project_font_coverage_with(&mut original, &sources, &[text], (320, 180)).unwrap());
    }

    #[test]
    fn warm_alias_rejects_face_order_and_provenance_before_any_mutation() {
        let alias = "jvid-collection-warm-order-fixture";
        let primary = FontSource {
            family: alias.into(),
            ..fixture_source("primary.ttf", PRIMARY)
        };
        let fallback = FontSource {
            family: alias.into(),
            ..fixture_source("fallback.ttf", FALLBACK)
        };
        load_project_fonts(&[primary.clone(), fallback.clone()]).unwrap();
        assert!(load_project_fonts(&[fallback.clone(), primary.clone()]).is_err());
        let new_alias = "jvid-collection-rejected-batch-fixture";
        let unrelated = FontSource {
            family: new_alias.into(),
            ..primary.clone()
        };
        let relabeled = FontSource {
            original_family: String::new(),
            ..primary.clone()
        };
        assert!(load_project_fonts(&[unrelated, relabeled, fallback.clone()]).is_err());
        assert!(project_font_sources(&[new_alias.into()]).is_empty());
        assert_eq!(
            project_font_sources(&[alias.into()]),
            vec![primary, fallback]
        );
    }

    #[test]
    fn portable_coverage_rejects_installed_fallback_preemption() {
        let mut original = fixture_fonts();
        let text = Text {
            content: "AΩ".into(),
            family: PRIMARY.into(),
            weight: 400,
            ..Text::default()
        };
        let sources = [
            fixture_source("primary.ttf", PRIMARY),
            fixture_source("fallback.ttf", FALLBACK),
        ];
        let alias = "jvid-collection-installed-fallback-fixture";
        let cold = |installed| {
            let mut db = cosmic_text::fontdb::Database::new();
            for source in &sources {
                let (_, faces) = read_font_faces(source).unwrap();
                alias_faces(&mut db, faces, alias);
            }
            if installed {
                db.load_font_file(fixture("environment.ttf")).unwrap();
            }
            Fonts::new(FontSystem::new_with_locale_and_db("en-US".into(), db))
        };
        let collected = Text {
            family: alias.into(),
            ..text.clone()
        };
        let mut isolated = cold(false);
        let mut changed_environment = cold(true);
        let expected = masks(&mut original, &text, (320, 180));
        assert!(
            expected.fill.iter().any(|alpha| *alpha > 0),
            "fixture must rasterize visible glyphs"
        );
        assert_eq!(
            expected.fill,
            masks(&mut isolated, &collected, (320, 180)).fill
        );
        assert!(
            expected.fill != masks(&mut changed_environment, &collected, (320, 180)).fill,
            "different installed fallback must change visible glyph pixels"
        );
        assert!(!project_font_coverage_with(&mut original, &sources, &[text], (320, 180)).unwrap());
    }

    #[test]
    fn portable_coverage_rejects_weight_that_requires_system_fallback() {
        let mut original = fixture_fonts();
        let text = Text {
            content: "AB".into(),
            family: PRIMARY.into(),
            weight: 500,
            ..Text::default()
        };
        assert!(
            !project_font_coverage_with(
                &mut original,
                &[fixture_source("primary.ttf", PRIMARY)],
                &[text],
                (320, 180)
            )
            .unwrap()
        );
    }

    #[test]
    fn portable_coverage_rejects_primary_changes_from_alias_face_union() {
        let mut original = fixture_fonts();
        let text = Text {
            content: "AΩ".into(),
            family: PRIMARY.into(),
            weight: 400,
            ..Text::default()
        };
        // Both actual faces are present, but fallback-first aliasing changes Latin A.
        let sources = [
            fixture_source("fallback.ttf", FALLBACK),
            fixture_source("primary.ttf", PRIMARY),
        ];
        assert!(!project_font_coverage_with(&mut original, &sources, &[text], (320, 180)).unwrap());
    }

    #[test]
    fn portable_coverage_reports_requested_family_on_fallback_file_as_unresolved() {
        let mut original = fixture_fonts();
        let text = Text {
            content: "AΩ".into(),
            family: PRIMARY.into(),
            weight: 400,
            ..Text::default()
        };
        let sources = [
            fixture_source("primary.ttf", PRIMARY),
            fixture_source("fallback.ttf", PRIMARY),
        ];
        assert!(!project_font_coverage_with(&mut original, &sources, &[text], (320, 180)).unwrap());
    }

    #[test]
    fn warm_alias_rejects_changed_font_bytes_and_preserves_queued_resources() {
        let alias = "jvid-collection-warm-fixture";
        let path = std::env::temp_dir().join(format!("jvid-warm-font-{}.ttf", std::process::id()));
        std::fs::copy(fixture("primary.ttf"), &path).unwrap();
        let first = FontSource {
            family: alias.into(),
            original_family: PRIMARY.into(),
            path: path.clone(),
        };
        load_project_fonts(std::slice::from_ref(&first)).unwrap();
        let text = Text {
            content: "AB".into(),
            family: alias.into(),
            weight: 400,
            ..Text::default()
        };
        let before = rasterize(&text, None, (320, 180));
        assert!(
            before.as_chunks::<4>().0.iter().any(|pixel| pixel[3] > 0),
            "queued resource must contain visible glyphs"
        );
        std::fs::copy(fixture("alternate.ttf"), &path).unwrap();
        assert!(load_project_fonts(std::slice::from_ref(&first)).is_err());
        assert_eq!(project_font_sources(&[alias.into()]), vec![first]);
        assert_eq!(before, rasterize(&text, None, (320, 180)));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn warm_alias_accepts_identical_relocated_bytes_after_original_disappears() {
        let root = std::env::temp_dir().join(format!("jvid-font-relocate-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let old = root.join("original.ttf");
        let moved = root.join("moved.ttf");
        std::fs::copy(fixture("primary.ttf"), &old).unwrap();
        let alias = "jvid-collection-relocation-fixture";
        let source = FontSource {
            family: alias.into(),
            original_family: PRIMARY.into(),
            path: old.clone(),
        };
        load_project_fonts(std::slice::from_ref(&source)).unwrap();
        let text = Text {
            content: "AB".into(),
            family: alias.into(),
            weight: 400,
            ..Text::default()
        };
        let before = rasterize(&text, None, (320, 180));
        assert!(
            before.as_chunks::<4>().0.iter().any(|pixel| pixel[3] > 0),
            "queued resource must contain visible glyphs"
        );
        std::fs::rename(old, &moved).unwrap();
        let relocated = FontSource {
            path: moved,
            ..source
        };
        load_project_fonts(std::slice::from_ref(&relocated)).unwrap();
        assert_eq!(project_font_sources(&[alias.into()]), vec![relocated]);
        assert!(Arc::ptr_eq(&before, &rasterize(&text, None, (320, 180))));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn collected_font_alias_wins_over_same_named_installed_face() {
        use cosmic_text::fontdb::{Family, Query, Source};
        let regular = include_bytes!("../../suite-ui/assets/Inter-Regular.ttf").to_vec();
        let medium = include_bytes!("../../suite-ui/assets/Inter-Medium.ttf").to_vec();
        let path =
            std::env::temp_dir().join(format!("jvid-collected-font-{}.ttf", std::process::id()));
        std::fs::write(&path, &regular).unwrap();
        {
            let mut fonts = fonts().lock().unwrap();
            // Simulate an installed face using the same family and queried weight.
            let mut database = cosmic_text::fontdb::Database::new();
            let ids = database.load_font_source(Source::Binary(Arc::new(medium)));
            for id in ids {
                let mut face = database.face(id).unwrap().clone();
                face.weight = cosmic_text::fontdb::Weight::NORMAL;
                fonts.system.db_mut().push_face_info(face);
            }
        }
        let alias = "jvid-collection-isolation-test";
        load_project_fonts(&[FontSource {
            family: alias.into(),
            original_family: "Inter".into(),
            path: path.clone(),
        }])
        .unwrap();
        let fonts = fonts().lock().unwrap();
        let id = fonts
            .system
            .db()
            .query(&Query {
                families: &[Family::Name(alias)],
                ..Default::default()
            })
            .unwrap();
        assert!(
            fonts
                .system
                .db()
                .with_face_data(id, |bytes, _| bytes == regular)
                .unwrap()
        );
        std::fs::remove_file(path).unwrap();
    }

    /// Shapes `s` with the system fonts; `None` when no installed font covers it.
    fn shape(s: &str) -> Option<(bool, Vec<u16>)> {
        shape_with(&mut fonts().lock().unwrap().system, s)
    }

    fn shape_with(system: &mut FontSystem, s: &str) -> Option<(bool, Vec<u16>)> {
        let mut buf = Buffer::new(system, Metrics::new(32.0, 40.0));
        buf.set_text(system, s, &Attrs::new(), Shaping::Advanced, None);
        buf.shape_until_scroll(system, false);
        let run = buf.layout_runs().next()?;
        let ids: Vec<u16> = run.glyphs.iter().map(|g| g.glyph_id).collect();
        // Which fallback font shaped it differs per OS, so say, for failures.
        let faces: Vec<_> = run
            .glyphs
            .iter()
            .filter_map(|g| system.db().face(g.font_id))
            .map(|face| face.post_script_name.clone())
            .collect();
        eprintln!("{s}: {ids:?} from {faces:?}");
        // Glyph 0 is .notdef: the font fell back to a tofu box.
        (!ids.contains(&0)).then_some((run.rtl, ids))
    }

    #[test]
    fn every_listed_family_resolves_to_a_face() {
        let names = families();
        let f = fonts().lock().unwrap();
        for name in names {
            let q = cosmic_text::fontdb::Query {
                families: &[cosmic_text::fontdb::Family::Name(name)],
                ..Default::default()
            };
            assert!(f.system.db().query(&q).is_some(), "{name}");
        }
    }

    /// Skips a shaping test, except on Linux CI, which installs the fonts.
    fn no_font(script: &str) {
        assert!(
            !(cfg!(target_os = "linux") && std::env::var_os("CI").is_some()),
            "Linux CI must have a font covering {script}"
        );
        eprintln!("skipping: no installed font covers {script}");
    }

    #[test]
    fn arabic_shapes_right_to_left_with_joined_forms() {
        let Some((rtl, ids)) = shape("سلام") else {
            no_font("Arabic");
            return;
        };
        assert!(rtl, "Arabic runs right to left");
        assert!(!ids.is_empty(), "Arabic produces shaped glyphs");
        // Joining picks contextual forms: the same letter differs alone and joined. How many
        // glyphs lam-alef makes is up to the font (DejaVu: one ligature, Segoe UI: two).
        let (_, alone) = shape("س").unwrap();
        assert!(
            !ids.contains(&alone[0]),
            "initial seen is not the isolated form"
        );
    }

    #[test]
    fn cjk_shapes_one_glyph_per_character() {
        let Some((rtl, ids)) = shape("你好世界") else {
            no_font("CJK");
            return;
        };
        assert!(!rtl);
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn bundled_cjk_faces_draw_title_presets_without_system_fonts() {
        // The faces jvid-app passes to `set_faces`, linked only into this test.
        let faces = [
            include_bytes!("../../suite-ui/assets/NotoSansCJK-ja.otf").as_slice(),
            include_bytes!("../../suite-ui/assets/NotoSansCJK-zh-Hans.otf"),
            include_bytes!("../../suite-ui/assets/NotoSansCJK-ko.otf"),
        ];
        let mut db = cosmic_text::fontdb::Database::new();
        for source in sources(&faces) {
            db.load_font_source(source);
        }
        // The ja, zh-Hans and ko title, lower-third and caption preset texts.
        for (locale, text) in [
            ("ja", "タイトル"),
            ("ja", "名前"),
            ("ja", "キャプション"),
            ("zh-Hans", "说明文字"),
            ("ko", "타이틀"),
        ] {
            let mut system = FontSystem::new_with_locale_and_db(locale.into(), db.clone());
            let (_, ids) = shape_with(&mut system, text).unwrap_or_else(|| panic!("{text} tofu"));
            assert_eq!(ids.len(), text.chars().count(), "{text}");
        }
    }

    #[test]
    fn raster_draws_fill_box_and_anchor() {
        let size = (320, 180);
        let text = Text {
            content: "Hi".into(),
            size: 40.0,
            box_color: [0.0, 0.0, 1.0, 1.0],
            box_padding: 4.0,
            ..Text::default()
        };
        let img = rasterize(&text, Some([1.0, 0.0, 0.0, 1.0]), size);
        let px = |x: u32, y: u32| &img[((y * size.0 + x) * 4) as usize..][..4];
        assert_eq!(px(0, 0), [0, 0, 0, 0], "outside the box is transparent");
        let red = (0..img.len() / 4).filter(|k| img[k * 4..][..4] == [255, 0, 0, 255]);
        let blue = (0..img.len() / 4).filter(|k| img[k * 4..][..4] == [0, 0, 255, 255]);
        let (red, blue): (Vec<_>, Vec<_>) = (red.collect(), blue.collect());
        if red.is_empty() {
            no_font("Latin");
            return;
        }
        assert!(!blue.is_empty(), "the box shows around the glyphs");
        let cx = red.iter().map(|k| (k % 320) as f32).sum::<f32>() / red.len() as f32;
        assert!((cx - 160.0).abs() < 20.0, "centered on the anchor: {cx}");
        assert!(Arc::ptr_eq(
            &img,
            &rasterize(&text, Some([1.0, 0.0, 0.0, 1.0]), size)
        ));
    }

    #[test]
    fn bounds_cover_exactly_the_drawn_pixels() {
        let size = (320, 180);
        let text = Text {
            content: "Hi".into(),
            size: 40.0,
            box_color: [0.0, 0.0, 1.0, 1.0],
            box_padding: 4.0,
            ..Text::default()
        };
        let Some([x0, y0, x1, y1]) = drawn_bounds(&text, size) else {
            no_font("Latin");
            return;
        };
        assert!(
            x1 - x0 < size.0 / 2 && y1 - y0 < size.1 / 2,
            "smaller than the frame"
        );
        let img = rasterize(&text, None, size);
        for (k, px) in img.chunks(4).enumerate() {
            let (x, y) = (k as u32 % size.0, k as u32 / size.0);
            let inside = (x0..x1).contains(&x) && (y0..y1).contains(&y);
            assert_eq!(px[3] > 0, inside, "at {x},{y}");
        }
    }

    #[test]
    fn carets_step_right_and_down_and_hits_find_them() {
        let size = (640, 360);
        let text = Text {
            content: "Hi there\nab".into(),
            size: 40.0,
            ..Text::default()
        };
        if drawn_bounds(&text, size).is_none() {
            no_font("Latin");
            return;
        }
        let c = carets(&text, size);
        assert_eq!(c.len(), 12);
        let xs: Vec<f32> = c[..=8].iter().map(|c| c[0]).collect();
        assert!(xs.windows(2).all(|w| w[0] < w[1]), "{xs:?}");
        let [_, top, bottom] = c[9];
        assert!(top >= c[0][2] - 1.0 && bottom > top, "line two sits below");
        for i in [0, 3, 9, 11] {
            let [x, top, bottom] = c[i];
            assert_eq!(hit(&text, size, [x + 1.0, (top + bottom) / 2.0]), i);
        }
    }

    #[test]
    fn recoloring_reuses_the_masks() {
        let mut f = fonts().lock().unwrap();
        let t = Text::default();
        let red = Text {
            color: [1.0, 0.0, 0.0, 1.0],
            box_color: [0.0, 0.0, 1.0, 1.0],
            ..t.clone()
        };
        let a = masks(&mut f, &t, (64, 36));
        assert!(Arc::ptr_eq(&a, &masks(&mut f, &red, (64, 36))));
    }

    #[test]
    fn dilate_grows_by_radius() {
        let mut m = vec![0u8; 25];
        m[12] = 255;
        let d = dilate(&m, (5, 5), 1.0);
        assert_eq!(d.iter().filter(|v| **v == 255).count(), 5, "a plus shape");
        assert_eq!(bounds(&d, 5), Some([1, 1, 4, 4]));
    }
}
