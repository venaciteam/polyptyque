//! Nommage des fichiers de sortie.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::format::{Item, StrftimeItems};
use tr::tr;

/// Modèle par défaut en français : `2026-10-20_14h32_Podcast_Discord`.
pub const DEFAULT_TEMPLATE: &str = "{date}_{heure}_{scène}_{source}";

/// Heure des noms de fichiers en français : `14h32`.
const FILE_TIME_FORMAT: &str = "%Hh%M";

/// Modèle par défaut d'une nouvelle configuration, dans la langue de
/// l'interface (en anglais : `{date}_{time}_{scene}_{source}`). Les variables
/// des deux langues sont reconnues ([`render`]).
pub fn default_template() -> String {
    tr!("{date}_{heure}_{scène}_{source}")
}

pub const TEMPLATE_HELP: &str = "Variables : {date} (2026-10-20), {heure} (14h32), {scène}, {source}";

pub struct NameParts<'a> {
    pub date: &'a str,
    pub time: &'a str,
    pub scene: &'a str,
    pub source: &'a str,
}

/// Date et heure locales, au format des journaux et des fichiers internes
/// (indépendant de la langue).
pub fn now_parts() -> (String, String) {
    let now = chrono::Local::now();
    (now.format("%Y-%m-%d").to_string(), now.format(FILE_TIME_FORMAT).to_string())
}

/// Date et heure locales pour les noms des fichiers enregistrés : la date
/// reste au format ISO dans toutes les langues (tri chronologique), l'heure
/// reste sur 24 h, écrite à la façon de la langue (`14h32`, `14-32`).
pub fn file_name_parts() -> (String, String) {
    let now = chrono::Local::now();
    // Format venu du catalogue : un format invalide ferait échouer
    // l'affichage de chrono, on garde alors celui du français.
    let translated = tr!("%Hh%M");
    let format = if StrftimeItems::new(&translated).any(|item| matches!(item, Item::Error)) {
        FILE_TIME_FORMAT
    } else {
        translated.as_str()
    };
    (now.format("%Y-%m-%d").to_string(), now.format(format).to_string())
}

/// Remplace les variables du modèle puis nettoie le résultat pour en faire
/// un nom de fichier valide.
pub fn render(template: &str, parts: &NameParts) -> String {
    let raw = template
        .replace("{date}", parts.date)
        .replace("{heure}", parts.time)
        .replace("{time}", parts.time)
        .replace("{scène}", parts.scene)
        .replace("{scene}", parts.scene)
        .replace("{source}", parts.source);
    sanitize(&raw)
}

/// Rend un nom utilisable comme nom de fichier sous Windows (le système le
/// plus restrictif) : caractères interdits remplacés, noms réservés évités,
/// longueur limitée.
pub fn sanitize(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    out = out.trim().trim_end_matches(['.', ' ']).to_string();
    if out.chars().count() > 150 {
        out = out.chars().take(150).collect();
    }
    if out.is_empty() {
        out = "source".to_string();
    }
    let upper = out.to_uppercase();
    let stem = upper.split('.').next().unwrap_or("");
    let reserved = matches!(stem, "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        out.insert(0, '_');
    }
    out
}

/// Chemin libre dans `dir` : `nom.ext`, sinon `nom (2).ext`, `nom (3).ext`…
/// `taken` contient les chemins déjà réservés pendant la session (les
/// fichiers ne sont créés qu'au démarrage de l'encodeur).
pub fn unique_path(dir: &Path, stem: &str, ext: &str, taken: &HashSet<PathBuf>) -> PathBuf {
    let candidate = |n: u32| {
        let name = if n <= 1 { format!("{stem}.{ext}") } else { format!("{stem} ({n}).{ext}") };
        dir.join(name)
    };
    let mut n = 1;
    loop {
        let path = candidate(n);
        if !path.exists() && !taken.contains(&path) {
            return path;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_default_template() {
        let parts = NameParts { date: "2026-10-20", time: "14h32", scene: "Podcast", source: "Discord" };
        assert_eq!(render(DEFAULT_TEMPLATE, &parts), "2026-10-20_14h32_Podcast_Discord");
    }

    #[test]
    fn accepts_unaccented_variables() {
        let parts = NameParts { date: "d", time: "t", scene: "s", source: "x" };
        assert_eq!(render("{scene}-{time}-{source}", &parts), "s-t-x");
    }

    #[test]
    fn sanitizes_forbidden_characters() {
        assert_eq!(sanitize("a/b:c*d?"), "a_b_c_d_");
        assert_eq!(sanitize("  fin.  "), "fin");
        assert_eq!(sanitize(""), "source");
        assert_eq!(sanitize("CON"), "_CON");
        assert_eq!(sanitize("com1"), "_com1");
        assert_eq!(sanitize("Console"), "Console");
    }

    #[test]
    fn unique_path_skips_taken_names() {
        let dir = std::env::temp_dir();
        let mut taken = HashSet::new();
        let stem = format!("polyptyque-test-{}", crate::config::new_id());
        let first = unique_path(&dir, &stem, "mkv", &taken);
        assert!(first.ends_with(format!("{stem}.mkv")));
        taken.insert(first);
        let second = unique_path(&dir, &stem, "mkv", &taken);
        assert!(second.ends_with(format!("{stem} (2).mkv")));
    }
}
