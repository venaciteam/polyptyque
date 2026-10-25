//! Langue de l'interface (voir « Langues et formats » dans le cahier des
//! charges) : choix de la langue et application à toutes les parties du
//! programme.
//!
//! Deux catalogues `.po`, intégrés à l'exécutable :
//! - `lang/en/…` : tous les textes en anglais (interface Slint et messages
//!   Rust des crates `polyptyque-*`) ;
//! - `lang/fr/…` : les textes anglais propres aux composants de Slint (menu
//!   des champs de texte, sélecteurs de date et d'heure), et les pluriels de
//!   l'interface, pour que Slint applique la règle française.
//!
//! Le français étant la langue du code, un texte absent d'un catalogue
//! s'affiche en français.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use polyptyque_core::config::LanguageChoice;
use polyptyque_core::i18n::{Catalog, French};

/// Contenu du catalogue anglais, lu à la compilation.
const ENGLISH_PO: &str = include_str!("../lang/en/LC_MESSAGES/polyptyque-app.po");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    French,
    English,
}

static ENGLISH: AtomicBool = AtomicBool::new(false);

impl Language {
    /// Langue d'affichage de Windows : le français si Windows est en
    /// français, l'anglais pour **toute autre langue**. Slint seul, faute de
    /// catalogue pour la langue du système, garderait la langue du code, le
    /// français : un Windows allemand afficherait du français.
    ///
    /// `POLYPTYQUE_LANG=fr` ou `en` impose une langue (essais).
    pub fn system() -> Self {
        match std::env::var("POLYPTYQUE_LANG").as_deref() {
            Ok("fr") => return Self::French,
            Ok("en") => return Self::English,
            _ => {}
        }
        match sys_locale::get_locale() {
            Some(locale) if locale.to_ascii_lowercase().starts_with("fr") => Self::French,
            _ => Self::English,
        }
    }

    /// Langue du réglage « Langue ».
    pub fn from_choice(choice: LanguageChoice) -> Self {
        match choice {
            LanguageChoice::Auto => Self::system(),
            LanguageChoice::French => Self::French,
            LanguageChoice::English => Self::English,
        }
    }

    /// Langue en cours, pour les mises en forme (nombres, listes…).
    pub fn current() -> Self {
        if ENGLISH.load(Ordering::Relaxed) {
            Self::English
        } else {
            Self::French
        }
    }

    /// Nom du dossier de son catalogue dans `lang/`.
    fn code(self) -> &'static str {
        match self {
            Self::French => "fr",
            Self::English => "en",
        }
    }
}

fn english_catalog() -> Option<Arc<Catalog>> {
    static CATALOG: OnceLock<Option<Arc<Catalog>>> = OnceLock::new();
    CATALOG
        .get_or_init(|| match Catalog::parse(ENGLISH_PO) {
            Ok(catalog) if catalog.is_empty() => {
                log::error!("catalogue anglais vide");
                None
            }
            Ok(catalog) => Some(Arc::new(catalog)),
            Err(e) => {
                log::error!("{e:#}");
                None
            }
        })
        .clone()
}

/// Langue des messages calculés en Rust, dans toutes les crates. Peut être
/// appelée avant la création de la fenêtre (erreur au démarrage).
pub fn set_language(language: Language) {
    let catalog = match language {
        Language::French => None,
        Language::English => english_catalog(),
    };
    polyptyque_core::i18n::set_catalog(catalog.clone());
    polyptyque_engine::set_catalog(catalog.clone());
    #[cfg(windows)]
    polyptyque_windows::set_catalog(catalog.clone());
    match catalog {
        Some(catalog) => tr::set_translator!(catalog),
        None => tr::set_translator!(French),
    }
    ENGLISH.store(language == Language::English, Ordering::Relaxed);
    log::info!("langue : {}", language.code());
}

/// Langue des textes de l'interface Slint. Slint exige que la fenêtre
/// existe déjà.
pub fn apply_to_ui(language: Language) {
    if let Err(e) = slint::select_bundled_translation(language.code()) {
        log::warn!("langue {} indisponible dans l'interface : {e:?}", language.code());
    }
}
