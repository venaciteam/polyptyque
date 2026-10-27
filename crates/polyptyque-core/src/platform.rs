//! Ce que chaque système d'exploitation doit fournir à Polyptyque.
//!
//! Seule l'implémentation de ce trait change d'un système à l'autre ; le
//! reste du logiciel (moteur, interface) n'en dépend que par lui.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use crate::config::BrowserFamily;

/// Identifiant opaque d'une fenêtre (un `HWND` sous Windows).
pub type WindowHandle = isize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub handle: WindowHandle,
    pub pid: u32,
    pub title: String,
    pub class: String,
    pub exe: PathBuf,
}

/// Périphérique d'entrée audio (micro).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct BrowserInstall {
    pub name: String,
    pub exe_path: PathBuf,
    pub family: BrowserFamily,
}

/// Image NV12 à la résolution de sortie, partagée sans copie entre la
/// capture et l'encodeur (une image répétée ne coûte qu'un compteur).
pub type Nv12Frame = Arc<Vec<u8>>;

pub trait VideoCapture {
    /// Renvoie une nouvelle image si le contenu de la fenêtre a changé
    /// depuis l'appel précédent. Une erreur signifie que la capture est
    /// perdue et doit être recréée.
    fn poll(&mut self) -> anyhow::Result<Option<Nv12Frame>>;
}

pub struct AudioPacket {
    /// Échantillons flottants 32 bits entrelacés.
    pub data: Vec<u8>,
    /// Horodatage du premier échantillon, dans la même base de temps que
    /// [`Platform::now_hns`], s'il est fiable.
    pub timestamp_hns: Option<u64>,
    /// Le paquet est déclaré silencieux par le système.
    pub silent: bool,
}

pub trait AudioCapture {
    /// Attend au plus `timeout_ms` puis ajoute à `out` les paquets reçus.
    fn read(&mut self, timeout_ms: u32, out: &mut Vec<AudioPacket>) -> anyhow::Result<()>;
}

/// Tube nommé par lequel un flux brut est envoyé à FFmpeg.
pub trait StreamPipe: Send {
    /// Chemin à donner à FFmpeg comme entrée.
    fn ffmpeg_path(&self) -> String;
    /// Attend que FFmpeg ouvre le tube, puis renvoie de quoi y écrire.
    fn connect(self: Box<Self>) -> anyhow::Result<Box<dyn Write + Send>>;
}

pub trait Platform: Send + Sync {
    /// Fenêtres principales visibles, de la plus récente à la plus ancienne.
    fn list_windows(&self) -> Vec<WindowInfo>;
    fn is_window_alive(&self, handle: WindowHandle) -> bool;
    fn is_window_minimized(&self, handle: WindowHandle) -> bool;

    /// Fait revenir une fenêtre réduite et la replace sur un écran branché
    /// si besoin, sans lui donner le focus. Peut attendre le programme de la
    /// fenêtre : à appeler hors du fil de l'interface.
    fn restore_window(&self, _handle: WindowHandle) -> anyhow::Result<()> {
        anyhow::bail!(tr::tr!("non disponible sur ce système"))
    }

    /// Demande à une fenêtre de se fermer, comme un clic sur sa croix : son
    /// programme enregistre sa session, ou demande confirmation s'il a des
    /// données non enregistrées. N'attend pas la fermeture.
    fn close_window(&self, _handle: WindowHandle) -> anyhow::Result<()> {
        anyhow::bail!(tr::tr!("non disponible sur ce système"))
    }
    fn process_command_line(&self, pid: u32) -> Option<String>;
    fn detect_browsers(&self) -> Vec<BrowserInstall>;

    /// Horloge commune, en centaines de nanosecondes.
    fn now_hns(&self) -> u64;

    /// À appeler sur le fil qui utilisera la capture.
    fn open_video_capture(
        &self,
        handle: WindowHandle,
        width: u32,
        height: u32,
        capture_cursor: bool,
    ) -> anyhow::Result<Box<dyn VideoCapture>>;

    /// Son produit par le processus `pid` et ses sous-processus. À appeler
    /// sur le fil qui utilisera la capture.
    fn open_audio_capture(&self, pid: u32, sample_rate: u32, channels: u16) -> anyhow::Result<Box<dyn AudioCapture>>;

    /// Micros disponibles.
    fn list_microphones(&self) -> Vec<AudioDevice>;

    /// Son d'un micro (`None` : celui choisi par défaut dans Windows). À
    /// appeler sur le fil qui utilisera la capture.
    fn open_microphone_capture(
        &self,
        device_id: Option<&str>,
        sample_rate: u32,
        channels: u16,
    ) -> anyhow::Result<Box<dyn AudioCapture>>;

    fn create_stream_pipe(&self, name: &str) -> anyhow::Result<Box<dyn StreamPipe>>;

    /// Réglages propres au système pour les processus FFmpeg (pas de
    /// fenêtre, pas de propagation du Ctrl+C…).
    fn prepare_encoder_process(&self, command: &mut Command);

    /// Empêche la mise en veille et affine la précision des minuteries
    /// pendant un enregistrement. À appeler depuis le fil principal.
    fn begin_recording_session(&self);
    fn end_recording_session(&self);

    fn free_disk_space(&self, path: &Path) -> Option<u64>;

    /// Système, pour un rapport de problème : « Windows 11 Pro 24H2 (build
    /// 26100.2033) ».
    fn os_description(&self) -> String {
        std::env::consts::OS.to_string()
    }

    /// Horloge du format régional du système : vrai pour 12 h (AM/PM).
    fn uses_twelve_hour_clock(&self) -> bool {
        false
    }

    /// Cartes graphiques, pour un rapport de problème.
    fn graphics_adapters(&self) -> Vec<String> {
        Vec::new()
    }

    /// Erreurs récentes du pilote graphique et plantages de Polyptyque ou
    /// de FFmpeg, une ligne chacun (voir `report::SystemEvent::line`), pour
    /// un rapport de problème. `None` : journaux du système illisibles.
    fn graphics_driver_events(&self) -> Option<Vec<String>> {
        None
    }

    /// Ouvre une adresse web dans le navigateur par défaut, ou un fichier
    /// avec son programme habituel.
    fn shell_open(&self, _target: &str) -> anyhow::Result<()> {
        anyhow::bail!(tr::tr!("non disponible sur ce système"))
    }

    /// Montre un fichier dans le gestionnaire de fichiers, sélectionné.
    fn reveal_file(&self, _path: &Path) -> anyhow::Result<()> {
        anyhow::bail!(tr::tr!("non disponible sur ce système"))
    }
}
