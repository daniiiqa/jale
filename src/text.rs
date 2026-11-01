//! El texto de jale con las fuentes de los themes (las de pango, como las
//! escribe rofi: "Terminess Nerd Font Bold 13", "… Display Italic 15"):
//! medir, cortar con "…" y dibujar glifo por glifo sobre tiny-skia.

use crate::rasi::Color;
use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Style, SwashCache, SwashContent, Weight};
use std::collections::HashMap;
use tiny_skia::Pixmap;

#[derive(Clone, PartialEq, Debug)]
pub struct Font {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    /// pixeles lógicos
    pub px: f32,
}

pub struct Text {
    fonts: FontSystem,
    cache: SwashCache,
    parsed: HashMap<String, Font>,
}

impl Text {
    pub fn new() -> Text {
        Text { fonts: FontSystem::new(), cache: SwashCache::new(), parsed: HashMap::new() }
    }

    /// "Familia Estilo 13" → la familia que exista (la más larga que case),
    /// el grueso y si es itálica; el número son puntos de pango (96 dpi).
    pub fn font(&mut self, spec: &str) -> Font {
        if let Some(f) = self.parsed.get(spec) {
            return f.clone();
        }
        let mut words: Vec<&str> = spec.trim().trim_matches('"').split_whitespace().collect();
        let pt = words.last().and_then(|w| w.parse::<f32>().ok());
        if pt.is_some() {
            words.pop();
        }
        let px = pt.unwrap_or(11.0) * 96.0 / 72.0;
        let has = |fam: &str| self.fonts.db().faces().any(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(fam)));
        let mut font = Font { family: "monospace".into(), weight: 400, italic: false, px };
        for cut in (1..=words.len()).rev() {
            let fam = words[..cut].join(" ");
            if has(&fam) {
                font.family = fam;
                for w in &words[cut..] {
                    match w.to_lowercase().as_str() {
                        "thin" => font.weight = 100,
                        "extralight" | "ultralight" => font.weight = 200,
                        "light" => font.weight = 300,
                        "medium" => font.weight = 500,
                        "semibold" | "demibold" => font.weight = 600,
                        "bold" => font.weight = 700,
                        "extrabold" | "ultrabold" => font.weight = 800,
                        "black" | "heavy" => font.weight = 900,
                        "italic" | "oblique" => font.italic = true,
                        _ => {}
                    }
                }
                break;
            }
        }
        self.parsed.insert(spec.to_string(), font.clone());
        font
    }

    pub fn line_height(f: &Font) -> f32 {
        (f.px * 1.3).ceil()
    }

    fn shape(&mut self, s: &str, f: &Font, scale: f32) -> Buffer {
        let mut b = Buffer::new(&mut self.fonts, Metrics::new(f.px * scale, Self::line_height(f) * scale));
        let attrs = Attrs::new().family(Family::Name(&f.family)).weight(Weight(f.weight)).style(if f.italic { Style::Italic } else { Style::Normal });
        let mut bb = b.borrow_with(&mut self.fonts);
        bb.set_size(None, None);
        bb.set_text(s, &attrs, Shaping::Advanced, None);
        bb.shape_until_scroll(false);
        b
    }

    /// Ancho en px lógicos.
    pub fn width(&mut self, s: &str, f: &Font) -> f32 {
        if s.is_empty() {
            return 0.0;
        }
        let mut b = self.shape(s, f, 1.0);
        let w = b.borrow_with(&mut self.fonts).layout_runs().map(|r| r.line_w).fold(0.0, f32::max);
        w
    }

    /// Lo que cabe en `max` px: el texto entero o cortado con "…".
    pub fn fit(&mut self, s: &str, f: &Font, max: f32) -> String {
        if self.width(s, f) <= max {
            return s.to_string();
        }
        let chars: Vec<char> = s.chars().collect();
        let (mut lo, mut hi) = (0, chars.len());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            let t: String = chars[..mid].iter().collect::<String>() + "…";
            if self.width(&t, f) <= max { lo = mid } else { hi = mid - 1 }
        }
        chars[..lo].iter().collect::<String>() + "…"
    }

    /// Dibuja `s` con su caja de renglón empezando en (x, y) (px del buffer).
    pub fn draw(&mut self, pix: &mut Pixmap, s: &str, f: &Font, c: Color, x: f32, y: f32, scale: f32) {
        if s.is_empty() || c.a <= 0.0 {
            return;
        }
        let mut b = self.shape(s, f, scale);
        let mut glyphs = Vec::new();
        for run in b.borrow_with(&mut self.fonts).layout_runs() {
            for g in run.glyphs {
                glyphs.push(g.physical((x, y + run.line_y), 1.0));
            }
        }
        for p in glyphs {
            let Some(img) = self.cache.get_image(&mut self.fonts, p.cache_key).as_ref() else { continue };
            let (left, top) = (img.placement.left, img.placement.top);
            let (w, h) = (img.placement.width as i32, img.placement.height as i32);
            for iy in 0..h {
                for ix in 0..w {
                    let (px, py) = (p.x + left + ix, p.y - top + iy);
                    let i = (iy * w + ix) as usize;
                    match img.content {
                        SwashContent::Mask => blend(pix, px, py, c, img.data[i] as f32 / 255.0),
                        SwashContent::SubpixelMask => {
                            let d = &img.data[i * 4..i * 4 + 3];
                            blend(pix, px, py, c, (d[0] as f32 + d[1] as f32 + d[2] as f32) / 765.0)
                        }
                        SwashContent::Color => {
                            let d = &img.data[i * 4..i * 4 + 4];
                            let col = Color { r: d[0] as f32 / 255.0, g: d[1] as f32 / 255.0, b: d[2] as f32 / 255.0, a: 1.0 };
                            blend(pix, px, py, col, d[3] as f32 / 255.0)
                        }
                    }
                }
            }
        }
    }
}

/// Un pixel de color `c` con cobertura `cov` encima de lo que haya (todo
/// premultiplicado, como tiny-skia).
fn blend(pix: &mut Pixmap, x: i32, y: i32, c: Color, cov: f32) {
    if x < 0 || y < 0 || x >= pix.width() as i32 || y >= pix.height() as i32 {
        return;
    }
    let a = (c.a * cov).clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let i = (y as usize * pix.width() as usize + x as usize) * 4;
    let d = &mut pix.data_mut()[i..i + 4];
    let over = |src: f32, dst: u8| (src * a * 255.0 + dst as f32 * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
    d[0] = over(c.r, d[0]);
    d[1] = over(c.g, d[1]);
    d[2] = over(c.b, d[2]);
    d[3] = over(1.0, d[3]);
}
