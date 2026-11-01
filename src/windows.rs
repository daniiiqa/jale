//! `jale -show window`: las ventanas abiertas (como `rofi -show window`),
//! la más reciente arriba; elegir una la enfoca (y te lleva a su workspace).

use crate::Item;
use std::process::Command;

pub struct Win {
    pub addr: String,
}

/// Las ventanas, en el orden en que las usaste (la de ahora primero).
pub fn load() -> (Vec<Win>, Vec<Item>) {
    let clients: serde_json::Value = Command::new("hyprctl")
        .args(["clients", "-j"])
        .output()
        .ok()
        .and_then(|o| serde_json::from_slice(&o.stdout).ok())
        .unwrap_or_default();
    let mut list: Vec<&serde_json::Value> = clients
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["mapped"].as_bool().unwrap_or(true) && !c["hidden"].as_bool().unwrap_or(false))
        .collect();
    list.sort_by_key(|c| c["focusHistoryID"].as_i64().unwrap_or(99));
    let wins = list.iter().map(|c| Win { addr: c["address"].as_str().unwrap_or_default().to_string() }).collect();
    let items = list
        .iter()
        .map(|c| {
            let class = c["class"].as_str().unwrap_or_default();
            let title = c["title"].as_str().filter(|t| !t.is_empty()).unwrap_or(class);
            let ws = c["workspace"]["name"].as_str().unwrap_or_default();
            Item {
                text: title.to_string(),
                sub: format!("{class} · {ws}"),
                search: format!("{title} {class}"),
                icon: (!class.is_empty()).then(|| class.to_lowercase()),
                raw: title.to_string(),
                active: false,
                urgent: false,
            }
        })
        .collect();
    (wins, items)
}

pub fn focus(w: &Win) {
    let home = std::env::var("HOME").unwrap_or_default();
    let _ = Command::new(format!("{home}/.config/hypr/scripts/Hypr.sh")).args(["focus", &format!("address:{}", w.addr)]).status();
}
