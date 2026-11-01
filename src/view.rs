//! El dibujo de jale con el theme de rofi: el mismo árbol de widgets
//! (window → mainbox → inputbar · message · listview → element →
//! element-icon · element-text) y su modelo de cajas: margen, borde por lado,
//! radio, padding, fondo (color o imagen), hijos en fila o columna con su
//! `spacing`, `expand` para repartir el espacio, y la lista en cuadrícula
//! (`columns` × `lines`). Lo que no sale (mode-switcher sin modo, message sin
//! mensaje, `enabled: false`) no ocupa lugar.

use crate::Item;
use crate::rasi::{self, Color, Theme};
use crate::text::{Font, Text};
use disco::icon::Icons;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tiny_skia::{FillRule, Mask, Paint, PathBuilder, Pixmap, PixmapPaint, Rect as SkRect, Stroke, Transform};

#[derive(Clone, Copy, Default, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// Lo que se dibuja: el menú en este momento.
pub struct State<'a> {
    pub items: &'a [Item],
    pub matches: &'a [usize],
    /// posición elegida (en `matches`) y el primer renglón visible
    pub sel: usize,
    pub top: usize,
    pub query: &'a str,
    pub prompt: &'a str,
    pub mesg: Option<&'a str>,
    /// el botón del mode-switcher (las apps: display-drun); None, sin botón
    pub mode: Option<&'a str>,
    pub cursor_on: bool,
    pub lines: Option<usize>,
}

/// Lo que quedó después de dibujar: tamaño, dónde cayó cada renglón (px
/// lógicos → posición en `matches`), y la forma de la lista.
#[derive(Default)]
pub struct Laid {
    pub w: f32,
    pub h: f32,
    pub hits: Vec<(Rect, usize)>,
    pub columns: usize,
    pub lines: usize,
}

struct BoxP {
    margin: [f32; 4],
    border: [f32; 4],
    padding: [f32; 4],
    radius: f32,
    bg: Color,
    bc: Color,
    image: Option<PathBuf>,
    font: Font,
}

impl BoxP {
    /// Lo que se come la caja por fuera del contenido: (izq+der, arr+aba).
    fn extra(&self) -> (f32, f32) {
        let s = |i: usize| self.margin[i] + self.border[i] + self.padding[i];
        (s(1) + s(3), s(0) + s(2))
    }
}

pub struct View {
    pub theme: Theme,
    text: Text,
    icons: Icons,
    images: HashMap<(PathBuf, u32), Option<Pixmap>>,
    show_icons: bool,
    /// el ancho del monitor: los % de rofi son de la pantalla
    mon_w: f32,
}

fn kind(name: &str) -> &'static str {
    match name {
        "listview" => "list",
        "element-icon" => "icon",
        "dummy" | "scrollbar" => "spacer",
        "prompt" | "textbox-prompt-colon" | "entry" | "textbox" | "button" | "element-text" | "num-filtered-rows" | "num-rows" | "textbox-num-sep" | "case-indicator" => "text",
        _ => "box",
    }
}

fn default_children(name: &str) -> Vec<String> {
    let v: &[&str] = match name {
        "window" => &["mainbox"],
        "mainbox" => &["inputbar", "message", "listview", "mode-switcher"],
        "inputbar" => &["prompt", "textbox-prompt-colon", "entry"],
        "message" => &["textbox"],
        "mode-switcher" => &["button"],
        "listbox" => &["message", "listview"],
        "element" => &["element-icon", "element-text"],
        _ => &[],
    };
    v.iter().map(|s| s.to_string()).collect()
}

fn horizontal_default(name: &str) -> bool {
    matches!(name, "inputbar" | "mode-switcher" | "element")
}

impl View {
    pub fn new(config: &Path, extra: Option<&str>, apps: bool) -> View {
        let mut theme = Theme::load(config);
        if !config.exists() {
            theme = Theme::parse_str(FALLBACK);
        }
        if let Some(e) = extra {
            theme.add(e);
        }
        let show_icons = theme.config("show-icons").map_or(apps, |v| rasi::boolean(&v));
        View { theme, text: Text::new(), icons: Icons::new(), images: HashMap::new(), show_icons, mon_w: 1920.0 }
    }

    fn get(&self, path: &[String], state: Option<&str>, prop: &str) -> Option<String> {
        let p: Vec<&str> = path.iter().map(String::as_str).collect();
        self.theme.get(&p, state, prop)
    }

    fn boxp(&mut self, path: &[String], state: Option<&str>) -> BoxP {
        let font_spec = self.get(path, state, "font").unwrap_or_else(|| "monospace 11".into());
        let font = self.text.font(&rasi::string(&font_spec));
        let em = font.px;
        let side = |p: &str| self.get(path, state, p).map_or([0.0; 4], |v| rasi::sides(&v, em));
        BoxP {
            margin: side("margin"),
            border: side("border"),
            padding: side("padding"),
            radius: self.get(path, state, "border-radius").map_or(0.0, |v| rasi::sides(&v, em)[0]),
            bg: self.get(path, state, "background-color").and_then(|v| rasi::color(&v)).unwrap_or(Color::NONE),
            bc: self.get(path, state, "border-color").and_then(|v| rasi::color(&v)).unwrap_or(Color::NONE),
            image: self.get(path, state, "background-image").and_then(|v| rasi::url(&v)),
            font,
        }
    }

    fn enabled(&self, path: &[String], st: &State) -> bool {
        let name = path.last().map(String::as_str).unwrap_or("");
        if self.get(path, None, "enabled").is_some_and(|v| v.trim() == "false") {
            return false;
        }
        match name {
            "message" => st.mesg.is_some(),
            "mode-switcher" | "button" => st.mode.is_some(),
            "element-icon" => self.show_icons,
            "scrollbar" => false,
            _ => true,
        }
    }

    fn children(&self, path: &[String]) -> Vec<String> {
        let name = path.last().map(String::as_str).unwrap_or("");
        self.get(path, None, "children").map(|v| rasi::list(&v)).unwrap_or_else(|| default_children(name))
    }

    fn horizontal(&self, path: &[String], state: Option<&str>) -> bool {
        let name = path.last().map(String::as_str).unwrap_or("");
        match self.get(path, state, "orientation") {
            Some(v) => v.trim() == "horizontal",
            None => horizontal_default(name),
        }
    }

    fn spacing(&mut self, path: &[String], state: Option<&str>, em: f32) -> f32 {
        self.get(path, state, "spacing").map_or(0.0, |v| rasi::dist(&v, em, 0.0))
    }

    /// El texto de un widget de texto.
    fn label(&self, path: &[String], state: Option<&str>, st: &State, item: Option<usize>) -> (String, bool) {
        let name = path.last().map(String::as_str).unwrap_or("");
        match name {
            "prompt" => (st.prompt.to_string(), false),
            "textbox-prompt-colon" => (self.get(path, state, "str").map_or(":".into(), |v| rasi::string(&v)), false),
            "entry" if st.query.is_empty() => (self.get(path, state, "placeholder").map(|v| rasi::string(&v)).unwrap_or_default(), true),
            "entry" => (st.query.to_string(), false),
            "textbox" => (st.mesg.unwrap_or("").to_string(), false),
            "button" => (st.mode.unwrap_or("").to_string(), false),
            "element-text" => (item.map(|i| st.items[i].text.clone()).unwrap_or_default(), false),
            "num-filtered-rows" => (st.matches.len().to_string(), false),
            "num-rows" => (st.items.len().to_string(), false),
            "textbox-num-sep" => (self.get(path, state, "str").map_or("/".into(), |v| rasi::string(&v)), false),
            _ => (String::new(), false),
        }
    }

    fn expands(&self, path: &[String]) -> bool {
        let name = path.last().map(String::as_str).unwrap_or("");
        match self.get(path, None, "expand") {
            Some(v) => rasi::boolean(&v),
            None => matches!(name, "entry" | "dummy" | "listview" | "textbox" | "mainbox" | "listbox" | "element-text" | "message"),
        }
    }

    /// El ancho natural de un widget dentro de una fila (None: se estira).
    fn natural_w(&mut self, path: &[String], state: Option<&str>, st: &State, item: Option<usize>) -> Option<f32> {
        if self.expands(path) {
            return None;
        }
        let b = self.boxp(path, state);
        let (ex, _) = b.extra();
        let name = path.last().cloned().unwrap_or_default();
        Some(
            ex + match kind(&name) {
                "text" => {
                    let (t, _) = self.label(path, state, st, item);
                    self.text.width(&t, &b.font).ceil()
                }
                "icon" => self.get(path, state, "size").map_or(b.font.px * 1.2, |v| rasi::dist(&v, b.font.px, self.mon_w)),
                "box" => {
                    let kids: Vec<String> = self.children(path);
                    let horiz = self.horizontal(path, state);
                    let sp = self.spacing(path, state, b.font.px);
                    let mut ws = Vec::new();
                    for k in kids {
                        let mut p = path.to_vec();
                        p.push(k);
                        if self.enabled(&p, st) {
                            ws.push(self.natural_w(&p, state, st, item).unwrap_or(0.0));
                        }
                    }
                    if horiz { ws.iter().sum::<f32>() + sp * ws.len().saturating_sub(1) as f32 } else { ws.iter().cloned().fold(0.0, f32::max) }
                }
                _ => 0.0,
            },
        )
    }

    /// Mide (pix None) o dibuja un widget de ancho `w` en (x, y); devuelve
    /// su alto. Todo en px lógicos; `s` es la escala del buffer.
    #[allow(clippy::too_many_arguments)]
    fn node(&mut self, path: &mut Vec<String>, state: Option<&str>, x: f32, y: f32, w: f32, st: &State, item: Option<usize>, pix: &mut Option<&mut Pixmap>, s: f32, laid: &mut Laid) -> f32 {
        let name = path.last().cloned().unwrap_or_default();
        let b = self.boxp(path, state);
        let (ex, ey) = b.extra();
        let (cx, cy) = (x + b.margin[3] + b.border[3] + b.padding[3], y + b.margin[0] + b.border[0] + b.padding[0]);
        let cw = (w - ex).max(0.0);
        let k = kind(&name);

        // el alto del contenido, para pintar la caja antes que lo de dentro
        let content_h = match k {
            "text" => Text::line_height(&b.font),
            // como rofi: el ícono nunca más ancho que su lugar (las miniaturas
            // de los wallpapers piden un % de la pantalla)
            "icon" => self.get(path, state, "size").map_or(b.font.px * 1.2, |v| rasi::dist(&v, b.font.px, self.mon_w)).min(cw),
            "spacer" => 0.0,
            // al medir también: la forma de la lista (columnas, renglones) se usa
            // antes de dibujar (-selected-row, el modo ventanas)
            "list" => self.list(path, cx, cy, cw, st, &mut None, s, laid),
            _ => self.boxed(path, state, cx, cy, cw, st, item, &mut None, s, laid),
        };
        let h = content_h + ey;
        if let Some(p) = pix.as_deref_mut() {
            let outer = Rect { x: x + b.margin[3], y: y + b.margin[0], w: w - b.margin[1] - b.margin[3], h: h - b.margin[0] - b.margin[2] };
            self.paint_box(p, &b, outer, s);
            match k {
                "text" => {
                    let (t, placeholder) = self.label(path, state, st, item);
                    let color_prop = if placeholder { "placeholder-color" } else { "text-color" };
                    let col = self
                        .get(path, state, color_prop)
                        .and_then(|v| rasi::color(&v))
                        .or_else(|| self.get(path, state, "text-color").and_then(|v| rasi::color(&v)))
                        .unwrap_or(Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 });
                    let t = self.text.fit(&t, &b.font, cw);
                    let tw = self.text.width(&t, &b.font);
                    let ha = self.get(path, state, "horizontal-align").and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(0.0);
                    let tx = cx + (cw - tw).max(0.0) * ha;
                    self.text.draw(p, &t, &b.font, col, tx * s, cy * s, s);
                    // el cursor de la entrada, tras lo escrito
                    if name == "entry" && st.cursor_on {
                        let at = if placeholder { tx } else { tx + tw };
                        let cwid = self.get(path, state, "cursor-width").map_or(2.0, |v| rasi::dist(&v, b.font.px, 0.0));
                        let tc = self.get(path, state, "text-color").and_then(|v| rasi::color(&v)).unwrap_or(col);
                        fill(p, Rect { x: at + 1.0, y: cy + 2.0, w: cwid, h: content_h - 4.0 }, 0.0, tc, s);
                    }
                }
                "icon" => {
                    if let Some(name) = item.and_then(|i| st.items[i].icon.clone())
                        && let Some(icon) = self.icons.get(&name)
                    {
                        let side = content_h;
                        let ix = cx + (cw - side).max(0.0) * self.get(path, state, "horizontal-align").and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(0.5);
                        let src: &Pixmap = &icon.pix;
                        let k = side / src.width().max(src.height()) as f32;
                        let (dw, dh) = (src.width() as f32 * k, src.height() as f32 * k);
                        let t = Transform::from_row(k * s, 0.0, 0.0, k * s, (ix + (side - dw) / 2.0) * s, (cy + (side - dh) / 2.0) * s);
                        let paint = PixmapPaint { quality: tiny_skia::FilterQuality::Bicubic, ..Default::default() };
                        p.draw_pixmap(0, 0, src.as_ref(), &paint, t, None);
                    }
                }
                "list" => {
                    self.list(path, cx, cy, cw, st, pix, s, laid);
                }
                "spacer" => {}
                _ => {
                    self.boxed(path, state, cx, cy, cw, st, item, pix, s, laid);
                }
            }
        }
        h
    }

    /// Un contenedor: sus hijos en fila o en columna.
    #[allow(clippy::too_many_arguments)]
    fn boxed(&mut self, path: &mut Vec<String>, state: Option<&str>, x: f32, y: f32, w: f32, st: &State, item: Option<usize>, pix: &mut Option<&mut Pixmap>, s: f32, laid: &mut Laid) -> f32 {
        let em = self.boxp(path, state).font.px;
        let sp = self.spacing(path, state, em);
        let horiz = self.horizontal(path, state);
        let kids: Vec<String> = self.children(path).into_iter().filter(|k| {
            let mut p = path.clone();
            p.push(k.clone());
            self.enabled(&p, st)
        }).collect();
        if kids.is_empty() {
            return 0.0;
        }
        if !horiz {
            let mut cy = y;
            for (i, k) in kids.iter().enumerate() {
                path.push(k.clone());
                let h = self.node(path, state, x, cy, w, st, item, pix, s, laid);
                path.pop();
                cy += h + if i + 1 < kids.len() { sp } else { 0.0 };
            }
            return cy - y;
        }
        // en fila: los de ancho propio primero; los que se estiran se reparten el resto
        let mut widths: Vec<Option<f32>> = Vec::new();
        for k in &kids {
            path.push(k.clone());
            widths.push(self.natural_w(path, state, st, item));
            path.pop();
        }
        let fixed: f32 = widths.iter().flatten().sum();
        let stretch = widths.iter().filter(|w| w.is_none()).count().max(1) as f32;
        let room = (w - fixed - sp * (kids.len() - 1) as f32).max(0.0) / stretch;
        // el alto de la fila: el del más alto; luego cada uno a ese alto
        let mut hmax: f32 = 0.0;
        for (k, kw) in kids.iter().zip(&widths) {
            path.push(k.clone());
            hmax = hmax.max(self.node(path, state, 0.0, 0.0, kw.unwrap_or(room), st, item, &mut None, s, &mut Laid::default()));
            path.pop();
        }
        if pix.is_some() {
            let mut cx = x;
            for (k, kw) in kids.iter().zip(&widths) {
                let kw = kw.unwrap_or(room);
                path.push(k.clone());
                let h = self.node(path, state, 0.0, 0.0, kw, st, item, &mut None, s, &mut Laid::default());
                // centrado en el alto de la fila (vertical-align de rofi, a la mitad)
                let va = self.get(path, state, "vertical-align").and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(0.5);
                self.node(path, state, cx, y + (hmax - h) * va, kw, st, item, pix, s, laid);
                path.pop();
                cx += kw + sp;
            }
        }
        hmax
    }

    /// La lista: `columns` × `lines` elementos, desde el renglón `top`.
    #[allow(clippy::too_many_arguments)]
    fn list(&mut self, path: &mut Vec<String>, x: f32, y: f32, w: f32, st: &State, pix: &mut Option<&mut Pixmap>, s: f32, laid: &mut Laid) -> f32 {
        let em = self.boxp(path, None).font.px;
        let cols = self.get(path, None, "columns").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).max(1);
        let lines = st.lines.or_else(|| self.get(path, None, "lines").and_then(|v| v.trim().parse::<usize>().ok())).unwrap_or(10).max(1);
        let sp = self.spacing(path, None, em);
        let fixed = self.get(path, None, "fixed-height").is_none_or(|v| rasi::boolean(&v));
        let ew = ((w - sp * (cols - 1) as f32) / cols as f32).max(1.0);
        path.push("element".into());
        let sample = st.matches.first().copied().or(if st.items.is_empty() { None } else { Some(0) });
        let eh = self.node(path, Some("normal.normal"), 0.0, 0.0, ew, st, sample, &mut None, s, &mut Laid::default());
        let rows_total = st.matches.len().div_ceil(cols);
        let rows = if fixed { lines } else { rows_total.min(lines) };
        laid.columns = cols;
        laid.lines = lines;
        if pix.is_some() {
            for r in 0..rows {
                for c in 0..cols {
                    let pos = (st.top + r) * cols + c;
                    let Some(&i) = st.matches.get(pos) else { continue };
                    let it = &st.items[i];
                    let kind = if it.urgent { "urgent" } else if it.active { "active" } else { "normal" };
                    let base = if pos == st.sel { "selected" } else if (st.top + r) % 2 == 1 { "alternate" } else { "normal" };
                    let state = format!("{base}.{kind}");
                    let (ex, ey) = (x + c as f32 * (ew + sp), y + r as f32 * (eh + sp));
                    self.node(path, Some(&state), ex, ey, ew, st, Some(i), pix, s, laid);
                    laid.hits.push((Rect { x: ex, y: ey, w: ew, h: eh }, pos));
                }
            }
        }
        path.pop();
        if rows == 0 { 0.0 } else { rows as f32 * eh + (rows - 1) as f32 * sp }
    }

    /// Fondo, imagen y borde de una caja.
    fn paint_box(&mut self, pix: &mut Pixmap, b: &BoxP, r: Rect, s: f32) {
        if r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        let radius = b.radius.min(r.w / 2.0).min(r.h / 2.0);
        fill(pix, r, radius, b.bg, s);
        if let Some(img_path) = &b.image
            && let Some(img) = self.image(img_path, (r.w * s) as u32)
        {
            // ancho de la caja, recortada abajo; dentro del redondeo
            let mut mask = Mask::new(pix.width(), pix.height());
            if let (Some(m), Some(path)) = (mask.as_mut(), rounded(Rect { x: r.x * s, y: r.y * s, w: r.w * s, h: r.h * s }, radius * s)) {
                m.fill_path(&path, FillRule::Winding, true, Transform::identity());
            }
            pix.draw_pixmap((r.x * s) as i32, (r.y * s) as i32, img.as_ref(), &PixmapPaint::default(), Transform::identity(), mask.as_ref());
        }
        let bw = b.border;
        if bw.iter().all(|&v| v <= 0.0) || b.bc.a <= 0.0 {
            return;
        }
        if bw.iter().all(|&v| (v - bw[0]).abs() < 0.01) {
            let half = bw[0] / 2.0;
            if let Some(path) = rounded(Rect { x: (r.x + half) * s, y: (r.y + half) * s, w: (r.w - bw[0]) * s, h: (r.h - bw[0]) * s }, (radius - half).max(0.0) * s) {
                pix.stroke_path(&path, &paint(b.bc), &Stroke { width: bw[0] * s, ..Default::default() }, Transform::identity(), None);
            }
        } else {
            // un borde distinto por lado (la barra de un lado, la raya de abajo)
            let [t, rt, bt, l] = bw;
            for side in [
                Rect { x: r.x, y: r.y, w: r.w, h: t },
                Rect { x: r.x + r.w - rt, y: r.y, w: rt, h: r.h },
                Rect { x: r.x, y: r.y + r.h - bt, w: r.w, h: bt },
                Rect { x: r.x, y: r.y, w: l, h: r.h },
            ] {
                if side.w > 0.0 && side.h > 0.0 {
                    fill(pix, side, 0.0, b.bc, s);
                }
            }
        }
    }

    /// Una imagen de fondo al ancho `w` (el wallpaper de classic), guardada
    /// en ~/.cache/jale como las miniaturas de rollo.
    fn image(&mut self, path: &Path, w: u32) -> Option<Pixmap> {
        let real = std::fs::canonicalize(path).ok()?;
        let key = (real.clone(), w);
        if let Some(hit) = self.images.get(&key) {
            return hit.clone();
        }
        let loaded = (|| {
            let mtime = std::fs::metadata(&real).and_then(|m| m.modified()).ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&(&real, mtime, w), &mut h);
            let dir = PathBuf::from(std::env::var("HOME").ok()?).join(".cache/jale");
            let out = dir.join(format!("{:016x}.ppm", std::hash::Hasher::finish(&h)));
            if !out.exists() {
                std::fs::create_dir_all(&dir).ok()?;
                std::process::Command::new("magick").arg(format!("{}[0]", real.display())).args(["-resize", &format!("{w}x")]).arg(format!("ppm:{}", out.display())).status().ok()?;
            }
            ppm(&std::fs::read(out).ok()?)
        })();
        self.images.insert(key, loaded.clone());
        loaded
    }

    /// Medir: el tamaño de la ventana (px lógicos) en un monitor de `mon_w`.
    pub fn layout(&mut self, st: &State, mon_w: f32, pix: Option<&mut Pixmap>, s: f32) -> Laid {
        let mut laid = Laid::default();
        self.mon_w = mon_w;
        let mut path = vec!["window".to_string()];
        let b = self.boxp(&path, None);
        let w = self.get(&path, None, "width").map_or(640.0, |v| rasi::dist(&v, b.font.px, mon_w)).min(mon_w);
        let mut pix = pix;
        laid.h = self.node(&mut path, None, 0.0, 0.0, w, st, None, &mut None, s, &mut laid);
        laid.w = w;
        if pix.is_some() {
            laid.hits.clear();
            self.node(&mut path, None, 0.0, 0.0, w, st, None, &mut pix, s, &mut laid);
        }
        laid
    }
}

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8((c.r * 255.0) as u8, (c.g * 255.0) as u8, (c.b * 255.0) as u8, (c.a * 255.0) as u8);
    p.anti_alias = true;
    p
}

fn rounded(r: Rect, radius: f32) -> Option<tiny_skia::Path> {
    if r.w <= 0.0 || r.h <= 0.0 {
        return None;
    }
    let rad = radius.min(r.w / 2.0).min(r.h / 2.0);
    if rad <= 0.0 {
        return SkRect::from_xywh(r.x, r.y, r.w, r.h).map(PathBuilder::from_rect);
    }
    let mut pb = PathBuilder::new();
    let (x, y, w, h) = (r.x, r.y, r.w, r.h);
    pb.move_to(x + rad, y);
    pb.line_to(x + w - rad, y);
    pb.quad_to(x + w, y, x + w, y + rad);
    pb.line_to(x + w, y + h - rad);
    pb.quad_to(x + w, y + h, x + w - rad, y + h);
    pb.line_to(x + rad, y + h);
    pb.quad_to(x, y + h, x, y + h - rad);
    pb.line_to(x, y + rad);
    pb.quad_to(x, y, x + rad, y);
    pb.close();
    pb.finish()
}

/// Un rectángulo (redondeado) en px lógicos.
fn fill(pix: &mut Pixmap, r: Rect, radius: f32, c: Color, s: f32) {
    if c.a <= 0.0 {
        return;
    }
    if let Some(path) = rounded(Rect { x: r.x * s, y: r.y * s, w: r.w * s, h: r.h * s }, radius * s) {
        pix.fill_path(&path, &paint(c), FillRule::Winding, Transform::identity(), None);
    }
}

/// Un PPM binario (P6) como Pixmap opaco.
fn ppm(data: &[u8]) -> Option<Pixmap> {
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        while i < data.len() && data[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < data.len() && !data[i].is_ascii_whitespace() {
            i += 1;
        }
        fields.push(std::str::from_utf8(&data[start..i]).ok()?);
    }
    let (w, h): (u32, u32) = (fields[1].parse().ok()?, fields[2].parse().ok()?);
    let px = data.get(i + 1..i + 1 + (w * h * 3) as usize)?;
    let mut pix = Pixmap::new(w, h)?;
    for (d, s) in pix.data_mut().chunks_exact_mut(4).zip(px.chunks_exact(3)) {
        d.copy_from_slice(&[s[0], s[1], s[2], 255]);
    }
    Some(pix)
}

/// Si no hay theme de rofi: uno sobrio para que jale funcione igual.
const FALLBACK: &str = r#"
* { background-color: transparent; text-color: #d8d8d8; font: "monospace 12"; }
window { width: 560px; background-color: #1c1c1ccc; border: 1px; border-color: #5a5a5a; border-radius: 8px; padding: 12px; }
mainbox { spacing: 10px; children: [ "inputbar", "message", "listview" ]; }
inputbar { spacing: 8px; padding: 8px 10px; border-radius: 6px; background-color: #2a2a2a; children: [ "prompt", "entry" ]; }
prompt { text-color: #8ab4f8; }
entry { placeholder: "Search"; placeholder-color: #777777; }
message { padding: 4px 10px; }
textbox { text-color: #999999; }
listview { lines: 8; spacing: 2px; fixed-height: false; }
element { padding: 7px 10px; spacing: 10px; border-radius: 6px; }
element selected.normal { background-color: #8ab4f8; text-color: #1c1c1c; }
element-icon { size: 22px; }
"#;
