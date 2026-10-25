//! Mise en forme des valeurs affichées, dans la langue de l'interface.

use std::time::Duration;

use tr::tr;

use crate::i18n::Language;

/// Nombre à virgule dans la langue de l'interface : « 1,2 » ou « 1.2 ».
fn decimal(value: f64, digits: usize) -> String {
    let text = format!("{value:.digits$}");
    match Language::current() {
        Language::French => text.replace('.', ","),
        Language::English => text,
    }
}

/// Nombre entier avec séparateur de milliers : « 12 000 » (espace
/// insécable), « 12,000 ».
pub fn integer(value: u64) -> String {
    let separator = match Language::current() {
        Language::French => '\u{a0}',
        Language::English => ',',
    };
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(separator);
        }
        out.push(digit);
    }
    out
}

/// Taille lisible : « 850 Mo », « 1,2 Go » (« 850 MB », « 1.2 GB »).
pub fn bytes(value: u64) -> String {
    let v = value as f64;
    if v >= 1e9 {
        tr!("{} Go", decimal(v / 1e9, 1))
    } else if v >= 1e6 {
        tr!("{} Mo", decimal(v / 1e6, 0))
    } else {
        tr!("{} Ko", decimal(v / 1e3, 0))
    }
}

/// Durée « 02:14:37 ».
pub fn clock(duration: Duration) -> String {
    let s = duration.as_secs();
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// Durée approximative : « 31 h », « 45 min ».
pub fn hours(seconds: f64) -> String {
    if seconds >= 3600.0 {
        tr!("{} h", format!("{:.0}", seconds / 3600.0))
    } else {
        tr!("{} min", format!("{:.0}", (seconds / 60.0).max(0.0)))
    }
}

/// Niveau crête (0 à 1) ramené sur une échelle perçue : -60 dB à 0 dB.
pub fn level(peak: f32) -> f32 {
    if peak <= 0.0 {
        return 0.0;
    }
    ((20.0 * peak.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// Première lettre en majuscule.
pub fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Liste : « A », « A et B », « A, B et C » (« A, B and C »).
pub fn join_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [single] => single.clone(),
        // Pluriel selon le nombre d'éléments avant le dernier : l'anglais
        // ajoute une virgule à partir de trois (« A, B, and C »).
        [rest @ .., last] => tr!("{} et {}" | "{} et {}" % rest.len(), rest.join(", "), last),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(bytes(1_234_000_000), "1,2 Go");
        assert_eq!(bytes(850_000_000), "850 Mo");
        assert_eq!(clock(Duration::from_secs(8077)), "02:14:37");
        assert_eq!(hours(31.0 * 3600.0), "31 h");
        assert_eq!(level(1.0), 1.0);
        assert_eq!(level(0.0), 0.0);
        assert!((level(0.001) - 0.0).abs() < 1e-6);
        assert_eq!(capitalize("discord"), "Discord");
        assert_eq!(integer(12000), "12\u{a0}000");
        assert_eq!(integer(500), "500");
        assert_eq!(integer(1_234_567), "1\u{a0}234\u{a0}567");
        let names = ["« A »".to_string(), "« B »".into(), "« C »".into()];
        assert_eq!(join_list(&names[..1]), "« A »");
        assert_eq!(join_list(&names[..2]), "« A » et « B »");
        assert_eq!(join_list(&names), "« A », « B » et « C »");
    }
}
