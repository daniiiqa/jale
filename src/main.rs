//! jale: el launcher de discada. Sin argumentos, las apps (lo que más usas
//! arriba); con -dmenu, un menú con lo que llegue por stdin, compatible con
//! lo que los scripts le mandaban a rofi:
//!
//!   jale                                  las apps
//!   jale -show window                     las ventanas abiertas
//!   jale -dump-fonts [-config f]          las fuentes del theme (FontSize.py)
//!   jale -dmenu [-p prompt] [-mesg texto] [-format s|i] [-no-custom]
//!               [-markup-rows] [-l renglones] [-selected-row n]
//!               [-kb-custom-N "Control-Delete"] [-a filas] [-u filas]
//!               [-config archivo.rasi]
//!
//! Se ve con el theme del look, en el formato de rofi (.rasi): ~/.config/jale/
//! config.rasi (menu.rasi en -dmenu), o el -config que pase cada script, con
//! sus @import y el -theme-str que traiga. LookBuild copia el de cada look
//! (hypr/looks/<look>/jale/theme.rasi) y wallust escribe sus colores.
//!
//! En -dmenu cada renglón puede traer su ícono como en rofi:
//! "texto\0icon\x1fnombre-o-ruta". Sale con 1 si se cancela; con un
//! -kb-custom-N, devuelve el renglón elegido y sale con 9+N, como rofi. Las
//! opciones de rofi que no aplican (-i, -theme-str, -show…) se ignoran.
//! Una segunda llamada cierra la que esté abierta.

mod apps;
mod fuzzy;
mod rasi;
mod text;
mod thumbs;
mod ui;
mod view;
mod windows;

use std::io::{Read, Write};
use std::path::PathBuf;

/// Un renglón del menú.
pub struct Item {
    pub text: String,
    /// lo de al lado, más tenue (de qué es la app)
    pub sub: String,
    /// en dónde se busca
    pub search: String,
    pub icon: Option<String>,
    /// lo que se devuelve (el renglón tal cual llegó, sin lo del ícono)
    pub raw: String,
    /// -a / -u de rofi: se pinta con `element *.active` / `*.urgent`
    pub active: bool,
    pub urgent: bool,
}

pub struct Opts {
    pub prompt: String,
    /// el .rasi con que se ve
    pub config: PathBuf,
    /// lo que dice el botón del mode-switcher (las apps); None en -dmenu
    pub mode: Option<String>,
    /// -l: cuántos renglones (si no, los `lines` del theme)
    pub lines: Option<usize>,
    /// -theme-str: reglas encima del theme
    pub theme_str: Option<String>,
    /// -kb-custom-N: (N, la combinación tal cual, "Control-Delete")
    pub custom_keys: Vec<(u8, String)>,
    pub mesg: Option<String>,
    pub selected: usize,
    pub no_custom: bool,
}

pub enum Outcome {
    Pick(usize),
    /// un -kb-custom-N sobre el renglón elegido
    Key(u8, Option<usize>),
    Custom(String),
    Cancel,
}

fn pidfile() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from).join("jale.pid")
}

/// Quitar el markup de pango para mostrarlo (rofi -markup-rows).
fn strip_markup(s: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' if tag => tag = false,
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn dmenu_items(markup: bool) -> Vec<Item> {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    input
        .lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let mut parts = line.split('\0');
            let raw = parts.next().unwrap_or("").to_string();
            let mut icon = None;
            for meta in parts {
                let mut kv = meta.split('\x1f');
                if kv.next() == Some("icon") {
                    icon = kv.next().filter(|v| !v.is_empty()).map(str::to_string);
                }
            }
            let text = if markup { strip_markup(&raw) } else { raw.clone() };
            Item { search: text.clone(), text, sub: String::new(), icon, raw, active: false, urgent: false }
        })
        .collect()
}

/// "0,3,5-7" (lo de -a y -u de rofi) → los renglones.
fn rows(spec: &str) -> Vec<usize> {
    spec.split(',')
        .flat_map(|part| {
            let part = part.trim();
            match part.split_once('-') {
                Some((a, b)) => match (a.parse::<usize>(), b.parse::<usize>()) {
                    (Ok(a), Ok(b)) => (a..=b).collect(),
                    _ => Vec::new(),
                },
                None => part.parse().ok().into_iter().collect(),
            }
        })
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dmenu = args.iter().any(|a| a == "-dmenu");
    let show_windows = args.windows(2).any(|w| w[0] == "-show" && w[1] == "window");
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let jale = |f: &str| home.join(".config/jale").join(f);
    let config_arg = args.windows(2).find(|w| w[0] == "-config").map(|w| w[1].clone());
    let expand = |c: &str| c.strip_prefix("~/").map_or_else(|| PathBuf::from(c), |r| home.join(r));

    // para FontSize.py: "selector<TAB>fuente", una por regla con font
    if args.iter().any(|a| a == "-dump-fonts") {
        let cfg = config_arg.as_deref().map_or_else(|| jale("config.rasi"), expand);
        for (sel, font) in rasi::Theme::load(&cfg).fonts() {
            println!("{sel}\t{font}");
        }
        return;
    }

    // otra jale abierta: se cierra (y si eran las apps, esta llamada era para eso)
    if let Some(pid) = std::fs::read_to_string(pidfile()).ok().and_then(|p| p.trim().parse::<i32>().ok())
        && pid != std::process::id() as i32
        && unsafe { libc::kill(pid, 0) } == 0
    {
        unsafe { libc::kill(pid, libc::SIGTERM) };
        if !dmenu && !show_windows {
            return;
        }
    }
    let _ = std::fs::write(pidfile(), std::process::id().to_string());

    let mut opts = Opts {
        // sin -p, en -dmenu no hay prompt (las apps lo toman del theme)
        prompt: if dmenu { String::new() } else { "jale".into() },
        config: if dmenu && jale("menu.rasi").exists() { jale("menu.rasi") } else { jale("config.rasi") },
        mode: None,
        lines: None,
        theme_str: None,
        custom_keys: Vec::new(),
        mesg: None,
        selected: 0,
        no_custom: !dmenu,
    };
    let (mut active_rows, mut urgent_rows) = (String::new(), String::new());
    let (mut index, mut markup) = (false, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-p" => opts.prompt = it.next().cloned().unwrap_or_default(),
            "-mesg" => opts.mesg = it.next().map(|m| strip_markup(m)).filter(|m| !m.is_empty()),
            "-format" => index = it.next().is_some_and(|f| f == "i"),
            "-l" | "-lines" => opts.lines = it.next().and_then(|n| n.parse().ok()).map(|n: usize| n.clamp(1, 40)),
            "-config" => {
                if let Some(c) = it.next() {
                    let p = expand(c);
                    if p.exists() {
                        opts.config = p;
                    }
                }
            }
            "-theme-str" => opts.theme_str = it.next().cloned(),
            "-a" => active_rows = it.next().cloned().unwrap_or_default(),
            "-u" => urgent_rows = it.next().cloned().unwrap_or_default(),
            "-selected-row" => opts.selected = it.next().and_then(|n| n.parse().ok()).unwrap_or(0),
            "-no-custom" => opts.no_custom = true,
            "-markup-rows" => markup = true,
            // de rofi, con valor, que aquí no aplican
            "-theme" | "-show" | "-modi" | "-filter" | "-matching" | "-font" => {
                it.next();
            }
            k if k.starts_with("-kb-custom-") => {
                if let (Ok(n), Some(combo)) = (k["-kb-custom-".len()..].parse::<u8>(), it.next()) {
                    opts.custom_keys.push((n, combo.clone()));
                }
            }
            k if k.starts_with("-kb-") => {
                it.next();
            }
            _ => {}
        }
    }

    let code = if dmenu {
        let mut items = dmenu_items(markup);
        for i in rows(&active_rows) {
            if let Some(it) = items.get_mut(i) {
                it.active = true;
            }
        }
        for i in rows(&urgent_rows) {
            if let Some(it) = items.get_mut(i) {
                it.urgent = true;
            }
        }
        let raws: Vec<String> = items.iter().map(|it| it.raw.clone()).collect();
        let order: Vec<f64> = vec![0.0; items.len()];
        match ui::run(items, order, opts) {
            Ok(Outcome::Pick(i)) => {
                let out = if index { i.to_string() } else { raws[i].clone() };
                let _ = writeln!(std::io::stdout(), "{out}");
                0
            }
            Ok(Outcome::Custom(q)) => {
                let _ = writeln!(std::io::stdout(), "{}", if index { "-1".into() } else { q });
                0
            }
            Ok(Outcome::Key(n, pick)) => {
                if let Some(i) = pick {
                    let _ = writeln!(std::io::stdout(), "{}", if index { i.to_string() } else { raws[i].clone() });
                }
                9 + n as i32
            }
            Ok(Outcome::Cancel) => 1,
            Err(e) => {
                eprintln!("jale: {e}");
                1
            }
        }
    } else if show_windows {
        let label = rasi::Theme::load(&opts.config).config("display-window").map(|v| rasi::string(&v));
        if let Some(l) = label {
            opts.prompt = l.clone();
            opts.mode = Some(l);
        }
        let (wins, items) = windows::load();
        // como alt-tab: elegida la de antes, no la de ahora
        opts.selected = usize::from(wins.len() > 1);
        let order: Vec<f64> = (0..items.len()).map(|i| (items.len() - i) as f64).collect();
        match ui::run(items, order, opts) {
            Ok(Outcome::Pick(i)) => {
                windows::focus(&wins[i]);
                0
            }
            Ok(_) => 1,
            Err(e) => {
                eprintln!("jale: {e}");
                1
            }
        }
    } else {
        // el botón y el prompt del lanzador, como en rofi (display-drun)
        let label = rasi::Theme::load(&opts.config).config("display-drun").map(|v| rasi::string(&v));
        if let Some(l) = label {
            opts.prompt = l.clone();
            opts.mode = Some(l);
        }
        let list = apps::load();
        let hist = apps::history();
        let items = apps::items(&list);
        let order: Vec<f64> = list.iter().map(|a| apps::frecency(hist.get(&a.id))).collect();
        match ui::run(items, order, opts) {
            Ok(Outcome::Pick(i)) => {
                apps::launch(&list[i]);
                0
            }
            Ok(_) => 1,
            Err(e) => {
                eprintln!("jale: {e}");
                1
            }
        }
    };
    let _ = std::fs::remove_file(pidfile());
    std::process::exit(code);
}
