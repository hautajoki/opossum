//! The image pipeline: PNG, SVG and JPEG in, the bytes to upload out. Pixels are worked on as straight (not
//! premultiplied) RGBA, so the colour bleed writes under alpha 0 survives encoding.

use std::io::Cursor;

use resvg::{tiny_skia, usvg};

/// Part of every asset input hash (`pipeline`). Bumped by hand only when old outputs are wrong, never because a
/// dependency moved bytes: the goldens below say when bytes move, and bumping re-uploads every image.
pub const IMAGE_VERSION: u32 = 1;

/// Roblox refuses an image of 8000 pixels or more on either side; the caller reports a larger output as too large.
pub const MAX_SIDE: u32 = 7999;

/// Every decoder refuses a header above this on either side before allocating (a decompression bomb guard).
const MAX_DECODE: u32 = 16384;

/// Tessera's seven presets: `Original` keeps the source size, the others resize into a fixed box with `Fit::Contain`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Preset {
    Original,
    Square512,
    ExperienceIcon,
    Thumbnail,
    GamePassIcon,
    ProductIcon,
    BadgeIcon,
}

impl Preset {
    pub const ALL: [Preset; 7] = [
        Preset::Original,
        Preset::Square512,
        Preset::ExperienceIcon,
        Preset::Thumbnail,
        Preset::GamePassIcon,
        Preset::ProductIcon,
        Preset::BadgeIcon,
    ];

    /// The name config spells it with, which is also the one the input hash carries.
    pub fn name(self) -> &'static str {
        match self {
            Preset::Original => "original",
            Preset::Square512 => "square-512",
            Preset::ExperienceIcon => "experience-icon",
            Preset::Thumbnail => "thumbnail",
            Preset::GamePassIcon => "game-pass-icon",
            Preset::ProductIcon => "product-icon",
            Preset::BadgeIcon => "badge-icon",
        }
    }

    pub fn parse(name: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.name() == name)
    }

    pub fn size(self) -> Option<(u32, u32)> {
        match self {
            Preset::Original => None,
            Preset::Thumbnail => Some((1920, 1080)),
            _ => Some((512, 512)),
        }
    }
}

/// An asset set's image options; the shorthand set form means `DEFAULT`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ImageOptions {
    pub preset: Preset,
    pub bleed: bool,
}

impl ImageOptions {
    pub const DEFAULT: ImageOptions = ImageOptions { preset: Preset::Original, bleed: true };
}

/// How a source goes into a box of another shape: whole and letterboxed, or filling it and cropped centred.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fit {
    Contain,
    Cover,
}

/// What to upload: the source file's own bytes (an already-right PNG or a JPEG nothing resizes), or a new PNG.
#[derive(PartialEq, Eq, Debug)]
pub enum Image {
    Unchanged { w: u32, h: u32 },
    Png { bytes: Vec<u8>, w: u32, h: u32 },
}

/// The `pipeline` part of a file's input hash: `Some` for the files `process` may rewrite under these options, `None`
/// for one uploaded as it is.
pub fn pipeline(ext: &str, opts: ImageOptions) -> Option<String> {
    let rewrites = match ext {
        ".png" | ".svg" => true,
        ".jpg" | ".jpeg" => opts.preset != Preset::Original,
        _ => false,
    };
    rewrites.then(|| format!("image-{IMAGE_VERSION};{};bleed={}", opts.preset.name(), u8::from(opts.bleed)))
}

/// An asset image (`ext` lowercase with its dot) through its set's options.
pub fn process(ext: &str, bytes: &[u8], opts: ImageOptions) -> Result<Image, String> {
    image(ext, bytes, opts.preset.size().map(|(w, h)| (w, h, Fit::Contain)), opts.bleed)
}

/// A resource image (an icon or a thumbnail) into exactly `w`×`h`, bled.
pub fn fit(ext: &str, bytes: &[u8], w: u32, h: u32, fit: Fit) -> Result<Image, String> {
    image(ext, bytes, Some((w, h, fit)), true)
}

/// Straight RGBA, row-major.
struct Pixels {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

fn image(ext: &str, bytes: &[u8], target: Option<(u32, u32, Fit)>, bleed: bool) -> Result<Image, String> {
    let resizes = |w, h| target.is_some_and(|(tw, th, _)| (tw, th) != (w, h));
    let src = match ext {
        ".png" => {
            let src = decode_png(bytes)?;
            // Tessera's no-op rule: nothing to resize and nothing for a bleed to fill keeps the file's own bytes.
            let (holes, seeds) = (src.px.chunks(4).any(|p| p[3] == 0), src.px.chunks(4).any(|p| p[3] > 0));
            if !resizes(src.w, src.h) && !(bleed && holes && seeds) {
                return Ok(Image::Unchanged { w: src.w, h: src.h });
            }
            src
        }
        ".svg" => render_svg(bytes, target)?,
        ".jpg" | ".jpeg" => {
            let (w, h) = jpeg_size(bytes)?;
            if !resizes(w, h) {
                return Ok(Image::Unchanged { w, h });
            }
            decode_jpeg(bytes, w, h)?
        }
        _ => return Err(format!("{ext} is not an image the pipeline reads")),
    };
    let mut out = match target {
        Some((w, h, fit)) if (w, h) != (src.w, src.h) => resize(&src, w, h, fit),
        _ => src,
    };
    if bleed {
        self::bleed(&mut out);
    }
    Ok(Image::Png { bytes: encode(&out)?, w: out.w, h: out.h })
}

fn check_size(what: &str, w: u32, h: u32) -> Result<(), String> {
    if w == 0 || h == 0 || w > MAX_DECODE || h > MAX_DECODE {
        return Err(format!("{what} dimensions {w}x{h} are outside 1..{MAX_DECODE}"));
    }
    Ok(())
}

/// Palette, gray, 16-bit and tRNS all expand to 8-bit straight RGBA.
fn decode_png(bytes: &[u8]) -> Result<Pixels, String> {
    let fail = |e: png::DecodingError| format!("PNG cannot be decoded: {e}");
    let mut d = png::Decoder::new_with_limits(Cursor::new(bytes), png::Limits { bytes: usize::MAX });
    d.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let (w, h) = d.read_header_info().map_err(fail)?.size();
    check_size("PNG", w, h)?;
    let mut r = d.read_info().map_err(fail)?;
    let mut buf = vec![0; r.output_buffer_size().ok_or("PNG is too large to decode")?];
    r.next_frame(&mut buf).map_err(fail)?;
    let (color, _) = r.output_color_type();
    let n = (w * h) as usize;
    let px = match color {
        png::ColorType::Rgba => buf[..n * 4].to_vec(),
        png::ColorType::Rgb => buf[..n * 3].chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf[..n * 2].chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf[..n].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("PNG palette was not expanded".into()),
    };
    Ok(Pixels { w, h, px })
}

/// Width and height from the first SOF0/SOF1/SOF2 segment (baseline, extended, progressive), without decoding.
fn jpeg_size(bytes: &[u8]) -> Result<(u32, u32), String> {
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return Err("JPEG does not start with an SOI marker".into());
    }
    let mut i = 2;
    loop {
        // Markers may be padded with any number of 0xFF fill bytes.
        while bytes.get(i) == Some(&0xFF) && bytes.get(i + 1) == Some(&0xFF) {
            i += 1;
        }
        let (Some(&0xFF), Some(&m)) = (bytes.get(i), bytes.get(i + 1)) else {
            return Err("JPEG has no frame header (SOF) before its data".into());
        };
        i += 2;
        if m == 0x01 || (0xD0..=0xD7).contains(&m) {
            continue;
        }
        if m == 0xD9 || m == 0xDA {
            return Err("JPEG has no frame header (SOF) before its data".into());
        }
        let len = match bytes.get(i..i + 2) {
            Some(b) if u16::from_be_bytes([b[0], b[1]]) >= 2 => usize::from(u16::from_be_bytes([b[0], b[1]])),
            _ => return Err("JPEG has a truncated segment".into()),
        };
        if matches!(m, 0xC0..=0xC2) {
            let Some(s) = bytes.get(i + 3..i + 7).filter(|_| len >= 7) else {
                return Err("JPEG frame header is truncated".into());
            };
            let (h, w) = (u32::from(u16::from_be_bytes([s[0], s[1]])), u32::from(u16::from_be_bytes([s[2], s[3]])));
            check_size("JPEG", w, h)?;
            return Ok((w, h));
        }
        if matches!(m, 0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            return Err(format!("JPEG uses an encoding Roblox does not read (SOF{}); save it as baseline", m - 0xC0));
        }
        i += len;
    }
}

fn decode_jpeg(bytes: &[u8], w: u32, h: u32) -> Result<Pixels, String> {
    use zune_jpeg::zune_core::{colorspace::ColorSpace, options::DecoderOptions};
    let opts = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGBA)
        .set_max_width(MAX_DECODE as usize)
        .set_max_height(MAX_DECODE as usize);
    let px = zune_jpeg::JpegDecoder::new_with_options(Cursor::new(bytes), opts)
        .decode()
        .map_err(|e| format!("JPEG cannot be decoded: {e:?}"))?;
    if px.len() != (w * h * 4) as usize {
        return Err("JPEG decoded to a different size than its header says".into());
    }
    Ok(Pixels { w, h, px })
}

const SVG_NS: &str = "http://www.w3.org/2000/svg";

/// Renders the viewBox at one pixel per unit, or with a target at the scale that fits it (min for contain, max for
/// cover), leaving the letterbox or crop to `resize`. `currentColor` is opaque white: icons are tinted in-engine.
fn render_svg(bytes: &[u8], target: Option<(u32, u32, Fit)>) -> Result<Pixels, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "SVG is not UTF-8")?;
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(text, opts).map_err(|e| format!("SVG cannot be parsed: {e}"))?;
    // usvg without its text and raster-image features would drop these silently and upload a hole.
    for n in doc.descendants().filter(|n| n.tag_name().namespace() == Some(SVG_NS)) {
        match n.tag_name().name() {
            "text" => return Err("SVG has <text>, which opossum does not render; convert text to paths".into()),
            "image" => return Err("SVG embeds an <image>, which opossum does not render".into()),
            "foreignObject" => return Err("SVG has a <foreignObject>, which opossum does not render".into()),
            _ => {}
        }
    }
    let root = doc.root_element();
    let number = |name: &str| {
        let v = root.attribute(name)?.trim();
        v.strip_suffix("px").unwrap_or(v).parse().ok()
    };
    let (vw, vh) = match root.attribute("viewBox") {
        Some(v) => {
            let n: Vec<f64> = v.split([' ', ',', '\t', '\n', '\r']).filter_map(|s| s.parse().ok()).collect();
            if n.len() == 4 { (n[2], n[3]) } else { (0.0, 0.0) }
        }
        None => (number("width").unwrap_or(0.0), number("height").unwrap_or(0.0)),
    };
    if !(vw.is_finite() && vh.is_finite() && vw > 0.0 && vh > 0.0) {
        return Err("SVG must have a positive viewBox".into());
    }
    let (w, h) = match target {
        None => (vw.round(), vh.round()),
        Some((tw, th, fit)) => {
            let (sx, sy) = (f64::from(tw) / vw, f64::from(th) / vh);
            let s = if fit == Fit::Contain { sx.min(sy) } else { sx.max(sy) };
            ((vw * s).round().max(1.0), (vh * s).round().max(1.0))
        }
    };
    if !(1.0..=f64::from(MAX_DECODE)).contains(&w) || !(1.0..=f64::from(MAX_DECODE)).contains(&h) {
        return Err(format!("SVG renders at {w}x{h}, outside 1..{MAX_DECODE}"));
    }
    let (w, h) = (w as u32, h as u32);

    // Splice color="#ffffff" onto the root element, replacing a colour it already has.
    let mut svg = text.to_string();
    match root.attributes().find(|a| a.name() == "color" && a.namespace().is_none()) {
        Some(a) => svg.replace_range(a.range_value(), "#ffffff"),
        None => {
            let start = root.range().start + 1;
            let name = text[start..].find(|c: char| c.is_whitespace() || c == '>' || c == '/').unwrap_or(0);
            svg.insert_str(start + name, " color=\"#ffffff\"");
        }
    }
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).map_err(|e| format!("SVG cannot be read: {e}"))?;
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or("SVG raster cannot be allocated")?;
    let size = tree.size();
    let scale = tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height());
    resvg::render(&tree, scale, &mut pixmap.as_mut());
    Ok(Pixels { w, h, px: pixmap.take_demultiplied() })
}

/// Tessera's alpha bleed, exactly: a multi-source BFS from every pixel with alpha > 0 in row-major order, neighbours
/// in the order W N E S NW NE SW SE, each reached pixel taking the RGB of the one it was reached from and keeping its
/// alpha. Ties go to the earliest-queued source. No seeds, or no holes, changes nothing.
fn bleed(img: &mut Pixels) {
    const DIRS: [(i64, i64); 8] = [(-1, 0), (0, -1), (1, 0), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)];
    let (w, h) = (i64::from(img.w), i64::from(img.h));
    let n = (w * h) as usize;
    let mut seen: Vec<bool> = img.px.chunks(4).map(|p| p[3] > 0).collect();
    let mut queue: Vec<usize> = (0..n).filter(|&i| seen[i]).collect();
    if queue.is_empty() || queue.len() == n {
        return;
    }
    let mut head = 0;
    while head < queue.len() {
        let p = queue[head];
        head += 1;
        let (x, y) = (p as i64 % w, p as i64 / w);
        for (dx, dy) in DIRS {
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx >= w || ny >= h {
                continue;
            }
            let q = (ny * w + nx) as usize;
            if seen[q] {
                continue;
            }
            seen[q] = true;
            img.px.copy_within(p * 4..p * 4 + 3, q * 4);
            queue.push(q);
        }
    }
}

/// Catmull-Rom (a = -0.5).
fn catmull_rom(x: f32) -> f32 {
    let x = x.abs();
    if x < 1.0 {
        (1.5 * x - 2.5) * x * x + 1.0
    } else if x < 2.0 {
        ((-0.5 * x + 2.5) * x - 4.0) * x + 2.0
    } else {
        0.0
    }
}

/// For each output sample `from..to` of a `src`-long line scaled to `len`, its first source index and normalised
/// weights. Shrinking widens the kernel by the scale; taps outside the source are dropped.
fn taps(src: u32, len: u32, from: u32, to: u32) -> Vec<(usize, Vec<f32>)> {
    let scale = src as f32 / len as f32;
    let widen = scale.max(1.0);
    (from..to)
        .map(|i| {
            let c = (i as f32 + 0.5) * scale - 0.5;
            let lo = ((c - 2.0 * widen).ceil().max(0.0)) as usize;
            let hi = ((c + 2.0 * widen).floor() as i64).min(i64::from(src) - 1).max(lo as i64) as usize;
            let mut ws: Vec<f32> = (lo..=hi).map(|j| catmull_rom((j as f32 - c) / widen)).collect();
            let sum: f32 = ws.iter().sum();
            if sum != 0.0 {
                ws.iter_mut().for_each(|w| *w /= sum);
            }
            (lo, ws)
        })
        .collect()
}

/// Scales `src` by `min` (contain) or `max` (cover) of the axis ratios, centred in a transparent `w`×`h` canvas
/// (integer-divided offsets; cover crops through a negative one). Filtering is premultiplied, so the colour under
/// transparent pixels never fringes into what is visible.
fn resize(src: &Pixels, w: u32, h: u32, fit: Fit) -> Pixels {
    let (sx, sy) = (f64::from(w) / f64::from(src.w), f64::from(h) / f64::from(src.h));
    let s = if fit == Fit::Contain { sx.min(sy) } else { sx.max(sy) };
    let rw = (f64::from(src.w) * s).round().max(1.0) as i64;
    let rh = (f64::from(src.h) * s).round().max(1.0) as i64;
    let (ox, oy) = ((i64::from(w) - rw) / 2, (i64::from(h) - rh) / 2);
    // The window of the scaled image the canvas shows.
    let (x0, x1) = ((-ox).max(0), (i64::from(w) - ox).min(rw));
    let (y0, y1) = ((-oy).max(0), (i64::from(h) - oy).min(rh));
    let (cols, rows) = (taps(src.w, rw as u32, x0 as u32, x1 as u32), taps(src.h, rh as u32, y0 as u32, y1 as u32));

    // Horizontal pass over only the source rows the vertical taps read, premultiplying as it goes.
    let first = rows.first().map_or(0, |r| r.0);
    let last = rows.iter().map(|r| r.0 + r.1.len()).max().unwrap_or(0);
    let sw = src.w as usize;
    let mut across = vec![0f32; (last - first) * cols.len() * 4];
    for y in first..last {
        for (x, (lo, ws)) in cols.iter().enumerate() {
            let out = &mut across[((y - first) * cols.len() + x) * 4..][..4];
            for (k, wt) in ws.iter().enumerate() {
                let p = &src.px[(y * sw + lo + k) * 4..][..4];
                let a = f32::from(p[3]) / 255.0;
                let pre = [f32::from(p[0]) * a, f32::from(p[1]) * a, f32::from(p[2]) * a, f32::from(p[3])];
                (0..4).for_each(|c| out[c] += pre[c] * wt);
            }
        }
    }
    let mut px = vec![0u8; (w * h * 4) as usize];
    for (y, (lo, ws)) in rows.iter().enumerate() {
        for x in 0..cols.len() {
            let mut v = [0f32; 4];
            for (k, wt) in ws.iter().enumerate() {
                let p = &across[((lo - first + k) * cols.len() + x) * 4..][..4];
                (0..4).for_each(|c| v[c] += p[c] * wt);
            }
            let a = v[3].clamp(0.0, 255.0);
            let at = (((y0 + oy) as usize + y) * w as usize + (x0 + ox) as usize + x) * 4;
            if a.round() == 0.0 {
                continue;
            }
            let k = 255.0 / a;
            for c in 0..3 {
                px[at + c] = (v[c].clamp(0.0, a) * k).round() as u8;
            }
            px[at + 3] = a.round() as u8;
        }
    }
    Pixels { w, h, px }
}

/// RGBA8 at the png crate's best compression with the adaptive filter, no extra chunks.
fn encode(img: &Pixels) -> Result<Vec<u8>, String> {
    let fail = |e: png::EncodingError| format!("PNG cannot be encoded: {e}");
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, img.w, img.h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.set_compression(png::Compression::High);
    let mut writer = e.write_header().map_err(fail)?;
    writer.write_image_data(&img.px).map_err(fail)?;
    writer.finish().map_err(fail)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(bytes: &[u8]) -> String {
        blake3::hash(bytes).to_hex().to_string()
    }

    fn png_bytes(img: &Image) -> &[u8] {
        let Image::Png { bytes, .. } = img else { panic!("expected a new PNG, got {img:?}") };
        bytes
    }

    fn png_of(img: &Image) -> Pixels {
        decode_png(png_bytes(img)).unwrap()
    }

    fn at(img: &Pixels, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.w + x) * 4) as usize;
        img.px[i..i + 4].try_into().unwrap()
    }

    const ALPHA_PNG: &[u8] = include_bytes!("../tests/fixtures/alpha.png");
    const OPAQUE_PNG: &[u8] = include_bytes!("../tests/fixtures/opaque.png");
    const PHOTO_JPG: &[u8] = include_bytes!("../tests/fixtures/photo.jpg");
    const LUCIDE_SVG: &[u8] = include_bytes!("../tests/fixtures/lucide.svg");
    const RED_SVG: &[u8] = include_bytes!("../tests/fixtures/red.svg");
    const TEXT_SVG: &[u8] = include_bytes!("../tests/fixtures/text.svg");

    #[test]
    fn bleed_fills_holes_from_the_nearest_earliest_source() {
        let line = |px: &[[u8; 4]]| Pixels { w: px.len() as u32, h: 1, px: px.concat() };
        let (red, blue, clear) = ([255, 0, 0, 255], [0, 0, 255, 255], [0, 0, 0, 0]);
        let mut img = line(&[red, clear, blue, clear, clear]);
        bleed(&mut img);
        // px1 is between both sources and red queued first; px4 is reached from blue only.
        assert_eq!(img.px, line(&[red, [255, 0, 0, 0], blue, [0, 0, 255, 0], [0, 0, 255, 0]]).px);
    }

    #[test]
    fn a_png_with_nothing_to_bleed_uploads_its_own_bytes() {
        assert_eq!(process(".png", OPAQUE_PNG, ImageOptions::DEFAULT), Ok(Image::Unchanged { w: 2, h: 1 }));
        // With bleed off, transparency alone changes nothing either.
        let off = ImageOptions { preset: Preset::Original, bleed: false };
        assert_eq!(process(".png", ALPHA_PNG, off), Ok(Image::Unchanged { w: 4, h: 2 }));
    }

    #[test]
    fn a_png_with_holes_is_bled_and_reencoded() {
        let out = process(".png", ALPHA_PNG, ImageOptions::DEFAULT).unwrap();
        let img = png_of(&out);
        assert_eq!((img.w, img.h), (4, 2));
        // The fixture's opaque red at (0,0) and blue at (3,1); every hole takes a colour and stays transparent.
        assert_eq!(at(&img, 1, 0), [255, 0, 0, 0]);
        assert_eq!(at(&img, 2, 1), [0, 0, 255, 0]);
        assert_eq!(hash(png_bytes(&out)), "48a8ba7acb29b6c7a0ed7c9379ddfacebe449612ec81f1ef1fb42c54dd72159e");
    }

    #[test]
    fn a_jpeg_passes_through_unless_a_preset_resizes_it() {
        assert_eq!(jpeg_size(PHOTO_JPG), Ok((4, 2)));
        assert_eq!(process(".jpg", PHOTO_JPG, ImageOptions::DEFAULT), Ok(Image::Unchanged { w: 4, h: 2 }));
        let square = ImageOptions { preset: Preset::Square512, bleed: true };
        let img = png_of(&process(".jpg", PHOTO_JPG, square).unwrap());
        assert_eq!((img.w, img.h), (512, 512));
        // Letterboxed above and below, bled so the box is coloured under alpha 0.
        assert_eq!(at(&img, 256, 16)[3], 0);
        assert_eq!(at(&img, 256, 256)[3], 255);
        assert_ne!(at(&img, 256, 16), [0, 0, 0, 0]);
    }

    #[test]
    fn the_jpeg_header_reader_never_panics() {
        // A deterministic xorshift over truncated and byte-flipped copies of the fixture's headers.
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..20_000 {
            let mut b = PHOTO_JPG[..(next() as usize % 300).min(PHOTO_JPG.len())].to_vec();
            for _ in 0..next() % 4 {
                let at = next() as usize % b.len().max(1);
                if let Some(x) = b.get_mut(at) {
                    *x = next() as u8;
                }
            }
            let _ = jpeg_size(&b);
        }
    }

    #[test]
    fn current_color_is_opaque_white() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 2 2"><rect width="2" height="2" fill="currentColor"/></svg>"#;
        let img = png_of(&process(".svg", svg, ImageOptions::DEFAULT).unwrap());
        assert_eq!((img.w, img.h), (2, 2));
        assert_eq!(at(&img, 1, 1), [255, 255, 255, 255]);
        // A colour already on the root is replaced, not doubled (a second attribute would not parse).
        let img = png_of(&process(".svg", RED_SVG, ImageOptions::DEFAULT).unwrap());
        assert_eq!(at(&img, img.w / 2, img.h / 2), [255, 255, 255, 255]);
    }

    #[test]
    fn contain_letterboxes_a_wide_svg() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 4 2"><rect width="4" height="2" fill="#ffffff"/></svg>"##;
        for preset in [Preset::Square512, Preset::BadgeIcon] {
            let img = png_of(&process(".svg", svg, ImageOptions { preset, bleed: true }).unwrap());
            assert_eq!((img.w, img.h, at(&img, 256, 16)[3], at(&img, 256, 256)[3]), (512, 512, 0, 255));
        }
        // Cover fills the box and crops the sides instead.
        let img = png_of(&fit(".svg", svg, 512, 512, Fit::Cover).unwrap());
        assert_eq!((at(&img, 256, 16)[3], at(&img, 0, 0)[3]), (255, 255));
        let img = png_of(&fit(".png", ALPHA_PNG, 1920, 1080, Fit::Cover).unwrap());
        assert_eq!((img.w, img.h), (1920, 1080));
    }

    #[test]
    fn svgs_that_would_lose_content_are_refused() {
        let err = process(".svg", TEXT_SVG, ImageOptions::DEFAULT).unwrap_err();
        assert!(err.contains("convert text to paths"), "{err}");
        let no_box = br#"<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0h1v1z"/></svg>"#;
        assert_eq!(process(".svg", no_box, ImageOptions::DEFAULT).unwrap_err(), "SVG must have a positive viewBox");
    }

    #[test]
    fn the_hash_pipeline_names_preset_and_bleed() {
        assert_eq!(pipeline(".svg", ImageOptions::DEFAULT).as_deref(), Some("image-1;original;bleed=1"));
        assert_eq!(pipeline(".jpg", ImageOptions::DEFAULT), None);
        let thumb = ImageOptions { preset: Preset::Thumbnail, bleed: false };
        assert_eq!(pipeline(".jpeg", thumb).as_deref(), Some("image-1;thumbnail;bleed=0"));
        assert_eq!(pipeline(".ogg", thumb), None);
        assert!(Preset::ALL.iter().all(|&p| Preset::parse(p.name()) == Some(p)));
    }

    // tiny-skia's and zune-jpeg's SIMD paths differ by architecture, so raster bytes are pinned on the one the goldens
    // were made on; the assertions above hold everywhere.
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    #[test]
    fn raster_goldens() {
        let golden = |ext, src, opts| hash(png_bytes(&process(ext, src, opts).unwrap()));
        let square = ImageOptions { preset: Preset::Square512, bleed: true };
        assert_eq!(
            golden(".svg", LUCIDE_SVG, ImageOptions::DEFAULT),
            "d3008d37cc392106d9d64e9fcc78ae32e41aa969932a8ebc2d87df59c4ed8da2"
        );
        assert_eq!(
            golden(".svg", RED_SVG, ImageOptions::DEFAULT),
            "07383ff5a786e4c61ff86b3ffc74e6bd0b6459612a99fee258905566f9c4c48e"
        );
        assert_eq!(
            golden(".jpg", PHOTO_JPG, square),
            "6626765cef369399d86adc655877f4ea94959c4d9a7f7e2cfac151f4ddf69351"
        );
    }
}
