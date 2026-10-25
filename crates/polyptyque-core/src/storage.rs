//! Écriture sûre des fichiers de Polyptyque (réglages, journal de session).

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use tr::tr;

/// Écrit `data` dans `path` sans jamais laisser un fichier à moitié écrit :
/// fichier temporaire forcé sur le disque, puis renommage. Une coupure de
/// courant laisse soit l'ancien contenu, soit le nouveau.
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| tr!("création du dossier {}", dir.display()))?;
    }
    let tmp = sibling(path, ".tmp");
    let mut file = File::create(&tmp).with_context(|| tr!("écriture de {}", tmp.display()))?;
    file.write_all(data).with_context(|| tr!("écriture de {}", tmp.display()))?;
    file.sync_all().with_context(|| tr!("écriture sur le disque de {}", tmp.display()))?;
    drop(file);
    fs::rename(&tmp, path).with_context(|| tr!("remplacement de {}", path.display()))?;
    Ok(())
}

/// Vrai si l'on peut écrire dans `dir` (créé au besoin) : disque branché,
/// droits suffisants, place pour un fichier vide.
pub fn dir_writable(dir: &Path) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(format!(".polyptyque-essai-{}", crate::config::new_id()));
    let writable = fs::write(&probe, b"").is_ok();
    let _ = fs::remove_file(&probe);
    writable
}

/// Durée de conservation des journaux.
pub const LOG_RETENTION: Duration = Duration::from_secs(30 * 24 * 3600);

/// Efface les journaux de Polyptyque (`polyptyque_….log`, dossiers
/// `session_…` et rapports de problème `rapport_….txt` ou `report_….txt`) plus anciens que
/// `max_age`. Rien d'autre n'est touché.
/// Renvoie le nombre d'éléments effacés.
pub fn prune_old_logs(dir: &Path, max_age: Duration) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let ours = (meta.is_file() && name.starts_with("polyptyque_") && name.ends_with(".log"))
            || (meta.is_file()
                && crate::report::FILE_PREFIXES.iter().any(|p| name.starts_with(p))
                && name.ends_with(".txt"))
            || (meta.is_dir() && name.starts_with("session_"));
        let old = meta.modified().ok().and_then(|m| now.duration_since(m).ok()).is_some_and(|age| age > max_age);
        if !ours || !old {
            continue;
        }
        let result = if meta.is_dir() { fs::remove_dir_all(entry.path()) } else { fs::remove_file(entry.path()) };
        if result.is_ok() {
            removed += 1;
        }
    }
    removed
}

/// `config.toml` + `.precedent` → `config.toml.precedent`.
pub fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_content() {
        let dir = std::env::temp_dir().join(format!("polyptyque-storage-{}", crate::config::new_id()));
        let path = dir.join("sous-dossier").join("fichier.toml");
        write_atomic(&path, b"premier").unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        assert!(!sibling(&path, ".tmp").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn writable_dirs() {
        let dir = std::env::temp_dir().join(format!("polyptyque-storage-{}", crate::config::new_id()));
        assert!(dir_writable(&dir.join("nouveau")));
        assert_eq!(fs::read_dir(dir.join("nouveau")).unwrap().count(), 0);
        let file = dir.join("fichier");
        fs::write(&file, b"").unwrap();
        // Un fichier à la place du dossier : impossible d'y écrire.
        assert!(!dir_writable(&file.join("sous-dossier")));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_old_logs_are_pruned() {
        let dir = std::env::temp_dir().join(format!("polyptyque-storage-{}", crate::config::new_id()));
        fs::create_dir_all(dir.join("session_2026-01-01_10h00")).unwrap();
        fs::write(dir.join("polyptyque_2026-01-01_10h00.log"), b"").unwrap();
        fs::write(dir.join("autre.txt"), b"").unwrap();
        // Rien n'est assez vieux.
        assert_eq!(prune_old_logs(&dir, LOG_RETENTION), 0);
        // Tout ce qui est à nous est « vieux » avec une durée nulle.
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(prune_old_logs(&dir, Duration::ZERO), 2);
        assert!(dir.join("autre.txt").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sibling_appends_suffix() {
        assert_eq!(sibling(Path::new("a/config.toml"), ".precedent"), Path::new("a/config.toml.precedent"));
    }
}
