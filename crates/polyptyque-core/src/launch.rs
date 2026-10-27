//! Fin inattendue d'un lancement (plantage, fermeture forcée par le pilote
//! graphique, coupure de courant) : un témoin est posé au démarrage et retiré
//! à chaque fermeture normale. Témoin encore présent au lancement suivant :
//! le précédent s'est arrêté sans passer par là.
//!
//! Le témoin contient le nom du journal du lancement : le rapport de problème
//! désigne ainsi celui qui s'est mal terminé.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use crate::storage;

/// Début de la ligne écrite au journal quand le lancement précédent s'est
/// mal terminé ; le rapport de problème la cherche dans les journaux.
pub const UNEXPECTED_END: &str = "lancement précédent terminé sans fermeture normale";

/// Pose le témoin du lancement en cours (`log_name` : nom de son journal).
/// Renvoie le nom du journal du lancement précédent s'il ne s'est pas
/// terminé normalement (« inconnu » s'il n'est pas lisible).
pub fn begin(marker: &Path, log_name: &str) -> Option<String> {
    let previous = match fs::read_to_string(marker) {
        Ok(text) => Some(text.trim().to_string()).filter(|t| !t.is_empty()).or_else(|| Some("inconnu".into())),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(_) => Some("inconnu".into()),
    };
    if let Err(e) = storage::write_atomic(marker, log_name.as_bytes()) {
        log::warn!("témoin de lancement non écrit : {e:#}");
    }
    previous
}

/// Fermeture normale : retire le témoin.
pub fn end(marker: &Path) {
    if let Err(e) = fs::remove_file(marker) {
        if e.kind() != ErrorKind::NotFound {
            log::warn!("témoin de lancement non retiré : {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launch_without_normal_end_is_reported_once() {
        let dir = std::env::temp_dir().join(format!("polyptyque-lancement-{}", crate::config::new_id()));
        fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("lancement-en-cours.txt");
        assert_eq!(begin(&marker, "polyptyque_a.log"), None);
        // Pas de `end` : plantage.
        assert_eq!(begin(&marker, "polyptyque_b.log").as_deref(), Some("polyptyque_a.log"));
        end(&marker);
        assert_eq!(begin(&marker, "polyptyque_c.log"), None);
        end(&marker);
        end(&marker);
        fs::remove_dir_all(dir).unwrap();
    }
}
