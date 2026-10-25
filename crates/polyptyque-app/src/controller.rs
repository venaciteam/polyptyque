//! Contrôleur : relie l'interface (le « pont » Slint) au moteur.
//!
//! Tout se passe sur le fil de l'interface, sauf les opérations longues
//! (recherche de l'encodeur, ouverture des fenêtres, finalisation des
//! fichiers), confiées à des fils séparés qui rendent la main par
//! `slint::invoke_from_event_loop`.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use polyptyque_core::browsers::{classify_executable, is_tor_browser, program_name};
use polyptyque_core::config::{
    BrowserFamily, BrowserRef, ClockChoice, Codec, Config, HardwarePreference, LanguageChoice, PasteError, Scene,
    Settings, Source, SourceKind, UpdateChannel,
};
use polyptyque_core::encoder::{bytes_per_hour, EncoderChoice};
use polyptyque_core::hotkeys;
use polyptyque_core::naming;
use polyptyque_core::paths;
use polyptyque_core::platform::{BrowserInstall, Platform, WindowInfo};
use polyptyque_core::recovery::SessionRecord;
use polyptyque_core::storage;
use polyptyque_engine::remux;
use polyptyque_engine::resolve::{self, PrepareEvent};
use polyptyque_engine::{probe, MicrophonePlan, Session, SessionPlan, SourceState, SourceStatus, SourceSummary};
use slint::{CloseRequestResponse, ComponentHandle, Image, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use tr::tr;

use polyptyque_core::schedule::{self, Planner, Slot};

use crate::convert::{ConversionEvent, Converter, JobKind};
use crate::format;
use crate::meter::{MicMeter, SourceMeters};
use crate::preview::{self, PreviewManager, PreviewState};
use crate::update;
use crate::{AppWindow, Bridge, IncidentFile, SceneItem, ScheduleItem, SourceCard, SourceItem, WindowItem};

mod report;
mod scheduling;
use scheduling::Prompt;

const LAUNCH_WAIT: Duration = Duration::from_secs(30);
const DISK_STOP_THRESHOLD: u64 = 1_000_000_000;
const DISK_WARNING_THRESHOLD: u64 = 10_000_000_000;
const BACKLOG_WARNING_SECONDS: f32 = 2.0;
/// Fenêtre perdue pendant l'enregistrement : délai avant de rouvrir le
/// programme automatiquement (le temps qu'il redémarre seul, après une mise
/// à jour par exemple), puis entre deux essais.
const AUTO_REOPEN_DELAY: Duration = Duration::from_secs(20);
const AUTO_REOPEN_RETRY: Duration = Duration::from_secs(60);
/// Essais automatiques par disparition, puis sur tout l'enregistrement :
/// au-delà, seul le bouton « Rouvrir » reste.
const AUTO_REOPEN_TRIES: u32 = 3;
const AUTO_REOPEN_SESSION_TRIES: u32 = 10;
const TOAST_DURATION: Duration = Duration::from_secs(7);
/// Délai avant la première recherche de mise à jour, puis intervalle entre
/// deux recherches (pour les sessions longues).
const UPDATE_FIRST_CHECK: Duration = Duration::from_secs(5);
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 3600);
/// Choix « Mises à jour » des réglages : 0 versions stables, 1 stables et
/// bêta, 2 ne pas rechercher.
const UPDATES_OFF: i32 = 2;
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Raison affichée par Windows si l'on éteint pendant un enregistrement.
#[cfg(windows)]
fn shutdown_reason() -> String {
    tr!("Enregistrement en cours : l'arrêter dans Polyptyque pour finaliser les fichiers.")
}

const RESOLUTIONS: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)];
const FRAME_RATES: [u32; 5] = [24, 25, 30, 50, 60];
const AUDIO_BITRATES: [u32; 5] = [128, 160, 192, 256, 320];
const HARDWARE: [HardwarePreference; 3] = [HardwarePreference::Auto, HardwarePreference::Gpu, HardwarePreference::Cpu];

/// Actions qui demandent une confirmation.
#[derive(Clone)]
enum Confirm {
    DeleteScene,
    RemoveSource(String),
    QuitWhileRecording,
    QuitWhileConverting,
    CancelConversion,
    InstallUpdate,
    /// Reprendre l'enregistrement de cette scène, arrêté par une mise en
    /// veille.
    #[cfg_attr(not(windows), allow(dead_code))]
    ResumeAfterSleep(String),
    /// Réparer ces fichiers mal fermés.
    RepairFiles(Vec<PathBuf>),
    /// Arrêter l'enregistrement dont une programmation a la charge.
    StopScheduled,
    /// Quitter alors qu'une programmation doit encore lancer un
    /// enregistrement.
    QuitWithSchedules,
}

/// Fenêtre à montrer dès que l'interface est libre (aucun enregistrement,
/// aucune autre fenêtre ouverte).
enum Deferred {
    Incident(Option<SessionRecord>, Vec<(PathBuf, FileReport)>),
    Repair(Vec<PathBuf>),
}

/// Où en est la mise à jour proposée.
#[derive(Clone, Copy, PartialEq, Eq)]
enum UpdateState {
    None,
    Available,
    Downloading,
    /// Téléchargée : s'installe au redémarrage de Polyptyque.
    Ready,
}

/// Suivi de la file de conversion MP4, pour l'affichage.
#[derive(Default)]
struct ConversionProgress {
    /// Fichiers demandés depuis que la file était vide, dont réparations.
    total: usize,
    repairs: usize,
    /// Le fichier en cours est une réparation.
    repairing: bool,
    finished: usize,
    succeeded: usize,
    failures: Vec<String>,
    cancelled: bool,
}

/// Résultat de la préparation d'un enregistrement.
struct Prepared {
    ffmpeg: PathBuf,
    encoder: EncoderChoice,
    missing: Vec<String>,
    /// Dossier d'enregistrement : celui des réglages, ou celui de secours.
    output_dir: PathBuf,
    output_note: Option<String>,
}

/// Raccourci clavier global déclenché.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum HotkeyAction {
    ToggleRecording,
    ShowWindow,
    TogglePreviews,
}

pub struct App {
    ui: slint::Weak<AppWindow>,
    platform: Arc<dyn Platform>,
    config: RefCell<Config>,
    config_path: PathBuf,
    profiles_root: PathBuf,
    session: RefCell<Option<Session>>,
    /// Fil qui finalise un enregistrement arrêté.
    stop_thread: RefCell<Option<thread::JoinHandle<()>>>,
    busy: Cell<bool>,
    previews: RefCell<PreviewManager>,
    /// Niveau sonore des sources que le moteur n'écoute pas.
    meters: RefCell<SourceMeters>,
    /// Test du micro, pendant que les réglages sont ouverts.
    mic_meter: RefCell<Option<MicMeter>>,
    /// Définition actuelle des aperçus, et celle qui attend d'être confirmée
    /// (avec le nombre de mesures concordantes).
    preview_size: Cell<(u32, u32)>,
    pending_preview_size: Cell<Option<((u32, u32), u32)>>,
    cards: Rc<VecModel<SourceCard>>,
    browsers: RefCell<Vec<BrowserInstall>>,
    windows: RefCell<Vec<WindowInfo>>,
    confirm: RefCell<Option<Confirm>>,
    toast_timer: Timer,
    free_space: Cell<Option<u64>>,
    last_disk_check: Cell<Option<Instant>>,
    disk_check_running: Cell<bool>,
    quitting: Cell<bool>,
    /// Windows ferme la session (arrêt, redémarrage, déconnexion) : il ferme
    /// lui-même les navigateurs.
    session_ending: Cell<bool>,
    /// Sources en cours de préparation pour rejoindre l'enregistrement.
    joining: RefCell<HashSet<String>>,
    /// Numéro de l'enregistrement en cours : une préparation commencée
    /// pendant un enregistrement précédent est ignorée.
    session_number: Cell<u64>,
    converter: RefCell<Option<Converter>>,
    conversion: RefCell<ConversionProgress>,
    /// Fenêtres perdues pendant l'enregistrement, par source.
    lost_windows: RefCell<HashMap<String, LostWindow>>,
    /// Journal de l'enregistrement en cours, tel qu'écrit sur le disque.
    session_record: RefCell<Option<SessionRecord>>,
    /// Enregistrement interrompu trouvé au lancement, en attente d'une
    /// décision (réparer, reprendre).
    incident: RefCell<Option<Incident>>,
    incident_files: Rc<VecModel<IncidentFile>>,
    /// Réparations proposées par un bilan, à lancer à la fin de
    /// l'enregistrement démarré entre-temps.
    pending_repairs: RefCell<Vec<PathBuf>>,
    deferred: RefCell<Vec<Deferred>>,
    /// Ouverture des fenêtres de la scène en cours.
    opening_windows: Cell<bool>,
    /// Source copiée (Ctrl+C), à coller dans une autre scène.
    copied_source: RefCell<Option<String>>,
    /// Micros proposés dans les réglages : identifiant et nom.
    mic_choices: RefCell<Vec<(String, String)>>,
    /// Un problème de micro a déjà été signalé pendant cet enregistrement.
    mic_warned: Cell<bool>,
    /// Signalement d'un problème en cours.
    report: RefCell<report::ReportState>,
    /// Nouvelle version trouvée, et où en est son installation.
    update: RefCell<Option<update::Available>>,
    update_state: Cell<UpdateState>,
    update_checking: Cell<bool>,
    #[cfg(windows)]
    hotkeys: RefCell<Option<polyptyque_windows::hotkeys::HotkeyListener>>,
    /// La fermeture de session Windows est surveillée.
    #[cfg(windows)]
    session_watch: Cell<bool>,
    /// Enregistrement arrêté par une mise en veille : scène et heure, pour
    /// proposer de le reprendre au réveil.
    #[cfg(windows)]
    sleep_stop: RefCell<Option<(String, String)>>,
    /// Programmation : décisions (créneaux, démarrages, arrêts), et créneaux
    /// déjà réglés gardés dans ce fichier.
    planner: RefCell<Planner>,
    schedule_state_path: PathBuf,
    /// Créneau dont l'enregistrement est en train de démarrer.
    scheduled_start: RefCell<Option<Slot>>,
    /// Question posée par la programmation, en attente de réponse.
    prompt: RefCell<Option<Prompt>>,
    /// Anti-veille des programmations en place.
    schedules_awake: Cell<bool>,
    /// Mise en veille annoncée par Windows, depuis cet instant : la
    /// programmation attend le réveil.
    suspended: Cell<Option<chrono::DateTime<chrono::Utc>>>,
    /// Quitter malgré une programmation active : déjà confirmé.
    quit_with_schedules: Cell<bool>,
    /// Programmations affichées, et formulaire : programmation modifiée
    /// (aucune : nouvelle), scènes proposées, jours cochés.
    schedule_items: Rc<VecModel<ScheduleItem>>,
    editing_schedule: RefCell<Option<String>>,
    schedule_scene_ids: RefCell<Vec<String>>,
    schedule_days: Rc<VecModel<bool>>,
    /// Lancé par Windows à l'ouverture de session : la fenêtre est réduite
    /// dès qu'elle existe.
    minimize_pending: Cell<bool>,
}

/// Enregistrement interrompu trouvé au lancement.
struct Incident {
    /// Scène à reprendre, si elle existe encore.
    scene_id: Option<String>,
    /// Fichiers interrompus, à réparer.
    repair: Vec<PathBuf>,
}

/// État d'un fichier d'un enregistrement interrompu.
enum FileReport {
    Missing,
    Empty,
    Complete { seconds: f64, size: u64 },
    NeedsRepair { size: u64 },
    Unreadable(String),
}

/// Suivi d'une fenêtre perdue pendant l'enregistrement.
#[derive(Default)]
struct LostWindow {
    /// Début de la disparition en cours.
    since: Option<Instant>,
    last_try: Option<Instant>,
    /// Réouvertures automatiques pour cette disparition, puis pour tout
    /// l'enregistrement.
    tries: u32,
    session_tries: u32,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

/// Exécute `f` avec l'application, sur le fil de l'interface.
fn with_app<R>(f: impl FnOnce(&Rc<App>) -> R) -> Option<R> {
    let app = APP.with(|a| a.borrow().clone())?;
    Some(f(&app))
}

/// Depuis n'importe quel fil : planifie `f` sur le fil de l'interface.
fn post(f: impl FnOnce(&Rc<App>) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        with_app(f);
    });
}

pub struct Paths {
    pub config: PathBuf,
    pub profiles: PathBuf,
    /// Créneaux de programmation déjà réglés.
    pub schedule_state: PathBuf,
}

/// `at_startup` : lancé par Windows à l'ouverture de session (réduit, sans
/// ouvrir les fenêtres de la scène).
pub fn run(platform: Arc<dyn Platform>, paths: Paths, initial: Option<Config>, at_startup: bool) -> Result<()> {
    let ui = AppWindow::new().context(tr!("création de la fenêtre"))?;
    crate::i18n::apply_to_ui(crate::i18n::Language::current());
    let (mut config, warning) = match initial {
        Some(config) => (config, None),
        None => Config::load_or_default(&paths.config),
    };
    // Version portable déplacée, autre PC : profils et navigateurs retrouvés.
    let browsers = if config.has_missing_browser() { platform.detect_browsers() } else { Vec::new() };
    if config.relocate(&paths.profiles, &browsers) {
        log::info!("profils de navigateur ou navigateurs retrouvés à leur nouvel emplacement");
        if let Err(e) = config.save(&paths.config) {
            log::warn!("enregistrement de la configuration : {e:#}");
        }
    }
    let (planner, planner_warning) = Planner::load(&paths.schedule_state);
    if let Some(warning) = planner_warning {
        log::warn!("{warning}");
    }
    let app = Rc::new(App {
        ui: ui.as_weak(),
        platform,
        config: RefCell::new(config),
        config_path: paths.config,
        profiles_root: paths.profiles,
        session: RefCell::new(None),
        stop_thread: RefCell::new(None),
        busy: Cell::new(false),
        previews: RefCell::new(PreviewManager::default()),
        meters: RefCell::new(SourceMeters::default()),
        mic_meter: RefCell::new(None),
        preview_size: Cell::new(preview::default_size()),
        pending_preview_size: Cell::new(None),
        cards: Rc::new(VecModel::default()),
        browsers: RefCell::new(Vec::new()),
        windows: RefCell::new(Vec::new()),
        confirm: RefCell::new(None),
        toast_timer: Timer::default(),
        free_space: Cell::new(None),
        last_disk_check: Cell::new(None),
        disk_check_running: Cell::new(false),
        quitting: Cell::new(false),
        session_ending: Cell::new(false),
        joining: RefCell::new(HashSet::new()),
        session_number: Cell::new(0),
        converter: RefCell::new(None),
        conversion: RefCell::new(ConversionProgress::default()),
        lost_windows: RefCell::new(HashMap::new()),
        session_record: RefCell::new(None),
        incident: RefCell::new(None),
        incident_files: Rc::new(VecModel::default()),
        pending_repairs: RefCell::new(Vec::new()),
        deferred: RefCell::new(Vec::new()),
        opening_windows: Cell::new(false),
        copied_source: RefCell::new(None),
        mic_choices: RefCell::new(Vec::new()),
        mic_warned: Cell::new(false),
        report: RefCell::new(report::ReportState::default()),
        update: RefCell::new(None),
        update_state: Cell::new(UpdateState::None),
        update_checking: Cell::new(false),
        #[cfg(windows)]
        hotkeys: RefCell::new(None),
        #[cfg(windows)]
        session_watch: Cell::new(false),
        #[cfg(windows)]
        sleep_stop: RefCell::new(None),
        planner: RefCell::new(planner),
        schedule_state_path: paths.schedule_state,
        scheduled_start: RefCell::new(None),
        prompt: RefCell::new(None),
        schedules_awake: Cell::new(false),
        suspended: Cell::new(None),
        quit_with_schedules: Cell::new(false),
        schedule_items: Rc::new(VecModel::default()),
        editing_schedule: RefCell::new(None),
        schedule_scene_ids: RefCell::new(Vec::new()),
        schedule_days: Rc::new(VecModel::from(vec![false; 7])),
        minimize_pending: Cell::new(at_startup),
    });
    APP.with(|a| *a.borrow_mut() = Some(app.clone()));
    *app.converter.borrow_mut() =
        Some(Converter::start(app.platform.clone(), |event| post(move |app| app.on_conversion(event))));

    install_callbacks(&ui);
    let bridge = ui.global::<Bridge>();
    // Version portable : on le signale, avec l'emplacement de ses données.
    let version = match paths::portable_root() {
        Some(_) => tr!("{} · portable", VERSION),
        None => VERSION.to_string(),
    };
    bridge.set_version(version.into());
    bridge.set_cards(ModelRc::from(app.cards.clone()));
    bridge.set_incident_files(ModelRc::from(app.incident_files.clone()));
    bridge.set_schedules(ModelRc::from(app.schedule_items.clone()));
    bridge.set_schedule_days(ModelRc::from(app.schedule_days.clone()));
    bridge.set_can_browse(cfg!(windows));
    app.apply_language_and_clock();
    app.refresh_all();
    app.restart_previews();
    app.register_hotkeys();
    app.check_interrupted_session();
    // Emplacement inscrit au démarrage de Windows remis à jour (version
    // portable déplacée).
    app.apply_launch_at_startup(false);
    // Lancé par Windows : les fenêtres de la scène s'ouvriront au début du
    // créneau, comme pour tout enregistrement.
    if app.config.borrow().settings.open_windows_at_launch && !at_startup {
        app.open_scene_windows(true);
    }
    if let Some(message) = warning {
        app.toast(&message, 2);
    } else if let Some(version) = crate::UPDATED_TO.get() {
        app.toast(&tr!("Polyptyque a été mis à jour : version {}.", version), 1);
    }

    ui.window()
        .on_close_requested(|| with_app(|app| app.close_requested()).unwrap_or(CloseRequestResponse::HideWindow));

    let status_timer = Timer::default();
    status_timer.start(TimerMode::Repeated, Duration::from_millis(500), || {
        with_app(|app| app.tick_status());
    });
    let preview_timer = Timer::default();
    preview_timer.start(TimerMode::Repeated, Duration::from_millis(100), || {
        with_app(|app| app.tick_previews());
    });
    // Mises à jour : peu après le démarrage, puis régulièrement.
    Timer::single_shot(UPDATE_FIRST_CHECK, || {
        with_app(|app| app.check_for_updates());
    });
    let update_timer = Timer::default();
    update_timer.start(TimerMode::Repeated, UPDATE_CHECK_INTERVAL, || {
        with_app(|app| app.check_for_updates());
    });

    ui.run().context(tr!("boucle de l'interface"))?;

    app.previews.borrow_mut().stop();
    app.meters.borrow_mut().stop();
    app.mic_meter.borrow_mut().take();
    app.release_schedule_awake();
    // Créneau arrêté juste avant de quitter : à ne pas relancer.
    app.save_planner();
    // Fermeture par le système pendant un enregistrement : on finalise.
    let session = app.session.borrow_mut().take();
    if let Some(session) = session {
        session.stop();
        app.clear_session_record();
    }
    // Une conversion interrompue efface son fichier partiel ; le MKV reste.
    let converter = app.converter.borrow_mut().take();
    drop(converter);
    // Fermeture voulue par l'utilisateur : les navigateurs des sources
    // suivent. Jamais à la fermeture de la session Windows, qui les ferme
    // elle-même : fermer leurs fenêtres en même temps ferait perdre celles
    // que le navigateur doit rouvrir.
    if !app.session_ending.get() && app.config.borrow().settings.close_browsers_at_exit {
        app.close_source_browsers();
    }
    APP.with(|a| a.borrow_mut().take());
    Ok(())
}

fn install_callbacks(ui: &AppWindow) {
    let b = ui.global::<Bridge>();
    b.on_start_stop(|| {
        with_app(|a| a.start_stop());
    });
    b.on_select_scene(|id| {
        with_app(|a| a.select_scene(&id));
    });
    b.on_request_new_scene(|| {
        with_app(|a| a.open_scene_sheet(true));
    });
    b.on_request_rename_scene(|| {
        with_app(|a| a.open_scene_sheet(false));
    });
    b.on_request_duplicate_scene(|| {
        with_app(|a| a.open_scene_sheet_duplicate());
    });
    b.on_request_delete_scene(|| {
        with_app(|a| a.request_delete_scene());
    });
    b.on_confirm_scene_name(|| {
        with_app(|a| a.confirm_scene_name());
    });
    b.on_request_add_browser(|| {
        with_app(|a| a.open_browser_sheet());
    });
    b.on_confirm_add_browser(|| {
        with_app(|a| a.confirm_add_browser());
    });
    b.on_request_add_application(|| {
        with_app(|a| a.open_application_sheet());
    });
    b.on_refresh_windows(|| {
        with_app(|a| a.refresh_windows());
    });
    b.on_window_selected(|index| {
        with_app(|a| a.window_selected(index));
    });
    b.on_confirm_add_application(|| {
        with_app(|a| a.confirm_add_application());
    });
    b.on_request_remove_source(|| {
        with_app(|a| a.request_remove_source());
    });
    b.on_move_source(|delta| {
        with_app(|a| a.move_source(delta));
    });
    b.on_move_source_to(|from, to| {
        with_app(|a| a.move_source_to(from, to));
    });
    b.on_copy_source(|| {
        with_app(|a| a.copy_source());
    });
    b.on_paste_source(|| {
        with_app(|a| a.paste_source());
    });
    b.on_open_source(|id| {
        with_app(|a| a.open_source(&id));
    });
    b.on_restore_window(|id| {
        with_app(|a| a.restore_window(&id));
    });
    b.on_open_scene_windows(|| {
        with_app(|a| a.open_scene_windows(false));
    });
    b.on_incident_repair(|| {
        with_app(|a| a.incident_repair());
    });
    b.on_incident_resume(|| {
        with_app(|a| a.incident_resume());
    });
    b.on_request_report(|from_incident| {
        with_app(|a| a.request_report(from_incident));
    });
    b.on_prepare_report(|| {
        with_app(|a| a.prepare_report());
    });
    b.on_open_report(|| {
        with_app(|a| a.open_report());
    });
    b.on_open_report_page(|| {
        with_app(|a| a.open_report_page());
    });
    b.on_toggle_source(|id| {
        with_app(|a| a.toggle_source(&id));
    });
    b.on_open_settings(|| {
        with_app(|a| a.open_settings());
    });
    b.on_browse_output(|| {
        with_app(|a| a.browse_output());
    });
    b.on_save_settings(|| {
        with_app(|a| a.save_settings());
    });
    b.on_open_output_folder(|| {
        with_app(|a| a.open_output_folder());
    });
    b.on_toggle_previews(|| {
        with_app(|a| a.toggle_previews());
    });
    b.on_convert_files(|| {
        with_app(|a| a.convert_files());
    });
    b.on_install_update(|| {
        with_app(|a| a.install_update());
    });
    b.on_open_mic_privacy(|| {
        with_app(|a| a.open_mic_privacy());
    });
    b.on_request_cancel_conversion(|| {
        with_app(|a| a.request_cancel_conversion());
    });
    b.on_close_sheet(|| {
        with_app(|a| a.close_sheet());
    });
    b.on_confirm_accepted(|| {
        with_app(|a| a.confirm_accepted());
    });
    b.on_dismiss_toast(|| {
        with_app(|a| a.dismiss_toast());
    });
    b.on_open_schedules(|| {
        with_app(|a| a.open_schedules());
    });
    b.on_edit_schedule(|id| {
        with_app(|a| a.edit_schedule(&id));
    });
    b.on_toggle_schedule(|id, enabled| {
        with_app(|a| a.toggle_schedule(&id, enabled));
    });
    b.on_delete_schedule(|id| {
        with_app(|a| a.delete_schedule(&id));
    });
    b.on_save_schedule(|| {
        with_app(|a| a.save_schedule());
    });
    b.on_schedule_times_edited(|| {
        with_app(|a| a.schedule_times_edited());
    });
    b.on_toggle_schedule_day(|index| {
        with_app(|a| a.toggle_schedule_day(index));
    });
    b.on_close_schedule_editor(|| {
        with_app(|a| a.close_schedule_editor());
    });
    b.on_set_launch_at_startup(|enabled| {
        with_app(|a| a.set_launch_at_startup(enabled));
    });
    b.on_prompt_accepted(|| {
        with_app(|a| a.prompt_accepted());
    });
    b.on_prompt_declined(|| {
        with_app(|a| a.prompt_declined());
    });
}

impl App {
    fn with_bridge(&self, f: impl FnOnce(&Bridge)) {
        if let Some(ui) = self.ui.upgrade() {
            f(&ui.global::<Bridge>());
        }
    }

    fn with_bridge_value<R: Default>(&self, f: impl FnOnce(&Bridge) -> R) -> R {
        self.ui.upgrade().map(|ui| f(&ui.global::<Bridge>())).unwrap_or_default()
    }

    fn recording(&self) -> bool {
        self.session.borrow().is_some()
    }

    /// Enregistrement en cours ou opération longue : rien ne doit changer
    /// dans les scènes et les sources.
    fn locked(&self) -> bool {
        self.recording() || self.busy.get()
    }

    fn save(&self) {
        let mut config = self.config.borrow_mut();
        config.normalize();
        if let Err(e) = config.save(&self.config_path) {
            drop(config);
            self.toast(&tr!("Impossible d'enregistrer la configuration : {}", format!("{e:#}")), 3);
        }
    }

    // ----- Affichage -------------------------------------------------------

    fn refresh_all(&self) {
        let config = self.config.borrow();
        let scene = config.active_scene();
        let scene_sources = config.active_sources();
        let scenes: Vec<SceneItem> = config
            .scenes
            .iter()
            .map(|s| SceneItem {
                id: s.id.as_str().into(),
                name: s.name.as_str().into(),
                count: s.source_ids.len() as i32,
                active: s.id == config.active_scene,
            })
            .collect();
        // Une source partagée le dit : la modifier (se connecter dans son
        // navigateur…) la modifie dans toutes ses scènes.
        let sources: Vec<SourceItem> = scene_sources
            .iter()
            .map(|s| {
                let kind = match config.scenes_with(&s.id).len() {
                    0 | 1 => s.kind_label(),
                    n => tr!("{} · {n} scènes", s.kind_label(), n),
                };
                SourceItem {
                    id: s.id.as_str().into(),
                    name: s.name.as_str().into(),
                    kind: kind.into(),
                    browser: s.is_browser(),
                }
            })
            .collect();
        let previews = config.settings.previews;
        let cards: Vec<SourceCard> = scene_sources
            .iter()
            .map(|s| SourceCard {
                id: s.id.as_str().into(),
                name: s.name.as_str().into(),
                kind: s.kind_label().into(),
                state: if previews { tr!("Recherche…") } else { tr!("En attente") }.into(),
                tone: 0,
                preview: Image::default(),
                has_preview: false,
                placeholder: if previews { tr!("Recherche de la fenêtre…") } else { tr!("Aperçu désactivé") }.into(),
                level: 0.0,
                sound: false,
                size: "".into(),
                dropped: 0,
                warning: "".into(),
                can_open: false,
                can_restore: false,
                recording: false,
                busy: false,
            })
            .collect();
        let scene_name = scene.name.clone();
        let source_count = scene_sources.len();
        let microphone = config.settings.microphone.enabled;
        drop(config);
        let can_paste = self.can_paste();

        // Les aperçus et le moteur ne signalent que les changements : on garde
        // l'image et l'état déjà connus de chaque source.
        let previous: Vec<SourceCard> = self.cards.iter().collect();
        let recording = self.recording();
        let cards: Vec<SourceCard> = cards
            .into_iter()
            .map(|mut card| {
                let Some(old) = previous.iter().find(|c| c.id == card.id) else {
                    return card;
                };
                if previews || (recording && old.recording) {
                    card.state = old.state.clone();
                    card.tone = old.tone;
                }
                if previews {
                    card.preview = old.preview.clone();
                    card.has_preview = old.has_preview;
                    card.placeholder = old.placeholder.clone();
                }
                if previews || (recording && old.recording) {
                    card.can_open = old.can_open;
                    card.can_restore = old.can_restore;
                }
                if recording {
                    card.recording = old.recording;
                    card.busy = old.busy;
                    card.level = old.level;
                    card.sound = old.sound;
                    card.size = old.size.clone();
                    card.dropped = old.dropped;
                    card.warning = old.warning.clone();
                }
                card
            })
            .collect();
        self.cards.set_vec(cards);
        self.with_bridge(|b| {
            b.set_scenes(ModelRc::new(VecModel::from(scenes)));
            b.set_scene_name(scene_name.into());
            b.set_sources(ModelRc::new(VecModel::from(sources)));
            b.set_can_paste(can_paste);
            b.set_previews_enabled(previews);
            b.set_mic_enabled(microphone);
            if b.get_selected_source() >= source_count as i32 {
                b.set_selected_source(source_count as i32 - 1);
            }
        });
        self.refresh_disk(true);
        if !self.recording() {
            self.refresh_encoder_text();
        }
    }

    fn refresh_encoder_text(&self) {
        let config = self.config.borrow();
        let video = &config.settings.video;
        let text = tr!(
            "{}x{} · {} i/s · {} · {} kb/s",
            video.width,
            video.height,
            video.fps,
            video.codec.label(),
            format::integer(u64::from(video.bitrate_kbps))
        );
        drop(config);
        self.with_bridge(|b| b.set_encoder_text(text.into()));
    }

    /// Met à jour l'espace disque affiché (au plus toutes les 10 s, sauf si
    /// `force`). La mesure se fait hors du fil de l'interface : un disque
    /// réseau lent ne doit pas la figer.
    fn refresh_disk(&self, force: bool) {
        let due = self.last_disk_check.get().is_none_or(|t| t.elapsed() >= Duration::from_secs(10));
        if (!force && !due) || self.disk_check_running.get() {
            return;
        }
        self.last_disk_check.set(Some(Instant::now()));
        self.disk_check_running.set(true);
        // Pendant un enregistrement : le dossier réellement utilisé (celui de
        // secours si le dossier choisi est devenu inaccessible).
        let recording = self
            .session
            .try_borrow()
            .ok()
            .and_then(|s| s.as_ref().map(|s| (s.current_output_dir(), s.bytes_per_hour())));
        let (dir, per_hour) = match recording {
            Some(state) => state,
            None => {
                let config = self.config.borrow();
                let sources = config.active_scene().source_ids.len().max(1) as u64;
                let video = config.settings.video.bitrate_kbps;
                (
                    config.settings.output_dir.clone(),
                    bytes_per_hour(video, config.settings.audio.bitrate_kbps) * sources,
                )
            }
        };
        let platform = self.platform.clone();
        thread::spawn(move || {
            let free = platform.free_disk_space(&dir);
            post(move |app| app.apply_disk(free, per_hour));
        });
    }

    fn apply_disk(&self, free: Option<u64>, per_hour: u64) {
        self.disk_check_running.set(false);
        self.free_space.set(free);
        let (text, warning) = match free {
            Some(free) => {
                let autonomy = free as f64 / per_hour.max(1) as f64 * 3600.0;
                (
                    tr!("{} libres · ≈ {} d'enregistrement", format::bytes(free), format::hours(autonomy)),
                    free < DISK_WARNING_THRESHOLD || free < per_hour,
                )
            }
            None => (String::new(), false),
        };
        self.with_bridge(|b| {
            b.set_disk_text(text.into());
            b.set_disk_warning(warning);
        });
        if self.recording() && free.is_some_and(|free| free < DISK_STOP_THRESHOLD) {
            self.stop_recording(Some(tr!("Disque presque plein : arrêt automatique.")));
        }
    }

    fn toast(&self, message: &str, tone: i32) {
        log::info!("message affiché : {message}");
        self.with_bridge(|b| {
            b.set_toast_text(message.into());
            b.set_toast_tone(tone);
        });
        self.toast_timer.start(TimerMode::SingleShot, TOAST_DURATION, || {
            with_app(|a| a.dismiss_toast());
        });
    }

    fn dismiss_toast(&self) {
        self.with_bridge(|b| b.set_toast_text("".into()));
    }

    fn set_busy(&self, text: &str) {
        self.busy.set(!text.is_empty());
        self.with_bridge(|b| b.set_busy_text(text.into()));
    }

    fn update_card(&self, index: usize, f: impl FnOnce(&mut SourceCard)) {
        if let Some(mut card) = self.cards.row_data(index) {
            f(&mut card);
            self.cards.set_row_data(index, card);
        }
    }

    // ----- Aperçus ---------------------------------------------------------

    fn restart_previews(&self) {
        {
            let config = self.config.borrow();
            let mut previews = self.previews.borrow_mut();
            previews.stop();
            if config.settings.previews {
                *previews = PreviewManager::start(&self.platform, &config.active_sources(), self.preview_size.get());
            }
        }
        // Les vumètres suivent les mêmes sources, même aperçus coupés.
        self.restart_meters();
    }

    // ----- Vumètres --------------------------------------------------------

    fn restart_meters(&self) {
        let config = self.config.borrow();
        let mut meters = self.meters.borrow_mut();
        meters.stop();
        *meters = SourceMeters::start(&self.platform, &config.active_sources(), |id| self.engine_listens(id));
    }

    /// Le moteur écoute déjà le son de cette source (enregistrement,
    /// préparation, arrêt en cours) : sa carte affiche le niveau du moteur.
    fn engine_listens(&self, id: &str) -> bool {
        self.busy.get()
            || self.joining.borrow().contains(id)
            || self.session.borrow().as_ref().is_some_and(|s| s.is_recording(id))
    }

    /// Niveau des sources que le moteur n'écoute pas. `statuses` : état des
    /// sources de l'enregistrement en cours (vide sans enregistrement).
    fn tick_meters(&self, statuses: &[SourceStatus]) {
        let busy = self.busy.get();
        let now = self.platform.now_hns();
        let joining = self.joining.borrow();
        let meters = self.meters.borrow();
        for meter in meters.meters() {
            let id = meter.source_id.as_str();
            let engine = busy
                || joining.contains(id)
                || statuses.iter().any(|s| s.id == id && s.state != SourceState::Finished);
            meter.set_paused(engine);
            if engine {
                continue;
            }
            let (peak, recent) = meter.take(now);
            let level = format::level(peak);
            let Some(index) = (0..self.cards.row_count()).find(|&i| self.cards.row_data(i).is_some_and(|c| c.id == id))
            else {
                continue;
            };
            // Seulement si quelque chose change : la carte n'est pas redessinée pour rien.
            if self.cards.row_data(index).is_some_and(|c| c.level != level || c.sound != recent) {
                self.update_card(index, |card| {
                    card.level = level;
                    card.sound = recent;
                });
            }
        }
    }

    /// Test du micro dans les réglages : écouté seulement pendant qu'ils sont
    /// ouverts avec le micro activé, sur le micro choisi dans la liste (même
    /// pas encore enregistré).
    fn tick_mic_test(&self) {
        let wanted: Option<Option<String>> = self.with_bridge_value(|b| {
            (b.get_sheet() == "settings" && b.get_settings_mic() && !self.recording()).then(|| {
                let index = b.get_mic_index().max(0) as usize;
                self.mic_choices.borrow().get(index).map(|(id, _)| id.clone()).filter(|id| !id.is_empty())
            })
        });
        let mut mic_meter = self.mic_meter.borrow_mut();
        let Some(device) = wanted else {
            *mic_meter = None;
            return;
        };
        if mic_meter.as_ref().is_none_or(|m| m.device() != device.as_deref()) {
            *mic_meter = Some(MicMeter::start(&self.platform, device));
            self.with_bridge(|b| {
                b.set_mic_test_level(0.0);
                b.set_mic_test_sound(false);
                b.set_mic_test_error(SharedString::new());
            });
            return;
        }
        let Some(meter) = mic_meter.as_ref() else {
            return;
        };
        let (peak, recent) = meter.take(self.platform.now_hns());
        let error = meter.error().unwrap_or_default();
        self.with_bridge(|b| {
            b.set_mic_test_level(format::level(peak));
            b.set_mic_test_sound(recent);
            b.set_mic_test_error(error.into());
        });
    }

    /// Adapte la définition des aperçus à la taille des cartes. Un
    /// changement n'est appliqué qu'après deux mesures identiques (une
    /// seconde), pour ne pas recréer les captures pendant un redimensionnement.
    fn update_preview_size(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let card_width = ui.global::<Bridge>().get_card_width() * ui.window().scale_factor();
        let wanted = preview::size_for_card(card_width);
        if wanted == self.preview_size.get() {
            self.pending_preview_size.set(None);
            return;
        }
        let seen = match self.pending_preview_size.get() {
            Some((size, count)) if size == wanted => count + 1,
            _ => 1,
        };
        if seen < 2 {
            self.pending_preview_size.set(Some((wanted, seen)));
            return;
        }
        self.pending_preview_size.set(None);
        self.preview_size.set(wanted);
        log::info!("définition des aperçus : {}x{}", wanted.0, wanted.1);
        self.previews.borrow().set_size(wanted);
    }

    fn tick_previews(&self) {
        let previews = self.previews.borrow();
        for worker in previews.workers() {
            let (frame, state) = worker.take_update();
            if frame.is_none() && state.is_none() {
                continue;
            }
            let Some(index) = (0..self.cards.row_count())
                .find(|&i| self.cards.row_data(i).is_some_and(|c| c.id.as_str() == worker.source_id))
            else {
                continue;
            };
            self.update_card(index, |card| {
                if let Some(frame) = frame {
                    card.preview = Image::from_rgba8(frame);
                    card.has_preview = true;
                }
                if let Some(state) = state {
                    let (label, tone, placeholder, can_open) = match &state {
                        PreviewState::Searching => (tr!("Recherche…"), 0, tr!("Recherche de la fenêtre…"), false),
                        PreviewState::Live => (tr!("Fenêtre trouvée"), 1, String::new(), false),
                        PreviewState::Minimized => {
                            (tr!("Réduite"), 2, tr!("Fenêtre réduite : l'image est figée."), false)
                        }
                        PreviewState::Missing => (tr!("Introuvable"), 2, tr!("Fenêtre introuvable"), true),
                        PreviewState::Failed(e) => (tr!("Aperçu impossible"), 3, e.clone(), false),
                    };
                    if !matches!(state, PreviewState::Live | PreviewState::Minimized) {
                        card.has_preview = false;
                    }
                    card.placeholder = placeholder.into();
                    // Pendant l'enregistrement d'une source, l'état affiché et
                    // les boutons sont ceux du moteur.
                    if !card.recording {
                        card.state = label.into();
                        card.tone = tone;
                        card.can_open = can_open;
                        card.can_restore = matches!(state, PreviewState::Minimized);
                    }
                }
            });
        }
    }

    fn toggle_previews(&self) {
        let enabled = {
            let mut config = self.config.borrow_mut();
            config.settings.previews = !config.settings.previews;
            config.settings.previews
        };
        self.save();
        self.refresh_all();
        self.restart_previews();
        self.toast(&if enabled { tr!("Aperçus en direct activés.") } else { tr!("Aperçus en direct coupés.") }, 1);
    }

    // ----- Enregistrement --------------------------------------------------

    fn start_stop(&self) {
        if self.busy.get() {
            return;
        }
        if self.recording() {
            self.request_stop();
        } else if self.missed_prompt_open() {
            // Raccourci pendant la question du démarrage manqué : c'est un oui.
            self.prompt_accepted();
        } else {
            self.start_recording(None);
        }
    }

    /// Démarre toutes les sources de la scène, ou une seule (`only`).
    fn start_recording(&self, only: Option<&str>) {
        self.keep_incident_repairs();
        let config = self.config.borrow().clone();
        let scene = config.active_scene().clone();
        let mut sources = config.active_sources();
        if let Some(id) = only {
            sources.retain(|s| s.id == id);
        }
        if sources.is_empty() {
            self.toast(&tr!("Ajouter d'abord au moins une source à cette scène."), 2);
            self.flush_pending_repairs();
            return;
        }
        self.close_sheet();
        self.set_busy(&tr!("Recherche du meilleur encodeur…"));
        let platform = self.platform.clone();
        thread::spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| prepare(platform.as_ref(), &config, &sources)))
                .unwrap_or_else(|_| Err(anyhow!(tr!("erreur interne pendant la préparation (voir le journal)"))));
            post(move |app| app.finish_start(result, config, scene, sources));
        });
    }

    fn finish_start(&self, result: Result<Prepared>, config: Config, scene: Scene, sources: Vec<Source>) {
        self.set_busy("");
        if self.quitting.get() {
            let _ = slint::quit_event_loop();
            return;
        }
        let prepared = match result {
            Ok(prepared) => prepared,
            Err(e) => {
                self.toast(&tr!("Impossible de démarrer : {}", format!("{e:#}")), 3);
                self.recording_start_failed();
                self.flush_pending_repairs();
                self.show_next_deferred();
                return;
            }
        };
        let (date, time) = naming::now_parts();
        let mut settings = config.settings.clone();
        settings.output_dir = prepared.output_dir.clone();
        let plan = SessionPlan {
            scene_name: scene.name.clone(),
            sources: sources.clone(),
            settings,
            encoder: prepared.encoder,
            ffmpeg: prepared.ffmpeg,
            log_dir: paths::logs_dir().join(format!("session_{date}_{time}")),
            fallback_dir: paths::default_output_dir(),
            microphone: config
                .settings
                .microphone
                .enabled
                .then(|| MicrophonePlan { device_id: config.settings.microphone.device().map(str::to_string) }),
        };
        // Le moteur va écouter ces sources : leur écoute s'arrête avant.
        for source in &sources {
            self.meters.borrow().pause(&source.id);
        }
        match Session::start(self.platform.clone(), plan) {
            Ok(session) => {
                self.mic_warned.set(false);
                self.lost_windows.borrow_mut().clear();
                self.set_shutdown_block(true);
                let microphone = config.settings.microphone.enabled;
                self.with_bridge(|b| b.set_mic_recording(microphone));
                let record = SessionRecord {
                    date: date.clone(),
                    time: time.clone(),
                    scene_id: scene.id.clone(),
                    scene_name: scene.name.clone(),
                    files: session.outputs(),
                    ended_by_windows: false,
                };
                *self.session.borrow_mut() = Some(session);
                self.start_session_record(record);
                let video = &config.settings.video;
                let text = tr!(
                    "{} · {}x{} · {} i/s · {} kb/s",
                    prepared.encoder.label(),
                    video.width,
                    video.height,
                    video.fps,
                    format::integer(u64::from(video.bitrate_kbps))
                );
                self.with_bridge(|b| {
                    b.set_recording(true);
                    b.set_encoder_text(text.into());
                    b.set_elapsed("00:00:00".into());
                    b.set_sheet("".into());
                });
                let ids: Vec<&str> = sources.iter().map(|s| s.id.as_str()).collect();
                for i in 0..self.cards.row_count() {
                    self.update_card(i, |card| {
                        if ids.contains(&card.id.as_str()) {
                            card.recording = true;
                            card.can_open = false;
                            card.can_restore = false;
                        }
                    });
                }
                let (message, tone) = if let Some(note) = &prepared.output_note {
                    (tr!("Enregistrement démarré. {}", note), 2)
                } else if prepared.missing.is_empty() {
                    let message = match sources.as_slice() {
                        [single] if scene.source_ids.len() > 1 => {
                            tr!("Enregistrement de « {} » démarré.", single.name)
                        }
                        _ => tr!("Enregistrement démarré."),
                    };
                    (message, 1)
                } else {
                    (
                        tr!(
                            "Enregistrement démarré. Fenêtre introuvable : {}. Image noire en attendant son retour."
                                | "Enregistrement démarré. Fenêtres introuvables : {}. Image noire en attendant leur retour."
                                % prepared.missing.len(),
                            prepared.missing.join(", ")
                        ),
                        2,
                    )
                };
                // Enregistrement programmé (le message le précise), ou lancé à
                // la main pendant un créneau (l'arrêt automatique est proposé).
                self.recording_started(&message, tone);
            }
            Err(e) => {
                self.toast(&tr!("Impossible de démarrer : {}", format!("{e:#}")), 3);
                self.recording_start_failed();
                self.flush_pending_repairs();
                self.show_next_deferred();
            }
        }
    }

    /// Bouton d'une carte : démarre ou arrête cette seule source.
    fn toggle_source(&self, id: &str) {
        if self.busy.get() || self.joining.borrow().contains(id) {
            return;
        }
        let Some(source) = self.active_source(id) else {
            return;
        };
        let state = self.session.borrow().as_ref().map(|s| (s.is_recording(id), s.recording_count()));
        let Some((recording, count)) = state else {
            self.start_recording(Some(id));
            return;
        };
        if recording && count + self.joining.borrow().len() <= 1 {
            // Dernière source (et aucune en préparation) : c'est la fin de
            // l'enregistrement.
            self.request_stop();
        } else if recording {
            if let Some(session) = self.session.borrow_mut().as_mut() {
                session.stop_source(id);
            }
            self.set_card(id, |card| {
                card.busy = true;
                card.state = tr!("Finalisation…").into();
                card.tone = 0;
            });
            self.toast(
                &tr!("« {} » arrêtée : son fichier est finalisé, les autres sources continuent.", source.name),
                1,
            );
        } else {
            self.join_recording(source);
        }
    }

    /// Ajoute une source à l'enregistrement en cours : sa fenêtre est
    /// d'abord retrouvée ou ouverte, hors du fil de l'interface.
    fn join_recording(&self, source: Source) {
        self.joining.borrow_mut().insert(source.id.clone());
        self.set_card(&source.id, |card| {
            card.busy = true;
            card.state = tr!("Préparation…").into();
            card.tone = 0;
        });
        let platform = self.platform.clone();
        let number = self.session_number.get();
        thread::spawn(move || {
            let missing = catch_unwind(AssertUnwindSafe(|| {
                resolve::prepare_windows(platform.as_ref(), std::slice::from_ref(&source), LAUNCH_WAIT, |_| {})
            }))
            .unwrap_or_default();
            post(move |app| app.finish_join(source, missing, number));
        });
    }

    fn finish_join(&self, source: Source, missing: Vec<String>, number: u64) {
        if number != self.session_number.get() {
            // Préparation commencée pendant un enregistrement terminé depuis :
            // elle ne doit pas s'ajouter à un autre.
            return;
        }
        self.joining.borrow_mut().remove(&source.id);
        self.meters.borrow().pause(&source.id);
        let result = match self.session.borrow_mut().as_mut() {
            Some(session) => session.add_source(&source).map(|_| true),
            None => Ok(false),
        };
        match result {
            Ok(true) => {
                self.set_card(&source.id, |card| {
                    card.recording = true;
                    card.busy = false;
                    card.can_open = false;
                    card.can_restore = false;
                });
                if missing.is_empty() {
                    self.toast(&tr!("« {} » enregistre (nouveau fichier).", source.name), 1);
                } else {
                    self.toast(
                        &tr!(
                            "« {} » enregistre, mais sa fenêtre est introuvable : image noire en attendant.",
                            source.name
                        ),
                        2,
                    );
                }
            }
            Ok(false) => self.set_card(&source.id, |card| card.busy = false),
            Err(e) => {
                self.set_card(&source.id, |card| card.busy = false);
                self.toast(&tr!("Impossible d'ajouter « {} » : {}", source.name, format!("{e:#}")), 3);
                // Plus rien n'enregistre : l'enregistrement se termine.
                let idle = self.session.borrow().as_ref().is_some_and(|s| s.recording_count() == 0);
                if idle && self.joining.borrow().is_empty() {
                    self.stop_recording(None);
                }
            }
        }
    }

    fn set_card(&self, id: &str, f: impl FnOnce(&mut SourceCard)) {
        if let Some(index) = (0..self.cards.row_count()).find(|&i| self.cards.row_data(i).is_some_and(|c| c.id == id)) {
            self.update_card(index, f);
        }
    }

    fn stop_recording(&self, reason: Option<String>) {
        let Some(session) = self.session.borrow_mut().take() else {
            return;
        };
        // Arrêt voulu ou imposé (disque plein) : un enregistrement programmé
        // n'est pas relancé dans son créneau. Une mise en veille l'a déjà
        // rendu à la programmation, qui le reproposera au réveil.
        self.recording_stopping();
        // Les sources en préparation n'ont plus d'enregistrement à rejoindre.
        self.session_number.set(self.session_number.get() + 1);
        self.joining.borrow_mut().clear();
        self.set_busy(&tr!("Arrêt : finalisation des fichiers…"));
        let handle = thread::spawn(move || {
            let (summaries, reason, clean) = match catch_unwind(AssertUnwindSafe(move || session.stop())) {
                Ok(summaries) => (summaries, reason, true),
                Err(_) => (Vec::new(), Some(tr!("Erreur interne pendant l'arrêt (voir le journal).")), false),
            };
            post(move |app| app.finish_stop(summaries, reason, clean));
        });
        *self.stop_thread.borrow_mut() = Some(handle);
    }

    fn finish_stop(&self, summaries: Vec<SourceSummary>, reason: Option<String>, clean: bool) {
        let handle = self.stop_thread.borrow_mut().take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
        if clean {
            self.clear_session_record();
        } else {
            // Arrêt en erreur : fichiers peut-être mal fermés. Le journal est
            // gardé, et le bilan proposé juste après.
            self.session_record.borrow_mut().take();
        }
        self.set_shutdown_block(false);
        self.set_busy("");
        self.with_bridge(|b| {
            b.set_recording(false);
            b.set_mic_recording(false);
            b.set_mic_text(tr!("Micro").into());
            b.set_mic_tone(0);
        });
        self.refresh_all();
        self.restart_previews();
        if self.quitting.get() {
            let _ = slint::quit_event_loop();
            return;
        }
        // Une demande « arrêter et quitter » restée ouverte n'a plus d'objet.
        if matches!(*self.confirm.borrow(), Some(Confirm::QuitWhileRecording)) {
            self.close_sheet();
        }
        let errors: Vec<String> =
            summaries.iter().filter_map(|s| s.error.as_ref().map(|e| tr!("{} : {}", s.name, e))).collect();
        let total: u64 = summaries.iter().map(|s| s.file_size).sum();
        // Dossier réel des fichiers (celui de secours si le dossier choisi
        // était inaccessible).
        let dir = summaries
            .iter()
            .find_map(|s| s.outputs.first().and_then(|f| f.parent().map(Path::to_path_buf)))
            .unwrap_or_else(|| self.config.borrow().settings.output_dir.clone())
            .display()
            .to_string();
        let count: usize = summaries.iter().map(|s| s.outputs.len()).sum();
        let mut message = tr!(
            "{n} fichier enregistré, {} au total, dans {}." | "{n} fichiers enregistrés, {} au total, dans {}." % count,
            format::bytes(total),
            dir
        );
        if let Some(reason) = reason {
            message = format!("{reason} {message}");
        }
        if self.config.borrow().settings.convert_to_mp4 {
            let files: Vec<PathBuf> =
                summaries.iter().flat_map(|s| s.outputs.iter().filter(|p| p.exists()).cloned()).collect();
            if !files.is_empty() {
                message.push(' ');
                message.push_str(&tr!("Conversion MP4 lancée."));
                self.enqueue_conversions(files);
            }
        }
        if errors.is_empty() {
            self.toast(&message, 1);
        } else {
            self.toast(&tr!("{} Problèmes : {}", message, errors.join(" · ")), 3);
        }
        self.flush_pending_repairs();
        if clean {
            let files: Vec<PathBuf> =
                summaries.iter().flat_map(|s| s.outputs.iter().filter(|p| p.exists()).cloned()).collect();
            self.check_unfinalized(files);
        } else {
            self.check_interrupted_session();
        }
        self.show_next_deferred();
    }

    /// Lancé par Windows à l'ouverture de session : la fenêtre est réduite
    /// dès qu'elle existe (Slint ne peut pas la créer réduite).
    fn minimize_at_startup(&self) {
        if !self.minimize_pending.get() {
            return;
        }
        #[cfg(windows)]
        if self.main_window().is_none() {
            return;
        }
        self.minimize_pending.set(false);
        if let Some(ui) = self.ui.upgrade() {
            ui.window().set_minimized(true);
        }
    }

    fn tick_status(&self) {
        self.update_preview_size();
        self.watch_session_end();
        self.minimize_at_startup();
        self.tick_schedule();
        self.tick_mic_test();
        let statuses: (Vec<SourceStatus>, Vec<PathBuf>) = {
            let mut session = self.session.borrow_mut();
            let Some(session) = session.as_mut() else {
                // Emprunt rendu avant : la mesure du disque relit la session.
                drop(session);
                self.refresh_disk(false);
                self.tick_meters(&[]);
                return;
            };
            let elapsed = format::clock(session.elapsed());
            self.with_bridge(|b| b.set_elapsed(elapsed.into()));
            if let Some(micro) = session.microphone_status() {
                self.show_microphone(&micro);
            }
            (session.status(), session.outputs())
        };
        let (statuses, outputs) = statuses;
        self.update_session_record(outputs);
        let joining = self.joining.borrow().clone();
        for status in &statuses {
            let Some(index) = (0..self.cards.row_count())
                .find(|&i| self.cards.row_data(i).is_some_and(|c| c.id.as_str() == status.id))
            else {
                continue;
            };
            if joining.contains(status.id.as_str()) {
                // Source arrêtée qui rejoint l'enregistrement : « Préparation… ».
                continue;
            }
            if status.state == SourceState::Finished {
                // Source arrêtée seule : la carte redevient un simple aperçu.
                self.update_card(index, |card| {
                    if card.recording || card.busy {
                        card.recording = false;
                        card.busy = false;
                        card.can_open = false;
                        card.can_restore = false;
                        card.state = tr!("Fichier terminé").into();
                        card.tone = 0;
                        card.warning = SharedString::new();
                    }
                });
                continue;
            }
            self.update_card(index, |card| {
                // Une source en échec peut être relancée par son bouton.
                card.recording = status.state != SourceState::Failed;
                card.busy = status.state == SourceState::Finishing;
                let (state, tone) = match status.state {
                    SourceState::Connected => (tr!("Enregistre"), 1),
                    SourceState::Minimized => (tr!("Réduite (image figée)"), 2),
                    SourceState::Searching => (tr!("En attente de la fenêtre"), 2),
                    SourceState::Lost => (tr!("Fenêtre perdue"), 2),
                    SourceState::Failed => (tr!("Erreur"), 3),
                    SourceState::Finishing => (tr!("Finalisation…"), 0),
                    SourceState::Finished => (tr!("Terminée"), 0),
                };
                card.state = state.into();
                card.tone = tone;
                card.can_restore = status.state == SourceState::Minimized;
                card.can_open = matches!(status.state, SourceState::Lost | SourceState::Searching);
                card.level = format::level(status.audio_level);
                card.sound = status.audio_recent;
                card.size = format::bytes(status.file_size).into();
                card.dropped = status.frames_dropped as i32;
                card.warning = if let Some(error) = &status.error {
                    error.as_str().into()
                } else if status.backlog_seconds >= BACKLOG_WARNING_SECONDS {
                    tr!("L'encodeur prend du retard ({} s)", format!("{:.0}", status.backlog_seconds)).into()
                } else if status.part > 1 {
                    tr!("Encodeur relancé : partie {} en cours", status.part).into()
                } else {
                    SharedString::new()
                };
            });
        }
        self.reopen_lost_windows(&statuses);
        self.refresh_disk(false);
        self.tick_meters(&statuses);
    }

    /// Rouvre le programme d'une source dont la fenêtre a disparu pendant
    /// l'enregistrement (fermée par erreur, plantée), si la source l'autorise :
    /// toujours pour un navigateur, sur option pour une application.
    fn reopen_lost_windows(&self, statuses: &[SourceStatus]) {
        let now = Instant::now();
        let mut due = Vec::new();
        {
            let mut lost = self.lost_windows.borrow_mut();
            for status in statuses {
                let entry = lost.entry(status.id.clone()).or_default();
                if status.state != SourceState::Lost {
                    entry.since = None;
                    entry.last_try = None;
                    entry.tries = 0;
                    continue;
                }
                let since = *entry.since.get_or_insert(now);
                let waited = match entry.last_try {
                    Some(last) => now.duration_since(last) >= AUTO_REOPEN_RETRY,
                    None => now.duration_since(since) >= AUTO_REOPEN_DELAY,
                };
                if waited && entry.tries < AUTO_REOPEN_TRIES && entry.session_tries < AUTO_REOPEN_SESSION_TRIES {
                    due.push(status.id.clone());
                }
            }
        }
        for id in due {
            let Some(source) = self.active_source(&id).filter(Source::should_launch) else {
                continue;
            };
            if let Some(entry) = self.lost_windows.borrow_mut().get_mut(&id) {
                entry.tries += 1;
                entry.session_tries += 1;
                entry.last_try = Some(now);
            }
            // Hors du fil de l'interface : la recherche des fenêtres interroge
            // chaque processus.
            let platform = self.platform.clone();
            thread::spawn(move || {
                let windows = platform.list_windows();
                if resolve::find_window(platform.as_ref(), &source, &windows).is_some() {
                    // La fenêtre existe (capture refusée, par exemple) : la
                    // relancer en ouvrirait une seconde.
                    return;
                }
                // Firefox qui se ferme ou redémarre encore, programme déjà en
                // train de s'ouvrir : on réessaiera plus tard.
                let result = resolve::launch_once(&source);
                post(move |app| match result {
                    Ok(false) => {}
                    Ok(true) => {
                        log::info!("[{}] fenêtre perdue : programme rouvert automatiquement", source.name);
                        app.toast(&tr!("La fenêtre de « {} » a disparu : réouverture automatique.", source.name), 2);
                    }
                    Err(e) => log::warn!("[{}] réouverture automatique impossible : {e:#}", source.name),
                });
            });
        }
    }

    // ----- Reprise après incident ----------------------------------------

    fn start_session_record(&self, record: SessionRecord) {
        if let Err(e) = record.save(&paths::session_record_file()) {
            log::warn!("journal de session : {e:#}");
        }
        *self.session_record.borrow_mut() = Some(record);
    }

    /// Tient à jour la liste des fichiers du journal de session (nouvelles
    /// parties, sources ajoutées).
    fn update_session_record(&self, files: Vec<PathBuf>) {
        let mut record = self.session_record.borrow_mut();
        let Some(record) = record.as_mut() else {
            return;
        };
        if record.files != files {
            record.files = files;
            if let Err(e) = record.save(&paths::session_record_file()) {
                log::warn!("journal de session : {e:#}");
            }
        }
    }

    /// Arrêt normal : plus rien à reprendre.
    fn clear_session_record(&self) {
        self.session_record.borrow_mut().take();
        SessionRecord::clear(&paths::session_record_file());
    }

    /// Au lancement : si le journal de session existe encore, le précédent
    /// enregistrement s'est mal terminé. Ses fichiers sont examinés hors du
    /// fil de l'interface, puis le bilan est affiché.
    fn check_interrupted_session(&self) {
        let record = match SessionRecord::load(&paths::session_record_file()) {
            Ok(None) => return,
            Ok(Some(record)) => Some(record),
            Err(e) => {
                log::warn!("{e:#}");
                None
            }
        };
        log::warn!(
            "enregistrement précédent interrompu ({})",
            record.as_ref().map_or("journal illisible".to_string(), |r| format!("{} {}", r.date, r.time))
        );
        let platform = self.platform.clone();
        thread::spawn(move || {
            let files = record.as_ref().map(|r| r.files.clone()).unwrap_or_default();
            let reports =
                catch_unwind(AssertUnwindSafe(|| inspect_interrupted(platform.as_ref(), &files))).unwrap_or_default();
            post(move |app| app.show_incident(record, reports));
        });
    }

    fn show_incident(&self, record: Option<SessionRecord>, reports: Vec<(PathBuf, FileReport)>) {
        self.deferred.borrow_mut().push(Deferred::Incident(record, reports));
        self.show_next_deferred();
    }

    /// Montre la prochaine fenêtre en attente, si l'interface est libre : un
    /// bilan n'interrompt ni un enregistrement ni une fenêtre ouverte (des
    /// réglages en cours de saisie, par exemple).
    fn show_next_deferred(&self) {
        let sheet_open = self.with_bridge_value(|b| !b.get_sheet().is_empty());
        if sheet_open || self.recording() || self.busy.get() || self.deferred.borrow().is_empty() {
            return;
        }
        // Emprunt rendu avant d'afficher : l'affichage peut relire la file.
        let next = self.deferred.borrow_mut().remove(0);
        match next {
            Deferred::Incident(record, reports) => self.present_incident(record, reports),
            Deferred::Repair(files) => self.ask(
                Confirm::RepairFiles(files.clone()),
                &tr!("Fichiers à réparer"),
                &tr!(
                    "{n} fichier n'a pas été fermé correctement (encodeur arrêté de force). Il se lit, mais sans durée ni index. Le réparer maintenant, sans perte de qualité ?"
                        | "{n} fichiers n'ont pas été fermés correctement (encodeur arrêté de force). Ils se lisent, mais sans durée ni index. Les réparer maintenant, sans perte de qualité ?"
                        % files.len()
                ),
                &tr!("Réparer"),
            ),
        }
    }

    /// Un enregistrement démarre alors que le bilan d'un enregistrement
    /// interrompu est affiché : ses réparations attendent la fin du nouvel
    /// enregistrement plutôt que d'être oubliées.
    fn keep_incident_repairs(&self) {
        if let Some(incident) = self.incident.borrow_mut().as_mut() {
            self.pending_repairs.borrow_mut().append(&mut incident.repair);
        }
    }

    fn flush_pending_repairs(&self) {
        let files = std::mem::take(&mut *self.pending_repairs.borrow_mut());
        if !files.is_empty() {
            self.enqueue_jobs(files, JobKind::Repair);
        }
    }

    /// Après un arrêt normal : un fichier dont l'encodeur a été arrêté de
    /// force (figé, trop lent à finir) n'a ni durée ni index. Hors du fil de
    /// l'interface, chaque fichier est examiné, et la réparation proposée.
    fn check_unfinalized(&self, files: Vec<PathBuf>) {
        if files.is_empty() {
            return;
        }
        let platform = self.platform.clone();
        thread::spawn(move || {
            let reports =
                catch_unwind(AssertUnwindSafe(|| inspect_interrupted(platform.as_ref(), &files))).unwrap_or_default();
            let cut: Vec<PathBuf> = reports
                .into_iter()
                .filter(|(_, report)| matches!(report, FileReport::NeedsRepair { .. }))
                .map(|(path, _)| path)
                .collect();
            if !cut.is_empty() {
                post(move |app| {
                    log::warn!("{} fichier(s) mal fermé(s) à l'arrêt : réparation proposée", cut.len());
                    app.deferred.borrow_mut().push(Deferred::Repair(cut));
                    app.show_next_deferred();
                });
            }
        });
    }

    fn present_incident(&self, record: Option<SessionRecord>, reports: Vec<(PathBuf, FileReport)>) {
        for (path, report) in &reports {
            log::info!("fichier de l'enregistrement interrompu : {} : {}", path.display(), report_text(report));
        }
        let scene_id = record
            .as_ref()
            .and_then(|r| self.config.borrow().scenes.iter().find(|s| s.id == r.scene_id).map(|s| s.id.clone()));
        let repair: Vec<PathBuf> = reports
            .iter()
            .filter(|(_, report)| matches!(report, FileReport::NeedsRepair { .. }))
            .map(|(path, _)| path.clone())
            .collect();
        let text = match &record {
            Some(r) => {
                let (date, time) = (display_date(&r.date), display_time(&r.time));
                let cause = if r.ended_by_windows {
                    tr!(
                        "Windows s'est éteint, a redémarré ou a fermé la session pendant l'enregistrement commencé le {} à {} (scène « {} »).",
                        date,
                        time,
                        r.scene_name
                    )
                } else {
                    tr!(
                        "Polyptyque s'est fermé pendant l'enregistrement commencé le {} à {} (scène « {} »).",
                        date,
                        time,
                        r.scene_name
                    )
                };
                let files = if reports.is_empty() {
                    tr!("Aucun fichier n'avait encore été écrit.")
                } else {
                    tr!("État des fichiers :")
                };
                format!("{cause} {files}")
            }
            None => tr!("Polyptyque s'est fermé pendant un enregistrement, mais son journal est illisible : vérifier les fichiers dans le dossier d'enregistrement."),
        };
        let files: Vec<IncidentFile> = reports
            .iter()
            .map(|(path, report)| IncidentFile {
                name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default().into(),
                detail: report_text(report).into(),
            })
            .collect();
        let can_resume = scene_id.is_some();
        let can_repair = !repair.is_empty();
        *self.incident.borrow_mut() = Some(Incident { scene_id, repair });
        self.incident_files.set_vec(files);
        self.with_bridge(|b| {
            b.set_incident_text(text.into());
            b.set_incident_can_repair(can_repair);
            b.set_incident_can_resume(can_resume);
            b.set_sheet("incident".into());
        });
    }

    /// Met à jour l'état d'un fichier dans le bilan, s'il est affiché.
    fn set_incident_detail(&self, path: &Path, detail: String) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        for i in 0..self.incident_files.row_count() {
            if let Some(mut row) = self.incident_files.row_data(i) {
                if row.name == name {
                    row.detail = detail.as_str().into();
                    self.incident_files.set_row_data(i, row);
                }
            }
        }
    }

    fn incident_repair(&self) {
        let files = self.incident.borrow_mut().as_mut().map(|i| std::mem::take(&mut i.repair)).unwrap_or_default();
        if files.is_empty() {
            return;
        }
        self.with_bridge(|b| b.set_incident_can_repair(false));
        self.toast(&tr!("Réparation de {n} fichier lancée." | "Réparation de {n} fichiers lancée." % files.len()), 1);
        self.enqueue_jobs(files, JobKind::Repair);
    }

    fn incident_resume(&self) {
        let scene_id = self.incident.borrow().as_ref().and_then(|i| i.scene_id.clone());
        self.keep_incident_repairs();
        self.close_sheet();
        if let Some(id) = scene_id {
            if self.config.borrow().active_scene != id {
                self.select_scene(&id);
            }
            self.start_recording(None);
        }
    }

    // ----- Mises à jour -----------------------------------------------------

    fn check_for_updates(&self) {
        let choice = update_choice(&self.config.borrow().settings);
        if choice == UPDATES_OFF || self.update_state.get() != UpdateState::None || self.update_checking.get() {
            return;
        }
        let beta = choice == 1;
        self.update_checking.set(true);
        thread::spawn(move || {
            let result = catch_unwind(|| update::check(beta)).unwrap_or_else(|_| Err(anyhow!("erreur interne")));
            post(move |app| app.on_update_checked(result));
        });
    }

    fn on_update_checked(&self, result: Result<Option<update::Available>>) {
        self.update_checking.set(false);
        match result {
            Ok(Some(available)) => {
                log::info!("nouvelle version disponible : {}", available.version);
                let version = available.version.clone();
                *self.update.borrow_mut() = Some(available);
                self.set_update_state(UpdateState::Available);
                self.toast(
                    &tr!("Polyptyque {} est disponible : un clic sur la pastille en haut l'installe.", version),
                    1,
                );
            }
            Ok(None) => log::info!("Polyptyque est à jour"),
            // Sans connexion, ou version non installée : rien à signaler.
            Err(e) => log::info!("recherche de mise à jour impossible : {e:#}"),
        }
    }

    fn set_update_state(&self, state: UpdateState) {
        self.update_state.set(state);
        let version = self.update.borrow().as_ref().map(|u| u.version.clone()).unwrap_or_default();
        let (key, text) = match state {
            UpdateState::None => ("", String::new()),
            UpdateState::Available => ("available", tr!("Version {} disponible", version)),
            UpdateState::Downloading => ("downloading", tr!("Téléchargement de la mise à jour…")),
            UpdateState::Ready => ("ready", tr!("Redémarrer pour passer à la version {}", version)),
        };
        self.with_bridge(|b| {
            b.set_update_state(key.into());
            b.set_update_text(text.into());
        });
    }

    /// Une mise à jour ne doit jamais interrompre un enregistrement ou une
    /// conversion. Renvoie le message à afficher sinon.
    fn update_blocked(&self) -> Option<String> {
        if self.recording() || self.busy.get() {
            Some(tr!("La mise à jour pourra être installée une fois l'enregistrement arrêté."))
        } else if self.with_bridge_value(|b| b.get_converting()) {
            Some(tr!("La mise à jour pourra être installée une fois la conversion MP4 terminée."))
        } else {
            None
        }
    }

    fn install_update(&self) {
        let Some(available) = self.update.borrow().clone() else {
            return;
        };
        match self.update_state.get() {
            UpdateState::Available => {
                if let Some(message) = self.update_blocked() {
                    self.toast(&message, 2);
                    return;
                }
                self.ask(
                    Confirm::InstallUpdate,
                    &tr!("Mettre à jour Polyptyque"),
                    &tr!(
                        "Télécharger la version {} ({}) puis redémarrer Polyptyque ? Scènes, réglages et connexions des navigateurs sont conservés.",
                        available.version,
                        format::bytes(available.size)
                    ),
                    &tr!("Mettre à jour"),
                );
            }
            UpdateState::Ready => self.apply_update(),
            UpdateState::Downloading | UpdateState::None => {}
        }
    }

    fn download_update(&self) {
        let Some(available) = self.update.borrow().clone() else {
            return;
        };
        self.set_update_state(UpdateState::Downloading);
        self.with_bridge(|b| b.set_update_progress(0.0));
        thread::spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| {
                update::download(&available, &|progress| {
                    post(move |app| {
                        let text = tr!("Téléchargement de la mise à jour · {} %", format!("{:.0}", progress * 100.0));
                        app.with_bridge(|b| {
                            b.set_update_progress(progress);
                            b.set_update_text(text.into());
                        });
                    })
                })
            }))
            .unwrap_or_else(|_| Err(anyhow!(tr!("erreur interne"))));
            post(move |app| app.on_update_downloaded(result));
        });
    }

    fn on_update_downloaded(&self, result: Result<()>) {
        match result {
            Ok(()) => {
                self.set_update_state(UpdateState::Ready);
                match self.update_blocked() {
                    None => self.apply_update(),
                    Some(_) => self.toast(
                        &tr!("Mise à jour téléchargée : un clic sur la pastille l'installe une fois l'enregistrement terminé ; sinon, elle s'installera au prochain démarrage."),
                        1,
                    ),
                }
            }
            Err(e) => {
                log::warn!("téléchargement de la mise à jour : {e:#}");
                self.set_update_state(UpdateState::Available);
                self.toast(&tr!("Téléchargement de la mise à jour impossible (détail dans le journal)."), 3);
            }
        }
    }

    /// Ferme Polyptyque, installe la nouvelle version et la relance.
    fn apply_update(&self) {
        if let Some(message) = self.update_blocked() {
            self.toast(&message, 2);
            return;
        }
        let Some(available) = self.update.borrow().clone() else {
            return;
        };
        log::info!("installation de la version {}", available.version);
        self.previews.borrow_mut().stop();
        if let Err(e) = update::apply(&available) {
            log::warn!("installation de la mise à jour : {e:#}");
            self.toast(&tr!("Installation de la mise à jour impossible (détail dans le journal)."), 3);
        }
    }

    // ----- Micro -----------------------------------------------------------

    fn show_microphone(&self, status: &SourceStatus) {
        let (text, tone) = match status.state {
            SourceState::Connected => (tr!("Micro"), 1),
            SourceState::Searching => (tr!("Micro : ouverture…"), 0),
            SourceState::Lost => (tr!("Micro indisponible"), 3),
            SourceState::Failed => (tr!("Micro : erreur"), 3),
            SourceState::Minimized | SourceState::Finishing | SourceState::Finished => (tr!("Micro"), 0),
        };
        let level = format::level(status.audio_level);
        self.with_bridge(|b| {
            b.set_mic_text(text.into());
            b.set_mic_tone(tone);
            b.set_mic_level(level);
            b.set_mic_sound(status.audio_recent);
        });
        // L'explication (accès refusé, micro débranché…) n'est montrée qu'une fois.
        if tone == 3 && !self.mic_warned.get() {
            if let Some(error) = &status.error {
                self.mic_warned.set(true);
                self.toast(&tr!("Micro : {}", error), 3);
            }
        }
    }

    fn open_mic_privacy(&self) {
        if let Err(e) = Command::new("explorer").arg("ms-settings:privacy-microphone").spawn() {
            self.toast(&tr!("Impossible d'ouvrir les paramètres de Windows : {}", e), 3);
        }
    }

    // ----- Conversion MP4 --------------------------------------------------

    fn convert_files(&self) {
        let dir = self.config.borrow().settings.output_dir.clone();
        let mut files = self.pick_mkv_files(&dir);
        // Un fichier encore en cours d'écriture donnerait une copie tronquée.
        let in_progress = self.session.borrow().as_ref().map(|s| s.files_in_progress()).unwrap_or_default();
        let before = files.len();
        files.retain(|f| !in_progress.iter().any(|p| same_file(p, f)));
        let skipped = before - files.len();
        let count = files.len();
        if count > 0 {
            self.enqueue_conversions(files);
        }
        match (count, skipped) {
            (0, 0) => {}
            (0, _) => self.toast(
                &tr!("Ces fichiers sont en cours d'enregistrement : ils pourront être convertis une fois l'enregistrement arrêté."),
                2,
            ),
            (_, 0) => self.toast(&added_to_conversion(count), 1),
            (_, _) => self.toast(
                &format!(
                    "{} {}",
                    added_to_conversion(count),
                    tr!("{n} fichier en cours d'enregistrement a été ignoré."
                        | "{n} fichiers en cours d'enregistrement ont été ignorés." % skipped)
                ),
                2,
            ),
        }
    }

    #[cfg(windows)]
    fn pick_mkv_files(&self, dir: &std::path::Path) -> Vec<PathBuf> {
        let Some(ui) = self.ui.upgrade() else {
            return Vec::new();
        };
        let mut dialog = rfd::FileDialog::new()
            .set_title(tr!("Fichiers à convertir en MP4"))
            .add_filter(tr!("Enregistrements Polyptyque (MKV, MKA)"), &["mkv", "mka"])
            .set_parent(&ui.window().window_handle());
        if dir.exists() {
            dialog = dialog.set_directory(dir);
        }
        dialog.pick_files().unwrap_or_default()
    }

    /// Hors Windows (mode démonstration) : tous les MKV du dossier qui
    /// n'ont pas encore leur MP4.
    #[cfg(not(windows))]
    fn pick_mkv_files(&self, dir: &std::path::Path) -> Vec<PathBuf> {
        let files: Vec<PathBuf> = fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| remux::is_convertible(p) && !p.with_extension("mp4").exists())
                    .collect()
            })
            .unwrap_or_default();
        if files.is_empty() {
            self.toast(&tr!("Aucun fichier MKV à convertir dans le dossier des enregistrements."), 2);
        }
        files
    }

    fn enqueue_conversions(&self, files: Vec<PathBuf>) {
        self.enqueue_jobs(files, JobKind::Convert);
    }

    fn enqueue_jobs(&self, files: Vec<PathBuf>, kind: JobKind) {
        let converter = self.converter.borrow();
        let Some(converter) = converter.as_ref() else {
            self.toast(&tr!("La conversion n'est pas disponible (voir le journal)."), 3);
            return;
        };
        let mut progress = self.conversion.borrow_mut();
        for file in files {
            progress.total += 1;
            if kind == JobKind::Repair {
                progress.repairs += 1;
            }
            converter.enqueue(file, kind);
        }
        let text = conversion_text(&progress, 0.0);
        drop(progress);
        self.with_bridge(|b| {
            b.set_converting(true);
            b.set_conversion_text(text.into());
        });
    }

    fn on_conversion(&self, event: ConversionEvent) {
        match event {
            ConversionEvent::Started { input, kind } => {
                log::info!("traitement de {}", input.display());
                self.conversion.borrow_mut().repairing = kind == JobKind::Repair;
                if kind == JobKind::Repair {
                    self.set_incident_detail(&input, tr!("Réparation en cours…"));
                }
                let text = conversion_text(&self.conversion.borrow(), 0.0);
                self.with_bridge(|b| {
                    b.set_converting(true);
                    b.set_conversion_text(text.into());
                    b.set_conversion_progress(0.0);
                });
            }
            ConversionEvent::Progress(value) => {
                let text = conversion_text(&self.conversion.borrow(), value);
                self.with_bridge(|b| {
                    b.set_conversion_text(text.into());
                    b.set_conversion_progress(value);
                });
            }
            ConversionEvent::Finished { input, result } => {
                if self.conversion.borrow().repairing {
                    let detail = match &result {
                        Ok(_) => tr!("Réparé : prêt pour le montage"),
                        Err(e) => tr!("Échec de la réparation : {}", e),
                    };
                    self.set_incident_detail(&input, detail);
                }
                let mut progress = self.conversion.borrow_mut();
                progress.finished += 1;
                match result {
                    Ok(_) => progress.succeeded += 1,
                    Err(_) if progress.cancelled => {}
                    Err(e) => {
                        let name = input.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        progress.failures.push(tr!("{} : {}", name, e));
                    }
                }
            }
            ConversionEvent::Idle => {
                let progress = std::mem::take(&mut *self.conversion.borrow_mut());
                self.with_bridge(|b| {
                    b.set_converting(false);
                    b.set_conversion_progress(0.0);
                });
                if matches!(*self.confirm.borrow(), Some(Confirm::CancelConversion | Confirm::QuitWhileConverting)) {
                    self.close_sheet();
                }
                let what = if progress.repairs == progress.total {
                    tr!("Réparation")
                } else if progress.repairs == 0 {
                    tr!("Conversion MP4")
                } else {
                    tr!("Conversion et réparation")
                };
                let done = progress.succeeded;
                if progress.cancelled {
                    self.toast(
                        &tr!(
                            "{} : annulée, {n} fichier traité avant l'arrêt."
                                | "{} : annulée, {n} fichiers traités avant l'arrêt." % done,
                            what
                        ),
                        2,
                    );
                } else if progress.failures.is_empty() {
                    self.toast(
                        &tr!(
                            "{} : terminée, {n} fichier prêt pour le montage."
                                | "{} : terminée, {n} fichiers prêts pour le montage." % done,
                            what
                        ),
                        1,
                    );
                } else {
                    let failed = progress.failures.len();
                    self.toast(
                        &tr!(
                            "{} : {}, {}. {}",
                            what,
                            tr!("{n} réussite" | "{n} réussites" % done),
                            tr!("{n} échec" | "{n} échecs" % failed),
                            progress.failures.join(" · ")
                        ),
                        3,
                    );
                }
            }
        }
    }

    fn request_cancel_conversion(&self) {
        let remaining = {
            let progress = self.conversion.borrow();
            progress.total.saturating_sub(progress.finished)
        };
        self.ask(
            Confirm::CancelConversion,
            &tr!("Annuler la conversion"),
            &tr!(
                "Arrêter la conversion MP4 ({n} fichier restant) ? Le fichier en cours de conversion est effacé ; les MKV d'origine ne sont pas touchés."
                    | "Arrêter la conversion MP4 ({n} fichiers restants) ? Le fichier en cours de conversion est effacé ; les MKV d'origine ne sont pas touchés."
                    % remaining
            ),
            &tr!("Arrêter la conversion"),
        );
    }

    // ----- Scènes ----------------------------------------------------------

    fn select_scene(&self, id: &str) {
        if self.locked() {
            return;
        }
        self.config.borrow_mut().active_scene = id.to_string();
        self.save();
        self.with_bridge(|b| b.set_selected_source(-1));
        self.refresh_all();
        self.restart_previews();
    }

    fn open_scene_sheet(&self, new: bool) {
        let current = self.config.borrow().active_scene().name.clone();
        self.with_bridge(|b| {
            b.set_scene_name_input(if new { SharedString::new() } else { current.into() });
            b.set_sheet(if new { "scene-new" } else { "scene-rename" }.into());
        });
    }

    fn open_scene_sheet_duplicate(&self) {
        let name = tr!("{} (copie)", self.config.borrow().active_scene().name);
        self.with_bridge(|b| {
            b.set_scene_name_input(name.into());
            b.set_sheet("scene-duplicate".into());
        });
    }

    fn confirm_scene_name(&self) {
        if self.locked() {
            return;
        }
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let bridge = ui.global::<Bridge>();
        let name = bridge.get_scene_name_input().trim().to_string();
        if name.is_empty() {
            self.toast(&tr!("Nom de scène manquant."), 2);
            return;
        }
        let mode = bridge.get_sheet();
        {
            let mut config = self.config.borrow_mut();
            match mode.as_str() {
                "scene-new" => {
                    let scene = Scene::new(&name);
                    config.active_scene = scene.id.clone();
                    config.scenes.push(scene);
                }
                "scene-duplicate" => {
                    let id = config.active_scene.clone();
                    config.duplicate_scene(&id, &name);
                }
                _ => config.active_scene_mut().name = name,
            }
        }
        bridge.set_sheet("".into());
        self.save();
        let changed_scene = mode != "scene-rename";
        if changed_scene {
            bridge.set_selected_source(-1);
        }
        self.refresh_all();
        if changed_scene {
            self.restart_previews();
        }
    }

    fn request_delete_scene(&self) {
        let (id, name) = {
            let config = self.config.borrow();
            (config.active_scene.clone(), config.active_scene().name.clone())
        };
        // Les sources partagées avec d'autres scènes y restent.
        let (total, only_here) = {
            let config = self.config.borrow();
            (config.scene(&id).map_or(0, |s| s.source_ids.len()), config.sources_only_in(&id).len())
        };
        let mut text = vec![if total == 0 {
            tr!("La scène « {} » sera supprimée.", name)
        } else if only_here == 0 {
            tr!("La scène « {} » sera supprimée. Ses sources restent dans les autres scènes.", name)
        } else if only_here == total {
            tr!("La scène « {} » et ses sources seront supprimées.", name)
        } else {
            tr!(
                "La scène « {} » sera supprimée, ainsi que la source qui n'est dans aucune autre scène."
                    | "La scène « {} » sera supprimée, ainsi que les {n} sources qui ne sont dans aucune autre scène."
                        % only_here,
                name
            )
        }];
        let schedules = self.scene_schedule_count(&id);
        if schedules > 0 {
            text.push(tr!("Sa programmation aussi." | "Ses {n} programmations aussi." % schedules));
        }
        text.push(tr!("Les profils de navigateur restent sur le disque."));
        self.ask(
            Confirm::DeleteScene,
            &tr!("Supprimer la scène"),
            &text.join(" "),
            &tr!("Bouton : supprimer une scène" => "Supprimer"),
        );
    }

    // ----- Sources ---------------------------------------------------------

    fn open_browser_sheet(&self) {
        let browsers = self.platform.detect_browsers();
        let mut choices: Vec<SharedString> = browsers.iter().map(|b| b.name.as_str().into()).collect();
        choices.push(tr!("Autre navigateur…").into());
        let count = self.config.borrow().active_scene().source_ids.len() + 1;
        let default_name = browsers
            .first()
            .map(|b| format!("{} {count}", b.name.split_whitespace().last().unwrap_or(&b.name)))
            .unwrap_or_else(|| tr!("Navigateur {}", count));
        *self.browsers.borrow_mut() = browsers;
        self.with_bridge(|b| {
            b.set_browser_choices(ModelRc::new(VecModel::from(choices)));
            b.set_browser_index(0);
            b.set_browser_path("".into());
            b.set_new_name(default_name.into());
            b.set_new_url("".into());
            b.set_open_now(true);
            b.set_sheet("browser".into());
        });
    }

    fn confirm_add_browser(&self) {
        if self.locked() {
            return;
        }
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let bridge = ui.global::<Bridge>();
        let index = bridge.get_browser_index().max(0) as usize;
        let browser = {
            let browsers = self.browsers.borrow();
            match browsers.get(index) {
                Some(b) => BrowserRef { name: b.name.clone(), exe_path: b.exe_path.clone(), family: b.family },
                None => {
                    let path = PathBuf::from(bridge.get_browser_path().trim().trim_matches('"'));
                    if !path.exists() {
                        self.toast(&tr!("Ce fichier de navigateur n'existe pas."), 3);
                        return;
                    }
                    if is_tor_browser(&path) {
                        self.toast(
                            &tr!("Tor Browser s'ajoute comme source application (il efface tout à la fermeture)."),
                            2,
                        );
                        return;
                    }
                    let family = classify_executable(&path).unwrap_or(BrowserFamily::Chromium);
                    let name = program_name(&path).map(|n| format::capitalize(&n)).unwrap_or_else(|| tr!("Navigateur"));
                    BrowserRef { name, exe_path: path, family }
                }
            }
        };
        if bridge.get_new_name().trim().is_empty() {
            self.toast(&tr!("Nom de source manquant."), 2);
            return;
        }
        let url = bridge.get_new_url().to_string();
        let source = Source::browser(
            &bridge.get_new_name(),
            browser,
            Some(url.as_str()).filter(|u| !u.trim().is_empty()),
            &self.profiles_root,
        );
        let open_now = bridge.get_open_now();
        let name = source.name.clone();
        self.config.borrow_mut().add_source(source.clone());
        bridge.set_sheet("".into());
        self.save();
        self.refresh_all();
        self.restart_previews();
        if open_now {
            match resolve::launch(&source) {
                Ok(()) => self.toast(
                    &tr!(
                        "« {} » ajoutée : les connexions et les onglets préparés dans ce navigateur seront conservés.",
                        name
                    ),
                    1,
                ),
                Err(e) => self
                    .toast(&tr!("« {} » ajoutée, mais le navigateur ne s'ouvre pas : {}", name, format!("{e:#}")), 3),
            }
        } else {
            self.toast(&tr!("« {} » ajoutée.", name), 1);
        }
    }

    fn open_application_sheet(&self) {
        self.refresh_windows();
        self.with_bridge(|b| {
            b.set_window_index(-1);
            b.set_new_name("".into());
            b.set_relaunch(false);
            b.set_capture_cursor(true);
            b.set_sheet("application".into());
        });
    }

    fn refresh_windows(&self) {
        let windows = self.platform.list_windows();
        let items: Vec<WindowItem> = windows
            .iter()
            .map(|w| WindowItem {
                title: w.title.as_str().into(),
                program: program_name(&w.exe).unwrap_or_default().into(),
                browser: classify_executable(&w.exe).is_some(),
            })
            .collect();
        *self.windows.borrow_mut() = windows;
        self.with_bridge(|b| {
            b.set_windows(ModelRc::new(VecModel::from(items)));
            b.set_window_index(-1);
        });
    }

    fn window_selected(&self, index: i32) {
        let name = self
            .windows
            .borrow()
            .get(index.max(0) as usize)
            .map(|w| program_name(&w.exe).map(|n| format::capitalize(&n)).unwrap_or_else(|| w.title.clone()));
        if let Some(name) = name {
            self.with_bridge(|b| b.set_new_name(name.into()));
        }
    }

    fn confirm_add_application(&self) {
        if self.locked() {
            return;
        }
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let bridge = ui.global::<Bridge>();
        let index = bridge.get_window_index();
        let Some(window) = self.windows.borrow().get(index.max(0) as usize).cloned().filter(|_| index >= 0) else {
            return;
        };
        if bridge.get_new_name().trim().is_empty() {
            self.toast(&tr!("Nom de source manquant."), 2);
            return;
        }
        let source =
            Source::application(&bridge.get_new_name(), &window, bridge.get_relaunch(), bridge.get_capture_cursor());
        let name = source.name.clone();
        self.config.borrow_mut().add_source(source);
        bridge.set_sheet("".into());
        self.save();
        self.refresh_all();
        self.restart_previews();
        self.toast(&tr!("« {} » ajoutée.", name), 1);
    }

    /// Source de la scène active.
    fn active_source(&self, id: &str) -> Option<Source> {
        let config = self.config.borrow();
        config.active_scene().contains(id).then(|| config.source(id).cloned()).flatten()
    }

    fn selected_source(&self) -> Option<Source> {
        let index = self.ui.upgrade()?.global::<Bridge>().get_selected_source();
        let config = self.config.borrow();
        let id = config.active_scene().source_ids.get(usize::try_from(index).ok()?)?;
        config.source(id).cloned()
    }

    fn request_remove_source(&self) {
        let Some(source) = self.selected_source() else {
            return;
        };
        let others: Vec<String> = {
            let config = self.config.borrow();
            config
                .scenes_with(&source.id)
                .into_iter()
                .filter(|s| s.id != config.active_scene)
                .map(|s| tr!("« {} »", s.name))
                .collect()
        };
        let text = if others.is_empty() {
            let removed = tr!("La source « {} » sera supprimée.", source.name);
            if source.is_browser() {
                format!("{removed} {}", tr!("Son profil de navigateur reste sur le disque."))
            } else {
                removed
            }
        } else {
            tr!(
                "La source « {} » sera retirée de cette scène seulement : elle reste dans {}.",
                source.name,
                format::join_list(&others)
            )
        };
        self.ask(
            Confirm::RemoveSource(source.id.clone()),
            &tr!("Supprimer la source"),
            &text,
            &tr!("Bouton : retirer une source" => "Supprimer"),
        );
    }

    /// Mémorise la source sélectionnée, pour la coller dans une autre scène.
    /// Possible pendant un enregistrement : rien ne change.
    fn copy_source(&self) {
        let Some(source) = self.selected_source() else {
            return;
        };
        *self.copied_source.borrow_mut() = Some(source.id.clone());
        let can_paste = self.can_paste();
        self.with_bridge(|b| b.set_can_paste(can_paste));
        self.toast(&tr!("« {} » copiée, prête à être collée dans une autre scène (Ctrl+V).", source.name), 1);
    }

    /// La source copiée existe encore et n'est pas déjà dans la scène active
    /// (le collage reste refusé avec une explication au clavier).
    fn can_paste(&self) -> bool {
        let config = self.config.borrow();
        self.copied_source
            .borrow()
            .as_deref()
            .is_some_and(|id| config.source(id).is_some() && !config.active_scene().contains(id))
    }

    /// Ajoute la source copiée à la scène active : la même source (même
    /// fenêtre, même profil de navigateur), pas une copie.
    fn paste_source(&self) {
        if self.recording() {
            self.toast(&tr!("Impossible de coller une source pendant un enregistrement."), 2);
            return;
        }
        if self.busy.get() {
            return;
        }
        let Some(id) = self.copied_source.borrow().clone() else {
            self.toast(&tr!("Copier d'abord une source (Ctrl+C), puis la coller ici (Ctrl+V)."), 2);
            return;
        };
        let result = self.config.borrow_mut().paste_source(&id);
        let (name, scene) = {
            let config = self.config.borrow();
            (config.source(&id).map(|s| s.name.clone()).unwrap_or_default(), config.active_scene().name.clone())
        };
        match result {
            Ok(()) => {
                let index = self.config.borrow().active_scene().source_ids.len() as i32 - 1;
                self.save();
                self.refresh_all();
                self.with_bridge(|b| b.set_selected_source(index));
                self.restart_previews();
                self.toast(&tr!("« {} » collée dans « {} » : c'est la même source, partagée.", name, scene), 1);
            }
            Err(PasteError::AlreadyInScene) => {
                self.toast(&tr!("« {} » est déjà dans cette scène : une même source n'y figure qu'une fois.", name), 2)
            }
            Err(PasteError::Missing) => {
                self.copied_source.borrow_mut().take();
                self.refresh_all();
                self.toast(&tr!("La source copiée a été supprimée entre-temps."), 2);
            }
        }
    }

    /// Flèches : la source sélectionnée monte ou descend d'un rang.
    fn move_source(&self, delta: i32) {
        let index = self.with_bridge_value(|b| b.get_selected_source());
        self.move_source_to(index, index + delta);
    }

    /// Glisser-déposer (liste ou grille des aperçus) et flèches : la source
    /// passe de la position `from` à la position `to`, puis est
    /// sélectionnée.
    fn move_source_to(&self, from: i32, to: i32) {
        if self.locked() {
            return;
        }
        let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
            return;
        };
        if !self.config.borrow_mut().move_source(from, to) {
            return;
        }
        self.with_bridge(|b| b.set_selected_source(to as i32));
        self.save();
        self.refresh_all();
        self.restart_previews();
    }

    fn open_source(&self, id: &str) {
        let Some(source) = self.active_source(id) else {
            return;
        };
        if let SourceKind::Application { exe_path, .. } = &source.kind {
            if !exe_path.exists() {
                self.toast(
                    &tr!("Programme introuvable ({}) : la fenêtre est à ouvrir à la main.", exe_path.display()),
                    2,
                );
                return;
            }
        }
        match resolve::launch_once(&source) {
            Ok(true) => self.toast(&tr!("Ouverture de « {} »…", source.name), 1),
            Ok(false) => self.toast(
                &tr!(
                    "« {} » est déjà en train de s'ouvrir (ou de se fermer) : patienter quelques secondes.",
                    source.name
                ),
                2,
            ),
            Err(e) => self.toast(&tr!("Impossible d'ouvrir « {} » : {}", source.name, format!("{e:#}")), 3),
        }
    }

    /// Ouvre les fenêtres manquantes de la scène active : navigateurs, et
    /// applications dont la relance est activée. `auto` : au lancement de
    /// Polyptyque, sans message si tout est déjà ouvert. Hors du fil de
    /// l'interface : on attend que les fenêtres apparaissent.
    fn open_scene_windows(&self, auto: bool) {
        if self.opening_windows.get() {
            if !auto {
                self.toast(&tr!("Les fenêtres de la scène sont déjà en train de s'ouvrir."), 1);
            }
            return;
        }
        let sources: Vec<Source> =
            self.config.borrow().active_sources().into_iter().filter(Source::should_launch).collect();
        if sources.is_empty() {
            if !auto {
                self.toast(
                    &tr!("Aucune fenêtre à ouvrir : seuls les navigateurs, et les applications avec « Relancer », s'ouvrent depuis Polyptyque."),
                    2,
                );
            }
            return;
        }
        self.opening_windows.set(true);
        let platform = self.platform.clone();
        thread::spawn(move || {
            let mut opening = Vec::new();
            let mut failed = Vec::new();
            let mut missing = catch_unwind(AssertUnwindSafe(|| {
                resolve::prepare_windows(platform.as_ref(), &sources, LAUNCH_WAIT, |event| match event {
                    PrepareEvent::Launching(source) => {
                        if opening.is_empty() {
                            post(|app| app.toast(&tr!("Ouverture des fenêtres de la scène…"), 1));
                        }
                        opening.push(source.name.clone());
                    }
                    PrepareEvent::LaunchFailed(source, error) => {
                        log::warn!("[{}] ouverture impossible : {error}", source.name);
                        failed.push(source.name.clone());
                    }
                    _ => {}
                })
            }))
            .unwrap_or_default();
            missing.retain(|name| !failed.contains(name));
            post(move |app| app.finish_open_scene_windows(auto, opening, failed, missing));
        });
    }

    fn finish_open_scene_windows(&self, auto: bool, opening: Vec<String>, failed: Vec<String>, missing: Vec<String>) {
        self.opening_windows.set(false);
        if !failed.is_empty() {
            self.toast(
                &tr!("Impossible d'ouvrir : {} (programme introuvable ? voir le journal).", failed.join(", ")),
                3,
            );
        } else if !missing.is_empty() {
            self.toast(
                &tr!(
                    "Fenêtre toujours absente après 30 s : {}."
                        | "Fenêtres toujours absentes après 30 s : {}." % missing.len(),
                    missing.join(", ")
                ),
                2,
            );
        } else if !opening.is_empty() {
            log::info!("fenêtres de la scène ouvertes : {}", opening.join(", "));
            self.toast(&tr!("{n} fenêtre ouverte." | "{n} fenêtres ouvertes." % opening.len()), 1);
        } else if !auto {
            self.toast(&tr!("Toutes les fenêtres de la scène sont déjà ouvertes."), 1);
        }
    }

    /// Fait revenir la fenêtre réduite d'une source, et la replace sur un
    /// écran branché si besoin. Hors du fil de l'interface : la fenêtre d'un
    /// programme figé pourrait faire attendre.
    fn restore_window(&self, id: &str) {
        let Some(source) = self.active_source(id) else {
            return;
        };
        // Pendant l'enregistrement : la fenêtre suivie par le moteur. Sinon,
        // parmi celles qui correspondent, une fenêtre réduite d'abord.
        let tracked = self.session.borrow().as_ref().and_then(|s| s.tracked_window(&source.id));
        let platform = self.platform.clone();
        thread::spawn(move || {
            let handle = tracked.or_else(|| {
                let windows = platform.list_windows();
                let minimized: Vec<WindowInfo> =
                    windows.iter().filter(|w| platform.is_window_minimized(w.handle)).cloned().collect();
                resolve::find_window(platform.as_ref(), &source, &minimized)
                    .or_else(|| resolve::find_window(platform.as_ref(), &source, &windows))
                    .map(|w| w.handle)
            });
            let result = match handle {
                Some(handle) => platform.restore_window(handle),
                None => Err(anyhow!(tr!("fenêtre introuvable"))),
            };
            post(move |app| match result {
                Ok(()) => {
                    log::info!("[{}] fenêtre rétablie à la demande", source.name);
                    app.toast(&tr!("Fenêtre de « {} » rétablie.", source.name), 1);
                }
                Err(e) => app.toast(&tr!("Impossible de rétablir « {} » : {}", source.name, format!("{e:#}")), 3),
            });
        });
    }

    // ----- Réglages --------------------------------------------------------

    fn open_settings(&self) {
        let config = self.config.borrow();
        let s = &config.settings;
        let mut resolutions: Vec<SharedString> = RESOLUTIONS.iter().map(|(w, h)| format!("{w} × {h}").into()).collect();
        let resolution_index = match RESOLUTIONS.iter().position(|&r| r == (s.video.width, s.video.height)) {
            Some(i) => i,
            None => {
                resolutions.push(tr!("{} × {} (personnalisée)", s.video.width, s.video.height).into());
                resolutions.len() - 1
            }
        };
        let mut fps: Vec<SharedString> = FRAME_RATES.iter().map(|f| tr!("{} i/s", f).into()).collect();
        let fps_index = match FRAME_RATES.iter().position(|&f| f == s.video.fps) {
            Some(i) => i,
            None => {
                fps.push(tr!("{} i/s (personnalisé)", s.video.fps).into());
                fps.len() - 1
            }
        };
        // Liste commune aux trois raccourcis, valeurs personnalisées comprises.
        let mut hotkey_list: Vec<String> = std::iter::once(tr!("Aucun")).chain(hotkeys::choices()).collect();
        for custom in [&s.hotkeys.toggle_recording, &s.hotkeys.show_window, &s.hotkeys.toggle_previews] {
            if !custom.is_empty() && !hotkey_list.iter().any(|c| hotkeys::same(c, custom)) {
                hotkey_list.push(custom.clone());
            }
        }
        let hotkey_index = |value: &str| {
            if value.is_empty() {
                return 0;
            }
            hotkey_list.iter().position(|c| hotkeys::same(c, value)).map_or(0, |i| i as i32)
        };
        let mut audio_choices: Vec<SharedString> =
            AUDIO_BITRATES.iter().map(|a| SharedString::from(tr!("{} kb/s", a))).collect();
        let audio_index = match AUDIO_BITRATES.iter().position(|&a| a == s.audio.bitrate_kbps) {
            Some(i) => i,
            None => {
                audio_choices.push(tr!("{} kb/s (personnalisé)", s.audio.bitrate_kbps).into());
                audio_choices.len() - 1
            }
        };
        let values = (
            resolutions,
            resolution_index as i32,
            fps,
            fps_index as i32,
            s.video.bitrate_kbps.to_string(),
            match s.video.codec {
                Codec::H264 => 0,
                Codec::Hevc => 1,
            },
            HARDWARE.iter().position(|&h| h == s.video.hardware).unwrap_or(0) as i32,
            audio_index as i32,
            s.output_dir.display().to_string(),
            s.filename_template.clone(),
            s.previews,
            s.convert_to_mp4,
            update_choice(s),
            hotkey_index(&s.hotkeys.toggle_recording),
            hotkey_index(&s.hotkeys.show_window),
            hotkey_index(&s.hotkeys.toggle_previews),
        );
        let hotkey_choices: Vec<SharedString> = hotkey_list.into_iter().map(Into::into).collect();
        let language_index = match s.language {
            LanguageChoice::Auto => 0,
            LanguageChoice::French => 1,
            LanguageChoice::English => 2,
        };
        let clock_index = match s.clock {
            ClockChoice::Auto => 0,
            ClockChoice::H24 => 1,
            ClockChoice::H12 => 2,
        };
        // Micros : celui de Windows par défaut, puis ceux qui sont branchés ;
        // un micro choisi mais débranché reste proposé.
        let mut mics: Vec<(String, String)> = vec![(String::new(), tr!("Micro par défaut de Windows"))];
        mics.extend(self.platform.list_microphones().into_iter().map(|d| (d.id, d.name)));
        let mic = &s.microphone;
        let mic_index = match mics.iter().position(|(id, _)| *id == mic.device_id) {
            Some(index) => index,
            None => {
                mics.push((mic.device_id.clone(), tr!("{} (débranché)", mic.device_name)));
                mics.len() - 1
            }
        };
        let mic_enabled = mic.enabled;
        let s_open_windows = s.open_windows_at_launch;
        let s_close_browsers = s.close_browsers_at_exit;
        let s_startup = s.launch_at_startup;
        let mic_labels: Vec<SharedString> = mics.iter().map(|(_, name)| name.as_str().into()).collect();
        *self.mic_choices.borrow_mut() = mics;
        drop(config);
        self.with_bridge(|b| {
            b.set_resolution_choices(ModelRc::new(VecModel::from(values.0)));
            b.set_resolution_index(values.1);
            b.set_fps_choices(ModelRc::new(VecModel::from(values.2)));
            b.set_fps_index(values.3);
            b.set_bitrate_text(values.4.into());
            b.set_codec_choices(ModelRc::new(VecModel::from(vec![
                SharedString::from(tr!("H.264 : compatibilité maximale (recommandé)")),
                SharedString::from(tr!("HEVC : fichiers plus légers")),
            ])));
            b.set_codec_index(values.5);
            b.set_hardware_choices(ModelRc::new(VecModel::from(
                HARDWARE.iter().map(|h| SharedString::from(h.label())).collect::<Vec<_>>(),
            )));
            b.set_hardware_index(values.6);
            b.set_audio_choices(ModelRc::new(VecModel::from(audio_choices)));
            b.set_audio_index(values.7);
            b.set_output_dir(values.8.into());
            b.set_template(values.9.into());
            b.set_settings_previews(values.10);
            b.set_settings_convert(values.11);
            b.set_settings_update_index(values.12);
            b.set_settings_language_index(language_index);
            b.set_settings_clock_index(clock_index);
            b.set_settings_open_windows(s_open_windows);
            b.set_settings_close_browsers(s_close_browsers);
            b.set_settings_startup(s_startup);
            b.set_settings_mic(mic_enabled);
            b.set_mic_choices(ModelRc::new(VecModel::from(mic_labels)));
            b.set_mic_index(mic_index as i32);
            b.set_hotkey_choices(ModelRc::new(VecModel::from(hotkey_choices)));
            b.set_hotkey_record_index(values.13);
            b.set_hotkey_show_index(values.14);
            b.set_hotkey_previews_index(values.15);
            b.set_settings_error("".into());
            b.set_sheet("settings".into());
        });
    }

    fn browse_output(&self) {
        #[cfg(windows)]
        {
            let Some(ui) = self.ui.upgrade() else {
                return;
            };
            let current = Some(ui.global::<Bridge>().get_output_dir().to_string());
            let mut dialog = rfd::FileDialog::new()
                .set_title(tr!("Dossier des enregistrements"))
                .set_parent(&ui.window().window_handle());
            if let Some(dir) = current.filter(|d| std::path::Path::new(d).exists()) {
                dialog = dialog.set_directory(dir);
            }
            if let Some(folder) = dialog.pick_folder() {
                self.with_bridge(|b| b.set_output_dir(folder.display().to_string().into()));
            }
        }
    }

    fn save_settings(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let b = ui.global::<Bridge>();
        // Séparateurs de milliers acceptés : espaces, et virgule en anglais
        // (« 12,000 » ; en français, la virgule n'a pas ce sens).
        let mut separators = vec![' ', '\u{a0}', '\u{202f}'];
        if crate::i18n::Language::current() == crate::i18n::Language::English {
            separators.push(',');
        }
        let bitrate: u32 = match b.get_bitrate_text().trim().replace(separators.as_slice(), "").parse() {
            Ok(value) if (250..=500_000).contains(&value) => value,
            _ => {
                b.set_settings_error(
                    tr!("Débit vidéo invalide : un nombre entre 250 et 500 000 kb/s est attendu.").into(),
                );
                return;
            }
        };
        let output_dir = b.get_output_dir().trim().trim_matches('"').to_string();
        if output_dir.is_empty() {
            b.set_settings_error(tr!("Dossier des enregistrements manquant.").into());
            return;
        }
        let choices: Vec<String> = b.get_hotkey_choices().iter().map(|c| c.to_string()).collect();
        let hotkey = |index: i32| {
            if index <= 0 {
                String::new()
            } else {
                choices.get(index as usize).cloned().unwrap_or_default()
            }
        };
        let chosen = [
            hotkey(b.get_hotkey_record_index()),
            hotkey(b.get_hotkey_show_index()),
            hotkey(b.get_hotkey_previews_index()),
        ];
        let assigned: Vec<&String> = chosen.iter().filter(|h| !h.is_empty()).collect();
        if (1..assigned.len()).any(|i| assigned[..i].contains(&assigned[i])) {
            b.set_settings_error(tr!("Un même raccourci ne peut servir qu'à une seule action.").into());
            return;
        }
        let previews_before;
        let previews_after;
        let updates_before;
        let updates_after;
        let startup_changed;
        let display_changed;
        {
            let mut config = self.config.borrow_mut();
            let s = &mut config.settings;
            if let Some(&(w, h)) = RESOLUTIONS.get(b.get_resolution_index().max(0) as usize) {
                s.video.width = w;
                s.video.height = h;
            }
            if let Some(&fps) = FRAME_RATES.get(b.get_fps_index().max(0) as usize) {
                s.video.fps = fps;
            }
            s.video.bitrate_kbps = bitrate;
            s.video.codec = if b.get_codec_index() == 1 { Codec::Hevc } else { Codec::H264 };
            s.video.hardware =
                HARDWARE.get(b.get_hardware_index().max(0) as usize).copied().unwrap_or(HardwarePreference::Auto);
            if let Some(&audio) = AUDIO_BITRATES.get(b.get_audio_index().max(0) as usize) {
                s.audio.bitrate_kbps = audio;
            }
            s.output_dir = PathBuf::from(output_dir);
            s.filename_template = b.get_template().trim().to_string();
            previews_before = s.previews;
            s.previews = b.get_settings_previews();
            previews_after = s.previews;
            s.convert_to_mp4 = b.get_settings_convert();
            updates_before = update_choice(s);
            let update_index = b.get_settings_update_index();
            s.check_updates = update_index != UPDATES_OFF;
            if s.check_updates {
                let chosen = if update_index == 1 { UpdateChannel::Beta } else { UpdateChannel::Stable };
                // Tant que le choix reste celui de la version en cours, rien
                // n'est figé : une bêta passera seule aux versions stables une
                // fois la version stable installée.
                if s.update_channel.is_some() || chosen != UpdateChannel::of_version(VERSION) {
                    s.update_channel = Some(chosen);
                }
            }
            updates_after = update_choice(s);
            s.open_windows_at_launch = b.get_settings_open_windows();
            s.close_browsers_at_exit = b.get_settings_close_browsers();
            startup_changed = s.launch_at_startup != b.get_settings_startup();
            s.launch_at_startup = b.get_settings_startup();
            let language = match b.get_settings_language_index() {
                1 => LanguageChoice::French,
                2 => LanguageChoice::English,
                _ => LanguageChoice::Auto,
            };
            let clock = match b.get_settings_clock_index() {
                1 => ClockChoice::H24,
                2 => ClockChoice::H12,
                _ => ClockChoice::Auto,
            };
            display_changed = (s.language, s.clock) != (language, clock);
            s.language = language;
            s.clock = clock;
            s.microphone.enabled = b.get_settings_mic();
            if let Some((id, name)) = self.mic_choices.borrow().get(b.get_mic_index().max(0) as usize) {
                if *id != s.microphone.device_id {
                    s.microphone.device_id = id.clone();
                    s.microphone.device_name = if id.is_empty() { String::new() } else { name.clone() };
                }
            }
            let [record, show, previews] = chosen;
            s.hotkeys.toggle_recording = record;
            s.hotkeys.show_window = show;
            s.hotkeys.toggle_previews = previews;
        }
        b.set_sheet("".into());
        self.save();
        if display_changed {
            self.apply_language_and_clock();
        }
        self.refresh_all();
        if previews_before != previews_after || display_changed {
            self.restart_previews();
        }
        if display_changed {
            self.refresh_display_texts();
        }
        self.register_hotkeys();
        if startup_changed {
            self.apply_launch_at_startup(true);
        }
        self.toast(&tr!("Réglages enregistrés."), 1);
        if updates_after != updates_before && updates_after != UPDATES_OFF {
            // Une version trouvée sur l'autre canal n'est plus proposée.
            if self.update_state.get() == UpdateState::Available {
                *self.update.borrow_mut() = None;
                self.set_update_state(UpdateState::None);
            }
            self.check_for_updates();
        }
    }

    /// Langue et horloge des réglages, appliquées partout : textes de
    /// l'interface, messages de toutes les crates, heures affichées.
    fn apply_language_and_clock(&self) {
        let (language, clock) = {
            let config = self.config.borrow();
            (config.settings.language, config.settings.clock)
        };
        let language = crate::i18n::Language::from_choice(language);
        crate::i18n::set_language(language);
        crate::i18n::apply_to_ui(language);
        let twelve_hour = match clock {
            ClockChoice::Auto => self.platform.uses_twelve_hour_clock(),
            ClockChoice::H24 => false,
            ClockChoice::H12 => true,
        };
        polyptyque_core::clock::set_twelve_hour(twelve_hour);
        self.with_bridge(|b| b.set_twelve_hour(twelve_hour));
    }

    /// Après un changement de langue ou d'horloge : textes calculés en Rust
    /// et déjà affichés, recalculés (l'interface Slint suit d'elle-même).
    fn refresh_display_texts(&self) {
        if !self.recording() {
            self.refresh_encoder_text();
            self.with_bridge(|b| b.set_mic_text(tr!("Micro").into()));
        }
        self.refresh_disk(true);
        self.refresh_schedule_texts();
        self.set_update_state(self.update_state.get());
        self.apply_launch_at_startup(false);
    }

    fn open_output_folder(&self) {
        let dir = self.config.borrow().settings.output_dir.clone();
        if let Err(e) = fs::create_dir_all(&dir) {
            self.toast(&tr!("Impossible de créer {} : {}", dir.display(), e), 3);
            return;
        }
        let opener = if cfg!(windows) { "explorer" } else { "xdg-open" };
        if let Err(e) = Command::new(opener).arg(&dir).spawn() {
            self.toast(&tr!("Impossible d'ouvrir le dossier : {}", e), 3);
        }
    }

    // ----- Fenêtres modales et confirmations --------------------------------

    fn close_sheet(&self) {
        // Signalement ouvert depuis le bilan d'incident : retour au bilan.
        if self.report_closing() {
            return;
        }
        self.confirm.borrow_mut().take();
        self.quit_with_schedules.set(false);
        if self.incident.borrow_mut().take().is_some() {
            // Bilan d'un enregistrement interrompu vu : il n'est plus proposé.
            SessionRecord::clear(&paths::session_record_file());
        }
        self.with_bridge(|b| {
            b.set_sheet("".into());
            b.set_settings_error("".into());
        });
        if !self.deferred.borrow().is_empty() {
            // Après l'action en cours (qui démarre peut-être un enregistrement).
            post(|app| app.show_next_deferred());
        }
    }

    fn ask(&self, action: Confirm, title: &str, text: &str, button: &str) {
        let danger = !matches!(action, Confirm::InstallUpdate | Confirm::ResumeAfterSleep(_) | Confirm::RepairFiles(_));
        *self.confirm.borrow_mut() = Some(action);
        self.with_bridge(|b| {
            b.set_confirm_danger(danger);
            b.set_confirm_title(title.into());
            b.set_confirm_text(text.into());
            b.set_confirm_button(button.into());
            b.set_sheet("confirm".into());
        });
    }

    fn confirm_accepted(&self) {
        let action = self.confirm.borrow_mut().take();
        self.with_bridge(|b| b.set_sheet("".into()));
        match action {
            Some(Confirm::DeleteScene) | Some(Confirm::RemoveSource(_)) if self.locked() => {}
            Some(Confirm::DeleteScene) => {
                {
                    let mut config = self.config.borrow_mut();
                    if config.scenes.len() <= 1 {
                        return;
                    }
                    let id = config.active_scene.clone();
                    config.scenes.retain(|s| s.id != id);
                    config.normalize();
                }
                self.save();
                self.refresh_all();
                self.restart_previews();
            }
            Some(Confirm::RemoveSource(id)) => {
                self.config.borrow_mut().remove_source(&id);
                self.save();
                self.refresh_all();
                self.restart_previews();
            }
            Some(Confirm::InstallUpdate) => self.download_update(),
            Some(Confirm::RepairFiles(files)) => self.enqueue_jobs(files, JobKind::Repair),
            Some(Confirm::ResumeAfterSleep(scene_id)) => {
                if !self.recording() && !self.busy.get() {
                    if self.config.borrow().active_scene != scene_id
                        && self.config.borrow().scenes.iter().any(|s| s.id == scene_id)
                    {
                        self.select_scene(&scene_id);
                    }
                    self.start_recording(None);
                }
            }
            Some(Confirm::StopScheduled) => {
                if self.recording() && !self.busy.get() {
                    self.stop_recording(None);
                }
            }
            Some(Confirm::QuitWithSchedules) => {
                self.quit_with_schedules.set(true);
                // La suite comme une fermeture normale (conversion en cours…).
                if matches!(self.close_requested(), CloseRequestResponse::HideWindow) {
                    let _ = slint::quit_event_loop();
                }
            }
            Some(Confirm::CancelConversion) => {
                if let Some(converter) = self.converter.borrow().as_ref() {
                    self.conversion.borrow_mut().cancelled = true;
                    converter.cancel_all();
                }
            }
            Some(Confirm::QuitWhileConverting) => {
                if self.recording() || self.busy.get() {
                    // Un enregistrement a démarré entre-temps : il passe avant.
                    self.close_requested();
                } else {
                    let _ = slint::quit_event_loop();
                }
            }
            Some(Confirm::QuitWhileRecording) => {
                if self.recording() {
                    self.quitting.set(true);
                    self.stop_recording(None);
                } else if self.busy.get() {
                    // Préparation ou arrêt en cours : on quittera à la fin.
                    self.quitting.set(true);
                } else {
                    let _ = slint::quit_event_loop();
                }
            }
            None => {}
        }
    }

    fn close_requested(&self) -> CloseRequestResponse {
        if self.busy.get() {
            self.toast(&tr!("Opération en cours : Polyptyque pourra être fermé dans un instant."), 2);
            return CloseRequestResponse::KeepWindowShown;
        }
        if self.recording() {
            let owned = self.planner.borrow().owned().map(|slot| slot.end);
            let schedules = schedule::any_active(&self.config.borrow().schedules, chrono::Utc::now(), &chrono::Local);
            let mut text = match owned {
                Some(end) => tr!(
                    "Un enregistrement programmé est en cours (jusqu'à {}). L'arrêter et quitter Polyptyque ? Les fichiers seront finalisés proprement, et il ne sera pas relancé dans ce créneau.",
                    schedule::clock_label(end, &chrono::Local)
                ),
                None => tr!("Arrêter l'enregistrement et quitter Polyptyque ? Les fichiers seront finalisés proprement."),
            };
            if schedules {
                text.push(' ');
                text.push_str(&tr!("Les programmations ne se lancent pas tant que Polyptyque est fermé."));
            }
            self.ask(Confirm::QuitWhileRecording, &tr!("Enregistrement en cours"), &text, &tr!("Arrêter et quitter"));
            return CloseRequestResponse::KeepWindowShown;
        }
        if !self.quit_with_schedules.get() {
            if let Some(text) = self.quit_schedule_warning() {
                self.ask(Confirm::QuitWithSchedules, &tr!("Programmation active"), &text, &tr!("Quitter quand même"));
                return CloseRequestResponse::KeepWindowShown;
            }
        }
        if self.with_bridge_value(|b| b.get_converting()) {
            self.ask(
                Confirm::QuitWhileConverting,
                &tr!("Conversion en cours"),
                &tr!("Quitter maintenant interrompt la conversion MP4. Les fichiers MKV d'origine sont conservés : ils pourront être convertis plus tard."),
                &tr!("Quitter quand même"),
            );
            return CloseRequestResponse::KeepWindowShown;
        }
        self.previews.borrow_mut().stop();
        self.meters.borrow_mut().stop();
        self.mic_meter.borrow_mut().take();
        CloseRequestResponse::HideWindow
    }

    /// Ferme les fenêtres des navigateurs des sources, toutes scènes
    /// confondues, comme un clic sur leur croix. Les applications restent
    /// ouvertes.
    fn close_source_browsers(&self) {
        let sources = self.config.borrow().sources.clone();
        let windows = self.platform.list_windows();
        let targets = resolve::browser_windows(self.platform.as_ref(), &sources, &windows);
        let mut closed = 0;
        for window in &targets {
            match self.platform.close_window(window.handle) {
                Ok(()) => closed += 1,
                Err(e) => log::warn!("fermeture d'une fenêtre de navigateur (processus {}) : {e:#}", window.pid),
            }
        }
        if closed > 0 {
            log::info!("{closed} fenêtre(s) de navigateur fermée(s) avec Polyptyque");
        }
    }

    // ----- Fermeture de la session Windows ---------------------------------

    /// Poignée de la fenêtre principale (une fois créée par le système).
    #[cfg(windows)]
    fn main_window(&self) -> Option<isize> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let ui = self.ui.upgrade()?;
        let handle = ui.window().window_handle();
        match handle.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(window) => Some(window.hwnd.get()),
            _ => None,
        }
    }

    /// Arrêt, redémarrage ou déconnexion de Windows : surveillé dès que la
    /// fenêtre existe.
    #[cfg(windows)]
    fn watch_session_end(&self) {
        if self.session_watch.get() {
            return;
        }
        let Some(window) = self.main_window() else {
            return;
        };
        self.session_watch.set(true);
        let watched = polyptyque_windows::session_end::watch(window, |event| {
            with_app(|app| app.on_session_end(event)).unwrap_or(false)
        });
        if let Err(e) = watched {
            log::warn!("{e:#}");
        }
    }

    #[cfg(not(windows))]
    fn watch_session_end(&self) {}

    /// Renvoie vrai pour demander à Windows de ne pas fermer la session.
    #[cfg(windows)]
    fn on_session_end(&self, event: polyptyque_windows::session_end::SessionEnd) -> bool {
        use polyptyque_windows::session_end::SessionEnd;
        match event {
            // Appelé par Windows en plein traitement de message : aucune
            // panique possible ici. Dans le doute, on bloque la fermeture.
            SessionEnd::Query => {
                self.session.try_borrow().map_or(true, |s| s.is_some())
                    || self.stop_thread.try_borrow().map_or(true, |t| t.is_some())
            }
            SessionEnd::Ending => {
                self.session_ending.set(true);
                self.finalize_now();
                false
            }
            // Traités juste après, hors du traitement du message de Windows.
            SessionEnd::Suspend => {
                post(|app| app.on_suspend());
                false
            }
            SessionEnd::Resume => {
                post(|app| app.on_resume());
                false
            }
        }
    }

    /// Mise en veille pendant un enregistrement : impossible à empêcher. On
    /// arrête proprement plutôt que de laisser l'enregistrement reprendre au
    /// réveil avec toute la durée de la veille en image figée.
    #[cfg(windows)]
    fn on_suspend(&self) {
        // Enregistrement programmé : la question du démarrage manqué sera
        // posée au réveil, s'il a lieu pendant le créneau.
        let scheduled = self.schedule_suspend();
        if !self.recording() {
            return;
        }
        log::warn!("mise en veille pendant l'enregistrement : arrêt propre");
        if !scheduled {
            let scene_id = self.config.borrow().active_scene.clone();
            let (_, time) = naming::now_parts();
            *self.sleep_stop.borrow_mut() = Some((scene_id, time));
        }
        self.stop_recording(Some(tr!("Mise en veille de l'ordinateur : enregistrement arrêté.")));
    }

    #[cfg(windows)]
    fn on_resume(&self) {
        self.schedule_resume();
        let Some((scene_id, time)) = self.sleep_stop.borrow_mut().take() else {
            return;
        };
        log::info!("sortie de veille : reprise de l'enregistrement proposée");
        self.ask(
            Confirm::ResumeAfterSleep(scene_id),
            &tr!("Reprendre l'enregistrement ?"),
            &tr!(
                "L'enregistrement a été arrêté proprement à {}, juste avant la mise en veille de l'ordinateur. Le reprendre maintenant (dans de nouveaux fichiers) ?",
                display_time(&time)
            ),
            &tr!("Reprendre"),
        );
    }

    /// La session Windows se ferme malgré tout : les fichiers sont
    /// finalisés tout de suite, avant que Windows ne ferme Polyptyque.
    #[cfg(windows)]
    fn finalize_now(&self) {
        if let Ok(mut previews) = self.previews.try_borrow_mut() {
            previews.stop();
        }
        let session = self.session.try_borrow_mut().ok().and_then(|mut s| s.take());
        if let Some(session) = session {
            let summaries = session.stop();
            log::info!("{} source(s) finalisée(s) avant la fermeture de session", summaries.len());
        }
        // Arrêt déjà en cours : on attend qu'il se termine.
        let stopping = self.stop_thread.try_borrow_mut().ok().and_then(|mut t| t.take());
        let stopped_by_user = stopping.is_some();
        if let Some(handle) = stopping {
            let _ = handle.join();
        }
        if let Ok(mut record) = self.session_record.try_borrow_mut() {
            match record.take() {
                // Enregistrement coupé par Windows : au prochain lancement, le
                // bilan proposera de le reprendre.
                Some(mut interrupted) if !stopped_by_user => {
                    interrupted.ended_by_windows = true;
                    if let Err(e) = interrupted.save(&paths::session_record_file()) {
                        log::warn!("journal de session : {e:#}");
                    }
                }
                _ => SessionRecord::clear(&paths::session_record_file()),
            }
        }
        // Une conversion interrompue efface son fichier partiel.
        let converter = self.converter.try_borrow_mut().ok().and_then(|mut c| c.take());
        drop(converter);
    }

    /// Déclare (ou retire) l'enregistrement en cours auprès de Windows, qui
    /// l'affiche si l'on tente d'éteindre ou de se déconnecter.
    #[cfg(windows)]
    fn set_shutdown_block(&self, on: bool) {
        let Some(window) = self.main_window() else {
            return;
        };
        if on {
            if let Err(e) = polyptyque_windows::session_end::block(window, &shutdown_reason()) {
                log::warn!("{e:#}");
            }
        } else {
            polyptyque_windows::session_end::unblock(window);
        }
        polyptyque_windows::session_end::restart_after_interruption(on);
    }

    #[cfg(not(windows))]
    fn set_shutdown_block(&self, _: bool) {}

    // ----- Raccourcis clavier globaux --------------------------------------

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn on_hotkey(&self, action: HotkeyAction) {
        log::info!("raccourci : {action:?}");
        match action {
            HotkeyAction::ToggleRecording => self.start_stop(),
            HotkeyAction::ShowWindow => {
                if let Some(ui) = self.ui.upgrade() {
                    let window = ui.window();
                    window.set_minimized(!window.is_minimized());
                }
            }
            HotkeyAction::TogglePreviews => self.toggle_previews(),
        }
    }

    #[cfg(windows)]
    fn register_hotkeys(&self) {
        use polyptyque_windows::hotkeys::HotkeyListener;
        let settings = self.config.borrow().settings.hotkeys.clone();
        let bindings = [
            (settings.toggle_recording.as_str(), HotkeyAction::ToggleRecording),
            (settings.show_window.as_str(), HotkeyAction::ShowWindow),
            (settings.toggle_previews.as_str(), HotkeyAction::TogglePreviews),
        ];
        let parsed: Vec<_> =
            bindings.iter().filter_map(|(text, action)| Some((hotkeys::parse(text)?, *action))).collect();
        // Remplacer l'ancien écouteur libère d'abord ses raccourcis.
        self.hotkeys.borrow_mut().take();
        let (keys, actions): (Vec<_>, Vec<_>) = parsed.into_iter().unzip();
        match HotkeyListener::start(keys, move |index| {
            if let Some(&action) = actions.get(index) {
                post(move |app| app.on_hotkey(action));
            }
        }) {
            Ok((listener, failed)) => {
                *self.hotkeys.borrow_mut() = Some(listener);
                if !failed.is_empty() {
                    self.toast(
                        &tr!(
                            "Raccourci déjà utilisé par un autre programme : {}"
                                | "Raccourcis déjà utilisés par un autre programme : {}" % failed.len(),
                            failed.join(", ")
                        ),
                        2,
                    );
                }
            }
            Err(e) => log::warn!("raccourcis indisponibles : {e:#}"),
        }
    }

    #[cfg(not(windows))]
    fn register_hotkeys(&self) {}
}

/// Recherche de l'encodeur et ouverture des fenêtres (hors du fil de
/// l'interface : peut durer plusieurs secondes).
fn prepare(platform: &dyn Platform, config: &Config, sources: &[Source]) -> Result<Prepared> {
    let ffmpeg = probe::locate_ffmpeg()
        .ok_or_else(|| anyhow!(tr!("FFmpeg introuvable : ffmpeg.exe doit se trouver à côté de polyptyque.exe")))?;
    let video = &config.settings.video;
    let encoder = probe::select_encoder(platform, &ffmpeg, video.hardware, video.codec).ok_or_else(|| {
        anyhow!(tr!(
            "aucun encodeur {} utilisable avec le réglage « {} ». Essayer un autre matériel ou le format H.264.",
            video.codec.label(),
            video.hardware.label()
        ))
    })?;
    // Dossier choisi inaccessible (disque débranché, autre PC) : dossier de
    // secours plutôt qu'un refus de démarrer.
    let chosen = &config.settings.output_dir;
    let (output_dir, output_note) = if storage::dir_writable(chosen) {
        (chosen.clone(), None)
    } else {
        let fallback = paths::default_output_dir();
        if !storage::dir_writable(&fallback) {
            return Err(anyhow!(tr!("impossible d'écrire dans {} ni dans {}", chosen.display(), fallback.display())));
        }
        log::warn!("dossier {} inaccessible : enregistrement dans {}", chosen.display(), fallback.display());
        let note = tr!("Dossier {} inaccessible : enregistrement dans {}.", chosen.display(), fallback.display());
        (fallback, Some(note))
    };
    post(|app| app.set_busy(&tr!("Ouverture des fenêtres…")));
    let missing = resolve::prepare_windows(platform, sources, LAUNCH_WAIT, |event| {
        if let PrepareEvent::Launching(source) = event {
            let text = tr!("Ouverture de « {} »…", source.name);
            post(move |app| app.set_busy(&text));
        }
    });
    Ok(Prepared { ffmpeg, encoder, missing, output_dir, output_note })
}

/// Choix « Mises à jour » affiché dans les réglages (voir `UPDATES_OFF`).
fn update_choice(settings: &Settings) -> i32 {
    if !settings.check_updates {
        UPDATES_OFF
    } else if settings.update_channel(VERSION) == UpdateChannel::Beta {
        1
    } else {
        0
    }
}

/// Même fichier, quelle que soit l'écriture du chemin (majuscules, `..`).
fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Texte de la pastille de conversion : « MP4 · 2 sur 3 · 45 % » (ou
/// « Réparation · … »).
fn conversion_text(progress: &ConversionProgress, value: f32) -> String {
    let current = (progress.finished + 1).min(progress.total.max(1));
    let percent = (value * 100.0).round() as u32;
    match (progress.repairing, progress.total > 1) {
        (true, true) => tr!("Réparation · {} sur {} · {} %", current, progress.total, percent),
        (true, false) => tr!("Réparation · {} %", percent),
        (false, true) => tr!("MP4 · {} sur {} · {} %", current, progress.total, percent),
        (false, false) => tr!("MP4 · {} %", percent),
    }
}

fn added_to_conversion(count: usize) -> String {
    tr!("{n} fichier ajouté à la conversion MP4." | "{n} fichiers ajoutés à la conversion MP4." % count)
}

/// Examine les fichiers d'un enregistrement interrompu.
fn inspect_interrupted(platform: &dyn Platform, files: &[PathBuf]) -> Vec<(PathBuf, FileReport)> {
    let ffmpeg = probe::locate_ffmpeg();
    files
        .iter()
        .map(|path| {
            let report = match fs::metadata(path) {
                Err(_) => FileReport::Missing,
                Ok(meta) if meta.len() == 0 => FileReport::Empty,
                Ok(meta) => match &ffmpeg {
                    None => FileReport::Unreadable(tr!("FFmpeg introuvable")),
                    Some(ffmpeg) => match remux::inspect(platform, ffmpeg, path) {
                        Ok(info) if remux::needs_repair(&info) => FileReport::NeedsRepair { size: meta.len() },
                        Ok(info) => FileReport::Complete { seconds: info.duration.unwrap_or(0.0), size: meta.len() },
                        Err(e) => FileReport::Unreadable(format!("{e:#}")),
                    },
                },
            };
            (path.clone(), report)
        })
        .collect()
}

fn report_text(report: &FileReport) -> String {
    match report {
        FileReport::Missing => tr!("Introuvable : déplacé ou supprimé ?"),
        FileReport::Empty => tr!("Vide : rien n'avait été écrit"),
        FileReport::Complete { seconds, size } => {
            tr!(
                "Complet · {} · {}",
                format::clock(Duration::try_from_secs_f64(*seconds).unwrap_or_default()),
                format::bytes(*size)
            )
        }
        FileReport::NeedsRepair { size } => tr!("Interrompu, lisible, à réparer · {}", format::bytes(*size)),
        FileReport::Unreadable(e) => tr!("Illisible : {}", e),
    }
}

/// Date d'un journal (« 2026-10-21 ») écrite avec le nom du mois :
/// « mer. 21 oct. » (« Wed, Oct 21 »).
fn display_date(date: &str) -> String {
    match chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d") {
        Ok(date) => polyptyque_core::clock::date(date, chrono::Local::now().date_naive()),
        Err(_) => date.to_string(),
    }
}

/// Heure d'un journal (« 14h32 ») selon l'horloge : « 14:32 », « 2:32 PM ».
fn display_time(time: &str) -> String {
    schedule::TimeOfDay::parse(time).map_or_else(|| time.to_string(), schedule::TimeOfDay::label)
}
