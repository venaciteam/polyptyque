//! Programmation des enregistrements, côté interface : exécute les décisions
//! du planificateur (`polyptyque_core::schedule`), pose les questions,
//! affiche les programmations et les modifie.

use chrono::{DateTime, Datelike, Duration as Span, Local, NaiveDate, Timelike, Utc};
use polyptyque_core::schedule::{
    self, clock_label, day_name, when_label, Action, Repeat, Schedule, ScheduleError, Slot, TimeOfDay, PROMPT_COUNTDOWN,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tr::tr;

use super::{App, Confirm};
use crate::{format, ScheduleItem};

/// Question posée par la programmation, en attente de réponse.
pub(super) enum Prompt {
    /// Démarrage manqué : sans réponse, l'enregistrement démarre.
    Missed { slot: Slot, deadline: DateTime<Utc> },
    /// Enregistrement lancé à la main pendant un créneau : sans réponse, la
    /// programmation ne l'arrête pas.
    KeepAutoStop { slot: Slot, deadline: DateTime<Utc> },
    /// Enregistrement manuel déjà en cours au début du créneau.
    ManualRunning { slot: Slot },
}

/// Choix « Répétition » du formulaire.
const REPEAT_ONCE: i32 = 0;
const REPEAT_DAILY: i32 = 1;
const REPEAT_DAYS: i32 = 2;

impl App {
    // ----- Décisions du planificateur --------------------------------------

    /// Deux fois par seconde : anti-veille, questions en attente, décisions.
    pub(super) fn tick_schedule(&self) {
        let now = Utc::now();
        self.update_schedule_awake(now);
        // Mise en veille en cours : on attend le réveil (sans même faire
        // tourner les comptes à rebours). Passé 30 s, la veille a eu lieu
        // (ou a été annulée) : on reprend.
        if let Some(since) = self.suspended.get() {
            if now - since < Span::seconds(30) {
                return;
            }
            self.suspended.set(None);
        }
        self.tick_prompt(now);
        let busy = self.busy.get() || self.scheduled_start.borrow().is_some();
        let recording = self.recording();
        let action = {
            let config = self.config.borrow();
            self.planner.borrow_mut().tick(&config.schedules, now, &Local, recording, busy)
        };
        if let Some(action) = action {
            self.on_schedule_action(action, now);
        }
        self.save_planner();
        self.refresh_schedule_pill(now);
        if self.with_bridge_value(|b| b.get_sheet() == "schedules") {
            self.refresh_schedule_list(now);
        }
    }

    pub(super) fn save_planner(&self) {
        if let Err(e) = self.planner.borrow_mut().save_if_changed(&self.schedule_state_path) {
            log::warn!("{e:#}");
        }
    }

    fn on_schedule_action(&self, action: Action, now: DateTime<Utc>) {
        match action {
            Action::Start(slot) => self.start_scheduled(slot),
            Action::AskMissed(slot) => {
                log::info!("créneau « {} » commencé sans Polyptyque : question posée", self.slot_scene(&slot));
                *self.prompt.borrow_mut() = Some(Prompt::Missed { slot, deadline: now + PROMPT_COUNTDOWN });
                // La question doit se voir : la fenêtre réduite revient.
                if let Some(ui) = self.ui.upgrade() {
                    ui.window().set_minimized(false);
                }
                self.show_prompt(now);
            }
            Action::ManualRunning(slot) => {
                log::info!(
                    "créneau « {} » : enregistrement manuel déjà en cours, rien n'est lancé",
                    self.slot_scene(&slot)
                );
                if self.prompt.borrow().is_some() {
                    self.toast(&self.manual_running_text(&slot), 1);
                } else {
                    *self.prompt.borrow_mut() = Some(Prompt::ManualRunning { slot });
                    self.show_prompt(now);
                }
            }
            Action::Stop(slot) => {
                log::info!("fin du créneau « {} » : arrêt programmé", self.slot_scene(&slot));
                self.stop_recording(Some(tr!("Fin du créneau programmé.")));
            }
        }
    }

    /// Lance la scène d'un créneau qui commence (ou dont le démarrage
    /// manqué est accepté).
    fn start_scheduled(&self, slot: Slot) {
        let scene = self.config.borrow().scene(&slot.scene_id).map(|s| (s.name.clone(), s.source_ids.len()));
        let problem = match scene {
            None => Some(tr!("sa scène n'existe plus")),
            Some((name, 0)) => Some(tr!("la scène « {} » n'a aucune source", name)),
            Some(_) if self.recording() || self.busy.get() => Some(tr!("un enregistrement est déjà en cours")),
            Some(_) => None,
        };
        if let Some(problem) = problem {
            log::warn!("créneau programmé non lancé : {problem}");
            self.planner.borrow_mut().give_up(&slot);
            self.toast(&tr!("Enregistrement programmé impossible : {}.", problem), 3);
            return;
        }
        log::info!("créneau « {} » : démarrage de l'enregistrement programmé", self.slot_scene(&slot));
        if self.config.borrow().active_scene != slot.scene_id {
            self.select_scene(&slot.scene_id);
        }
        *self.scheduled_start.borrow_mut() = Some(slot.clone());
        self.start_recording(None);
        // Refusé tout de suite (aucune source) : rien ne se prépare.
        if !self.busy.get() && !self.recording() && self.scheduled_start.borrow_mut().take().is_some() {
            self.planner.borrow_mut().give_up(&slot);
        }
    }

    /// Un enregistrement vient de démarrer (`message` : celui du démarrage).
    /// Programmé, il est confié à la programmation ; lancé à la main pendant
    /// un créneau, on propose de garder l'arrêt automatique.
    pub(super) fn recording_started(&self, message: &str, tone: i32) {
        let now = Utc::now();
        if let Some(slot) = self.scheduled_start.borrow_mut().take() {
            self.toast(
                &format!(
                    "{message} {}",
                    tr!("Enregistrement programmé : arrêt prévu à {}.", clock_label(slot.end, &Local))
                ),
                tone,
            );
            self.planner.borrow_mut().started(slot);
            return;
        }
        self.toast(message, tone);
        let slot = {
            let config = self.config.borrow();
            self.planner.borrow().manual_slot(&config.schedules, now, &Local)
        };
        let Some(slot) = slot else {
            return;
        };
        // Plus rien à lancer dans ce créneau, quelle que soit la réponse.
        self.planner.borrow_mut().claim(&slot, false);
        let deadline = (now + PROMPT_COUNTDOWN).min(slot.end);
        *self.prompt.borrow_mut() = Some(Prompt::KeepAutoStop { slot, deadline });
        self.show_prompt(now);
    }

    /// L'enregistrement en cours s'arrête pour de bon (arrêt voulu, disque
    /// plein, fin du créneau) : un enregistrement programmé n'est pas relancé
    /// dans son créneau, et la question sur l'arrêt automatique n'a plus
    /// d'objet.
    pub(super) fn recording_stopping(&self) {
        self.planner.borrow_mut().recording_stopped();
        if matches!(*self.prompt.borrow(), Some(Prompt::KeepAutoStop { .. })) {
            self.close_prompt();
        }
    }

    /// Le démarrage d'un enregistrement a échoué.
    pub(super) fn recording_start_failed(&self) {
        if let Some(slot) = self.scheduled_start.borrow_mut().take() {
            self.planner.borrow_mut().give_up(&slot);
        }
    }

    // ----- Questions ---------------------------------------------------------

    fn show_prompt(&self, now: DateTime<Utc>) {
        let guard = self.prompt.borrow();
        let Some(prompt) = guard.as_ref() else {
            return;
        };
        let (kind, title, text, accept, decline) = match prompt {
            Prompt::Missed { slot, .. } => (
                "missed",
                tr!("Enregistrement programmé"),
                tr!(
                    "Le créneau « {} » a commencé à {} sans que Polyptyque puisse le lancer (fermé, ou ordinateur en veille). Lancer l'enregistrement maintenant, jusqu'à {} ?",
                    self.slot_scene(slot),
                    clock_label(slot.start, &Local),
                    clock_label(slot.end, &Local)
                ),
                tr!("Enregistrer maintenant"),
                tr!("Ne pas enregistrer"),
            ),
            Prompt::KeepAutoStop { slot, .. } => {
                let end = clock_label(slot.end, &Local);
                (
                    "auto-stop",
                    tr!("Créneau programmé en cours"),
                    tr!(
                        "Le créneau d'enregistrement « {} » est en cours et se termine à {end}. Arrêter automatiquement cet enregistrement à {end} ?",
                        self.slot_scene(slot),
                        end
                    ),
                    tr!("Arrêter à {}", end),
                    tr!("Ne pas l'arrêter"),
                )
            }
            Prompt::ManualRunning { slot } => (
                "info",
                tr!("Créneau programmé"),
                self.manual_running_text(slot),
                tr!("Compris"),
                String::new(),
            ),
        };
        drop(guard);
        let countdown = self.countdown_text(now);
        self.with_bridge(|b| {
            b.set_prompt_title(title.into());
            b.set_prompt_text(text.into());
            b.set_prompt_accept(accept.into());
            b.set_prompt_decline(decline.into());
            b.set_prompt_countdown(countdown.into());
            b.set_prompt_kind(kind.into());
        });
    }

    fn manual_running_text(&self, slot: &Slot) -> String {
        tr!(
            "Le créneau « {} » commence, mais un enregistrement est déjà en cours : il continue tel quel. La programmation ne lance rien et ne l'arrêtera pas à {}.",
            self.slot_scene(slot),
            clock_label(slot.end, &Local)
        )
    }

    fn countdown_text(&self, now: DateTime<Utc>) -> String {
        match self.prompt.borrow().as_ref() {
            Some(Prompt::Missed { deadline, .. }) => {
                tr!("Sans réponse, l'enregistrement démarre dans {}.", remaining(*deadline - now))
            }
            Some(Prompt::KeepAutoStop { slot, deadline }) => tr!(
                "Sans réponse dans {}, il continuera après {}.",
                remaining(*deadline - now),
                clock_label(slot.end, &Local)
            ),
            _ => String::new(),
        }
    }

    /// Compte à rebours, et réponse par défaut une fois le délai écoulé.
    fn tick_prompt(&self, now: DateTime<Utc>) {
        let expired = match self.prompt.borrow().as_ref() {
            None => return,
            Some(Prompt::Missed { slot, deadline }) => {
                if now >= slot.end {
                    // Créneau terminé avant la réponse : plus rien à lancer.
                    Some(false)
                } else {
                    (now >= *deadline).then_some(true)
                }
            }
            Some(Prompt::KeepAutoStop { deadline, .. }) => (now >= *deadline).then_some(false),
            Some(Prompt::ManualRunning { slot }) => (now >= slot.end).then_some(true),
        };
        match expired {
            Some(true) => {
                log::info!("question de la programmation sans réponse : réponse par défaut (oui)");
                self.prompt_accepted();
            }
            Some(false) => {
                log::info!("question de la programmation sans réponse : réponse par défaut (non)");
                self.prompt_declined();
            }
            None => {
                let text = self.countdown_text(now);
                self.with_bridge(|b| b.set_prompt_countdown(text.into()));
            }
        }
    }

    fn close_prompt(&self) {
        self.prompt.borrow_mut().take();
        self.with_bridge(|b| b.set_prompt_kind(SharedString::new()));
    }

    /// Une question de démarrage manqué est affichée.
    pub(super) fn missed_prompt_open(&self) -> bool {
        matches!(*self.prompt.borrow(), Some(Prompt::Missed { .. }))
    }

    pub(super) fn prompt_accepted(&self) {
        let prompt = self.prompt.borrow_mut().take();
        self.with_bridge(|b| b.set_prompt_kind(SharedString::new()));
        match prompt {
            Some(Prompt::Missed { slot, .. }) => {
                if Utc::now() >= slot.end {
                    self.planner.borrow_mut().give_up(&slot);
                } else {
                    self.start_scheduled(slot);
                }
            }
            Some(Prompt::KeepAutoStop { slot, .. }) => {
                if self.recording() && Utc::now() < slot.end {
                    log::info!(
                        "arrêt automatique accepté pour l'enregistrement manuel ({})",
                        clock_label(slot.end, &Local)
                    );
                    self.toast(&tr!("L'enregistrement s'arrêtera à {}.", clock_label(slot.end, &Local)), 1);
                    self.planner.borrow_mut().claim(&slot, true);
                }
            }
            Some(Prompt::ManualRunning { .. }) | None => {}
        }
        self.save_planner();
    }

    pub(super) fn prompt_declined(&self) {
        let prompt = self.prompt.borrow_mut().take();
        self.with_bridge(|b| b.set_prompt_kind(SharedString::new()));
        match prompt {
            Some(Prompt::Missed { slot, .. }) => {
                log::info!("démarrage manqué refusé : rien dans ce créneau");
                self.planner.borrow_mut().give_up(&slot);
                self.toast(&tr!("Enregistrement programmé annulé pour ce créneau. Les suivants restent prévus."), 1);
            }
            Some(Prompt::KeepAutoStop { slot, .. }) if self.recording() => {
                log::info!("enregistrement manuel gardé après la fin du créneau");
                self.toast(&tr!("L'enregistrement continuera après {}.", clock_label(slot.end, &Local)), 1);
            }
            Some(Prompt::KeepAutoStop { .. }) => {}
            Some(Prompt::ManualRunning { .. }) | None => {}
        }
        self.save_planner();
    }

    // ----- Arrêt et fermeture ------------------------------------------------

    /// Arrêt demandé à la main (bouton, raccourci, dernière source) : pendant
    /// un enregistrement dont la programmation a la charge, on confirme.
    pub(super) fn request_stop(&self) {
        let owned = self.planner.borrow().owned().cloned();
        match owned {
            Some(slot) => self.ask(
                Confirm::StopScheduled,
                &tr!("Créneau d'enregistrement en cours"),
                &tr!(
                    "Un créneau d'enregistrement programmé est en cours (« {} », jusqu'à {}). L'arrêter quand même ? Il ne sera pas relancé avant le prochain créneau.",
                    self.slot_scene(&slot),
                    clock_label(slot.end, &Local)
                ),
                &tr!("Arrêter"),
            ),
            None => self.stop_recording(None),
        }
    }

    /// Texte de l'avertissement avant de quitter, si une programmation doit
    /// encore lancer un enregistrement.
    pub(super) fn quit_schedule_warning(&self) -> Option<String> {
        let now = Utc::now();
        let config = self.config.borrow();
        let slot = schedule::next_slot(&config.schedules, now, &Local)?;
        let scene = config.scene(&slot.scene_id).map(|s| s.name.clone()).unwrap_or_default();
        Some(tr!(
            "« {} » est programmé {} : Polyptyque doit rester ouvert pour le lancer. Quitter quand même ?",
            scene,
            when_label(slot.start, now, &Local)
        ))
    }

    /// Mise en veille : l'enregistrement programmé interrompu sera proposé
    /// au réveil (question du démarrage manqué) ; rien n'est lancé d'ici là.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(super) fn schedule_suspend(&self) -> bool {
        self.suspended.set(Some(Utc::now()));
        let owned = self.planner.borrow().owned().is_some();
        if owned {
            self.planner.borrow_mut().recording_interrupted();
        }
        owned
    }

    /// Réveil : une question de démarrage manqué restée affichée pendant la
    /// veille reprend son compte à rebours de zéro, pour laisser le temps de
    /// répondre à qui revient devant l'écran.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(super) fn schedule_resume(&self) {
        self.suspended.set(None);
        let now = Utc::now();
        if let Some(Prompt::Missed { deadline, .. }) = self.prompt.borrow_mut().as_mut() {
            *deadline = now + PROMPT_COUNTDOWN;
        }
        self.tick_prompt(now);
    }

    // ----- Anti-veille, lancement au démarrage --------------------------------

    fn update_schedule_awake(&self, now: DateTime<Utc>) {
        let active = schedule::any_active(&self.config.borrow().schedules, now, &Local);
        if active == self.schedules_awake.get() {
            return;
        }
        self.schedules_awake.set(active);
        log::info!(
            "{}",
            if active {
                "programmation active : mise en veille et extinction de l'écran empêchées"
            } else {
                "plus de programmation active : mise en veille de nouveau permise"
            }
        );
        #[cfg(windows)]
        polyptyque_windows::keep_awake_for_schedules(active);
    }

    /// Fin de Polyptyque : la veille est de nouveau permise.
    pub(super) fn release_schedule_awake(&self) {
        if self.schedules_awake.replace(false) {
            #[cfg(windows)]
            polyptyque_windows::keep_awake_for_schedules(false);
        }
    }

    /// Inscrit ou retire Polyptyque du démarrage de Windows selon le réglage.
    /// `explicit` : choix que l'utilisateur vient de faire (lève aussi une
    /// désactivation faite dans les Paramètres de Windows).
    pub(super) fn apply_launch_at_startup(&self, explicit: bool) {
        let enabled = self.config.borrow().settings.launch_at_startup;
        #[cfg(windows)]
        {
            use polyptyque_windows::startup;
            let result = if enabled {
                std::env::current_exe().map_err(anyhow::Error::from).and_then(|exe| startup::enable(&exe, explicit))
            } else if explicit {
                startup::disable()
            } else {
                Ok(())
            };
            if let Err(e) = result {
                log::warn!("{e:#}");
                self.toast(&tr!("Lancement au démarrage de Windows : {}", format!("{e:#}")), 3);
            } else if explicit {
                log::info!("lancement au démarrage de Windows : {}", if enabled { "activé" } else { "désactivé" });
            }
        }
        #[cfg(not(windows))]
        let _ = (enabled, explicit);
        let note = self.startup_note();
        self.with_bridge(|b| {
            b.set_startup_enabled(enabled);
            b.set_startup_note(note.into());
        });
    }

    fn startup_note(&self) -> String {
        let enabled = self.config.borrow().settings.launch_at_startup;
        #[cfg(windows)]
        if enabled && polyptyque_windows::startup::blocked_by_windows() {
            return tr!("Windows empêche ce lancement (désactivé dans Paramètres > Applications > Démarrage) : le réactiver là-bas, ou couper puis remettre cette option.");
        }
        if enabled && polyptyque_core::paths::portable_root().is_some() {
            return tr!("Version portable : ce lancement est inscrit dans Windows. Couper l'option avant d'emporter le dossier sur un autre PC.");
        }
        String::new()
    }

    pub(super) fn set_launch_at_startup(&self, enabled: bool) {
        self.config.borrow_mut().settings.launch_at_startup = enabled;
        self.save();
        self.apply_launch_at_startup(true);
    }

    // ----- Affichage -----------------------------------------------------------

    fn slot_scene(&self, slot: &Slot) -> String {
        self.config.borrow().scene(&slot.scene_id).map(|s| s.name.clone()).unwrap_or_else(|| tr!("scène supprimée"))
    }

    /// Pastille de la barre du haut : prochain créneau, ou fin prévue de
    /// l'enregistrement programmé.
    /// Textes de la programmation recalculés (changement de langue ou
    /// d'horloge).
    pub(super) fn refresh_schedule_texts(&self) {
        let now = Utc::now();
        self.refresh_schedule_pill(now);
        self.refresh_schedule_list(now);
    }

    fn refresh_schedule_pill(&self, now: DateTime<Utc>) {
        let owned = self.planner.borrow().owned().cloned();
        let (text, running) = match owned {
            Some(slot) if self.recording() => (tr!("Arrêt programmé à {}", clock_label(slot.end, &Local)), true),
            _ => {
                let config = self.config.borrow();
                let planner = self.planner.borrow();
                let scene = |slot: &Slot| config.scene(&slot.scene_id).map(|s| s.name.clone()).unwrap_or_default();
                // Créneau en cours encore à traiter (question posée,
                // démarrage) : il passe avant le suivant.
                let current = config
                    .schedules
                    .iter()
                    .filter(|s| s.enabled)
                    .filter_map(|s| s.slot_at(now, &Local))
                    .find(|slot| !planner.is_done(&slot.key()));
                let text = match current {
                    Some(slot) => tr!("{} · créneau jusqu'à {}", scene(&slot), clock_label(slot.end, &Local)),
                    None => schedule::next_slot(&config.schedules, now, &Local)
                        .map(|slot| format!("{} · {}", scene(&slot), when_label(slot.start, now, &Local)))
                        .unwrap_or_default(),
                };
                (text, false)
            }
        };
        self.with_bridge(|b| {
            if b.get_schedule_text() != text.as_str() {
                b.set_schedule_text(text.into());
            }
            if b.get_schedule_running() != running {
                b.set_schedule_running(running);
            }
        });
    }

    pub(super) fn open_schedules(&self) {
        self.refresh_schedule_list(Utc::now());
        self.apply_launch_at_startup(false);
        self.with_bridge(|b| b.set_sheet("schedules".into()));
    }

    fn refresh_schedule_list(&self, now: DateTime<Utc>) {
        let owned = self.planner.borrow().owned().cloned().filter(|_| self.recording());
        let config = self.config.borrow();
        let items: Vec<ScheduleItem> = config
            .schedules
            .iter()
            .map(|s| {
                let locked = owned.as_ref().is_some_and(|slot| slot.schedule_id == s.id);
                let next = if locked {
                    tr!("Enregistre jusqu'à {}", clock_label(owned.as_ref().map_or(now, |o| o.end), &Local))
                } else if !s.enabled {
                    tr!("Désactivée")
                } else if let Some(slot) = s.slot_at(now, &Local) {
                    if self.planner.borrow().is_done(&slot.key()) {
                        tr!(
                            "Créneau en cours, sans enregistrement programmé (jusqu'à {})",
                            clock_label(slot.end, &Local)
                        )
                    } else {
                        tr!("Créneau en cours")
                    }
                } else if let Some(slot) = s.next_slot(now, &Local) {
                    tr!("Prochain : {}", when_label(slot.start, now, &Local))
                } else {
                    tr!("Terminée")
                };
                ScheduleItem {
                    id: s.id.as_str().into(),
                    scene: config.scene(&s.scene_id).map(|scene| scene.name.as_str()).unwrap_or_default().into(),
                    times: s.times_label().into(),
                    repeat: s.repeat_label().into(),
                    next: next.into(),
                    enabled: s.enabled,
                    locked,
                }
            })
            .collect();
        drop(config);
        let model = &self.schedule_items;
        if model.row_count() != items.len() {
            model.set_vec(items);
            return;
        }
        for (i, item) in items.into_iter().enumerate() {
            if model.row_data(i).as_ref() != Some(&item) {
                model.set_row_data(i, item);
            }
        }
    }

    // ----- Modification --------------------------------------------------------

    /// Ouvre le formulaire d'une programmation (`id` vide : nouvelle).
    pub(super) fn edit_schedule(&self, id: &str) {
        if self.is_locked_schedule(id) {
            self.toast(
                &tr!("Cette programmation enregistre en ce moment : modifiable une fois son créneau terminé."),
                2,
            );
            return;
        }
        let config = self.config.borrow();
        let existing = config.schedules.iter().find(|s| s.id == id).cloned();
        let scenes: Vec<(String, String)> = config.scenes.iter().map(|s| (s.id.clone(), s.name.clone())).collect();
        let scene_id = existing.as_ref().map_or(config.active_scene.clone(), |s| s.scene_id.clone());
        drop(config);
        let now = Local::now();
        let (start, end, repeat) = match &existing {
            Some(s) => (s.start, s.end, s.repeat.clone()),
            None => {
                // Par défaut : l'heure pile suivante, pendant une heure, aujourd'hui.
                let hour = (now.hour() + 1) % 24;
                let start = TimeOfDay::new(hour, 0).unwrap_or(TimeOfDay::new(21, 0).expect("heure valide"));
                let end = TimeOfDay::new((hour + 1) % 24, 0).unwrap_or(start);
                let date =
                    if hour == 0 { now.date_naive().succ_opt().unwrap_or(now.date_naive()) } else { now.date_naive() };
                (start, end, Repeat::Once { date })
            }
        };
        let (repeat_index, date, days) = match &repeat {
            Repeat::Once { date } => (REPEAT_ONCE, *date, vec![false; 7]),
            Repeat::Weekly { days } => {
                let flags: Vec<bool> = (1..=7u8).map(|d| days.contains(&d)).collect();
                let index = if flags.iter().all(|f| *f) { REPEAT_DAILY } else { REPEAT_DAYS };
                (index, now.date_naive(), flags)
            }
        };
        let scene_index = scenes.iter().position(|(id, _)| *id == scene_id).unwrap_or(0) as i32;
        let names: Vec<SharedString> = scenes.iter().map(|(_, name)| name.as_str().into()).collect();
        *self.schedule_scene_ids.borrow_mut() = scenes.into_iter().map(|(id, _)| id).collect();
        *self.editing_schedule.borrow_mut() = existing.map(|s| s.id);
        self.schedule_days.set_vec(days);
        self.with_bridge(|b| {
            b.set_schedule_scene_choices(ModelRc::new(VecModel::from(names)));
            b.set_schedule_scene_index(scene_index);
            b.set_schedule_start(start.label().into());
            b.set_schedule_end(end.label().into());
            b.set_schedule_repeat_index(repeat_index);
            b.set_schedule_error(SharedString::new());
            b.set_sheet(if id.is_empty() { "schedule-new" } else { "schedule-edit" }.into());
        });
        self.set_schedule_date(date, now.date_naive());
        self.schedule_times_edited();
    }

    /// Listes de la date (« une seule fois ») : années proposées, l'année en
    /// cours et la suivante, plus celle de la date si elle n'en fait pas
    /// partie (programmation passée qu'on modifie).
    fn set_schedule_date(&self, date: NaiveDate, today: NaiveDate) {
        let mut years = vec![today.year(), today.year() + 1];
        if !years.contains(&date.year()) {
            years.push(date.year());
            years.sort_unstable();
        }
        let year_index = years.iter().position(|y| *y == date.year()).unwrap_or(0) as i32;
        let year_names: Vec<SharedString> = years.iter().map(|y| y.to_string().into()).collect();
        let month_names: Vec<SharedString> = (1..=12).map(|m| polyptyque_core::clock::month_name(m).into()).collect();
        *self.schedule_years.borrow_mut() = years;
        self.with_bridge(|b| {
            b.set_schedule_year_choices(ModelRc::new(VecModel::from(year_names)));
            b.set_schedule_year_index(year_index);
            b.set_schedule_month_choices(ModelRc::new(VecModel::from(month_names)));
            b.set_schedule_month_index(date.month0() as i32);
            b.set_schedule_day_index(date.day0() as i32);
        });
        self.schedule_date_edited();
    }

    /// Date choisie : liste des jours du mois (le jour se cale sur le
    /// dernier si le mois est plus court : 31 octobre → 30 novembre), et
    /// jour de la semaine rappelé sous les listes.
    pub(super) fn schedule_date_edited(&self) {
        let date = self.schedule_date();
        let days: Vec<SharedString> = (1..=polyptyque_core::clock::days_in_month(date.year(), date.month()))
            .map(|d| d.to_string().into())
            .collect();
        let note = tr!("Le créneau commence un {}.", day_name(date.weekday().number_from_monday() as u8));
        self.with_bridge(|b| {
            b.set_schedule_day_choices(ModelRc::new(VecModel::from(days)));
            b.set_schedule_day_index(date.day0() as i32);
            b.set_schedule_date_note(note.into());
        });
    }

    /// Date des trois listes, toujours valide (jour ramené au dernier du
    /// mois).
    fn schedule_date(&self) -> NaiveDate {
        let (day, month, year_index) = self.with_bridge_value(|b| {
            (b.get_schedule_day_index(), b.get_schedule_month_index(), b.get_schedule_year_index())
        });
        let today = Local::now().date_naive();
        let year = self.schedule_years.borrow().get(year_index.max(0) as usize).copied().unwrap_or(today.year());
        let month = (month.clamp(0, 11) + 1) as u32;
        let day = ((day.max(0) + 1) as u32).min(polyptyque_core::clock::days_in_month(year, month));
        NaiveDate::from_ymd_opt(year, month, day).unwrap_or(today)
    }

    /// Heures saisies : signale une fin le lendemain.
    pub(super) fn schedule_times_edited(&self) {
        let (start, end) = self.with_bridge_value(|b| {
            (TimeOfDay::parse(&b.get_schedule_start()), TimeOfDay::parse(&b.get_schedule_end()))
        });
        let note = match (start, end) {
            (Some(start), Some(end)) if end < start => tr!("Se termine le lendemain."),
            _ => String::new(),
        };
        self.with_bridge(|b| b.set_schedule_times_note(note.into()));
    }

    pub(super) fn toggle_schedule_day(&self, index: i32) {
        let index = index.max(0) as usize;
        if let Some(value) = self.schedule_days.row_data(index) {
            self.schedule_days.set_row_data(index, !value);
        }
    }

    pub(super) fn close_schedule_editor(&self) {
        self.editing_schedule.borrow_mut().take();
        self.open_schedules();
    }

    pub(super) fn save_schedule(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let b = ui.global::<crate::Bridge>();
        let fail = |message: &str| b.set_schedule_error(message.into());
        let editing = self.editing_schedule.borrow().clone();
        if editing.as_deref().is_some_and(|id| self.is_locked_schedule(id)) {
            fail(&tr!("Cette programmation enregistre en ce moment : modifiable une fois son créneau terminé."));
            return;
        }
        let Some(scene_id) =
            self.schedule_scene_ids.borrow().get(b.get_schedule_scene_index().max(0) as usize).cloned()
        else {
            fail(&tr!("Aucune scène choisie."));
            return;
        };
        let Some(start) = TimeOfDay::parse(&b.get_schedule_start()) else {
            fail(&tr!("Heure de début invalide (exemple : {}).", example(21, 0)));
            return;
        };
        let Some(end) = TimeOfDay::parse(&b.get_schedule_end()) else {
            fail(&tr!("Heure de fin invalide (exemple : {}).", example(22, 30)));
            return;
        };
        let now = Utc::now();
        let repeat = match b.get_schedule_repeat_index() {
            REPEAT_ONCE => Repeat::Once { date: self.schedule_date() },
            REPEAT_DAILY => Repeat::Weekly { days: (1..=7).collect() },
            _ => Repeat::Weekly {
                days: (1..=7u8).filter(|d| self.schedule_days.row_data(usize::from(d - 1)).unwrap_or(false)).collect(),
            },
        };
        let mut schedule = Schedule::new(&scene_id, start, end, repeat);
        if let Some(id) = &editing {
            schedule.id = id.clone();
            schedule.enabled = self.config.borrow().schedules.iter().find(|s| s.id == *id).is_none_or(|s| s.enabled);
        }
        if let Err(e) = schedule.validate(&self.config.borrow().schedules, now, &Local) {
            fail(&format::capitalize(&self.schedule_error_text(&e)));
            return;
        }
        {
            let mut config = self.config.borrow_mut();
            match config.schedules.iter_mut().find(|s| s.id == schedule.id) {
                Some(existing) => *existing = schedule.clone(),
                None => config.schedules.push(schedule.clone()),
            }
        }
        self.planner.borrow_mut().schedule_changed(&schedule.id, now);
        self.save();
        self.save_planner();
        log::info!(
            "programmation enregistrée : {} · {} · {}",
            self.config.borrow().scene(&schedule.scene_id).map(|s| s.name.clone()).unwrap_or_default(),
            schedule.times_label(),
            schedule.repeat_label()
        );
        self.editing_schedule.borrow_mut().take();
        let next = schedule.next_slot(now, &Local).or_else(|| schedule.slot_at(now, &Local));
        let message = match next {
            Some(slot) if slot.contains(now) => tr!("Programmation enregistrée : son créneau a déjà commencé."),
            Some(slot) => tr!("Programmation enregistrée : prochain créneau {}.", when_label(slot.start, now, &Local)),
            None => tr!("Programmation enregistrée."),
        };
        self.open_schedules();
        self.toast(&message, 1);
    }

    pub(super) fn toggle_schedule(&self, id: &str, enabled: bool) {
        if self.is_locked_schedule(id) {
            return;
        }
        let now = Utc::now();
        let Some(mut schedule) = self.config.borrow().schedules.iter().find(|s| s.id == id).cloned() else {
            return;
        };
        schedule.enabled = enabled;
        if enabled {
            if let Err(e) = schedule.validate(&self.config.borrow().schedules, now, &Local) {
                self.toast(&tr!("Impossible de l'activer : {}", self.schedule_error_text(&e)), 2);
                self.refresh_schedule_list(now);
                return;
            }
        }
        if let Some(existing) = self.config.borrow_mut().schedules.iter_mut().find(|s| s.id == id) {
            existing.enabled = enabled;
        }
        self.planner.borrow_mut().schedule_changed(id, now);
        self.save();
        self.save_planner();
        self.refresh_schedule_list(now);
    }

    pub(super) fn delete_schedule(&self, id: &str) {
        if self.is_locked_schedule(id) {
            self.toast(
                &tr!("Cette programmation enregistre en ce moment : supprimable une fois son créneau terminé."),
                2,
            );
            return;
        }
        self.config.borrow_mut().schedules.retain(|s| s.id != id);
        self.planner.borrow_mut().schedule_changed(id, Utc::now());
        self.save();
        self.save_planner();
        self.refresh_schedule_list(Utc::now());
        self.toast(&tr!("Programmation supprimée."), 1);
    }

    /// Programmation dont l'enregistrement est en cours : ni modifiable ni
    /// supprimable avant la fin de son créneau.
    fn is_locked_schedule(&self, id: &str) -> bool {
        self.recording() && self.planner.borrow().owned().is_some_and(|slot| slot.schedule_id == id)
    }

    fn schedule_error_text(&self, error: &ScheduleError) -> String {
        match error {
            ScheduleError::SameTimes => tr!("l'heure de fin doit être différente de l'heure de début."),
            ScheduleError::NoDay => tr!("aucun jour choisi."),
            ScheduleError::InPast => tr!("ce créneau est déjà passé : choisir une autre date ou une autre heure."),
            ScheduleError::Overlap(id) => {
                let config = self.config.borrow();
                match config.schedules.iter().find(|s| s.id == *id) {
                    Some(other) => tr!(
                        "ce créneau chevauche la programmation « {} » ({}, {}) : un seul enregistrement à la fois.",
                        config.scene(&other.scene_id).map(|s| s.name.as_str()).unwrap_or_default(),
                        other.times_label(),
                        other.repeat_label().to_lowercase()
                    ),
                    None => tr!("ce créneau chevauche une autre programmation : un seul enregistrement à la fois."),
                }
            }
        }
    }

    /// Nombre de programmations d'une scène (pour la confirmation de sa
    /// suppression).
    pub(super) fn scene_schedule_count(&self, scene_id: &str) -> usize {
        self.config.borrow().schedules.iter().filter(|s| s.scene_id == scene_id).count()
    }
}

/// Heure d'exemple dans un message, selon l'horloge.
fn example(hour: u32, minute: u32) -> String {
    polyptyque_core::clock::time(hour, minute)
}

/// « 1 min 47 s », « 12 s ».
fn remaining(span: Span) -> String {
    let seconds = span.num_seconds().max(0);
    if seconds >= 60 {
        tr!("{} min {} s", seconds / 60, format!("{:02}", seconds % 60))
    } else {
        tr!("{} s", seconds)
    }
}
