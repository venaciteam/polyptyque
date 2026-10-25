//! Polyptyque : enregistrement simultané de plusieurs fenêtres, chacune dans
//! son propre fichier avec son propre son. Interface graphique.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod controller;
mod convert;
#[cfg(not(windows))]
mod demo;
mod format;
mod i18n;
mod meter;
mod preview;
mod update;

use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use anyhow::Result;
use polyptyque_core::{naming, paths, storage};
use simplelog::{ConfigBuilder, LevelFilter, WriteLogger};

slint::include_modules!();

/// Version installée par une mise à jour, si Polyptyque vient d'être
/// relancé par elle.
pub static UPDATED_TO: OnceLock<String> = OnceLock::new();

/// Argument donné par Windows quand il lance Polyptyque à l'ouverture de
/// session (voir le réglage « Lancer Polyptyque au démarrage de Windows »).
#[cfg(windows)]
const STARTUP_ARG: &str = polyptyque_windows::startup::ARG;
#[cfg(not(windows))]
const STARTUP_ARG: &str = "--demarrage-windows";

fn main() {
    // Installateur et mises à jour : doit passer en premier (il peut
    // terminer le programme, par exemple pendant une installation).
    #[cfg(windows)]
    velopack::VelopackApp::build()
        .on_restarted(|version| {
            let _ = UPDATED_TO.set(version.to_string());
        })
        // Désinstallation : Windows ne doit plus lancer un programme absent.
        .on_before_uninstall_fast_callback(|_| {
            if let Ok(exe) = std::env::current_exe() {
                polyptyque_windows::startup::disable_for(&exe);
            }
        })
        .run();
    let at_startup = std::env::args().skip(1).any(|arg| arg == STARTUP_ARG);
    // Une seule copie : vérifié avant d'ouvrir le journal, qui écraserait
    // celui de la copie déjà ouverte s'il porte la même minute.
    #[cfg(windows)]
    let _instance = {
        use polyptyque_windows::single_instance::{self, Instance};
        match single_instance::acquire(!at_startup) {
            Instance::First(lock) => lock,
            Instance::AlreadyOpen => return,
        }
    };
    let log = init_logging();
    log::info!("Polyptyque {} (interface graphique)", env!("CARGO_PKG_VERSION"));
    i18n::set_language(i18n::Language::system());
    if at_startup {
        log::info!("lancé par Windows à l'ouverture de session");
    }
    if let Some(root) = paths::portable_root() {
        log::info!("version portable : données dans {}", root.join(paths::PORTABLE_DATA_DIR).display());
    }
    if let Err(e) = launch(at_startup) {
        log::error!("{e:#}");
        let log = log.map(|p| p.display().to_string()).unwrap_or_default();
        show_fatal(&tr::tr!("{}\n\nJournal : {}", format!("{e:#}"), log));
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn launch(at_startup: bool) -> Result<()> {
    let platform = Arc::new(polyptyque_windows::WindowsPlatform::new());
    let paths = controller::Paths {
        config: paths::config_file(),
        profiles: paths::profiles_dir(),
        schedule_state: paths::schedule_state_file(),
    };
    controller::run(platform, paths, None, at_startup)
}

/// Hors Windows : plateforme de démonstration, dans un dossier temporaire.
#[cfg(not(windows))]
fn launch(at_startup: bool) -> Result<()> {
    let root = std::env::temp_dir().join("polyptyque-demo");
    let platform = demo::DemoPlatform::new(&root);
    let config = root.join("config.toml");
    let initial = (!config.exists()).then(|| platform.seed_config());
    let paths = controller::Paths {
        config,
        profiles: root.join("profils"),
        schedule_state: root.join("programmations-etat.toml"),
    };
    controller::run(Arc::new(platform), paths, initial, at_startup)
}

fn show_fatal(message: &str) {
    #[cfg(windows)]
    {
        let _ = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Polyptyque")
            .set_description(message)
            .show();
    }
    #[cfg(not(windows))]
    eprintln!("{}", tr::tr!("Erreur : {}", message));
}

fn init_logging() -> Option<PathBuf> {
    let dir = paths::logs_dir();
    fs::create_dir_all(&dir).ok()?;
    let (date, time) = naming::now_parts();
    let path = dir.join(format!("polyptyque_{date}_{time}.log"));
    let file = File::create(&path).ok()?;
    let mut builder = ConfigBuilder::new();
    let _ = builder.set_time_offset_to_local();
    let _ = WriteLogger::init(LevelFilter::Info, builder.build(), file);
    std::panic::set_hook(Box::new(|info| log::error!("erreur interne : {info}")));
    let removed = storage::prune_old_logs(&dir, storage::LOG_RETENTION);
    if removed > 0 {
        log::info!("{removed} journal(aux) de plus de 30 jours effacé(s)");
    }
    Some(path)
}
