//! Rapport de problème (« Signaler un problème » ; voir le cahier des
//! charges) : un fichier texte que la personne joint à une issue GitHub.
//!
//! Une issue est publique : le dossier personnel et le nom du compte sont
//! masqués dans tout le rapport. Le rapport est écrit en français, comme les
//! journaux qu'il reprend. Polyptyque n'envoie rien lui-même.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::naming;

/// Journaux des derniers lancements repris : un plantage se lit dans celui
/// du lancement précédent.
const LAUNCH_LOGS: usize = 3;
/// Fin de chaque journal de lancement reprise (une session de 12 h peut en
/// écrire bien plus).
const LAUNCH_LOG_TAIL: usize = 1_500_000;
/// Fin de chaque journal FFmpeg du dernier enregistrement.
const FFMPEG_LOG_LINES: usize = 60;

/// Ce que l'interface sait du système et des réglages.
pub struct ReportInfo {
    /// « 1.0.0-beta.5 (installée) ».
    pub version: String,
    /// « Windows 11 Pro 24H2 (26100.2033) ».
    pub system: String,
    /// « fr (Windows : fr-FR) ».
    pub language: String,
    pub graphics: Vec<String>,
    /// Résumé des réglages, sans aucun chemin.
    pub settings: Vec<String>,
}

/// Masque les données personnelles d'un texte : dossier personnel remplacé
/// par `%USERPROFILE%`, nom du compte retiré des chemins (une partie de
/// chemin égale au nom du compte, par exemple `D:\Leeva\Vidéos`).
pub struct Masker {
    home: Vec<String>,
    account: Option<String>,
}

impl Masker {
    pub fn new(home: Option<&Path>, account: Option<&str>) -> Self {
        let home = home
            .map(|h| h.display().to_string().trim_end_matches(['\\', '/']).to_string())
            .filter(|h| h.len() > 3)
            .map(|h| vec![h.replace('/', "\\"), h.replace('\\', "/")])
            .unwrap_or_default();
        // Un nom trop court masquerait n'importe quoi.
        let account = account.map(str::trim).filter(|a| a.chars().count() >= 2).map(str::to_string);
        Self { home, account }
    }

    /// Dossier personnel et nom du compte de la session en cours.
    pub fn from_environment() -> Self {
        let home = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf());
        let account = std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .ok()
            .or_else(|| home.as_ref().and_then(|h| h.file_name()).map(|n| n.to_string_lossy().into_owned()));
        Self::new(home.as_deref(), account.as_deref())
    }

    pub fn mask(&self, text: &str) -> String {
        let mut out = text.to_string();
        for home in &self.home {
            out = replace_ignore_ascii_case(&out, home, "%USERPROFILE%", |_, _| true);
        }
        if let Some(account) = &self.account {
            let is_separator = |c: Option<char>| matches!(c, Some('\\' | '/'));
            let ends_segment = |c: Option<char>| matches!(c, None | Some('\\' | '/' | '"' | '\'' | ' ' | '\n' | ')'));
            out = replace_ignore_ascii_case(&out, account, "<compte>", |before, after| {
                is_separator(before) && ends_segment(after)
            });
        }
        out
    }
}

/// Remplace `needle` (casse des lettres ASCII ignorée, comme dans les
/// chemins Windows) quand `accept(caractère avant, caractère après)`.
fn replace_ignore_ascii_case(
    text: &str,
    needle: &str,
    replacement: &str,
    accept: impl Fn(Option<char>, Option<char>) -> bool,
) -> String {
    if needle.is_empty() {
        return text.to_string();
    }
    // Minuscules ASCII : mêmes positions d'octets que le texte d'origine.
    let haystack = text.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut from = 0;
    while let Some(found) = haystack[from..].find(&needle_lower) {
        let start = from + found;
        let end = start + needle_lower.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        if accept(before, after) {
            out.push_str(&text[last..start]);
            out.push_str(replacement);
            last = end;
        }
        from = end;
    }
    out.push_str(&text[last..]);
    out
}

/// Texte complet du rapport, masqué.
pub fn build(info: &ReportInfo, logs_dir: &Path, masker: &Masker) -> String {
    let (date, time) = naming::now_parts();
    let mut out = String::new();
    out.push_str("Rapport Polyptyque\n==================\n\n");
    out.push_str(&format!("Version : {}\n", info.version));
    out.push_str(&format!("Système : {}\n", info.system));
    out.push_str(&format!("Langue : {}\n", info.language));
    let graphics = if info.graphics.is_empty() { "inconnue".to_string() } else { info.graphics.join(" ; ") };
    out.push_str(&format!("Cartes graphiques : {graphics}\n"));
    out.push_str(&format!("Rapport créé le {date} à {time}\n\n"));
    out.push_str("Réglages\n--------\n");
    for line in &info.settings {
        out.push_str(line);
        out.push('\n');
    }

    let launches =
        newest(logs_dir, |name, is_dir| !is_dir && name.starts_with("polyptyque_") && name.ends_with(".log"));
    out.push_str(&format!("\nJournaux des derniers lancements ({})\n", launches.len().min(LAUNCH_LOGS)));
    out.push_str("------------------------------------\n");
    // Du plus ancien au plus récent : l'ordre des événements.
    for path in launches.iter().take(LAUNCH_LOGS).rev() {
        out.push_str(&format!("\n=== {} ===\n", file_name(path)));
        out.push_str(&tail_bytes(path, LAUNCH_LOG_TAIL));
    }

    let sessions = newest(logs_dir, |name, is_dir| is_dir && name.starts_with("session_"));
    if let Some(session) = sessions.first() {
        out.push_str(&format!("\nDernier enregistrement : {}\n", file_name(session)));
        out.push_str("--------------------------------------------\n");
        let mut logs = newest(session, |name, is_dir| !is_dir && name.ends_with(".log"));
        logs.sort();
        for path in logs {
            out.push_str(&format!("\n=== {} (fin) ===\n", file_name(&path)));
            out.push_str(&tail_lines(&path, FFMPEG_LOG_LINES));
        }
    }
    masker.mask(&out)
}

/// Préfixes des noms de rapport : `rapport_…` en français, `report_…` en
/// anglais (le nom est vu par la personne qui le joint).
pub const FILE_PREFIXES: [&str; 2] = ["rapport_", "report_"];

/// Écrit le rapport dans `dir` : `<prefix><date>_<heure>.txt`.
pub fn write(dir: &Path, prefix: &str, text: &str) -> Result<PathBuf> {
    fs::create_dir_all(dir).with_context(|| format!("création du dossier {}", dir.display()))?;
    // Nom vu par la personne qui le joint : heure à la façon de sa langue.
    let (date, time) = naming::file_name_parts();
    let path = naming::unique_path(dir, &format!("{prefix}{date}_{time}"), "txt", &Default::default());
    fs::write(&path, text).with_context(|| format!("écriture de {}", path.display()))?;
    Ok(path)
}

/// Adresse de la page « nouvelle issue » avec un formulaire du dépôt
/// (`.github/ISSUE_TEMPLATE/<template>`), champs pré-remplis par leur
/// identifiant.
pub fn issue_url(repository: &str, template: &str, fields: &[(&str, &str)]) -> String {
    let mut url = format!("{}/issues/new?template={}", repository.trim_end_matches('/'), percent_encode(template));
    for (id, value) in fields {
        url.push_str(&format!("&{}={}", percent_encode(id), percent_encode(value)));
    }
    url
}

fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Éléments de `dir` retenus par `keep(nom, est_un_dossier)`, du plus récent
/// au plus ancien (les noms commencent par la date et l'heure).
fn newest(dir: &Path, keep: impl Fn(&str, bool) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|e| {
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            keep(&e.file_name().to_string_lossy(), is_dir)
        })
        .map(|e| (e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), e.path()))
        .collect();
    // Date de modification d'abord : deux lancements dans la même minute
    // ont le même nom de journal à un suffixe près.
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    found.into_iter().map(|(_, path)| path).collect()
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Fin d'un fichier, au plus `max` octets, coupée au début d'une ligne.
fn tail_bytes(path: &Path, max: usize) -> String {
    let Ok(data) = fs::read(path) else {
        return "(illisible)\n".into();
    };
    if data.len() <= max {
        return ensure_newline(String::from_utf8_lossy(&data).into_owned());
    }
    let tail = &data[data.len() - max..];
    let start = tail.iter().position(|&b| b == b'\n').map_or(0, |i| i + 1);
    let skipped = data.len() - tail.len() + start;
    ensure_newline(format!("[… {skipped} octets plus anciens omis …]\n{}", String::from_utf8_lossy(&tail[start..])))
}

fn tail_lines(path: &Path, count: usize) -> String {
    let Ok(text) = fs::read_to_string(path) else {
        return "(illisible)\n".into();
    };
    let lines: Vec<&str> = text.lines().collect();
    ensure_newline(lines[lines.len().saturating_sub(count)..].join("\n"))
}

fn ensure_newline(mut text: String) -> String {
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_home_folder_and_account_name() {
        let masker = Masker::new(Some(Path::new(r"C:\Users\Leeva")), Some("Leeva"));
        let text = r"enregistrement vers C:\Users\leeva\Videos\Polyptyque\a.mkv ; portable : D:\Leeva\Polyptyque\données ; c:/users/LEEVA/AppData";
        assert_eq!(
            masker.mask(text),
            r"enregistrement vers %USERPROFILE%\Videos\Polyptyque\a.mkv ; portable : D:\<compte>\Polyptyque\données ; %USERPROFILE%/AppData"
        );
        // Ailleurs que dans un chemin, le mot reste (« Leeva » peut être le
        // nom d'une scène, que l'avertissement mentionne).
        assert_eq!(masker.mask("scène Leeva"), "scène Leeva");
    }

    #[test]
    fn short_or_missing_names_mask_nothing() {
        let masker = Masker::new(None, Some("a"));
        assert_eq!(masker.mask(r"C:\a\b"), r"C:\a\b");
    }

    #[test]
    fn builds_a_prefilled_issue_address() {
        let url = issue_url(
            "https://github.com/venaciteam/Polyptyque",
            "bug-report.yml",
            &[("version", "1.0.0-beta.5 (portable)"), ("gpu", "NVIDIA GeForce RTX 3080 & co")],
        );
        assert_eq!(
            url,
            "https://github.com/venaciteam/Polyptyque/issues/new?template=bug-report.yml\
             &version=1.0.0-beta.5%20%28portable%29&gpu=NVIDIA%20GeForce%20RTX%203080%20%26%20co"
        );
    }

    #[test]
    fn report_takes_the_last_launches_and_the_last_session() {
        let dir = std::env::temp_dir().join(format!("polyptyque-rapport-{}", crate::config::new_id()));
        fs::create_dir_all(dir.join("session_2026-10-24_10h00")).unwrap();
        for (i, name) in ["polyptyque_2026-10-20_09h00.log", "polyptyque_2026-10-21_09h00.log"].iter().enumerate() {
            fs::write(dir.join(name), format!("lancement {i}\n")).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        fs::write(dir.join("session_2026-10-24_10h00").join("01-Discord.ffmpeg.log"), "frame=1\nfin\n").unwrap();
        let info = ReportInfo {
            version: "1.0.0".into(),
            system: "Windows".into(),
            language: "fr".into(),
            graphics: vec![],
            settings: vec!["Vidéo : 1920x1080".into()],
        };
        let text = build(&info, &dir, &Masker::new(None, None));
        let first = text.find("lancement 0").unwrap();
        let second = text.find("lancement 1").unwrap();
        assert!(first < second, "du plus ancien au plus récent");
        assert!(text.contains("=== 01-Discord.ffmpeg.log (fin) ===\nframe=1\nfin\n"));
        assert!(text.contains("Vidéo : 1920x1080"));
        let path = write(&dir, FILE_PREFIXES[0], &text).unwrap();
        assert!(file_name(&path).starts_with("rapport_"));
        fs::remove_dir_all(dir).unwrap();
    }
}
