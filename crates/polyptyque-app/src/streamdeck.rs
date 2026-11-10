//! Plugin Stream Deck livré avec Polyptyque : le paquet
//! (`city.vena.polyptyque.streamDeckPlugin`) est à côté de `polyptyque.exe`
//! et arrive avec les mises à jour de l'application. Si le plugin installé
//! dans Stream Deck est plus ancien, Polyptyque propose de le mettre à jour :
//! le paquet est ouvert, et Stream Deck demande lui-même la confirmation.
//!
//! Rien n'est proposé si le plugin n'est pas installé : Polyptyque ne pousse
//! pas le plugin aux personnes qui n'ont pas de Stream Deck.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Paquet livré à côté de `polyptyque.exe` (même nom que sur GitHub).
pub const PLUGIN_FILE: &str = "city.vena.polyptyque.streamDeckPlugin";
/// Dossier du plugin installé, dans `%APPDATA%\Elgato\StreamDeck\Plugins`.
const PLUGIN_DIR: &str = "city.vena.polyptyque.sdPlugin";
/// Taille maximale lue pour un manifeste (protection contre un fichier
/// abîmé).
const MANIFEST_LIMIT: u64 = 256 * 1024;

/// Paquet livré avec cette version de Polyptyque.
#[derive(Clone, Debug)]
pub struct Bundled {
    pub file: PathBuf,
    pub version: String,
}

/// Mise à jour proposée : plugin installé plus ancien que le paquet livré.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginUpdate {
    pub file: PathBuf,
    pub installed: String,
    pub bundled: String,
}

/// Paquet livré à côté de l'exécutable, et sa version. `None` : version
/// compilée à la main, ou paquet illisible.
pub fn bundled() -> Option<Bundled> {
    let exe = std::env::current_exe().ok()?;
    let file = exe.parent()?.join(PLUGIN_FILE);
    if !file.is_file() {
        return None;
    }
    match bundled_version(&file) {
        Some(version) => Some(Bundled { file, version }),
        None => {
            log::warn!("plugin Stream Deck : version du paquet livré illisible");
            None
        }
    }
}

/// Version lue dans le manifeste du paquet (une archive zip :
/// `city.vena.polyptyque.sdPlugin/manifest.json`).
fn bundled_version(file: &Path) -> Option<String> {
    let mut archive = zip::ZipArchive::new(File::open(file).ok()?).ok()?;
    let name = format!("{PLUGIN_DIR}/manifest.json");
    let entry = archive.by_name(&name).ok()?;
    let mut text = String::new();
    entry.take(MANIFEST_LIMIT).read_to_string(&mut text).ok()?;
    manifest_version(&text)
}

/// Version du plugin installé dans Stream Deck, `None` s'il ne l'est pas.
pub fn installed_version() -> Option<String> {
    let appdata = std::env::var_os("APPDATA")?;
    let manifest =
        Path::new(&appdata).join("Elgato").join("StreamDeck").join("Plugins").join(PLUGIN_DIR).join("manifest.json");
    let mut text = String::new();
    File::open(manifest).ok()?.take(MANIFEST_LIMIT).read_to_string(&mut text).ok()?;
    manifest_version(&text)
}

fn manifest_version(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    value.get("Version")?.as_str().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Mise à jour à proposer, d'après le plugin installé.
pub fn check(bundled: &Bundled) -> Option<PluginUpdate> {
    let installed = installed_version()?;
    is_newer(&bundled.version, &installed).then(|| PluginUpdate {
        file: bundled.file.clone(),
        installed,
        bundled: bundled.version.clone(),
    })
}

/// `candidate` est-elle plus récente que `current` ? Versions Stream Deck
/// en quatre nombres (« 0.1.0.3 ») ; une partie absente vaut 0. Une version
/// illisible n'est jamais plus récente.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| v.split('.').map(|p| p.trim().parse::<u64>()).collect::<Result<Vec<_>, _>>().ok();
    let (Some(mut a), Some(mut b)) = (parse(candidate), parse(current)) else {
        return false;
    };
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    a > b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparaison_des_versions() {
        assert!(is_newer("0.1.0.3", "0.1.0.2"));
        assert!(is_newer("0.2.0.0", "0.1.9.9"));
        assert!(is_newer("0.1.0.10", "0.1.0.9"));
        assert!(is_newer("1.0", "0.9.9.9"));
        assert!(!is_newer("0.1.0.2", "0.1.0.2"));
        assert!(!is_newer("0.1", "0.1.0.0"));
        assert!(!is_newer("0.1.0.1", "0.1.0.2"));
        assert!(!is_newer("abc", "0.1.0.0"));
        assert!(!is_newer("0.1.0.3", "x"));
    }

    #[test]
    fn version_du_manifeste() {
        assert_eq!(
            manifest_version("\u{feff}{\"Name\": \"Polyptyque\", \"Version\": \"0.1.0.3\"}").as_deref(),
            Some("0.1.0.3")
        );
        assert_eq!(manifest_version("{\"Version\": \"\"}"), None);
        assert_eq!(manifest_version("pas du json"), None);
    }

    #[test]
    fn version_du_paquet() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("polyptyque-test-plugin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(PLUGIN_FILE);
        {
            let mut zip = zip::ZipWriter::new(File::create(&file).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file(format!("{PLUGIN_DIR}/manifest.json"), options).unwrap();
            zip.write_all(b"{\"Version\": \"0.1.0.3\"}").unwrap();
            zip.finish().unwrap();
        }
        assert_eq!(bundled_version(&file).as_deref(), Some("0.1.0.3"));
        std::fs::write(&file, b"pas une archive").unwrap();
        assert_eq!(bundled_version(&file), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
