//! Traduction des messages (voir « Langues et formats » dans le cahier des
//! charges).
//!
//! Le français est la langue du code : chaque texte affiché est écrit en
//! français dans `tr!("…")`, et le catalogue anglais (format gettext `.po`)
//! donne sa version anglaise. Le crate `tr` range un catalogue **par crate** :
//! chaque crate qui affiche des messages expose donc sa propre fonction
//! `set_catalog`, que l'interface appelle pour toutes au changement de langue.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use tr::Translator;

/// Catalogue d'une langue, lu depuis le contenu d'un fichier `.po`.
///
/// Lu avec rspolib à partir du texte : le `PoTranslator` du crate `tr` passe
/// le contenu en octets, que rspolib 0.1 ignore pour un `.po` (catalogue vide,
/// donc rien de traduit).
pub struct Catalog {
    /// Clé : texte français, précédé de son contexte et de `\u{4}` s'il en a
    /// un (convention de gettext).
    singular: HashMap<String, String>,
    /// Clé : idem, puis `\u{0}` et la forme plurielle française.
    plural: HashMap<String, Vec<String>>,
    /// Règle du pluriel : `n > 1` (français) au lieu de `n != 1` (anglais).
    plural_above_one: bool,
}

fn key(context: Option<&str>, msgid: &str) -> String {
    match context {
        Some(context) => format!("{context}\u{4}{msgid}"),
        None => msgid.to_string(),
    }
}

impl Catalog {
    pub fn parse(po: &str) -> Result<Self> {
        let file = rspolib::pofile(po).map_err(|e| anyhow!("catalogue de traduction illisible : {e}"))?;
        let mut singular = HashMap::new();
        let mut plural = HashMap::new();
        for entry in file.entries.into_iter().filter(|e| !e.obsolete) {
            let base = key(entry.msgctxt.as_deref(), &entry.msgid);
            match entry.msgid_plural {
                Some(msgid_plural) => {
                    if !entry.msgstr_plural.is_empty() && entry.msgstr_plural.iter().all(|s| !s.is_empty()) {
                        plural.insert(format!("{base}\u{0}{msgid_plural}"), entry.msgstr_plural);
                    }
                }
                None => {
                    if let Some(msgstr) = entry.msgstr.filter(|s| !s.is_empty()) {
                        singular.insert(base, msgstr);
                    }
                }
            }
        }
        let rule = file.metadata.get("Plural-Forms").map(|f| f.replace(' ', "")).unwrap_or_default();
        let plural_above_one = rule.contains("plural=(n>1)") || rule.contains("plural=n>1");
        Ok(Self { singular, plural, plural_above_one })
    }

    /// Nombre de textes traduits.
    pub fn len(&self) -> usize {
        self.singular.len() + self.plural.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Translator for Catalog {
    fn translate<'a>(&'a self, string: &'a str, context: Option<&'a str>) -> Cow<'a, str> {
        // Un texte absent du catalogue garde sa version française plutôt
        // que de disparaître de l'écran.
        match self.singular.get(&key(context, string)) {
            Some(text) => Cow::Borrowed(text.as_str()),
            None => Cow::Borrowed(string),
        }
    }

    fn ntranslate<'a>(&'a self, n: u64, singular: &'a str, plural: &'a str, context: Option<&'a str>) -> Cow<'a, str> {
        let forms = self.plural.get(&format!("{}\u{0}{plural}", key(context, singular)));
        let index = if self.plural_above_one { usize::from(n > 1) } else { usize::from(n != 1) };
        match forms.and_then(|forms| forms.get(index).or_else(|| forms.last())) {
            Some(text) => Cow::Borrowed(text.as_str()),
            None => Cow::Borrowed(if n > 1 { plural } else { singular }),
        }
    }
}

/// Le français, langue du code : les textes restent tels quels, avec la
/// règle du pluriel française (« 0 fichier », « 1 fichier », « 2 fichiers »),
/// que `tr` n'applique pas de lui-même.
pub struct French;

impl Translator for French {
    fn translate<'a>(&'a self, string: &'a str, _context: Option<&'a str>) -> Cow<'a, str> {
        Cow::Borrowed(string)
    }

    fn ntranslate<'a>(&'a self, n: u64, singular: &'a str, plural: &'a str, _context: Option<&'a str>) -> Cow<'a, str> {
        Cow::Borrowed(if n > 1 { plural } else { singular })
    }
}

/// Langue des messages de cette crate : `None` pour le français.
pub fn set_catalog(catalog: Option<Arc<Catalog>>) {
    match catalog {
        Some(catalog) => tr::set_translator!(catalog),
        None => tr::set_translator!(French),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PO: &str = r#"msgid ""
msgstr ""
"Content-Type: text/plain; charset=UTF-8\n"
"Plural-Forms: nplurals=2;plural=(n != 1);\n"

msgid "Réglages"
msgstr "Settings"

msgctxt "Type de source"
msgid "Navigateur {}"
msgstr "{} browser"

msgid "Navigateur {}"
msgstr "Browser {}"

msgid "{n} fichier"
msgid_plural "{n} fichiers"
msgstr[0] "{n} file"
msgstr[1] "{n} files"

msgid "Pas encore traduit"
msgstr ""
"#;

    #[test]
    fn translates_with_context_and_plurals() {
        let catalog = Catalog::parse(PO).unwrap();
        assert_eq!(catalog.len(), 4);
        assert_eq!(catalog.translate("Réglages", None), "Settings");
        assert_eq!(catalog.translate("Navigateur {}", Some("Type de source")), "{} browser");
        assert_eq!(catalog.translate("Navigateur {}", None), "Browser {}");
        assert_eq!(catalog.ntranslate(1, "{n} fichier", "{n} fichiers", None), "{n} file");
        assert_eq!(catalog.ntranslate(0, "{n} fichier", "{n} fichiers", None), "{n} files");
        // Pas de traduction : le texte français reste.
        assert_eq!(catalog.translate("Pas encore traduit", None), "Pas encore traduit");
        assert_eq!(catalog.translate("Inconnu", None), "Inconnu");
    }

    #[test]
    fn french_plural_rule() {
        assert_eq!(French.ntranslate(0, "{n} fichier", "{n} fichiers", None), "{n} fichier");
        assert_eq!(French.ntranslate(2, "{n} fichier", "{n} fichiers", None), "{n} fichiers");
    }
}
