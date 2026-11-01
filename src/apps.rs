//! Las apps: los .desktop de siempre (los tuyos primero, luego los del
//! sistema y flatpak), y lo que más usas, que sale arriba (frecencia: veces
//! que la abriste, pesando más lo reciente).

use crate::Item;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct App {
    pub id: String,
    pub name: String,
    pub generic: String,
    pub keywords: String,
    pub exec: String,
    pub icon: String,
    pub terminal: bool,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

fn dirs() -> Vec<PathBuf> {
    let data_home = std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| home().join(".local/share"));
    let mut v = vec![data_home.join("applications"), data_home.join("flatpak/exports/share/applications")];
    let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    v.extend(sys.split(':').filter(|d| !d.is_empty()).map(|d| PathBuf::from(d).join("applications")));
    v.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    v
}

/// El idioma de discada (hypr/discada.conf → LANGUAGE), para Name[es].
fn lang() -> Option<String> {
    let conf = std::fs::read_to_string(home().join(".config/hypr/discada.conf")).ok()?;
    conf.lines().find_map(|l| l.strip_prefix("LANGUAGE=").map(|v| v.trim().to_string()))
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, base, out);
        } else if p.extension().is_some_and(|x| x == "desktop") {
            // el id de XDG: la ruta relativa con / → -
            let id = p.strip_prefix(base).unwrap_or(&p).to_string_lossy().replace('/', "-");
            out.push((id, p));
        }
    }
}

fn parse(id: String, text: &str, lang: Option<&str>) -> Option<App> {
    let mut in_entry = false;
    let mut kv: HashMap<&str, &str> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if in_entry && let Some((k, v)) = line.split_once('=') {
            kv.entry(k.trim()).or_insert(v.trim());
        }
    }
    let get = |k: &str| -> String {
        lang.and_then(|l| kv.get(format!("{k}[{l}]").as_str()).copied()).or_else(|| kv.get(k).copied()).unwrap_or("").to_string()
    };
    if kv.get("Type").copied() != Some("Application") || kv.get("NoDisplay") == Some(&"true") || kv.get("Hidden") == Some(&"true") {
        return None;
    }
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let shown = |list: &str| list.split(';').any(|d| !d.is_empty() && desktop.split(':').any(|x| x.eq_ignore_ascii_case(d)));
    if kv.get("OnlyShowIn").is_some_and(|l| !shown(l)) || kv.get("NotShowIn").is_some_and(|l| shown(l)) {
        return None;
    }
    let exec = kv.get("Exec")?.to_string();
    Some(App {
        id,
        name: get("Name"),
        generic: get("GenericName"),
        keywords: get("Keywords").replace(';', " "),
        exec,
        icon: kv.get("Icon").copied().unwrap_or("").to_string(),
        terminal: kv.get("Terminal") == Some(&"true"),
    })
}

pub fn load() -> Vec<App> {
    let lang = lang();
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for dir in dirs() {
        let mut files = Vec::new();
        walk(&dir, &dir, &mut files);
        for (id, path) in files {
            // el primero gana: los tuyos tapan a los del sistema
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path)
                && let Some(app) = parse(id, &text, lang.as_deref())
                && !app.name.is_empty()
            {
                apps.push(app);
            }
        }
    }
    // por nombre: así salen mientras no haya historial (y empatando en él)
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

// ─── lo más usado ───────────────────────────────────────────────────────────

fn history_path() -> PathBuf {
    std::env::var("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|_| home().join(".local/state")).join("jale/apps.json")
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// id → (veces, última vez)
pub fn history() -> HashMap<String, (u64, u64)> {
    let v: serde_json::Value = std::fs::read(history_path()).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default();
    v.as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.clone(), (v[0].as_u64()?, v[1].as_u64()?))))
        .collect()
}

/// Frecencia: cada vez que la abriste cuenta, menos entre más vieja.
pub fn frecency(h: Option<&(u64, u64)>) -> f64 {
    let Some(&(count, last)) = h else { return 0.0 };
    let days = now().saturating_sub(last) as f64 / 86400.0;
    count as f64 / (1.0 + days / 7.0)
}

fn remember(id: &str) {
    let mut h = history();
    let e = h.entry(id.to_string()).or_insert((0, 0));
    e.0 += 1;
    e.1 = now();
    let obj: serde_json::Map<String, serde_json::Value> = h.into_iter().map(|(k, (c, l))| (k, serde_json::json!([c, l]))).collect();
    let p = history_path();
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    let _ = std::fs::write(p, serde_json::Value::Object(obj).to_string());
}

/// Los renglones para la lista: nombre, y de qué es (su GenericName); se
/// busca también por sus palabras clave.
pub fn items(apps: &[App]) -> Vec<Item> {
    apps.iter()
        .map(|a| Item {
            text: a.name.clone(),
            sub: a.generic.clone(),
            search: format!("{} {} {}", a.name, a.generic, a.keywords),
            icon: (!a.icon.is_empty()).then(|| a.icon.clone()),
            raw: a.name.clone(),
            active: false,
            urgent: false,
        })
        .collect()
}

/// Abrirla, ya sin jale de por medio (su propia sesión).
pub fn launch(app: &App) {
    remember(&app.id);
    // fuera los códigos de campo (%f, %U…): no hay archivos que pasarle
    let mut cmd = String::new();
    let mut it = app.exec.chars().peekable();
    while let Some(c) = it.next() {
        if c == '%' {
            match it.next() {
                Some('%') => cmd.push('%'),
                _ => {}
            }
        } else {
            cmd.push(c);
        }
    }
    let cmd = if app.terminal { format!("kitty -e {cmd}") } else { cmd };
    let mut c = Command::new("sh");
    c.args(["-c", &format!("exec {cmd}")]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        use std::os::unix::process::CommandExt;
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let _ = c.spawn();
}
