//! Langue des messages du mode commande : celle de Polyptyque (réglage
//! « Langue », sinon celle de Windows), avec le catalogue de l'interface.
//! Le serveur MCP et le catalogue JSON parlent anglais : ils s'adressent à
//! un modèle de langage.

use std::sync::Arc;

use polyptyque_core::config::LanguageChoice;
use polyptyque_core::i18n::{Catalog, French};

/// Catalogue anglais de l'interface, commun à tous les exécutables.
const ENGLISH_PO: &str = include_str!("../../polyptyque-app/lang/en/LC_MESSAGES/polyptyque-app.po");

/// Vrai pour l'anglais : réglage de Polyptyque, sinon langue du système
/// (Windows en français → français, toute autre langue → anglais).
/// `POLYPTYQUE_LANG=fr` ou `en` impose une langue (essais).
pub fn english(choice: LanguageChoice) -> bool {
    match std::env::var("POLYPTYQUE_LANG").as_deref() {
        Ok("fr") => return false,
        Ok("en") => return true,
        _ => {}
    }
    match choice {
        LanguageChoice::French => false,
        LanguageChoice::English => true,
        LanguageChoice::Auto => {
            !sys_locale::get_locale().is_some_and(|locale| locale.to_ascii_lowercase().starts_with("fr"))
        }
    }
}

/// Langue des messages, dans toutes les crates utilisées.
pub fn set(english: bool) {
    let catalog = english.then(|| Catalog::parse(ENGLISH_PO).ok().map(Arc::new)).flatten();
    polyptyque_core::i18n::set_catalog(catalog.clone());
    #[cfg(windows)]
    polyptyque_windows::set_catalog(catalog.clone());
    match catalog {
        Some(catalog) => tr::set_translator!(catalog),
        None => tr::set_translator!(French),
    }
}
