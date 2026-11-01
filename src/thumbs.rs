//! Las miniaturas de las fotos (los wallpapers del menú de SUPER+W): se
//! decodifican una sola vez, en hilos aparte para que el menú abra al
//! instante, y se guardan chicas en ~/.cache/jale/thumbs (qoi: rápido de
//! leer). La siguiente vez solo se lee la chica.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tiny_skia::Pixmap;

/// El ancho de la miniatura (px): sobra para una celda de la cuadrícula.
const W: u32 = 512;
const EXTS: [&str; 9] = ["jpg", "jpeg", "png", "webp", "gif", "bmp", "tif", "tiff", "qoi"];

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

fn real(name: &str) -> Option<PathBuf> {
    let name = name.strip_prefix("file://").unwrap_or(name);
    let p = match name.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if name.starts_with('/') => PathBuf::from(name),
        None => return None,
    };
    Some(p)
}

/// ¿El ícono es una foto (una ruta a una imagen) y no uno del tema?
pub fn is_photo(name: &str) -> bool {
    real(name).is_some_and(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTS.iter().any(|x| x.eq_ignore_ascii_case(e))))
}

fn cache_file(path: &Path) -> Option<PathBuf> {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&(path, mtime, W), &mut h);
    Some(home().join(".cache/jale/thumbs").join(format!("{:016x}.qoi", std::hash::Hasher::finish(&h))))
}

fn to_pixmap(img: image::RgbaImage) -> Option<Pixmap> {
    let (w, h) = img.dimensions();
    let mut data = img.into_raw();
    // tiny-skia quiere el alfa premultiplicado
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as u16;
        if a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u16 * a + 127) / 255) as u8;
            }
        }
    }
    Pixmap::from_vec(data, tiny_skia::IntSize::from_wh(w, h)?)
}

/// La miniatura de `name`: de la caché, o hecha (y guardada) ahora.
pub fn load(name: &str) -> Option<Pixmap> {
    let path = std::fs::canonicalize(real(name)?).ok()?;
    let cache = cache_file(&path)?;
    if let Ok(img) = image::open(&cache) {
        return to_pixmap(img.into_rgba8());
    }
    let img = image::ImageReader::open(&path).ok()?.with_guessed_format().ok()?.decode().ok()?;
    let img = if img.width() > W { img.thumbnail(W, u32::MAX) } else { img };
    let rgba = img.into_rgba8();
    if std::fs::create_dir_all(cache.parent()?).is_ok() {
        // a un temporal y luego el nombre: otra jale nunca lee una a medias
        let tmp = cache.with_extension(format!("{}.tmp", std::process::id()));
        if rgba.save_with_format(&tmp, image::ImageFormat::Qoi).is_ok() {
            let _ = std::fs::rename(&tmp, &cache);
        }
    }
    to_pixmap(rgba)
}

/// Hacer las miniaturas de `names` (en ese orden: las de arriba primero) en
/// unos cuantos hilos; cada una que queda se manda por `done`.
pub fn spawn(names: Vec<String>, done: impl Fn(String, Option<Pixmap>) + Send + Sync + 'static) {
    if names.is_empty() {
        return;
    }
    let n = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 6).min(names.len());
    let queue = Arc::new(Mutex::new(names.into_iter()));
    let done = Arc::new(done);
    for _ in 0..n {
        let (queue, done) = (queue.clone(), done.clone());
        std::thread::spawn(move || {
            loop {
                let next = queue.lock().ok().and_then(|mut q| q.next());
                let Some(name) = next else { break };
                let pix = load(&name);
                done(name, pix);
            }
        });
    }
}
