//! Los themes de rofi (.rasi), leídos tal cual: así cada look trae su jale sin
//! copiar nada, y lo que edites en hypr/looks/<look>/rofi/theme.rasi se ve.
//!
//! De rasi se entiende lo que usan los looks de discada: bloques con
//! selectores (`element selected.normal, element-text selected { … }`),
//! `@import` (se mezcla: lo de después gana) y `@theme` (como en rofi: borra el
//! theme cargado hasta ahí, menos `configuration`, y carga ese), variables
//! (`@color12`, `var(x, y)`, definidas en `*`), `inherit` (lo del widget de
//! arriba), colores (#rgb, #rrggbb, #rrggbbaa, rgb[a](), transparent), medidas
//! (px, em, %), listas, cadenas y url("…", width).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct Theme {
    /// selector → propiedad → valor (crudo, sin resolver)
    rules: HashMap<String, HashMap<String, String>>,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

fn expand(p: &str, base: &Path) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else if p.starts_with('/') {
        PathBuf::from(p)
    } else {
        base.join(p)
    }
}

/// Quitar comentarios (/* */ y //) sin tocar lo que va entre comillas.
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let b: Vec<char> = src.chars().collect();
    let (mut i, mut quote) = (0, false);
    while i < b.len() {
        let c = b[i];
        if quote {
            out.push(c);
            if c == '"' {
                quote = false;
            }
            i += 1;
        } else if c == '"' {
            quote = true;
            out.push(c);
            i += 1;
        } else if c == '/' && b.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

impl Theme {
    pub fn load(file: &Path) -> Theme {
        let mut t = Theme::default();
        let mut seen = Vec::new();
        t.include(file, &mut seen);
        t
    }

    pub fn parse_str(src: &str) -> Theme {
        let mut t = Theme::default();
        t.add(src);
        t
    }

    /// Más reglas encima de las que hay (el -theme-str de los scripts).
    pub fn add(&mut self, src: &str) {
        self.parse(src, &home(), &mut Vec::new());
    }

    /// Las reglas que ponen fuente (selector, fuente tal cual), `*` primero;
    /// si `*` no tiene, la de `configuration` como `*`. Para FontSize.py.
    pub fn fonts(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self
            .rules
            .iter()
            .filter(|(sel, _)| sel.as_str() != "configuration")
            .filter_map(|(sel, r)| r.get("font").map(|f| (sel.clone(), string(&self.resolve_refs(f, 0)))))
            .collect();
        v.sort_by(|a, b| (a.0 != "*").cmp(&(b.0 != "*")).then(a.0.cmp(&b.0)));
        if !v.iter().any(|(s, _)| s == "*")
            && let Some(f) = self.config("font")
        {
            v.insert(0, ("*".into(), string(&f)));
        }
        v
    }

    fn include(&mut self, file: &Path, seen: &mut Vec<PathBuf>) {
        let file = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
        if seen.contains(&file) {
            return;
        }
        seen.push(file.clone());
        let Ok(src) = std::fs::read_to_string(&file) else { return };
        let base = file.parent().map(Path::to_path_buf).unwrap_or_else(home);
        self.parse(&src, &base, seen);
    }

    fn parse(&mut self, src: &str, base: &Path, seen: &mut Vec<PathBuf>) {
        let src = strip_comments(src);
        let mut rest = src.as_str();
        loop {
            rest = rest.trim_start();
            if rest.is_empty() {
                break;
            }
            // @import "…" / @theme "…"
            let theme = rest.starts_with("@theme");
            if let Some(r) = rest.strip_prefix("@import").or_else(|| rest.strip_prefix("@theme")) {
                let end = r.find(['\n', ';']).unwrap_or(r.len());
                let target = r[..end].trim().trim_matches('"');
                if !target.is_empty() {
                    if theme {
                        self.rules.retain(|sel, _| sel == "configuration");
                    }
                    self.include(&expand(target, base), seen);
                }
                rest = &r[end..];
                rest = rest.strip_prefix(';').unwrap_or(rest);
                continue;
            }
            let Some(open) = rest.find('{') else { break };
            let selectors: Vec<String> = rest[..open].split(',').map(|s| s.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|s| !s.is_empty()).collect();
            // la llave que cierra, sin contar las de entre comillas
            // (window-format: "{w} {c} {t}")
            let Some(close) = closing(&rest[open..]) else { break };
            let body = &rest[open + 1..open + close];
            let props = parse_body(body);
            for sel in selectors {
                let entry = self.rules.entry(sel).or_default();
                for (k, v) in &props {
                    entry.insert(k.clone(), v.clone());
                }
            }
            rest = &rest[open + close + 1..];
        }
    }

    /// Un ajuste del bloque `configuration` (show-icons, display-drun…).
    pub fn config(&self, prop: &str) -> Option<String> {
        self.rules.get("configuration").and_then(|r| r.get(prop)).map(|v| self.resolve_refs(v, 0))
    }

    /// El valor de `prop` para el widget al final de `path` (con su estado,
    /// p. ej. "selected.normal"), ya con variables e `inherit` resueltos.
    pub fn get(&self, path: &[&str], state: Option<&str>, prop: &str) -> Option<String> {
        let name = *path.last()?;
        let mut cands: Vec<String> = Vec::new();
        if let Some(st) = state {
            cands.push(format!("{name} {st}"));
            if let Some((first, _)) = st.split_once('.') {
                cands.push(format!("{name} {first}"));
            }
        }
        cands.push(name.to_string());
        let found = cands.iter().find_map(|c| self.rules.get(c).and_then(|r| r.get(prop)));
        // si el widget no dice, lo de `*` (y la fuente, la de configuration)
        let v = match found {
            Some(v) => v.clone(),
            None => match self.rules.get("*").and_then(|r| r.get(prop)) {
                Some(v) => v.clone(),
                None if prop == "font" => self.rules.get("configuration")?.get("font")?.clone(),
                None => return None,
            },
        };
        let v = self.resolve_refs(&v, 0);
        if v == "inherit" {
            return if path.len() > 1 { self.get(&path[..path.len() - 1], state, prop) } else { None };
        }
        Some(v)
    }

    /// @nombre y var(nombre, otro) → lo que diga `*` (o `configuration`).
    fn resolve_refs(&self, v: &str, depth: u8) -> String {
        if depth > 8 {
            return v.to_string();
        }
        let v = v.trim();
        let lookup = |name: &str| -> Option<String> {
            ["*", "configuration"].iter().find_map(|s| self.rules.get(*s).and_then(|r| r.get(name))).map(|x| self.resolve_refs(x, depth + 1))
        };
        if let Some(name) = v.strip_prefix('@') {
            return lookup(name.trim()).unwrap_or_default();
        }
        if let Some(inner) = v.strip_prefix("var(").and_then(|x| x.strip_suffix(')')) {
            let (name, fallback) = inner.split_once(',').map_or((inner, None), |(a, b)| (a, Some(b)));
            return lookup(name.trim()).or_else(|| fallback.map(|f| self.resolve_refs(f, depth + 1))).unwrap_or_default();
        }
        v.to_string()
    }
}

fn closing(block: &str) -> Option<usize> {
    let mut quote = false;
    for (i, c) in block.char_indices() {
        match c {
            '"' => quote = !quote,
            '}' if !quote => return Some(i),
            _ => {}
        }
    }
    None
}

fn parse_body(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    // partir por ; fuera de comillas, paréntesis y corchetes
    let (mut depth, mut quote, mut start) = (0i32, false, 0);
    let chars: Vec<(usize, char)> = body.char_indices().collect();
    let mut decls = Vec::new();
    for &(i, c) in &chars {
        match c {
            '"' => quote = !quote,
            '(' | '[' if !quote => depth += 1,
            ')' | ']' if !quote => depth -= 1,
            ';' if !quote && depth == 0 => {
                decls.push(&body[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    decls.push(&body[start..]);
    for d in decls {
        if let Some((k, v)) = d.split_once(':') {
            let k = k.trim();
            if !k.is_empty() {
                out.push((k.to_string(), v.trim().to_string()));
            }
        }
    }
    out
}

// ─── valores ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const NONE: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
}

pub fn color(v: &str) -> Option<Color> {
    let v = v.trim();
    if v == "transparent" || v == "none" {
        return Some(Color::NONE);
    }
    if let Some(h) = v.strip_prefix('#') {
        let n = |s: &str| u8::from_str_radix(s, 16).ok().map(|x| x as f32 / 255.0);
        let d = |c: char| c.to_digit(16).map(|x| (x * 17) as f32 / 255.0);
        let cs: Vec<char> = h.chars().collect();
        return match h.len() {
            3 | 4 => Some(Color { r: d(cs[0])?, g: d(cs[1])?, b: d(cs[2])?, a: if h.len() == 4 { d(cs[3])? } else { 1.0 } }),
            6 | 8 => Some(Color { r: n(&h[0..2])?, g: n(&h[2..4])?, b: n(&h[4..6])?, a: if h.len() == 8 { n(&h[6..8])? } else { 1.0 } }),
            _ => None,
        };
    }
    let inner = v.strip_prefix("rgba(").or_else(|| v.strip_prefix("rgb("))?.strip_suffix(')')?;
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    let ch = |s: &str| -> Option<f32> { if let Some(p) = s.strip_suffix('%') { p.parse::<f32>().ok().map(|x| x / 100.0) } else { s.parse::<f32>().ok().map(|x| x / 255.0) } };
    let a = match parts.get(3) {
        Some(s) => match s.strip_suffix('%') {
            Some(p) => p.parse::<f32>().ok()? / 100.0,
            None => s.parse::<f32>().ok()?,
        },
        None => 1.0,
    };
    Some(Color { r: ch(parts.first()?)?, g: ch(parts.get(1)?)?, b: ch(parts.get(2)?)?, a })
}

/// Una medida en px lógicos: `em` sobre la letra, `%` sobre `pct_of`.
pub fn dist(v: &str, em: f32, pct_of: f32) -> f32 {
    let v = v.trim();
    let num = |s: &str| s.trim().parse::<f32>().unwrap_or(0.0);
    if let Some(n) = v.strip_suffix("px") {
        num(n)
    } else if let Some(n) = v.strip_suffix("em") {
        num(n) * em
    } else if let Some(n) = v.strip_suffix("ch") {
        num(n) * em * 0.6
    } else if let Some(n) = v.strip_suffix('%') {
        num(n) / 100.0 * pct_of
    } else {
        num(v)
    }
}

/// 1 a 4 medidas como en CSS → [arriba, derecha, abajo, izquierda]. Las
/// palabras (solid, dash) se ignoran.
pub fn sides(v: &str, em: f32) -> [f32; 4] {
    let d: Vec<f32> = v.split_whitespace().filter(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '.' || c == '-')).map(|w| dist(w, em, 0.0)).collect();
    match d.len() {
        0 => [0.0; 4],
        1 => [d[0]; 4],
        2 => [d[0], d[1], d[0], d[1]],
        3 => [d[0], d[1], d[2], d[1]],
        _ => [d[0], d[1], d[2], d[3]],
    }
}

pub fn string(v: &str) -> String {
    v.trim().trim_matches('"').to_string()
}

pub fn list(v: &str) -> Vec<String> {
    v.trim().trim_start_matches('[').trim_end_matches(']').split(',').map(string).filter(|s| !s.is_empty()).collect()
}

pub fn boolean(v: &str) -> bool {
    v.trim() == "true"
}

/// url("ruta", width) → la ruta.
pub fn url(v: &str) -> Option<PathBuf> {
    let inner = v.trim().strip_prefix("url(")?.strip_suffix(')')?;
    let path = inner.split(',').next()?.trim().trim_matches('"');
    Some(expand(path, &home()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_resolves() {
        let t = Theme::parse_str(
            r#"
            * { phosphor: @color12; color12: #33ff66; text-color: @phosphor; font: "Mono 12"; }
            /* comentario */ element selected.normal, element-text selected { background-color: @phosphor; }
            element { padding: 3px 6px; text-color: inherit; }
            element-text { text-color: inherit; }
            configuration { window-format: "{w} {c} {t}"; show-icons: false; }
            entry { placeholder: "a; b"; }
        "#,
        );
        assert_eq!(t.get(&["element"], Some("selected.normal"), "background-color").as_deref(), Some("#33ff66"));
        assert_eq!(t.get(&["entry"], None, "placeholder").as_deref(), Some("\"a; b\""));
        assert_eq!(t.get(&["listview", "element", "element-text"], None, "text-color").as_deref(), Some("#33ff66"));
        assert_eq!(t.config("show-icons").as_deref(), Some("false"));
        // @theme borra lo de antes (menos configuration); @import no
        let mut t2 = Theme::parse_str("configuration { a: 1; } element-text { font: \"X 9\"; }");
        t2.rules.retain(|sel, _| sel == "configuration");
        assert!(t2.get(&["element-text"], None, "font").is_none());
        assert_eq!(t2.config("a").as_deref(), Some("1"));
        assert_eq!(sides("3px 6px", 10.0), [3.0, 6.0, 3.0, 6.0]);
        assert_eq!(color("#33ff66").map(|c| (c.g * 255.0).round()), Some(255.0));
        assert_eq!(color("rgba(0, 0, 0, 50%)").map(|c| c.a), Some(0.5));
    }
}

