//! Commandes du canal de pilotage (catalogue : `polyptyque_core::control::
//! catalog`). Chacune passe par les mêmes actions que l'interface, avec les
//! mêmes verrous ; ce que l'interface fait confirmer exige `confirm: true`.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Local, NaiveDate, SecondsFormat, Utc};
use polyptyque_core::browsers::{classify_executable, program_name};
use polyptyque_core::config::{
    BrowserFamily, BrowserRef, ClockChoice, CloseChoice, Codec, HardwarePreference, LanguageChoice, PasteError, Scene,
    Settings, Source, SourceKind, UpdateChannel,
};
use polyptyque_core::control::catalog::DAYS;
use polyptyque_core::control::protocol::{CommandError, Reason, PROTOCOL_VERSION};
use polyptyque_core::encoder::{bytes_per_hour, EncoderChoice, EncoderKind};
use polyptyque_core::hotkeys;
use polyptyque_core::paths;
use polyptyque_core::schedule::{self, Repeat, Schedule, Slot, TimeOfDay};
use polyptyque_engine::{probe, remux, SourceState};
use serde_json::{json, Value};
use slint::{ComponentHandle, Timer};
use tr::tr;

use super::control::{Reply, StartRequest};
use super::scheduling::Prompt;
use super::{post, with_app, App, Confirm, UpdateState, AUDIO_BITRATES, FRAME_RATES, RESOLUTIONS, VERSION};
use crate::format;
use crate::preview::PreviewState;

type Outcome = Result<Reply, CommandError>;

/// Délai de réponse de la personne à la confirmation du micro.
const MIC_ANSWER_LIMIT: Duration = Duration::from_secs(120);

// ----- Lecture des paramètres (déjà vérifiés par le catalogue) -------------------

fn text<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
}

fn flag(params: &Value, key: &str) -> Option<bool> {
    params.get(key).and_then(Value::as_bool)
}

fn int(params: &Value, key: &str) -> Option<i64> {
    params.get(key).and_then(Value::as_i64)
}

fn confirmed(params: &Value) -> bool {
    flag(params, "confirm").unwrap_or(false)
}

fn required<'a>(params: &'a Value, key: &str) -> Result<&'a str, CommandError> {
    text(params, key).ok_or_else(|| CommandError::invalid(tr!("Paramètre obligatoire manquant : « {} ».", key)))
}

/// Instant en heure locale, au format RFC 3339 (« 2026-10-25T21:00:00+01:00 »).
fn instant(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).to_rfc3339_opts(SecondsFormat::Secs, false)
}

// ----- Erreurs courantes -----------------------------------------------------

fn locked() -> CommandError {
    CommandError::new(Reason::Locked, tr!("Impossible pendant un enregistrement : l'arrêter d'abord."))
}

fn busy() -> CommandError {
    CommandError::new(Reason::Busy, tr!("Une opération est en cours dans Polyptyque : réessayer dans un instant."))
}

fn refused(message: String) -> CommandError {
    CommandError::new(Reason::Refused, message)
}

fn not_found(message: String) -> CommandError {
    CommandError::new(Reason::NotFound, message)
}

/// L'interface aurait posé cette question : à poser à la personne.
fn confirmation(question: &str) -> CommandError {
    CommandError::new(
        Reason::ConfirmationRequired,
        tr!("Confirmation requise : {} Relancer avec « confirm » une fois la personne d'accord.", question),
    )
    .with("question", question)
}

/// Dernier message d'erreur (ou d'avertissement) affiché depuis `since`,
/// pour expliquer un échec.
fn last_problem(app: &App, since: u64) -> Option<String> {
    app.control_problem_since(since)
}

// ----- Noms d'état stables ------------------------------------------------------

fn engine_state(state: SourceState) -> (&'static str, &'static str) {
    // (enregistrement, fenêtre)
    match state {
        SourceState::Searching => ("recording", "searching"),
        SourceState::Connected => ("recording", "found"),
        SourceState::Minimized => ("recording", "minimized"),
        SourceState::Lost => ("recording", "missing"),
        SourceState::Failed => ("failed", "unknown"),
        SourceState::Finishing => ("finishing", "unknown"),
        SourceState::Finished => ("off", "unknown"),
    }
}

fn preview_state(state: Option<&PreviewState>) -> &'static str {
    match state {
        Some(PreviewState::Live) => "found",
        Some(PreviewState::Minimized) => "minimized",
        Some(PreviewState::Missing) => "missing",
        Some(PreviewState::Searching) => "searching",
        Some(PreviewState::Failed(_)) | None => "unknown",
    }
}

fn family_name(family: BrowserFamily) -> &'static str {
    match family {
        BrowserFamily::Gecko => "gecko",
        BrowserFamily::Chromium => "chromium",
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

impl App {
    /// Exécute une commande déjà vérifiée par le catalogue.
    pub(super) fn run_command(&self, client: u64, method: &str, params: &Value) -> Reply {
        match self.dispatch(client, method, params) {
            Ok(reply) => reply,
            Err(error) => Reply::err(error),
        }
    }

    fn dispatch(&self, client: u64, method: &str, p: &Value) -> Outcome {
        match method {
            "hello" => self.cmd_hello(client, p),
            "status" => Ok(Reply::ok(self.control_state())),
            "subscribe" => {
                self.set_control_subscribed(client, true);
                Ok(Reply::ok(json!({ "subscribed": true, "state": self.control_state() })))
            }
            "unsubscribe" => {
                self.set_control_subscribed(client, false);
                Ok(Reply::ok(json!({ "subscribed": false })))
            }
            "system.info" => Ok(Reply::ok(self.system_info())),
            "encoders.test" => self.cmd_encoders_test(),
            "window.show" => {
                self.bring_window_forward();
                Ok(Reply::ok(json!({ "window": "visible" })))
            }
            "previews.set" => self.cmd_previews(p),
            "scenes.list" => Ok(Reply::ok(self.scenes_json())),
            "scenes.create" => self.cmd_scene_create(p),
            "scenes.rename" => self.cmd_scene_rename(p),
            "scenes.duplicate" => self.cmd_scene_duplicate(p),
            "scenes.delete" => self.cmd_scene_delete(p),
            "scenes.activate" => self.cmd_scene_activate(p),
            "scenes.open_windows" => self.cmd_open_windows(),
            "sources.list" => self.cmd_sources_list(p),
            "browsers.list" => Ok(Reply::ok(self.browsers_json())),
            "windows.list" => Ok(Reply::ok(self.windows_json())),
            "sources.add_browser" => self.cmd_add_browser(p),
            "sources.add_application" => self.cmd_add_application(p),
            "sources.add_to_scene" => self.cmd_add_to_scene(p),
            "sources.remove" => self.cmd_remove_source(p),
            "sources.move" => self.cmd_move_source(p),
            "sources.open_window" => self.cmd_open_window(p),
            "sources.restore_window" => self.cmd_restore_window(p),
            "recording.start" => self.cmd_start(client, p),
            "recording.stop" => self.cmd_stop(p),
            "prompt.answer" => self.cmd_prompt_answer(p),
            "settings.get" => Ok(Reply::ok(self.settings_json())),
            "settings.set" => self.cmd_settings_set(client, p),
            "microphones.list" => Ok(Reply::ok(self.microphones_json())),
            "schedules.list" => Ok(Reply::ok(self.schedules_json())),
            "schedules.create" => self.cmd_schedule_create(p),
            "schedules.update" => self.cmd_schedule_update(p),
            "schedules.enable" => self.cmd_schedule_enable(p),
            "schedules.delete" => self.cmd_schedule_delete(p),
            "recordings.list" => Ok(Reply::ok(self.recordings_json(p))),
            "recordings.open_folder" => {
                self.open_output_folder();
                Ok(Reply::ok(json!({ "folder": self.config.borrow().settings.output_dir.display().to_string() })))
            }
            "conversion.start" => self.cmd_convert(p),
            "conversion.cancel" => self.cmd_cancel_conversion(p),
            "update.install" => self.cmd_update(p),
            "app.quit" => self.cmd_quit(p),
            _ => Err(CommandError::internal(tr!("Commande « {} » non prise en charge par cette version.", method))),
        }
    }

    // ----- Recherche des éléments ---------------------------------------------------

    /// Scène désignée par son identifiant ou son nom (sans tenir compte de
    /// la casse).
    fn find_scene(&self, reference: &str) -> Result<Scene, CommandError> {
        let config = self.config.borrow();
        if let Some(scene) = config.scene(reference) {
            return Ok(scene.clone());
        }
        let wanted = reference.trim().to_lowercase();
        let found: Vec<&Scene> = config.scenes.iter().filter(|s| s.name.trim().to_lowercase() == wanted).collect();
        match found.as_slice() {
            [scene] => Ok((*scene).clone()),
            [] => Err(not_found(tr!("Scène introuvable : « {} ».", reference))
                .with("scenes", config.scenes.iter().map(|s| s.name.clone()).collect::<Vec<_>>())),
            several => Err(CommandError::new(
                Reason::Ambiguous,
                tr!("Plusieurs scènes s'appellent « {} » : la désigner par son identifiant.", reference),
            )
            .with("ids", several.iter().map(|s| s.id.clone()).collect::<Vec<_>>())),
        }
    }

    /// Scène du paramètre `scene`, ou la scène en cours.
    fn scene_param(&self, params: &Value) -> Result<Scene, CommandError> {
        match text(params, "scene") {
            Some(reference) => self.find_scene(reference),
            None => Ok(self.config.borrow().active_scene().clone()),
        }
    }

    /// Source désignée par son identifiant ou son nom, dans une scène ou
    /// parmi toutes les sources.
    fn find_source(&self, reference: &str, scene: Option<&Scene>) -> Result<Source, CommandError> {
        let config = self.config.borrow();
        let candidates: Vec<&Source> = match scene {
            Some(scene) => scene.source_ids.iter().filter_map(|id| config.source(id)).collect(),
            None => config.sources.iter().collect(),
        };
        if let Some(source) = candidates.iter().find(|s| s.id == reference) {
            return Ok((*source).clone());
        }
        let wanted = reference.trim().to_lowercase();
        let found: Vec<&&Source> = candidates.iter().filter(|s| s.name.trim().to_lowercase() == wanted).collect();
        match found.as_slice() {
            [source] => Ok((**source).clone()),
            [] => {
                let message = match scene {
                    Some(scene) => tr!("Source introuvable dans la scène « {} » : « {} ».", scene.name, reference),
                    None => tr!("Source introuvable : « {} ».", reference),
                };
                Err(not_found(message).with("sources", candidates.iter().map(|s| s.name.clone()).collect::<Vec<_>>()))
            }
            several => Err(CommandError::new(
                Reason::Ambiguous,
                tr!("Plusieurs sources s'appellent « {} » : la désigner par son identifiant.", reference),
            )
            .with("ids", several.iter().map(|s| s.id.clone()).collect::<Vec<_>>())),
        }
    }

    fn find_schedule(&self, id: &str) -> Result<Schedule, CommandError> {
        self.config
            .borrow()
            .schedules
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .ok_or_else(|| not_found(tr!("Programmation introuvable : « {} » (identifiants : schedules.list).", id)))
    }

    /// Rien ne doit changer pendant un enregistrement ou une opération
    /// longue (comme dans l'interface).
    fn check_unlocked(&self) -> Result<(), CommandError> {
        if self.recording() {
            Err(locked())
        } else if self.busy.get() {
            Err(busy())
        } else {
            Ok(())
        }
    }

    fn sheet(&self) -> String {
        self.with_bridge_value(|b| b.get_sheet().to_string())
    }

    // ----- Généralités ------------------------------------------------------------

    fn cmd_hello(&self, client: u64, params: &Value) -> Outcome {
        if let Some(name) = text(params, "client") {
            self.set_control_client_name(client, name);
            log::info!("pilotage : « {} » se présente", self.control_client_name(client));
        }
        Ok(Reply::ok(json!({
            "app": "Polyptyque",
            "version": VERSION,
            "protocol": PROTOCOL_VERSION,
            "language": self.language_code(),
            "platform": if cfg!(windows) { "windows" } else { "demo" },
        })))
    }

    fn language_code(&self) -> &'static str {
        match crate::i18n::Language::current() {
            crate::i18n::Language::French => "fr",
            crate::i18n::Language::English => "en",
        }
    }

    /// Fenêtre de Polyptyque affichée et au premier plan.
    pub(super) fn bring_window_forward(&self) {
        self.show_from_background();
        #[cfg(windows)]
        if let Some(window) = self.main_window() {
            polyptyque_windows::tray::bring_to_front(window);
        }
    }

    fn system_info(&self) -> Value {
        let settings = self.config.borrow().settings.clone();
        json!({
            "app": "Polyptyque",
            "version": VERSION,
            "protocol": PROTOCOL_VERSION,
            "portable": paths::portable_root().is_some(),
            "os": self.platform.os_description(),
            "graphics_adapters": self.platform.graphics_adapters(),
            "ffmpeg": probe::locate_ffmpeg().map(|p| p.display().to_string()),
            "language": self.language_code(),
            "output_dir": settings.output_dir.display().to_string(),
            "logs_dir": paths::logs_dir().display().to_string(),
            "free_bytes": self.free_space.get(),
        })
    }

    fn cmd_encoders_test(&self) -> Outcome {
        self.check_unlocked()?;
        if self.control.borrow().encoder_test_running {
            return Err(busy());
        }
        let Some(ffmpeg) = probe::locate_ffmpeg() else {
            return Err(CommandError::new(
                Reason::Unavailable,
                tr!("FFmpeg introuvable : ffmpeg.exe doit se trouver à côté de polyptyque.exe"),
            ));
        };
        {
            let mut control = self.control.borrow_mut();
            control.encoder_test_running = true;
            control.encoder_test = None;
        }
        let platform = self.platform.clone();
        thread::spawn(move || {
            let kinds = [
                (EncoderKind::Nvenc, "nvenc"),
                (EncoderKind::Amf, "amf"),
                (EncoderKind::Qsv, "qsv"),
                (EncoderKind::Software, "cpu"),
            ];
            let mut results = Vec::new();
            for (codec, codec_name) in [(Codec::H264, "h264"), (Codec::Hevc, "hevc")] {
                for (kind, kind_name) in kinds {
                    let choice = EncoderChoice { kind, codec };
                    let works = probe::probe(platform.as_ref(), &ffmpeg, choice);
                    results.push(json!({
                        "encoder": kind_name,
                        "codec": codec_name,
                        "label": choice.label(),
                        "works": works,
                    }));
                }
            }
            post(move |app| {
                let mut control = app.control.borrow_mut();
                control.encoder_test_running = false;
                control.encoder_test = Some(json!({ "encoders": results }));
            });
        });
        Ok(Reply::Wait {
            check: Box::new(|app| {
                let mut control = app.control.borrow_mut();
                (!control.encoder_test_running).then(|| Ok(control.encoder_test.take().unwrap_or(Value::Null)))
            }),
            limit: Duration::from_secs(280),
        })
    }

    fn cmd_previews(&self, params: &Value) -> Outcome {
        let enabled = flag(params, "enabled").unwrap_or(true);
        if self.config.borrow().settings.previews != enabled {
            self.toggle_previews();
        }
        Ok(Reply::ok(json!({ "previews": enabled })))
    }

    // ----- Scènes ---------------------------------------------------------------------

    fn scene_json(&self, scene: &Scene) -> Value {
        let config = self.config.borrow();
        json!({
            "id": scene.id,
            "name": scene.name,
            "active": scene.id == config.active_scene,
            "sources": scene.source_ids.iter().filter_map(|id| config.source(id)).map(|s| json!({ "id": s.id, "name": s.name })).collect::<Vec<_>>(),
            "schedules": config.schedules.iter().filter(|s| s.scene_id == scene.id).count(),
        })
    }

    fn scenes_json(&self) -> Value {
        let scenes = self.config.borrow().scenes.clone();
        let active = self.config.borrow().active_scene.clone();
        json!({
            "active_scene": active,
            "scenes": scenes.iter().map(|s| self.scene_json(s)).collect::<Vec<_>>(),
        })
    }

    fn scene_name_param(params: &Value) -> Result<String, CommandError> {
        let name = required(params, "name")?.trim().to_string();
        if name.is_empty() {
            return Err(CommandError::invalid(tr!("Nom de scène manquant.")));
        }
        Ok(name)
    }

    fn cmd_scene_create(&self, params: &Value) -> Outcome {
        self.check_unlocked()?;
        let name = Self::scene_name_param(params)?;
        let activate = flag(params, "activate").unwrap_or(true);
        let id = self.create_scene(&name, activate);
        let scene = self.find_scene(&id)?;
        Ok(Reply::ok(json!({ "scene": self.scene_json(&scene) })))
    }

    fn cmd_scene_rename(&self, params: &Value) -> Outcome {
        self.check_unlocked()?;
        let scene = self.find_scene(required(params, "scene")?)?;
        let name = Self::scene_name_param(params)?;
        self.rename_scene(&scene.id, &name);
        let scene = self.find_scene(&scene.id)?;
        Ok(Reply::ok(json!({ "scene": self.scene_json(&scene) })))
    }

    fn cmd_scene_duplicate(&self, params: &Value) -> Outcome {
        self.check_unlocked()?;
        let scene = self.scene_param(params)?;
        let name = match text(params, "name").map(str::trim) {
            Some("") => return Err(CommandError::invalid(tr!("Nom de scène manquant."))),
            Some(name) => name.to_string(),
            None => tr!("{} (copie)", scene.name),
        };
        let Some(id) = self.duplicate_scene(&scene.id, &name) else {
            return Err(not_found(tr!("Scène introuvable : « {} ».", scene.name)));
        };
        let copy = self.find_scene(&id)?;
        Ok(Reply::ok(json!({ "scene": self.scene_json(&copy) })))
    }

    fn cmd_scene_delete(&self, params: &Value) -> Outcome {
        self.check_unlocked()?;
        let scene = self.find_scene(required(params, "scene")?)?;
        if self.config.borrow().scenes.len() <= 1 {
            return Err(refused(tr!("La dernière scène ne peut pas être supprimée.")));
        }
        if !confirmed(params) {
            return Err(confirmation(&self.delete_scene_text(&scene.id)));
        }
        self.delete_scene(&scene.id);
        Ok(Reply::ok(json!({ "deleted": scene.id, "active_scene": self.config.borrow().active_scene })))
    }

    fn cmd_scene_activate(&self, params: &Value) -> Outcome {
        let scene = self.find_scene(required(params, "scene")?)?;
        if self.config.borrow().active_scene != scene.id {
            self.check_unlocked()?;
            self.select_scene(&scene.id);
        }
        Ok(Reply::ok(json!({ "scene": self.scene_json(&scene) })))
    }

    fn cmd_open_windows(&self) -> Outcome {
        if self.opening_windows.get() {
            return Err(busy());
        }
        self.control.borrow_mut().last_open = None;
        self.open_scene_windows(false);
        Ok(Reply::Wait {
            check: Box::new(|app| {
                (!app.opening_windows.get()).then(|| {
                    Ok(app
                        .control
                        .borrow_mut()
                        .last_open
                        .take()
                        .unwrap_or_else(|| json!({ "opened": [], "failed": [], "missing": [] })))
                })
            }),
            limit: Duration::from_secs(90),
        })
    }

    // ----- Sources ---------------------------------------------------------------------

    fn source_json(&self, source: &Source, position: Option<usize>) -> Value {
        let config = self.config.borrow();
        let mut value = json!({
            "id": source.id,
            "name": source.name,
            "kind": if source.is_browser() { "browser" } else { "application" },
            "relaunch": source.relaunch,
            "capture_cursor": source.capture_cursor,
            "scenes": config.scenes_with(&source.id).iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
        });
        if let Some(position) = position {
            value["position"] = json!(position + 1);
        }
        match &source.kind {
            SourceKind::Browser { browser, start_url, .. } => {
                value["browser"] = json!({ "name": browser.name, "family": family_name(browser.family) });
                if let Some(url) = start_url {
                    value["start_url"] = json!(url);
                }
            }
            // Le titre de fenêtre mémorisé n'est pas donné : il peut être
            // privé, comme ceux des fenêtres ouvertes.
            SourceKind::Application { exe_path, .. } => {
                value["program"] = json!(file_name(exe_path));
            }
        }
        value
    }

    fn cmd_sources_list(&self, params: &Value) -> Outcome {
        if flag(params, "all").unwrap_or(false) {
            let sources = self.config.borrow().sources.clone();
            return Ok(Reply::ok(
                json!({ "sources": sources.iter().map(|s| self.source_json(s, None)).collect::<Vec<_>>() }),
            ));
        }
        let scene = self.scene_param(params)?;
        let sources = self.config.borrow().scene_sources(&scene);
        Ok(Reply::ok(json!({
            "scene": { "id": scene.id, "name": scene.name },
            "sources": sources.iter().enumerate().map(|(i, s)| self.source_json(s, Some(i))).collect::<Vec<_>>(),
        })))
    }

    fn browsers_json(&self) -> Value {
        let browsers = self.platform.detect_browsers();
        json!({
            "browsers": browsers.iter().map(|b| json!({
                "name": b.name,
                "family": family_name(b.family),
                "path": b.exe_path.display().to_string(),
            })).collect::<Vec<_>>(),
        })
    }

    /// Fenêtres ouvertes : le programme de chacune ; le titre seulement si
    /// plusieurs fenêtres du même programme sont ouvertes (un titre peut
    /// être privé, et un assistant en ligne l'envoie à son fournisseur).
    fn windows_json(&self) -> Value {
        let windows = self.platform.list_windows();
        let items: Vec<Value> = windows
            .iter()
            .map(|w| {
                let same_program = windows.iter().filter(|o| o.exe == w.exe).count();
                let mut item = json!({
                    "id": w.handle.to_string(),
                    "program": program_name(&w.exe).map(|n| format::capitalize(&n)).unwrap_or_default(),
                    "executable": file_name(&w.exe),
                    "browser": classify_executable(&w.exe).is_some(),
                });
                if same_program > 1 {
                    item["title"] = json!(w.title);
                }
                item
            })
            .collect();
        json!({ "windows": items })
    }

    fn cmd_add_browser(&self, params: &Value) -> Outcome {
        self.check_unlocked()?;
        let scene = self.scene_param(params)?;
        let wanted = required(params, "browser")?.trim();
        let installed = self.platform.detect_browsers();
        let browser = match installed.iter().find(|b| b.name.eq_ignore_ascii_case(wanted)) {
            Some(b) => BrowserRef { name: b.name.clone(), exe_path: b.exe_path.clone(), family: b.family },
            None => {
                let path = Path::new(wanted.trim_matches('"'));
                if path.is_absolute() || path.exists() {
                    self.browser_from_path(path).map_err(|(message, _)| refused(message))?
                } else {
                    return Err(not_found(tr!("Navigateur introuvable : « {} ».", wanted))
                        .with("browsers", installed.iter().map(|b| b.name.clone()).collect::<Vec<_>>()));
                }
            }
        };
        let name = match text(params, "name").map(str::trim) {
            Some("") => return Err(CommandError::invalid(tr!("Nom de source manquant."))),
            Some(name) => name.to_string(),
            None => {
                let count = scene.source_ids.len() + 1;
                format!("{} {count}", browser.name.split_whitespace().last().unwrap_or(&browser.name))
            }
        };
        let open = flag(params, "open").unwrap_or(true);
        let id = self.add_browser_source(&name, browser, text(params, "start_url"), open, &scene.id);
        let source = self.find_source(&id, None)?;
        Ok(Reply::ok(json!({ "source": self.source_json(&source, None) })))
    }

    fn cmd_add_application(&self, params: &Value) -> Outcome {
        self.check_unlocked()?;
        let scene = self.scene_param(params)?;
        let window_id = required(params, "window")?;
        let window = window_id
            .trim()
            .parse::<isize>()
            .ok()
            .and_then(|handle| self.platform.list_windows().into_iter().find(|w| w.handle == handle))
            .ok_or_else(|| not_found(tr!("Fenêtre introuvable : « {} » (identifiants : windows.list).", window_id)))?;
        let name = match text(params, "name").map(str::trim) {
            Some("") => return Err(CommandError::invalid(tr!("Nom de source manquant."))),
            Some(name) => name.to_string(),
            None => program_name(&window.exe).map(|n| format::capitalize(&n)).unwrap_or_else(|| window.title.clone()),
        };
        let id = self.add_application_source(
            &name,
            &window,
            flag(params, "relaunch").unwrap_or(false),
            flag(params, "capture_cursor").unwrap_or(true),
            &scene.id,
        );
        let source = self.find_source(&id, None)?;
        Ok(Reply::ok(json!({ "source": self.source_json(&source, None) })))
    }

    fn cmd_add_to_scene(&self, params: &Value) -> Outcome {
        self.check_unlocked()?;
        let source = self.find_source(required(params, "source")?, None)?;
        let scene = self.find_scene(required(params, "scene")?)?;
        let result = self.config.borrow_mut().link_source(&scene.id, &source.id);
        match result {
            Ok(()) => {
                self.after_source_change(&scene.id);
                self.toast(
                    &tr!("« {} » collée dans « {} » : c'est la même source, partagée.", source.name, scene.name),
                    1,
                );
                let source = self.find_source(&source.id, None)?;
                Ok(Reply::ok(json!({ "source": self.source_json(&source, None) })))
            }
            Err(PasteError::AlreadyInScene) => Err(refused(tr!(
                "« {} » est déjà dans cette scène : une même source n'y figure qu'une fois.",
                source.name
            ))),
            Err(PasteError::Missing) => Err(not_found(tr!("Source introuvable : « {} ».", source.name))),
        }
    }

    fn cmd_remove_source(&self, params: &Value) -> Outcome {
        self.check_unlocked()?;
        let scene = self.scene_param(params)?;
        let source = self.find_source(required(params, "source")?, Some(&scene))?;
        if !confirmed(params) {
            return Err(confirmation(&self.remove_source_text(&source, &scene.id)));
        }
        self.config.borrow_mut().remove_source_from(&scene.id, &source.id);
        self.after_source_change(&scene.id);
        let remains = self.config.borrow().source(&source.id).is_some();
        Ok(Reply::ok(json!({ "removed": source.id, "scene": scene.id, "still_in_other_scenes": remains })))
    }

    fn cmd_move_source(&self, params: &Value) -> Outcome {
        self.check_unlocked()?;
        let scene = self.scene_param(params)?;
        let source = self.find_source(required(params, "source")?, Some(&scene))?;
        let count = scene.source_ids.len() as i64;
        let position = int(params, "position").unwrap_or(0);
        if !(1..=count).contains(&position) {
            return Err(refused(tr!("Place invalide : de 1 à {} dans cette scène.", count)));
        }
        let from = scene.source_ids.iter().position(|id| *id == source.id).unwrap_or(0);
        let to = (position - 1) as usize;
        if self.config.borrow_mut().move_source_in(&scene.id, from, to) {
            if self.config.borrow().active_scene == scene.id {
                self.with_bridge(|b| b.set_selected_source(to as i32));
            }
            self.after_source_change(&scene.id);
        }
        let sources = self.config.borrow().scene_sources(&self.find_scene(&scene.id)?);
        Ok(Reply::ok(
            json!({ "sources": sources.iter().enumerate().map(|(i, s)| self.source_json(s, Some(i))).collect::<Vec<_>>() }),
        ))
    }

    fn active_scene_source(&self, params: &Value) -> Result<Source, CommandError> {
        let scene = self.config.borrow().active_scene().clone();
        self.find_source(required(params, "source")?, Some(&scene))
    }

    fn cmd_open_window(&self, params: &Value) -> Outcome {
        let source = self.active_scene_source(params)?;
        self.open_source(&source.id);
        Ok(Reply::ok(json!({ "source": source.id })))
    }

    fn cmd_restore_window(&self, params: &Value) -> Outcome {
        let source = self.active_scene_source(params)?;
        self.restore_window(&source.id);
        // Le résultat arrive en message (la fenêtre d'un programme figé
        // peut faire attendre).
        Ok(Reply::ok(json!({ "source": source.id, "requested": true })))
    }

    // ----- Enregistrement ---------------------------------------------------------------

    fn cmd_start(&self, client: u64, params: &Value) -> Outcome {
        if self.busy.get() {
            return Err(busy());
        }
        if self.missed_prompt_open() {
            return Err(CommandError::new(
                Reason::Busy,
                tr!("Une question de la programmation attend une réponse : utiliser prompt.answer."),
            ));
        }
        let active = self.config.borrow().active_scene().clone();
        let scene = self.scene_param(params)?;
        let since = self.control.borrow().message_seq();
        let request = StartRequest {
            client: self.control_client_name(client),
            auto_stop: flag(params, "auto_stop").unwrap_or(false),
        };
        if self.recording() {
            if scene.id != active.id {
                return Err(CommandError::new(
                    Reason::Locked,
                    tr!(
                        "Un enregistrement de la scène « {} » est en cours : l'arrêter avant d'en changer.",
                        active.name
                    ),
                ));
            }
            let Some(reference) = text(params, "source") else {
                return Err(refused(tr!("Un enregistrement est déjà en cours.")));
            };
            // Une source de plus dans l'enregistrement en cours.
            let source = self.find_source(reference, Some(&active))?;
            if self.session.borrow().as_ref().is_some_and(|s| s.is_recording(&source.id)) {
                return Err(refused(tr!("« {} » est déjà en cours d'enregistrement.", source.name)));
            }
            if self.joining.borrow().contains(&source.id) {
                return Err(busy());
            }
            self.join_recording(source.clone());
            let id = source.id.clone();
            return Ok(Reply::Wait {
                check: Box::new(move |app| {
                    if app.joining.borrow().contains(&id) {
                        return None;
                    }
                    let joined = app.session.borrow().as_ref().is_some_and(|s| s.is_recording(&id));
                    Some(if joined {
                        Ok(json!({ "recording": true, "source": id, "joined": true }))
                    } else {
                        Err(refused(last_problem(app, since).unwrap_or_else(|| tr!("La source n'a pas pu démarrer."))))
                    })
                }),
                limit: Duration::from_secs(90),
            });
        }
        if probe::locate_ffmpeg().is_none() {
            return Err(CommandError::new(
                Reason::Unavailable,
                tr!("FFmpeg introuvable : ffmpeg.exe doit se trouver à côté de polyptyque.exe"),
            ));
        }
        if scene.id != active.id {
            self.select_scene(&scene.id);
        }
        let only = match text(params, "source") {
            Some(reference) => Some(self.find_source(reference, Some(&scene))?.id),
            None => None,
        };
        self.control.borrow_mut().start_request = Some(request);
        self.start_recording(only.as_deref());
        Ok(Reply::Wait {
            check: Box::new(move |app| {
                if app.busy.get() {
                    return None;
                }
                Some(if app.recording() {
                    Ok(app.recording_json())
                } else {
                    Err(refused(last_problem(app, since).unwrap_or_else(|| tr!("L'enregistrement n'a pas démarré."))))
                })
            }),
            limit: Duration::from_secs(120),
        })
    }

    /// Enregistrement en cours, pour la réponse au démarrage.
    fn recording_json(&self) -> Value {
        let config = self.config.borrow();
        let scene = config.active_scene();
        let session = self.session.borrow();
        let recording: Vec<Value> = config
            .active_sources()
            .iter()
            .filter(|s| session.as_ref().is_some_and(|session| session.is_recording(&s.id)))
            .map(|s| json!({ "id": s.id, "name": s.name }))
            .collect();
        json!({
            "recording": true,
            "scene": { "id": scene.id, "name": scene.name },
            "sources": recording,
            "missing_windows": self.control.borrow().last_missing,
            "output_dir": session.as_ref().map(|s| s.current_output_dir().display().to_string()),
            "scheduled_until": self.planner.borrow().owned().map(|slot| instant(slot.end)),
            "microphone": config.settings.microphone.enabled,
        })
    }

    fn cmd_stop(&self, params: &Value) -> Outcome {
        if !self.recording() {
            return Err(if self.busy.get() { busy() } else { refused(tr!("Aucun enregistrement en cours.")) });
        }
        if self.busy.get() {
            return Err(busy());
        }
        if let Some(reference) = text(params, "source") {
            let active = self.config.borrow().active_scene().clone();
            let source = self.find_source(reference, Some(&active))?;
            let (recording, count) = self
                .session
                .borrow()
                .as_ref()
                .map(|s| (s.is_recording(&source.id), s.recording_count()))
                .unwrap_or((false, 0));
            if !recording {
                return Err(refused(tr!("« {} » n'enregistre pas.", source.name)));
            }
            if count + self.joining.borrow().len() > 1 {
                // Les autres sources continuent : même chemin que le bouton
                // de sa carte.
                self.toggle_source(&source.id);
                return Ok(Reply::ok(json!({ "stopped": source.id, "finishing": true, "recording": true })));
            }
            // Dernière source : c'est la fin de l'enregistrement.
        }
        let owned = self.planner.borrow().owned().cloned();
        if let (Some(slot), false) = (&owned, confirmed(params)) {
            return Err(confirmation(&self.stop_scheduled_text(slot)));
        }
        self.control.borrow_mut().last_stop = None;
        self.stop_recording(None);
        Ok(Reply::Wait {
            check: Box::new(|app| {
                (!app.recording() && !app.busy.get()).then(|| {
                    Ok(app.control.borrow_mut().last_stop.take().unwrap_or_else(|| json!({ "recording": false })))
                })
            }),
            limit: Duration::from_secs(280),
        })
    }

    fn cmd_prompt_answer(&self, params: &Value) -> Outcome {
        let kind = match self.prompt.borrow().as_ref() {
            Some(Prompt::Missed { .. }) => "missed_start",
            Some(Prompt::KeepAutoStop { .. }) => "auto_stop",
            Some(Prompt::ManualRunning { .. }) => "info",
            None => return Err(refused(tr!("Aucune question en attente."))),
        };
        if flag(params, "accept").unwrap_or(false) {
            self.prompt_accepted();
        } else {
            self.prompt_declined();
        }
        Ok(Reply::ok(json!({ "answered": kind })))
    }

    // ----- Réglages ------------------------------------------------------------------

    fn settings_json(&self) -> Value {
        let s = self.config.borrow().settings.clone();
        let updates = if !s.check_updates {
            "off"
        } else if s.update_channel(VERSION) == UpdateChannel::Beta {
            "beta"
        } else {
            "stable"
        };
        json!({
            "output_dir": s.output_dir.display().to_string(),
            "filename_template": s.filename_template,
            "previews": s.previews,
            "convert_to_mp4": s.convert_to_mp4,
            "updates": updates,
            "open_windows_at_launch": s.open_windows_at_launch,
            "close_browsers_at_exit": s.close_browsers_at_exit,
            "launch_at_startup": s.launch_at_startup,
            "close_with_schedules": match s.close_with_schedules {
                CloseChoice::Ask => "ask",
                CloseChoice::Background => "background",
                CloseChoice::Quit => "quit",
            },
            "language": match s.language {
                LanguageChoice::Auto => "auto",
                LanguageChoice::French => "french",
                LanguageChoice::English => "english",
            },
            "clock": match s.clock {
                ClockChoice::Auto => "auto",
                ClockChoice::H24 => "24h",
                ClockChoice::H12 => "12h",
            },
            "remote_control": s.remote_control,
            "video": {
                "width": s.video.width,
                "height": s.video.height,
                "fps": s.video.fps,
                "bitrate_kbps": s.video.bitrate_kbps,
                "codec": match s.video.codec { Codec::H264 => "h264", Codec::Hevc => "hevc" },
                "hardware": match s.video.hardware {
                    HardwarePreference::Auto => "auto",
                    HardwarePreference::Gpu => "gpu",
                    HardwarePreference::Cpu => "cpu",
                },
            },
            "audio": { "bitrate_kbps": s.audio.bitrate_kbps },
            "microphone": {
                "enabled": s.microphone.enabled,
                "device_id": s.microphone.device_id,
                "device_name": s.microphone.device_name,
            },
            "hotkeys": {
                "toggle_recording": s.hotkeys.toggle_recording,
                "show_window": s.hotkeys.show_window,
                "toggle_previews": s.hotkeys.toggle_previews,
            },
            "choices": {
                "resolutions": RESOLUTIONS.iter().map(|(w, h)| json!({ "width": w, "height": h })).collect::<Vec<_>>(),
                "fps": FRAME_RATES,
                "audio_bitrates_kbps": AUDIO_BITRATES,
                "hotkeys": hotkeys::choices(),
            },
        })
    }

    fn microphones_json(&self) -> Value {
        let mut mics = vec![json!({ "id": "", "name": tr!("Micro par défaut de Windows") })];
        mics.extend(self.platform.list_microphones().into_iter().map(|d| json!({ "id": d.id, "name": d.name })));
        json!({ "microphones": mics })
    }

    /// Nouveaux réglages : les réglages actuels, modifiés par les champs
    /// donnés. Renvoie aussi vrai si le micro doit être activé (accord de
    /// la personne requis).
    fn settings_from(&self, params: &Value) -> Result<(Settings, bool), CommandError> {
        let current = self.config.borrow().settings.clone();
        let mut s = current.clone();
        let out_of_range = |name: &str, min: i64, max: i64| {
            refused(tr!("Réglage « {} » : une valeur de {} à {} est attendue.", name, min, max))
        };
        if let Some(dir) = text(params, "output_dir") {
            let dir = dir.trim().trim_matches('"');
            if dir.is_empty() {
                return Err(refused(tr!("Dossier des enregistrements manquant.")));
            }
            s.output_dir = PathBuf::from(dir);
        }
        if let Some(template) = text(params, "filename_template") {
            s.filename_template = template.trim().to_string();
        }
        if let Some(value) = flag(params, "previews") {
            s.previews = value;
        }
        if let Some(value) = flag(params, "convert_to_mp4") {
            s.convert_to_mp4 = value;
        }
        if let Some(updates) = text(params, "updates") {
            s.check_updates = updates != "off";
            if s.check_updates {
                let chosen = if updates == "beta" { UpdateChannel::Beta } else { UpdateChannel::Stable };
                // Comme dans les réglages : tant que le choix reste celui de
                // la version en cours, rien n'est figé.
                if s.update_channel.is_some() || chosen != UpdateChannel::of_version(VERSION) {
                    s.update_channel = Some(chosen);
                }
            }
        }
        if let Some(value) = flag(params, "open_windows_at_launch") {
            s.open_windows_at_launch = value;
        }
        if let Some(value) = flag(params, "close_browsers_at_exit") {
            s.close_browsers_at_exit = value;
        }
        if let Some(value) = flag(params, "launch_at_startup") {
            s.launch_at_startup = value;
        }
        if let Some(choice) = text(params, "close_with_schedules") {
            s.close_with_schedules = match choice {
                "background" => CloseChoice::Background,
                "quit" => CloseChoice::Quit,
                _ => CloseChoice::Ask,
            };
        }
        if let Some(language) = text(params, "language") {
            s.language = match language {
                "french" => LanguageChoice::French,
                "english" => LanguageChoice::English,
                _ => LanguageChoice::Auto,
            };
        }
        if let Some(clock) = text(params, "clock") {
            s.clock = match clock {
                "24h" => ClockChoice::H24,
                "12h" => ClockChoice::H12,
                _ => ClockChoice::Auto,
            };
        }
        if let Some(video) = params.get("video").filter(|v| v.is_object()) {
            let bounded = |key: &str, min: i64, max: i64| -> Result<Option<u32>, CommandError> {
                match int(video, key) {
                    Some(v) if (min..=max).contains(&v) => Ok(Some(v as u32)),
                    Some(_) => Err(out_of_range(&format!("video.{key}"), min, max)),
                    None => Ok(None),
                }
            };
            if let Some(width) = bounded("width", 64, 7680)? {
                s.video.width = width;
            }
            if let Some(height) = bounded("height", 64, 4320)? {
                s.video.height = height;
            }
            if s.video.width % 2 == 1 || s.video.height % 2 == 1 {
                return Err(refused(tr!("Largeur et hauteur doivent être paires.")));
            }
            if let Some(fps) = bounded("fps", 1, 240)? {
                s.video.fps = fps;
            }
            if let Some(bitrate) = bounded("bitrate_kbps", 250, 500_000)? {
                s.video.bitrate_kbps = bitrate;
            }
            if let Some(codec) = text(video, "codec") {
                s.video.codec = if codec == "hevc" { Codec::Hevc } else { Codec::H264 };
            }
            if let Some(hardware) = text(video, "hardware") {
                s.video.hardware = match hardware {
                    "gpu" => HardwarePreference::Gpu,
                    "cpu" => HardwarePreference::Cpu,
                    _ => HardwarePreference::Auto,
                };
            }
        }
        if let Some(audio) = params.get("audio").filter(|v| v.is_object()) {
            match int(audio, "bitrate_kbps") {
                Some(v) if (64..=512).contains(&v) => s.audio.bitrate_kbps = v as u32,
                Some(_) => return Err(out_of_range("audio.bitrate_kbps", 64, 512)),
                None => {}
            }
        }
        let mut enable_microphone = false;
        if let Some(mic) = params.get("microphone").filter(|v| v.is_object()) {
            if let Some(device) = text(mic, "device_id") {
                if device.is_empty() {
                    s.microphone.device_id.clear();
                    s.microphone.device_name.clear();
                } else if device != current.microphone.device_id {
                    let found = self.platform.list_microphones().into_iter().find(|d| d.id == device);
                    let Some(found) = found else {
                        return Err(not_found(tr!(
                            "Micro introuvable : « {} » (identifiants : microphones.list).",
                            device
                        )));
                    };
                    s.microphone.device_id = found.id;
                    s.microphone.device_name = found.name;
                }
            }
            match flag(mic, "enabled") {
                Some(true) if !current.microphone.enabled => enable_microphone = true,
                Some(value) => s.microphone.enabled = value,
                None => {}
            }
        }
        if let Some(keys) = params.get("hotkeys").filter(|v| v.is_object()) {
            for (key, slot) in [
                ("toggle_recording", &mut s.hotkeys.toggle_recording),
                ("show_window", &mut s.hotkeys.show_window),
                ("toggle_previews", &mut s.hotkeys.toggle_previews),
            ] {
                if let Some(value) = text(keys, key) {
                    let value = value.trim();
                    if !value.is_empty() && hotkeys::parse(value).is_none() {
                        return Err(refused(tr!(
                            "Raccourci invalide : « {} » (format attendu : Ctrl+Maj+F9, avec une touche F1 à F24).",
                            value
                        )));
                    }
                    *slot = value.to_string();
                }
            }
            let assigned: Vec<&String> =
                [&s.hotkeys.toggle_recording, &s.hotkeys.show_window, &s.hotkeys.toggle_previews]
                    .into_iter()
                    .filter(|h| !h.is_empty())
                    .collect();
            if (1..assigned.len()).any(|i| assigned[..i].iter().any(|other| hotkeys::same(other, assigned[i]))) {
                return Err(refused(tr!("Un même raccourci ne peut servir qu'à une seule action.")));
            }
        }
        Ok((s, enable_microphone))
    }

    fn cmd_settings_set(&self, client: u64, params: &Value) -> Outcome {
        self.check_unlocked()?;
        if matches!(self.sheet().as_str(), "settings" | "assistants") {
            return Err(CommandError::new(
                Reason::Busy,
                tr!("La fenêtre des réglages est ouverte dans Polyptyque : la fermer d'abord (elle remplacerait ces réglages)."),
            ));
        }
        let (mut settings, enable_microphone) = self.settings_from(params)?;
        if !enable_microphone {
            self.apply_settings(settings);
            return Ok(Reply::ok(json!({ "settings": self.settings_json() })));
        }
        // Le micro : seule la personne devant l'écran peut l'activer.
        if !self.sheet().is_empty() {
            return Err(CommandError::new(
                Reason::Busy,
                tr!("Une fenêtre est ouverte dans Polyptyque : la fermer d'abord, pour que la question du micro puisse s'afficher."),
            ));
        }
        settings.microphone.enabled = true;
        self.control.borrow_mut().mic_answer = None;
        let client = self.control_client_name(client);
        self.bring_window_forward();
        self.ask(
            Confirm::EnableMicrophone,
            &tr!("Activer le micro"),
            &tr!(
                "« {} » demande d'activer le micro. Ce qu'il capte sera alors enregistré dans un fichier séparé, pendant chaque enregistrement. Windows doit autoriser l'accès au micro pour les applications de bureau.",
                client
            ),
            &tr!("Activer le micro"),
        );
        log::info!("pilotage : activation du micro demandée par {client}, question posée");
        let deadline = Instant::now() + MIC_ANSWER_LIMIT;
        Ok(Reply::Wait {
            check: Box::new(move |app| {
                let answer = app.control.borrow_mut().mic_answer.take();
                match answer {
                    Some(true) if app.recording() || app.busy.get() => Some(Err(locked())),
                    Some(true) => {
                        app.apply_settings(settings.clone());
                        Some(Ok(json!({ "settings": app.settings_json() })))
                    }
                    Some(false) => Some(Err(CommandError::new(
                        Reason::Declined,
                        tr!("Activation du micro non acceptée (question refusée, ou fermée par une autre action) : aucun réglage n'a changé."),
                    ))),
                    None if Instant::now() >= deadline => {
                        if matches!(*app.confirm.borrow(), Some(Confirm::EnableMicrophone)) {
                            app.close_sheet();
                            app.control.borrow_mut().mic_answer = None;
                        }
                        Some(Err(CommandError::new(
                            Reason::Timeout,
                            tr!("Pas de réponse en deux minutes : le micro n'a pas été activé, aucun réglage n'a changé."),
                        )))
                    }
                    None => None,
                }
            }),
            limit: MIC_ANSWER_LIMIT + Duration::from_secs(10),
        })
    }

    // ----- Programmations ---------------------------------------------------------------

    fn schedule_json(&self, schedule: &Schedule) -> Value {
        let now = Utc::now();
        let config = self.config.borrow();
        let owned = self.planner.borrow().owned().cloned().filter(|_| self.recording());
        let recording = owned.as_ref().is_some_and(|slot| slot.schedule_id == schedule.id);
        let slot_json = |slot: &Slot| json!({ "starts_at": instant(slot.start), "ends_at": instant(slot.end) });
        let mut value = json!({
            "id": schedule.id,
            "scene": {
                "id": schedule.scene_id,
                "name": config.scene(&schedule.scene_id).map(|s| s.name.clone()).unwrap_or_default(),
            },
            "start": format!("{:02}:{:02}", schedule.start.hour(), schedule.start.minute()),
            "end": format!("{:02}:{:02}", schedule.end.hour(), schedule.end.minute()),
            "ends_next_day": schedule.crosses_midnight(),
            "enabled": schedule.enabled,
            "recording": recording,
            "current_slot": schedule.slot_at(now, &Local).map(|s| slot_json(&s)),
            "next_slot": schedule.next_slot(now, &Local).filter(|_| schedule.enabled).map(|s| slot_json(&s)),
        });
        match &schedule.repeat {
            Repeat::Once { date } => value["date"] = json!(date.format("%Y-%m-%d").to_string()),
            Repeat::Weekly { days } => {
                value["days"] =
                    json!(days.iter().filter_map(|d| DAYS.get(usize::from(*d).saturating_sub(1))).collect::<Vec<_>>())
            }
        }
        value
    }

    fn schedules_json(&self) -> Value {
        let schedules = self.config.borrow().schedules.clone();
        json!({
            "launch_at_startup": self.config.borrow().settings.launch_at_startup,
            "schedules": schedules.iter().map(|s| self.schedule_json(s)).collect::<Vec<_>>(),
        })
    }

    fn check_schedule_editor(&self) -> Result<(), CommandError> {
        if matches!(self.sheet().as_str(), "schedule-new" | "schedule-edit") {
            return Err(CommandError::new(
                Reason::Busy,
                tr!("Une programmation est en cours de modification dans Polyptyque : fermer son formulaire d'abord."),
            ));
        }
        Ok(())
    }

    /// Répétition donnée par `date` ou `days`.
    fn repeat_param(params: &Value) -> Result<Option<Repeat>, CommandError> {
        match (text(params, "date"), params.get("days").and_then(Value::as_array)) {
            (Some(_), Some(_)) => Err(CommandError::invalid(tr!("Donner date ou days, pas les deux."))),
            (Some(date), None) => NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d")
                .map(|date| Some(Repeat::Once { date }))
                .map_err(|_| CommandError::invalid(tr!("Date invalide : « {} » (format AAAA-MM-JJ).", date))),
            (None, Some(days)) => {
                let mut numbers: Vec<u8> = days
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(|d| DAYS.iter().position(|known| *known == d))
                    .map(|i| i as u8 + 1)
                    .collect();
                numbers.sort_unstable();
                numbers.dedup();
                Ok(Some(Repeat::Weekly { days: numbers }))
            }
            (None, None) => Ok(None),
        }
    }

    fn time_param(params: &Value, key: &str) -> Result<Option<TimeOfDay>, CommandError> {
        match text(params, key) {
            Some(value) => TimeOfDay::parse(value)
                .map(Some)
                .ok_or_else(|| CommandError::invalid(tr!("Heure invalide : « {} » (format HH:MM sur 24 h).", value))),
            None => Ok(None),
        }
    }

    fn cmd_schedule_create(&self, params: &Value) -> Outcome {
        self.check_schedule_editor()?;
        let scene = self.find_scene(required(params, "scene")?)?;
        let start = Self::time_param(params, "start")?
            .ok_or_else(|| CommandError::invalid(tr!("Heure de début manquante.")))?;
        let end =
            Self::time_param(params, "end")?.ok_or_else(|| CommandError::invalid(tr!("Heure de fin manquante.")))?;
        let repeat = Self::repeat_param(params)?
            .ok_or_else(|| CommandError::invalid(tr!("Donner date (une seule fois) ou days (chaque semaine).")))?;
        let mut schedule = Schedule::new(&scene.id, start, end, repeat);
        schedule.enabled = flag(params, "enabled").unwrap_or(true);
        let id = schedule.id.clone();
        self.store_schedule(schedule).map_err(refused)?;
        let schedule = self.find_schedule(&id)?;
        Ok(Reply::ok(json!({ "schedule": self.schedule_json(&schedule) })))
    }

    fn cmd_schedule_update(&self, params: &Value) -> Outcome {
        self.check_schedule_editor()?;
        let mut schedule = self.find_schedule(required(params, "schedule")?)?;
        if self.is_locked_schedule(&schedule.id) {
            return Err(CommandError::new(
                Reason::Locked,
                tr!("Cette programmation enregistre en ce moment : modifiable une fois son créneau terminé."),
            ));
        }
        if let Some(reference) = text(params, "scene") {
            schedule.scene_id = self.find_scene(reference)?.id;
        }
        if let Some(start) = Self::time_param(params, "start")? {
            schedule.start = start;
        }
        if let Some(end) = Self::time_param(params, "end")? {
            schedule.end = end;
        }
        if let Some(repeat) = Self::repeat_param(params)? {
            schedule.repeat = repeat;
        }
        let id = schedule.id.clone();
        self.store_schedule(schedule).map_err(refused)?;
        let schedule = self.find_schedule(&id)?;
        Ok(Reply::ok(json!({ "schedule": self.schedule_json(&schedule) })))
    }

    fn cmd_schedule_enable(&self, params: &Value) -> Outcome {
        self.check_schedule_editor()?;
        let schedule = self.find_schedule(required(params, "schedule")?)?;
        if self.is_locked_schedule(&schedule.id) {
            return Err(CommandError::new(
                Reason::Locked,
                tr!("Cette programmation enregistre en ce moment : modifiable une fois son créneau terminé."),
            ));
        }
        self.set_schedule_enabled(&schedule.id, flag(params, "enabled").unwrap_or(true)).map_err(refused)?;
        let schedule = self.find_schedule(&schedule.id)?;
        Ok(Reply::ok(json!({ "schedule": self.schedule_json(&schedule) })))
    }

    fn cmd_schedule_delete(&self, params: &Value) -> Outcome {
        self.check_schedule_editor()?;
        let schedule = self.find_schedule(required(params, "schedule")?)?;
        if self.is_locked_schedule(&schedule.id) {
            return Err(CommandError::new(
                Reason::Locked,
                tr!("Cette programmation enregistre en ce moment : supprimable une fois son créneau terminé."),
            ));
        }
        self.delete_schedule(&schedule.id);
        Ok(Reply::ok(json!({ "deleted": schedule.id })))
    }

    // ----- Fichiers --------------------------------------------------------------------

    fn recordings_json(&self, params: &Value) -> Value {
        let dir = self.config.borrow().settings.output_dir.clone();
        let limit = int(params, "limit").unwrap_or(50).clamp(1, 500) as usize;
        let mut files: Vec<(SystemTime, PathBuf, u64)> = fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter_map(|e| {
                        let path = e.path();
                        let extension = path.extension()?.to_string_lossy().to_lowercase();
                        if !["mkv", "mka", "mp4", "m4a"].contains(&extension.as_str()) {
                            return None;
                        }
                        let meta = e.metadata().ok().filter(|m| m.is_file())?;
                        Some((meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), path, meta.len()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        files.sort_by_key(|f| std::cmp::Reverse(f.0));
        let in_progress = self.session.borrow().as_ref().map(|s| s.files_in_progress()).unwrap_or_default();
        json!({
            "folder": dir.display().to_string(),
            "files": files.iter().take(limit).map(|(modified, path, size)| json!({
                "path": path.display().to_string(),
                "name": file_name(path),
                "size_bytes": size,
                "modified": DateTime::<Local>::from(*modified).to_rfc3339_opts(SecondsFormat::Secs, false),
                "recording": in_progress.iter().any(|p| super::same_file(p, path)),
            })).collect::<Vec<_>>(),
        })
    }

    fn cmd_convert(&self, params: &Value) -> Outcome {
        let files: Vec<PathBuf> = params
            .get("files")
            .and_then(Value::as_array)
            .map(|items| {
                items.iter().filter_map(Value::as_str).map(|f| PathBuf::from(f.trim().trim_matches('"'))).collect()
            })
            .unwrap_or_default();
        if files.is_empty() {
            return Err(CommandError::invalid(tr!("Aucun fichier à convertir.")));
        }
        for file in &files {
            if !file.is_file() {
                return Err(not_found(tr!("Fichier introuvable : {}", file.display())));
            }
            if !remux::is_convertible(file) {
                return Err(refused(tr!("Seuls les fichiers MKV et MKA se convertissent : {}", file.display())));
            }
        }
        let in_progress = self.session.borrow().as_ref().map(|s| s.files_in_progress()).unwrap_or_default();
        if let Some(busy_file) = files.iter().find(|f| in_progress.iter().any(|p| super::same_file(p, f))) {
            return Err(CommandError::new(
                Reason::Busy,
                tr!(
                    "En cours d'enregistrement : {} (à convertir une fois l'enregistrement arrêté).",
                    busy_file.display()
                ),
            ));
        }
        let count = files.len();
        self.enqueue_conversions(files);
        self.toast(&super::added_to_conversion(count), 1);
        Ok(Reply::ok(json!({ "queued": count })))
    }

    fn cmd_cancel_conversion(&self, params: &Value) -> Outcome {
        if !self.with_bridge_value(|b| b.get_converting()) {
            return Err(refused(tr!("Aucune conversion en cours.")));
        }
        if !confirmed(params) {
            return Err(confirmation(&self.cancel_conversion_text()));
        }
        if let Some(converter) = self.converter.borrow().as_ref() {
            self.conversion.borrow_mut().cancelled = true;
            converter.cancel_all();
        }
        Ok(Reply::ok(json!({ "cancelled": true })))
    }

    // ----- Polyptyque ---------------------------------------------------------------------

    fn cmd_update(&self, params: &Value) -> Outcome {
        let Some(available) = self.update.borrow().clone() else {
            return Err(CommandError::new(Reason::Unavailable, tr!("Aucune nouvelle version proposée.")));
        };
        match self.update_state.get() {
            UpdateState::Downloading => return Err(busy()),
            UpdateState::None => {
                return Err(CommandError::new(Reason::Unavailable, tr!("Aucune nouvelle version proposée.")))
            }
            UpdateState::Available | UpdateState::Ready => {}
        }
        if let Some(message) = self.update_blocked() {
            return Err(CommandError::new(Reason::Busy, message));
        }
        if !confirmed(params) {
            return Err(confirmation(&tr!(
                "Télécharger la version {} ({}) puis redémarrer Polyptyque ? Scènes, réglages et connexions des navigateurs sont conservés.",
                available.version,
                format::bytes(available.size)
            )));
        }
        if self.update_state.get() == UpdateState::Ready {
            // Réponse envoyée avant le redémarrage.
            Timer::single_shot(Duration::from_millis(300), || {
                with_app(|app| app.apply_update());
            });
        } else {
            self.download_update();
        }
        Ok(Reply::ok(json!({ "version": available.version, "restart": true })))
    }

    fn cmd_quit(&self, params: &Value) -> Outcome {
        if self.busy.get() {
            return Err(busy());
        }
        let confirm = confirmed(params);
        if self.recording() && !confirm {
            return Err(confirmation(&tr!(
                "Arrêter l'enregistrement et quitter Polyptyque ? Les fichiers seront finalisés proprement."
            )));
        }
        if self.with_bridge_value(|b| b.get_converting()) && !confirm {
            return Err(confirmation(&tr!(
                "Quitter maintenant interrompt la conversion MP4. Les fichiers MKV d'origine sont conservés : ils pourront être convertis plus tard."
            )));
        }
        if !confirm && self.schedules_need_polyptyque() {
            if let Some(next) = self.quit_schedule_next() {
                return Err(confirmation(&self.quit_schedule_warning(&next)));
            }
        }
        log::info!("pilotage : Polyptyque quitte à la demande d'un programme");
        // Réponse envoyée d'abord.
        Timer::single_shot(Duration::from_millis(300), || {
            with_app(|app| app.quit_from_control());
        });
        Ok(Reply::ok(json!({ "quitting": true })))
    }

    fn quit_from_control(&self) {
        self.quit_with_schedules.set(true);
        if self.recording() {
            self.quitting.set(true);
            self.stop_recording(None);
        } else if self.busy.get() {
            self.quitting.set(true);
        } else {
            self.previews.borrow_mut().stop();
            self.meters.borrow_mut().stop();
            self.mic_meter.borrow_mut().take();
            let _ = slint::quit_event_loop();
        }
    }

    // ----- État -------------------------------------------------------------------------

    /// État complet, pour `status` et l'état en direct.
    pub(super) fn control_state(&self) -> Value {
        let now = Utc::now();
        let config = self.config.borrow();
        let scene = config.active_scene();
        let control = self.control.borrow();
        let session = self.session.borrow();
        let sources: Vec<Value> = config
            .active_sources()
            .iter()
            .enumerate()
            .map(|(i, source)| {
                let status = control.statuses.iter().find(|s| s.id == source.id);
                let engine = session.as_ref().is_some_and(|s| s.is_recording(&source.id)).then_some(status).flatten();
                let joining = self.joining.borrow().contains(&source.id);
                let (recording, window) = match engine {
                    Some(status) => engine_state(status.state),
                    None if joining => ("starting", preview_state(control.previews.get(&source.id))),
                    None => ("off", preview_state(control.previews.get(&source.id))),
                };
                let mut value = json!({
                    "id": source.id,
                    "name": source.name,
                    "kind": if source.is_browser() { "browser" } else { "application" },
                    "position": i + 1,
                    "recording": recording,
                    "window": window,
                });
                if let Some(status) = engine {
                    value["file_size_bytes"] = json!(status.file_size);
                    value["dropped_frames"] = json!(status.frames_dropped);
                    value["file"] = json!(status.output.display().to_string());
                    if let Some(error) = &status.error {
                        value["error"] = json!(error);
                    }
                }
                value
            })
            .collect();
        let per_hour = match session.as_ref() {
            Some(session) => session.bytes_per_hour(),
            None => {
                bytes_per_hour(config.settings.video.bitrate_kbps, config.settings.audio.bitrate_kbps)
                    * scene.source_ids.len().max(1) as u64
            }
        };
        let free = self.free_space.get();
        let owned = self.planner.borrow().owned().cloned().filter(|_| session.is_some());
        let next = schedule::next_slot(&config.schedules, now, &Local);
        let scene_name = |id: &str| config.scene(id).map(|s| s.name.clone()).unwrap_or_default();
        let prompt = self.prompt.borrow().as_ref().map(|prompt| {
            let kind = match prompt {
                Prompt::Missed { .. } => "missed_start",
                Prompt::KeepAutoStop { .. } => "auto_stop",
                Prompt::ManualRunning { .. } => "info",
            };
            let (text, countdown) =
                self.with_bridge_value(|b| (b.get_prompt_text().to_string(), b.get_prompt_countdown().to_string()));
            json!({ "kind": kind, "text": text, "countdown": countdown })
        });
        let conversion = self.conversion.borrow();
        let converting = self.with_bridge_value(|b| b.get_converting());
        let window = if self.in_background.get() {
            "background"
        } else if self.ui.upgrade().is_some_and(|ui| ui.window().is_minimized()) {
            "minimized"
        } else {
            "visible"
        };
        let busy_text = self.with_bridge_value(|b| b.get_busy_text().to_string());
        json!({
            "recording": {
                "active": session.is_some(),
                "elapsed_seconds": session.as_ref().map(|s| s.elapsed().as_secs()),
                "scheduled_until": owned.as_ref().map(|slot| instant(slot.end)),
                "microphone": session.is_some() && config.settings.microphone.enabled,
            },
            "busy": Some(busy_text).filter(|t| !t.is_empty()),
            "scene": { "id": scene.id, "name": scene.name },
            "sources": sources,
            "disk": {
                "free_bytes": free,
                "hours_left": free.map(|f| ((f as f64 / per_hour.max(1) as f64) * 10.0).round() / 10.0),
                "folder": session.as_ref().map(|s| s.current_output_dir()).unwrap_or_else(|| config.settings.output_dir.clone()).display().to_string(),
            },
            "conversion": {
                "active": converting,
                "total": if converting { conversion.total } else { 0 },
                "finished": if converting { conversion.finished } else { 0 },
            },
            "next_schedule": next.map(|slot| json!({
                "schedule_id": slot.schedule_id,
                "scene": { "id": slot.scene_id, "name": scene_name(&slot.scene_id) },
                "starts_at": instant(slot.start),
                "ends_at": instant(slot.end),
            })),
            "prompt": prompt,
            "previews": config.settings.previews,
            "microphone": config.settings.microphone.enabled,
            "window": window,
            "update_available": matches!(self.update_state.get(), UpdateState::Available | UpdateState::Ready)
                .then(|| self.update.borrow().as_ref().map(|u| u.version.clone()))
                .flatten(),
            "control_clients": control.client_count(),
        })
    }
}
