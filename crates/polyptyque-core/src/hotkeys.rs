//! Raccourcis clavier globaux : format texte « Ctrl+Maj+F9 », lisible et
//! enregistrable dans la configuration. Les deux langues sont lues
//! (« Maj » ou « Shift ») : comparer deux raccourcis par [`parse`], pas par
//! leur texte.

use tr::tr;

/// Raccourcis proposés : Ctrl+Maj+F1 à F12, écrits dans la langue de
/// l'interface. Ils n'entrent en conflit ni avec AltGr (Ctrl+Alt sur un
/// clavier AZERTY) ni avec la saisie de texte.
pub fn choices() -> Vec<String> {
    (1..=12).map(|n| tr!("Ctrl+Maj+F{}", n)).collect()
}

/// Même raccourci, quelle que soit la langue dans laquelle il est écrit.
pub fn same(a: &str, b: &str) -> bool {
    match (parse(a), parse(b)) {
        (Some(a), Some(b)) => a == b,
        _ => a.eq_ignore_ascii_case(b),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Touche de fonction F1 à F24.
    pub function_key: u8,
}

/// Lit un raccourci. Vide ou invalide : aucun raccourci.
pub fn parse(text: &str) -> Option<Hotkey> {
    let mut hotkey = Hotkey { ctrl: false, shift: false, alt: false, function_key: 0 };
    for part in text.split('+').map(|p| p.trim().to_lowercase()) {
        match part.as_str() {
            "ctrl" | "control" => hotkey.ctrl = true,
            "maj" | "shift" => hotkey.shift = true,
            "alt" => hotkey.alt = true,
            key => {
                let n: u8 = key.strip_prefix('f')?.parse().ok()?;
                if !(1..=24).contains(&n) {
                    return None;
                }
                hotkey.function_key = n;
            }
        }
    }
    (hotkey.function_key != 0).then_some(hotkey)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_shortcuts() {
        assert_eq!(parse("Ctrl+Maj+F9"), Some(Hotkey { ctrl: true, shift: true, alt: false, function_key: 9 }));
        assert_eq!(parse("shift + f12").map(|h| (h.shift, h.function_key)), Some((true, 12)));
    }

    #[test]
    fn rejects_invalid_shortcuts() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("Ctrl+Maj"), None);
        assert_eq!(parse("Ctrl+A"), None);
        assert_eq!(parse("Ctrl+F25"), None);
    }

    #[test]
    fn every_choice_is_valid() {
        assert!(choices().iter().all(|c| parse(c).is_some()));
    }
}
