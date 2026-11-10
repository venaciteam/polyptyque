//! Mise à jour du plugin Stream Deck, côté interface (voir
//! `crate::streamdeck`) : pastille dans la barre du haut et bouton dans les
//! réglages tant que le plugin installé est plus ancien que celui livré.

use std::process::Command;
use std::time::Duration;

use tr::tr;

use super::App;
use crate::streamdeck::{self, Bundled, PluginUpdate};

/// Rythme de vérification du plugin installé (lecture d'un petit fichier).
pub(super) const CHECK_INTERVAL: Duration = Duration::from_secs(10);

pub(super) struct PluginState {
    /// Paquet livré, lu une fois au lancement.
    bundled: Option<Bundled>,
    /// Mise à jour proposée en ce moment.
    update: Option<PluginUpdate>,
}

impl PluginState {
    pub fn new() -> Self {
        let bundled = streamdeck::bundled();
        if let Some(b) = &bundled {
            log::info!("plugin Stream Deck livré : {}", b.version);
        }
        PluginState { bundled, update: None }
    }
}

impl App {
    /// Compare le plugin installé au paquet livré ; met à jour la pastille
    /// et les réglages si la réponse change.
    pub(super) fn check_plugin_update(&self) {
        let found = {
            let state = self.plugin.borrow();
            match &state.bundled {
                Some(bundled) => streamdeck::check(bundled),
                None => return,
            }
        };
        if self.plugin.borrow().update == found {
            return;
        }
        match &found {
            Some(u) => log::info!("plugin Stream Deck : mise à jour proposée ({} → {})", u.installed, u.bundled),
            None => log::info!("plugin Stream Deck : à jour"),
        }
        let (text, detail) = match &found {
            Some(u) => (
                tr!("Plugin Stream Deck : mise à jour"),
                tr!("Le plugin Stream Deck installé ({}) est plus ancien que celui de cette version de Polyptyque ({}).", u.installed, u.bundled),
            ),
            None => (String::new(), String::new()),
        };
        self.plugin.borrow_mut().update = found;
        self.with_bridge(|b| {
            b.set_plugin_update_text(text.into());
            b.set_plugin_update_detail(detail.into());
        });
    }

    /// Ouvre le paquet livré : Stream Deck demande la confirmation et
    /// remplace le plugin. La pastille disparaît à la vérification suivante.
    pub(super) fn install_plugin_update(&self) {
        let Some(update) = self.plugin.borrow().update.clone() else {
            return;
        };
        log::info!("plugin Stream Deck : installation de {} demandée", update.bundled);
        let opener = if cfg!(windows) { "explorer" } else { "xdg-open" };
        match Command::new(opener).arg(&update.file).spawn() {
            Ok(_) => self.toast(&tr!("Stream Deck va demander la confirmation de la mise à jour du plugin."), 1),
            Err(e) => self.toast(&tr!("Impossible d'ouvrir le plugin Stream Deck : {}", e), 3),
        }
    }
}
