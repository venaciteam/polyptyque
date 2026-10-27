//! Parcours de connexion. Les lectures et écritures restent hors du fil UI.

use std::collections::HashMap;
use std::path::PathBuf;
use std::thread;

use slint::{ComponentHandle, Model, ModelRc, VecModel};
use tr::tr;

use super::{post, App};
use crate::assistants::{self, Action, Assistant, Snapshot, State};
use crate::AssistantItem;

pub(super) struct SetupState {
    snapshots: Vec<Snapshot>,
    overrides: HashMap<Assistant, PathBuf>,
    busy: bool,
}

impl Default for SetupState {
    fn default() -> Self {
        let saved: HashMap<String, PathBuf> =
            std::fs::read(choices_path()).ok().and_then(|data| serde_json::from_slice(&data).ok()).unwrap_or_default();
        Self {
            snapshots: Vec::new(),
            busy: false,
            overrides: saved.into_iter().filter_map(|(id, path)| Assistant::from_id(&id).map(|a| (a, path))).collect(),
        }
    }
}

fn choices_path() -> PathBuf {
    #[cfg(windows)]
    return polyptyque_core::paths::local_data_dir().join("connexions-assistants.json");
    #[cfg(not(windows))]
    return polyptyque_core::paths::demo_dir().join("connexions-assistants.json");
}

impl App {
    pub(super) fn open_assistants(&self) {
        // Le bouton est un consentement explicite dans la fenêtre. Seule
        // l'autorisation de pilotage est appliquée ici ; les autres champs
        // des réglages restent intacts pour le retour à la feuille.
        let enabled = self.with_bridge_value(|b| b.get_settings_remote_control());
        if !enabled {
            return;
        }
        if !self.config.borrow().settings.remote_control {
            let mut config = self.config.borrow_mut();
            config.settings.remote_control = true;
            if let Err(e) = config.save(&self.config_path) {
                config.settings.remote_control = false;
                drop(config);
                self.toast(&tr!("Impossible d'enregistrer la configuration : {}", format!("{e:#}")), 3);
                return;
            }
            drop(config);
            self.apply_remote_control();
        }
        self.with_bridge(|b| {
            b.set_sheet("assistants".into());
            b.set_assistants_message("".into());
        });
        if self.with_bridge_value(|b| b.get_assistants().row_count() == 0) {
            self.with_bridge(|b| b.set_assistants_message(tr!("L'autorisation de pilotage est activée. Elle peut être retirée à tout moment dans les réglages.").into()));
        }
        self.refresh_assistants();
    }

    pub(super) fn close_assistants(&self) {
        self.with_bridge(|b| b.set_sheet("settings".into()));
    }

    pub(super) fn refresh_assistants(&self) {
        if self.assistant_setup.borrow().busy {
            return;
        }
        let overrides = self.assistant_setup.borrow().overrides.clone();
        self.assistant_setup.borrow_mut().busy = true;
        self.with_bridge(|b| b.set_assistants_busy(true));
        thread::spawn(move || {
            let cli = assistants::cli_path();
            let targets: Vec<_> = assistants::targets()
                .into_iter()
                .map(|mut target| {
                    if let Some(path) = overrides.get(&target.assistant) {
                        target.path = path.clone();
                        target.detected = true;
                    }
                    target
                })
                .collect();
            let snapshots = targets
                .into_iter()
                .map(|target| assistants::inspect(target, cli.as_deref().unwrap_or_else(|_| std::path::Path::new(""))))
                .collect();
            post(move |app| {
                let mut setup = app.assistant_setup.borrow_mut();
                setup.snapshots = snapshots;
                setup.busy = false;
                drop(setup);
                app.with_bridge(|b| {
                    b.set_assistants_busy(false);
                    if let Err(e) = cli {
                        b.set_assistants_message(e.to_string().into());
                        b.set_assistants_message_tone(3);
                    }
                });
                app.refresh_assistant_rows();
            });
        });
    }

    pub(super) fn refresh_assistant_rows(&self) {
        let connected = self.control_client_names();
        let setup = self.assistant_setup.borrow();
        let rows: Vec<_> = setup.snapshots.iter().map(|snapshot| {
            let assistant = snapshot.target.assistant;
            let active = connected.iter().any(|name| name == assistant.name() || (assistant == Assistant::ClaudeDesktop && name == "Claude"));
            let configured = matches!(snapshot.state, State::Configured | State::Repair);
            let (state, tone) = if active && configured {
                (tr!("Connecté"), 1)
            } else if active {
                (tr!("Session encore connectée"), 2)
            } else {
                match snapshot.state {
                    State::Missing => (tr!("Non détecté"), 0),
                    State::Available => (tr!("Détecté"), 0),
                    State::Configured => (tr!("Configuré · en attente"), 0),
                    State::Repair => (tr!("À réparer"), 2),
                    State::Conflict => (tr!("Nom déjà utilisé"), 2),
                    State::Error => (tr!("Configuration à vérifier"), 3),
                }
            };
            AssistantItem {
                id: assistant.id().into(), name: assistant.name().into(), state: state.into(), tone,
                detail: if active && matches!(snapshot.state, State::Available | State::Missing) {
                    tr!("La configuration est retirée, mais une session utilise encore Polyptyque. Relancer l'assistant pour terminer la déconnexion.")
                } else { snapshot.detail.clone() }.into(),
                configured,
                can_connect: cfg!(windows) && matches!(snapshot.state, State::Available | State::Configured | State::Repair),
                can_disconnect: cfg!(windows) && configured,
            }
        }).collect();
        self.with_bridge(|b| b.set_assistants(ModelRc::new(VecModel::from(rows))));
    }

    pub(super) fn configure_assistant(&self, id: &str, disconnect: bool) {
        if !cfg!(windows) || self.assistant_setup.borrow().busy {
            return;
        }
        let Some(assistant) = Assistant::from_id(id) else { return };
        let snapshot =
            self.assistant_setup.borrow().snapshots.iter().find(|s| s.target.assistant == assistant).cloned();
        let Some(Snapshot { target, state, .. }) = snapshot else { return };
        if !matches!(state, State::Available | State::Configured | State::Repair) {
            return;
        }
        let cli = match assistants::cli_path() {
            Ok(cli) => cli,
            Err(e) => {
                self.assistant_message(&e.to_string(), 3);
                return;
            }
        };
        self.assistant_setup.borrow_mut().busy = true;
        self.with_bridge(|b| {
            b.set_assistants_busy(true);
            b.set_assistants_message("".into());
        });
        thread::spawn(move || {
            let action = if disconnect { Action::Disconnect } else { Action::Connect };
            let result = assistants::configure(&target, &cli, action);
            let snapshot = assistants::inspect(target, &cli);
            post(move |app| {
                let mut setup = app.assistant_setup.borrow_mut();
                if let Some(row) = setup.snapshots.iter_mut().find(|s| s.target.assistant == assistant) {
                    *row = snapshot;
                }
                setup.busy = false;
                drop(setup);
                app.with_bridge(|b| b.set_assistants_busy(false));
                match result {
                    Ok(()) => app.assistant_message(
                        &if disconnect {
                            tr!(
                                "Connexion retirée pour {}. Relancer l'assistant pour terminer la déconnexion.",
                                assistant.name()
                            )
                        } else {
                            tr!("Connexion préparée pour {}. {}", assistant.name(), assistant.next_step())
                        },
                        1,
                    ),
                    Err(e) => app.assistant_message(&format!("{e:#}"), 3),
                }
                app.refresh_assistant_rows();
            });
        });
    }

    fn assistant_message(&self, message: &str, tone: i32) {
        self.with_bridge(|b| {
            b.set_assistants_message(message.into());
            b.set_assistants_message_tone(tone);
        });
    }

    pub(super) fn browse_assistant_config(&self, id: &str) {
        if self.assistant_setup.borrow().busy {
            return;
        }
        let Some(assistant) = Assistant::from_id(id) else { return };
        #[cfg(windows)]
        {
            let Some(ui) = self.ui.upgrade() else { return };
            let extensions = match assistant {
                Assistant::Codex => vec!["toml"],
                Assistant::Hermes => vec!["yaml", "yml"],
                _ => vec!["json", "json5"],
            };
            let path = rfd::FileDialog::new()
                .set_parent(&ui.window().window_handle())
                .set_title(tr!("Configuration de {}", assistant.name()))
                .add_filter(assistant.name(), &extensions)
                .pick_file();
            if let Some(path) = path {
                let mut overrides = self.assistant_setup.borrow().overrides.clone();
                overrides.insert(assistant, path);
                let saved: HashMap<&str, &PathBuf> = overrides.iter().map(|(a, path)| (a.id(), path)).collect();
                let result = serde_json::to_vec_pretty(&saved)
                    .map_err(anyhow::Error::from)
                    .and_then(|data| polyptyque_core::storage::write_atomic(&choices_path(), &data));
                if let Err(e) = result {
                    self.assistant_message(&tr!("Impossible de conserver le choix de configuration : {}", e), 3);
                    return;
                }
                self.assistant_setup.borrow_mut().overrides = overrides;
                self.refresh_assistants();
            }
        }
        #[cfg(not(windows))]
        let _ = assistant;
    }

    pub(super) fn copy_assistant_prompt(&self) {
        #[cfg(windows)]
        if let Err(e) = arboard::Clipboard::new()
            .and_then(|mut clipboard| clipboard.set_text(tr!("Liste mes scènes dans Polyptyque.")))
        {
            self.assistant_message(&tr!("Impossible de copier la demande : {}", e), 3);
        } else {
            self.assistant_message(&tr!("Demande copiée. La coller dans une conversation de l'assistant choisi."), 1);
        }
    }
}
