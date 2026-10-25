//! Connexion au Polyptyque ouvert, et lancement en arrière-plan s'il est
//! fermé.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use polyptyque_core::config::Config;
use polyptyque_core::control::protocol::{CommandError, Reason};
use polyptyque_core::control::transport::{Client, ConnectError};
use polyptyque_core::control::BACKGROUND_ARG;
use serde_json::{json, Value};
use tr::tr;

/// Attente du canal après un lancement.
const LAUNCH_WAIT: Duration = Duration::from_secs(30);

/// Configuration de Polyptyque : celle de la version installée ou
/// portable (le mode commande est livré dans le même dossier), ou celle du
/// mode démonstration hors Windows.
pub fn config_path() -> PathBuf {
    #[cfg(windows)]
    return polyptyque_core::paths::config_file();
    #[cfg(not(windows))]
    return polyptyque_core::paths::demo_dir().join("config.toml");
}

/// Configuration lue sans rien écrire.
pub fn read_config() -> Option<Config> {
    Config::read(&config_path())
}

fn control_enabled() -> bool {
    read_config().is_some_and(|c| c.settings.remote_control)
}

fn disabled() -> CommandError {
    CommandError::new(
        Reason::ControlDisabled,
        tr!("Le pilotage de Polyptyque est désactivé. La personne doit l'activer dans Polyptyque : Réglages, « Autoriser le pilotage par d'autres programmes »."),
    )
}

fn not_running() -> CommandError {
    CommandError::new(
        Reason::NotRunning,
        tr!("Polyptyque n'est pas ouvert. Le lancer en arrière-plan : « polyptyque-cli app launch » (outil app_launch)."),
    )
}

#[cfg(windows)]
fn open() -> Result<Box<dyn polyptyque_core::control::transport::Connection>, ConnectError> {
    Ok(Box::new(polyptyque_windows::control_pipe::connect()?))
}

#[cfg(unix)]
fn open() -> Result<Box<dyn polyptyque_core::control::transport::Connection>, ConnectError> {
    Ok(Box::new(polyptyque_core::control::unix::connect()?))
}

/// Connexion au Polyptyque ouvert par ce compte.
pub fn connect() -> Result<Client, CommandError> {
    match open() {
        Ok(connection) => Ok(Client::new(connection)),
        // Personne n'écoute : fermé, ou pilotage désactivé.
        Err(ConnectError::NotListening) => Err(if control_enabled() { not_running() } else { disabled() }),
        Err(ConnectError::NotOurs) => Err(CommandError::new(
            Reason::Refused,
            tr!("Le canal de commande n'appartient pas à ce compte Windows : connexion refusée."),
        )),
        Err(ConnectError::Io(e)) => {
            Err(CommandError::internal(tr!("Connexion à Polyptyque impossible : {}", e.to_string())))
        }
    }
}

/// Lance Polyptyque en arrière-plan s'il est fermé, puis attend que le
/// canal réponde.
pub fn launch() -> Result<Value, CommandError> {
    if connect().is_ok() {
        return Ok(json!({ "running": true, "launched": false }));
    }
    // Sans pilotage activé, le lancer ne servirait à rien.
    if !control_enabled() {
        return Err(disabled());
    }
    let exe = std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name(if cfg!(windows) { "polyptyque.exe" } else { "polyptyque" }))
        .filter(|exe| exe.exists())
        .ok_or_else(|| {
            CommandError::new(
                Reason::Unavailable,
                tr!("Polyptyque introuvable : polyptyque.exe doit se trouver à côté de polyptyque-cli.exe."),
            )
        })?;
    Command::new(&exe)
        .arg(BACKGROUND_ARG)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| CommandError::internal(tr!("Lancement de Polyptyque impossible : {}", e.to_string())))?;
    let deadline = Instant::now() + LAUNCH_WAIT;
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(300));
        if connect().is_ok() {
            return Ok(json!({ "running": true, "launched": true }));
        }
    }
    Err(CommandError::new(
        Reason::Timeout,
        tr!("Polyptyque a été lancé mais ne répond pas encore : réessayer dans un instant."),
    ))
}
