//! Emplacements standards des fichiers de Polyptyque.
//!
//! Version installée, sous Windows :
//! - configuration : `%APPDATA%\Polyptyque\config.toml`
//! - profils de navigateur et journaux : `%LOCALAPPDATA%\Polyptyque\…`
//!
//! Version portable : tout cela dans le dossier `données`, à côté du
//! programme, pour passer d'un PC à l'autre sans rien perdre.
//!
//! Dans les deux cas, enregistrements dans le dossier choisi, par défaut
//! `Vidéos\Polyptyque`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use directories::{BaseDirs, UserDirs};

const APP_DIR: &str = "Polyptyque";
/// Dossier des données de la version portable, à sa racine.
pub const PORTABLE_DATA_DIR: &str = "données";

/// Racine de la version portable, ou `None` pour la version installée.
///
/// Le zip portable (fabriqué par Velopack) contient le programme dans
/// `<racine>\current` et un marqueur `<racine>\.portable`. Les données vont
/// dans `<racine>\données`, hors de `current` : ce dossier est remplacé à
/// chaque mise à jour.
pub fn portable_root() -> Option<PathBuf> {
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    ROOT.get_or_init(|| std::env::current_exe().ok().and_then(|exe| portable_root_of(&exe))).clone()
}

fn portable_root_of(exe: &Path) -> Option<PathBuf> {
    let current = exe.parent()?;
    if !current.file_name().is_some_and(|n| n.eq_ignore_ascii_case(OsStr::new("current"))) {
        return None;
    }
    let root = current.parent()?;
    root.join(".portable").is_file().then(|| root.to_path_buf())
}

pub fn config_file() -> PathBuf {
    if let Some(root) = portable_root() {
        return root.join(PORTABLE_DATA_DIR).join("config.toml");
    }
    BaseDirs::new().map(|b| b.config_dir().join(APP_DIR)).unwrap_or_else(|| PathBuf::from(APP_DIR)).join("config.toml")
}

pub fn local_data_dir() -> PathBuf {
    if let Some(root) = portable_root() {
        return root.join(PORTABLE_DATA_DIR);
    }
    BaseDirs::new().map(|b| b.data_local_dir().join(APP_DIR)).unwrap_or_else(|| PathBuf::from(APP_DIR))
}

pub fn profiles_dir() -> PathBuf {
    local_data_dir().join("profils")
}

pub fn logs_dir() -> PathBuf {
    local_data_dir().join("journaux")
}

/// Journal de l'enregistrement en cours (voir `recovery`).
pub fn session_record_file() -> PathBuf {
    local_data_dir().join("session-en-cours.toml")
}

/// Témoin du lancement en cours de l'interface (voir `launch`).
pub fn launch_marker_file() -> PathBuf {
    local_data_dir().join("lancement-en-cours.txt")
}

/// Créneaux de programmation déjà réglés (voir `schedule::Planner`).
pub fn schedule_state_file() -> PathBuf {
    local_data_dir().join("programmations-etat.toml")
}

/// Hors Windows : dossier du mode démonstration (fausses fenêtres), où
/// l'interface range sa configuration ; le mode commande l'y lit aussi.
#[cfg(not(windows))]
pub fn demo_dir() -> PathBuf {
    std::env::temp_dir().join("polyptyque-demo")
}

pub fn default_output_dir() -> PathBuf {
    UserDirs::new()
        .and_then(|u| u.video_dir().map(|p| p.join(APP_DIR)))
        .or_else(|| BaseDirs::new().map(|b| b.home_dir().join("Videos").join(APP_DIR)))
        .unwrap_or_else(|| PathBuf::from(APP_DIR))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_layout_is_recognised() {
        let root = std::env::temp_dir().join(format!("polyptyque-portable-{}", crate::config::new_id()));
        let exe = root.join("current").join("polyptyque.exe");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        // Sans marqueur : version installée (même disposition de dossiers).
        assert_eq!(portable_root_of(&exe), None);
        std::fs::write(root.join(".portable"), b"").unwrap();
        assert_eq!(portable_root_of(&exe), Some(root.clone()));
        // Programme lancé ailleurs (développement) : pas portable.
        assert_eq!(portable_root_of(&root.join("polyptyque.exe")), None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
