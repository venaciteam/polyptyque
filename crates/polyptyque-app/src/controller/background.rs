//! Arrière-plan : fenêtre cachée, icône dans la zone de notification (près
//! de l'horloge), pour que les programmations se lancent sans fenêtre
//! ouverte. Lancé par Windows, Polyptyque y démarre ; fermer la fenêtre
//! alors qu'une programmation est prévue l'y envoie, selon le réglage ou
//! la réponse à la question posée.
//!
//! Hors Windows (mode démo), l'arrière-plan est une simple fenêtre réduite.

use polyptyque_core::config::CloseChoice;
use slint::ComponentHandle;
use tr::tr;

use super::App;

impl App {
    /// Lancé par Windows à l'ouverture de session : en arrière-plan dès que
    /// la fenêtre existe (Slint ne peut pas la créer cachée).
    pub(super) fn hide_at_startup(&self) {
        if !self.hide_pending.get() {
            return;
        }
        #[cfg(windows)]
        if self.main_window().is_none() {
            return;
        }
        self.hide_pending.set(false);
        self.hide_to_background();
    }

    /// Cache la fenêtre ; l'icône de la zone de notification la remplace.
    /// Aperçus et vumètres s'arrêtent : personne ne les regarde.
    #[cfg(windows)]
    pub(super) fn hide_to_background(&self) {
        let Some(window) = self.main_window() else {
            return;
        };
        if self.in_background.get() {
            return;
        }
        self.watch_tray(window);
        log::info!("Polyptyque continue en arrière-plan (zone de notification)");
        self.in_background.set(true);
        *self.tray_tip.borrow_mut() = self.tray_tip_text();
        polyptyque_windows::tray::set_recording(window, self.recording());
        polyptyque_windows::tray::show(window, &self.tray_tip.borrow());
        if let Some(ui) = self.ui.upgrade() {
            let _ = ui.hide();
        }
        self.previews.borrow_mut().stop();
        self.meters.borrow_mut().stop();
    }

    #[cfg(not(windows))]
    pub(super) fn hide_to_background(&self) {
        if let Some(ui) = self.ui.upgrade() {
            ui.window().set_minimized(true);
        }
    }

    /// Fait revenir la fenêtre, réduite ou cachée (arrière-plan).
    pub(super) fn show_from_background(&self) {
        // Une question posée juste après le lancement par Windows : la
        // fenêtre reste affichée.
        self.hide_pending.set(false);
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if !self.in_background.replace(false) {
            ui.window().set_minimized(false);
            return;
        }
        log::info!("fenêtre de Polyptyque de nouveau affichée");
        let _ = ui.show();
        #[cfg(windows)]
        if let Some(window) = self.main_window() {
            polyptyque_windows::tray::hide(window);
            polyptyque_windows::tray::bring_to_front(window);
        }
        self.restart_previews();
    }

    /// Fin de Polyptyque : l'icône ne doit pas rester dans la zone de
    /// notification.
    pub(super) fn leave_background(&self) {
        #[cfg(windows)]
        if self.in_background.replace(false) {
            if let Some(window) = self.main_window() {
                polyptyque_windows::tray::hide(window);
            }
        }
    }

    /// Actions sur l'icône (reçues une fois la fenêtre cachée).
    #[cfg(windows)]
    fn watch_tray(&self, window: isize) {
        use polyptyque_windows::tray::{self, TrayEvent};
        if self.tray_watch.replace(true) {
            return;
        }
        // Reçues pendant le traitement d'un message de Windows : la suite
        // est planifiée sur la boucle de l'interface.
        let watched = tray::watch(window, |event| match event {
            TrayEvent::Show => super::post(|app| app.show_from_background()),
            TrayEvent::Quit => super::post(|app| {
                log::info!("« Quitter Polyptyque » depuis la zone de notification");
                let _ = app.quit_requested(true);
            }),
        });
        if let Err(e) = watched {
            log::warn!("{e:#}");
        }
    }

    /// Info-bulle de l'icône : ce que fait Polyptyque en arrière-plan.
    fn tray_tip_text(&self) -> String {
        let mut tip = String::from("Polyptyque");
        if self.recording() {
            tip.push('\n');
            tip.push_str(&tr!("Enregistrement en cours"));
        }
        let schedule = self.with_bridge_value(|b| b.get_schedule_text().to_string());
        if !schedule.is_empty() {
            tip.push('\n');
            tip.push_str(&schedule);
        }
        tip
    }

    /// Deux fois par seconde : icône (point rouge pendant un
    /// enregistrement) et info-bulle tenues à jour.
    pub(super) fn tick_background(&self) {
        if !self.in_background.get() {
            return;
        }
        #[cfg(windows)]
        if let Some(window) = self.main_window() {
            polyptyque_windows::tray::set_recording(window, self.recording());
        }
        let tip = self.tray_tip_text();
        if *self.tray_tip.borrow() == tip {
            return;
        }
        #[cfg(windows)]
        if let Some(window) = self.main_window() {
            polyptyque_windows::tray::set_tip(window, &tip);
        }
        *self.tray_tip.borrow_mut() = tip;
    }

    /// Un enregistrement vient de démarrer : en arrière-plan, Windows
    /// l'annonce par une notification (l'icône peut être rangée sous « ^ »)
    /// et l'icône prend son point rouge. Programmé ou demandé par un
    /// programme, il ne doit jamais passer inaperçu.
    pub(super) fn notify_recording_started(&self, message: &str) {
        if !self.in_background.get() {
            return;
        }
        #[cfg(windows)]
        if let Some(window) = self.main_window() {
            polyptyque_windows::tray::set_recording(window, true);
            polyptyque_windows::tray::notify_user(window, &tr!("Enregistrement en cours"), message);
        }
        #[cfg(not(windows))]
        let _ = message;
    }

    // ----- Fermeture avec une programmation prévue ----------------------------

    /// Fermer la fenêtre alors qu'une programmation doit lancer un
    /// enregistrement : continuer en arrière-plan, ou quitter quand même ?
    pub(super) fn ask_close_with_schedules(&self, (scene, when): &(String, String)) {
        let text = tr!(
            "« {} » est programmé {} : Polyptyque doit rester ouvert pour le lancer. Il peut continuer en arrière-plan, avec son icône près de l'horloge.",
            scene,
            when
        );
        self.with_bridge(|b| {
            b.set_close_text(text.into());
            b.set_close_remember(false);
            b.set_sheet("close-schedules".into());
        });
    }

    /// Réponse à cette question. Case « Se souvenir de mon choix » cochée :
    /// le réglage est enregistré, la question ne sera plus posée.
    fn close_answered(&self, background: bool) {
        let remember = self.with_bridge_value(|b| b.get_close_remember());
        self.close_sheet();
        if remember {
            let choice = if background { CloseChoice::Background } else { CloseChoice::Quit };
            log::info!("fermeture avec une programmation prévue : choix retenu ({choice:?})");
            self.config.borrow_mut().settings.close_with_schedules = choice;
            self.save();
        }
        if background {
            self.hide_to_background();
        } else {
            self.quit_with_schedules.set(true);
            // La suite comme une fermeture normale (conversion en cours…).
            let _ = self.close_requested();
        }
    }

    pub(super) fn close_to_background(&self) {
        self.close_answered(true);
    }

    pub(super) fn close_anyway(&self) {
        self.close_answered(false);
    }
}
