//! Signalement d'un problème : rapport masqué à joindre à une issue GitHub
//! (voir `polyptyque_core::report` et le cahier des charges).

use std::path::PathBuf;
use std::thread;

use polyptyque_core::config::{HardwarePreference, SourceKind};
use polyptyque_core::report::{self, Masker, ReportInfo};
use polyptyque_core::{paths, platform::Platform};
use tr::tr;

use super::{post, update_choice, App, UPDATES_OFF, VERSION};
use crate::i18n::Language;
use crate::update::REPOSITORY;

/// Où en est le signalement en cours.
#[derive(Default)]
pub(super) struct ReportState {
    /// Fenêtre d'où vient le signalement (réglages, ou bilan d'un
    /// enregistrement interrompu) : on y revient en fermant, saisies
    /// comprises.
    return_to: &'static str,
    path: Option<PathBuf>,
    url: String,
}

impl App {
    pub(super) fn request_report(&self, from_incident: bool) {
        let return_to = if from_incident { "incident" } else { "settings" };
        *self.report.borrow_mut() = ReportState { return_to, ..ReportState::default() };
        self.with_bridge(|b| {
            b.set_report_path("".into());
            b.set_report_name("".into());
            b.set_sheet("report".into());
        });
    }

    /// Écrit le rapport (hors du fil de l'interface : lecture des journaux,
    /// cartes graphiques), puis montre le fichier et ouvre la page GitHub.
    pub(super) fn prepare_report(&self) {
        let info_settings = self.report_settings();
        let platform = self.platform.clone();
        self.set_busy(&tr!("Préparation du rapport…"));
        thread::spawn(move || {
            let (info, fields) = report_info(platform.as_ref(), info_settings);
            let text = report::build(&info, &paths::logs_dir(), &Masker::from_environment());
            // Préfixe du nom de fichier : `rapport_` ou `report_` (catalogue).
            let prefix = tr!("rapport_");
            let result = report::write(&paths::logs_dir(), &prefix, &text);
            post(move |app| app.finish_report(result, &fields));
        });
    }

    fn finish_report(&self, result: anyhow::Result<PathBuf>, fields: &[(&'static str, String)]) {
        self.set_busy("");
        let path = match result {
            Ok(path) => path,
            Err(e) => {
                log::warn!("rapport de problème : {e:#}");
                self.toast(&tr!("Impossible d'écrire le rapport : {}", format!("{e:#}")), 3);
                return;
            }
        };
        log::info!("rapport de problème écrit : {}", path.display());
        let template = match Language::current() {
            Language::French => "signalement.yml",
            Language::English => "bug-report.yml",
        };
        let fields: Vec<(&str, &str)> = fields.iter().map(|(id, value)| (*id, value.as_str())).collect();
        let url = report::issue_url(REPOSITORY, template, &fields);
        if let Err(e) = self.platform.reveal_file(&path) {
            log::warn!("rapport : dossier non ouvert : {e:#}");
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let full = path.display().to_string();
        {
            let mut state = self.report.borrow_mut();
            state.path = Some(path);
            state.url = url;
        }
        self.with_bridge(|b| {
            b.set_report_name(name.into());
            b.set_report_path(full.into());
        });
        self.open_report_page();
    }

    pub(super) fn open_report(&self) {
        let Some(path) = self.report.borrow().path.clone() else {
            return;
        };
        if let Err(e) = self.platform.shell_open(&path.display().to_string()) {
            self.toast(&tr!("Impossible d'ouvrir le rapport : {}", format!("{e:#}")), 3);
        }
    }

    pub(super) fn open_report_page(&self) {
        let url = self.report.borrow().url.clone();
        if url.is_empty() {
            return;
        }
        if let Err(e) = self.platform.shell_open(&url) {
            log::warn!("page GitHub non ouverte : {e:#}");
            self.toast(&tr!("Impossible d'ouvrir le navigateur. Adresse de la page : {}", url), 3);
        }
    }

    /// Fermeture de la fenêtre de signalement : retour à la fenêtre qui l'a
    /// ouverte. Renvoie vrai si la fermeture est traitée.
    pub(super) fn report_closing(&self) -> bool {
        let is_report = self.with_bridge_value(|b| b.get_sheet() == "report");
        if !is_report {
            return false;
        }
        let return_to = std::mem::take(&mut *self.report.borrow_mut()).return_to;
        let back = match return_to {
            "incident" => self.incident.borrow().is_some(),
            "settings" => true,
            _ => false,
        };
        if back {
            self.with_bridge(|b| b.set_sheet(return_to.into()));
        }
        back
    }

    /// Résumé des réglages, en français comme les journaux, sans chemin ni
    /// nom de périphérique.
    fn report_settings(&self) -> Vec<String> {
        let config = self.config.borrow();
        let s = &config.settings;
        let yes_no = |value: bool| if value { "oui" } else { "non" };
        let hardware = match s.video.hardware {
            HardwarePreference::Auto => "automatique",
            HardwarePreference::Gpu => "carte graphique",
            HardwarePreference::Cpu => "processeur",
        };
        let updates = match update_choice(s) {
            UPDATES_OFF => "aucune recherche",
            1 => "stables et bêta",
            _ => "stables",
        };
        let mut browsers: Vec<String> = Vec::new();
        let mut applications = 0;
        for source in &config.sources {
            match &source.kind {
                SourceKind::Browser { browser, .. } => {
                    if !browsers.contains(&browser.name) {
                        browsers.push(browser.name.clone());
                    }
                }
                SourceKind::Application { .. } => applications += 1,
            }
        }
        let enabled = config.schedules.iter().filter(|s| s.enabled).count();
        vec![
            format!(
                "Vidéo : {}x{}, {} i/s, {}, matériel : {hardware}, {} kb/s",
                s.video.width,
                s.video.height,
                s.video.fps,
                s.video.codec.label(),
                s.video.bitrate_kbps
            ),
            format!("Son : {} kb/s ; micro : {}", s.audio.bitrate_kbps, yes_no(s.microphone.enabled)),
            format!(
                "Conversion MP4 automatique : {} ; aperçus : {} ; ouvrir les fenêtres au lancement : {}",
                yes_no(s.convert_to_mp4),
                yes_no(s.previews),
                yes_no(s.open_windows_at_launch)
            ),
            format!(
                "Scènes : {} ; sources : {} (navigateurs : {} ; applications : {applications})",
                config.scenes.len(),
                config.sources.len(),
                if browsers.is_empty() { "aucun".to_string() } else { browsers.join(", ") }
            ),
            format!(
                "Programmations : {} (activées : {enabled}) ; lancement au démarrage de Windows : {}",
                config.schedules.len(),
                yes_no(s.launch_at_startup)
            ),
            format!("Mises à jour : {updates}"),
        ]
    }
}

/// Système et version pour le rapport, et champs pré-remplis de la page
/// GitHub (identifiants des champs des formulaires d'issue).
fn report_info(platform: &dyn Platform, settings: Vec<String>) -> (ReportInfo, Vec<(&'static str, String)>) {
    let portable = paths::portable_root().is_some();
    // Rapport en français, comme les journaux ; page GitHub dans la langue
    // de l'interface.
    let version = format!("{VERSION} ({})", if portable { "portable" } else { "installée" });
    let shown_version = if portable { tr!("{} (portable)", VERSION) } else { tr!("{} (installée)", VERSION) };
    let system = platform.os_description();
    let graphics = platform.graphics_adapters();
    let language = format!(
        "{} (Windows : {})",
        match Language::current() {
            Language::French => "fr",
            Language::English => "en",
        },
        sys_locale::get_locale().unwrap_or_else(|| "inconnue".into())
    );
    let fields = vec![("version", shown_version), ("windows", system.clone()), ("gpu", graphics.join(", "))];
    (ReportInfo { version, system, language, graphics, settings }, fields)
}
