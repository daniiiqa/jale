//! Búsqueda difusa: las letras de lo que escribes, en orden, en cualquier
//! parte del texto ("ffx" encuentra "Firefox"). Gana lo que empieza igual, lo
//! que pega letras seguidas y lo que cae en el inicio de una palabra.

/// None si no aparecen todas las letras en orden; más alto, mejor.
pub fn score(query: &[char], text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let mut qi = 0;
    let mut score = 0;
    let mut prev_match = false;
    let mut prev: Option<char> = None;
    let mut gap = 0;
    for (i, c) in text.chars().flat_map(char::to_lowercase).enumerate() {
        if qi < query.len() && c == query[qi] {
            score += 10;
            if prev_match {
                score += 15;
            }
            if prev.is_none_or(|p| !p.is_alphanumeric()) {
                score += 20;
            }
            if i == 0 {
                score += 30;
            }
            score -= gap.min(10);
            gap = 0;
            qi += 1;
            prev_match = true;
        } else {
            prev_match = false;
            if qi > 0 {
                gap += 1;
            }
        }
        prev = Some(c);
    }
    if qi < query.len() {
        return None;
    }
    // lo de arriba agarra las letras lo antes que puede ("code" en "Cowboy
    // Decode" sale de C-o y d-e); lo escrito tal cual y entero gana, más si
    // es el principio de una palabra
    let lower = text.to_lowercase();
    let q: String = query.iter().collect();
    if let Some(at) = lower.find(&q) {
        score += 60;
        if lower[..at].chars().next_back().is_none_or(|c| !c.is_alphanumeric()) {
            score += 40;
        }
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::score;
    fn q(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn finds_in_order() {
        assert!(score(&q("ffx"), "Firefox").is_some());
        assert!(score(&q("xff"), "Firefox").is_none());
    }

    #[test]
    fn prefers_starts_and_runs() {
        assert!(score(&q("fire"), "Firefox") > score(&q("fire"), "Wildfire"));
        assert!(score(&q("code"), "VS Code") > score(&q("code"), "Cowboy Decode"));
    }
}
