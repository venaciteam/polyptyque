//! État partagé entre les fils d'une source et l'interface.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use polyptyque_core::platform::WindowInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceState {
    /// Fenêtre pas encore trouvée : l'image est noire.
    Searching,
    Connected,
    /// Fenêtre réduite : l'image est figée.
    Minimized,
    /// Fenêtre disparue : image noire jusqu'à son retour.
    Lost,
    /// Encodeur arrêté : la source n'enregistre plus.
    Failed,
    /// Arrêt demandé : les dernières données sont écrites et le fichier
    /// est fermé.
    Finishing,
    Finished,
}

impl SourceState {
    pub fn label(self) -> &'static str {
        match self {
            SourceState::Searching => "Recherche",
            SourceState::Connected => "Connectée",
            SourceState::Minimized => "Réduite",
            SourceState::Lost => "Perdue",
            SourceState::Failed => "ERREUR",
            SourceState::Finishing => "Finalisation",
            SourceState::Finished => "Terminée",
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => SourceState::Connected,
            2 => SourceState::Minimized,
            3 => SourceState::Lost,
            4 => SourceState::Failed,
            5 => SourceState::Finished,
            6 => SourceState::Finishing,
            _ => SourceState::Searching,
        }
    }

    fn to_u8(self) -> u8 {
        match self {
            SourceState::Searching => 0,
            SourceState::Connected => 1,
            SourceState::Minimized => 2,
            SourceState::Lost => 3,
            SourceState::Failed => 4,
            SourceState::Finished => 5,
            SourceState::Finishing => 6,
        }
    }
}

pub(crate) struct Shared {
    state: AtomicU8,
    pub frames_total: AtomicU64,
    pub frames_new: AtomicU64,
    pub frames_dropped: AtomicU64,
    /// Images en attente d'encodage : un retard qui grandit signale un
    /// encodeur qui ne suit pas.
    pub video_backlog: AtomicU64,
    /// Octets transmis à l'encodeur (sert à vérifier qu'il avance encore).
    pub bytes_sent: AtomicU64,
    /// Position dans chaque flux depuis le départ de la source : images
    /// et octets de son déjà pris dans les files (écrits ou perdus). Sert à
    /// recaler image et son quand un encodeur est relancé.
    pub video_position: Arc<AtomicU64>,
    pub audio_position: Arc<AtomicU64>,
    /// Partie de fichier en cours (1, puis 2 après une relance…).
    pub part: AtomicU32,
    outputs: Mutex<Vec<PathBuf>>,
    finishing: AtomicBool,
    done: AtomicBool,
    pub audio_last_sound_hns: AtomicU64,
    audio_peak_bits: AtomicU32,
    target: Mutex<Option<WindowInfo>>,
    target_generation: AtomicU64,
    failed: AtomicBool,
    error: Mutex<Option<String>>,
    stop_at_hns: AtomicU64,
    /// Fils de capture bloqués à l'arrêt : l'écriture se termine sans eux.
    abandoned: AtomicBool,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(SourceState::Searching.to_u8()),
            frames_total: AtomicU64::new(0),
            frames_new: AtomicU64::new(0),
            frames_dropped: AtomicU64::new(0),
            video_backlog: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            video_position: Arc::new(AtomicU64::new(0)),
            audio_position: Arc::new(AtomicU64::new(0)),
            part: AtomicU32::new(1),
            outputs: Mutex::new(Vec::new()),
            finishing: AtomicBool::new(false),
            done: AtomicBool::new(false),
            audio_last_sound_hns: AtomicU64::new(0),
            audio_peak_bits: AtomicU32::new(0),
            target: Mutex::new(None),
            target_generation: AtomicU64::new(0),
            failed: AtomicBool::new(false),
            error: Mutex::new(None),
            stop_at_hns: AtomicU64::new(0),
            abandoned: AtomicBool::new(false),
        }
    }

    /// État affiché : un échec prime, puis l'arrêt en cours ou terminé.
    pub fn state(&self) -> SourceState {
        let state = SourceState::from_u8(self.state.load(Ordering::Relaxed));
        if state == SourceState::Failed {
            state
        } else if self.done.load(Ordering::SeqCst) {
            SourceState::Finished
        } else if self.finishing.load(Ordering::SeqCst) {
            SourceState::Finishing
        } else {
            state
        }
    }

    pub fn mark_finishing(&self) {
        self.finishing.store(true, Ordering::SeqCst);
    }

    pub fn mark_done(&self) {
        self.done.store(true, Ordering::SeqCst);
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    /// Fichiers de la source, dans l'ordre des parties.
    pub fn outputs(&self) -> Vec<PathBuf> {
        lock(&self.outputs).clone()
    }

    pub fn push_output(&self, path: PathBuf) {
        lock(&self.outputs).push(path);
    }

    pub fn remove_output(&self, path: &std::path::Path) {
        lock(&self.outputs).retain(|p| p != path);
    }

    /// Un échec est définitif : il n'est jamais remplacé par un autre état.
    pub fn set_state(&self, state: SourceState) {
        if !self.has_failed() {
            self.state.store(state.to_u8(), Ordering::Relaxed);
        }
    }

    pub fn set_target(&self, target: Option<WindowInfo>) {
        *lock(&self.target) = target;
        self.target_generation.fetch_add(1, Ordering::SeqCst);
    }

    pub fn target_pid(&self) -> Option<u32> {
        lock(&self.target).as_ref().map(|w| w.pid)
    }

    pub fn target_handle(&self) -> Option<polyptyque_core::platform::WindowHandle> {
        lock(&self.target).as_ref().map(|w| w.handle)
    }

    pub fn target_generation(&self) -> u64 {
        self.target_generation.load(Ordering::SeqCst)
    }

    pub fn fail(&self, message: String) {
        log::error!("{message}");
        let mut error = lock(&self.error);
        if !self.failed.swap(true, Ordering::SeqCst) {
            *error = Some(message);
        }
        self.state.store(SourceState::Failed.to_u8(), Ordering::Relaxed);
    }

    pub fn has_failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }

    /// Problème non bloquant, affiché à l'utilisateur.
    pub fn note_error(&self, message: String) {
        if !self.has_failed() {
            *lock(&self.error) = Some(message);
        }
    }

    pub fn clear_note(&self) {
        if !self.has_failed() {
            *lock(&self.error) = None;
        }
    }

    pub fn error(&self) -> Option<String> {
        lock(&self.error).clone()
    }

    pub fn request_stop(&self, at_hns: u64) {
        self.stop_at_hns.store(at_hns.max(1), Ordering::SeqCst);
    }

    pub fn stop_at(&self) -> Option<u64> {
        match self.stop_at_hns.load(Ordering::SeqCst) {
            0 => None,
            at => Some(at),
        }
    }

    pub fn abandon_capture(&self) {
        self.abandoned.store(true, Ordering::SeqCst);
    }

    pub fn capture_abandoned(&self) -> bool {
        self.abandoned.load(Ordering::SeqCst)
    }

    /// Les nombres flottants positifs se comparent comme leurs bits.
    pub fn record_peak(&self, peak: f32) {
        if peak.is_finite() && peak > 0.0 {
            self.audio_peak_bits.fetch_max(peak.to_bits(), Ordering::Relaxed);
        }
    }

    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.audio_peak_bits.swap(0, Ordering::Relaxed))
    }
}

/// Relances d'un fil de capture après une erreur interne, au-delà
/// desquelles la source passe en erreur.
const MAX_TASK_RESTARTS: u32 = 3;
/// Un fil qui a tourné sans erreur aussi longtemps repart avec un compteur
/// de relances à zéro.
const RESTART_FORGET_AFTER: std::time::Duration = std::time::Duration::from_secs(60);

/// Exécute `task` en le relançant s'il s'arrête sur une erreur interne
/// (le détail est déjà dans le journal, par le gestionnaire de paniques) :
/// une erreur dans une source ne passe jamais inaperçue et ne touche pas les
/// autres. `task` doit reprendre là où il s'était arrêté.
pub(crate) fn run_protected(shared: &Shared, name: &str, what: &str, mut task: impl FnMut()) {
    let mut restarts = 0;
    loop {
        let started = std::time::Instant::now();
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut task)).is_ok() {
            return;
        }
        // Des erreurs isolées, espacées, ne s'additionnent pas.
        if started.elapsed() >= RESTART_FORGET_AFTER {
            restarts = 0;
        }
        restarts += 1;
        if restarts > MAX_TASK_RESTARTS || shared.stop_at().is_some() {
            shared.fail(tr::tr!("[{}] erreur interne répétée dans {} (voir le journal)", name, what));
            return;
        }
        log::warn!("[{name}] erreur interne dans {what} : relance ({restarts}/{MAX_TASK_RESTARTS})");
        shared.note_error(tr::tr!("Erreur interne dans {} : relancée", what));
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_is_sticky() {
        let shared = Shared::new();
        shared.set_state(SourceState::Connected);
        shared.fail("premier".into());
        shared.fail("second".into());
        shared.set_state(SourceState::Connected);
        assert_eq!(shared.state(), SourceState::Failed);
        assert_eq!(shared.error().as_deref(), Some("premier"));
    }

    #[test]
    fn stop_phases_are_reported() {
        let shared = Shared::new();
        shared.set_state(SourceState::Connected);
        shared.mark_finishing();
        assert_eq!(shared.state(), SourceState::Finishing);
        shared.mark_done();
        assert_eq!(shared.state(), SourceState::Finished);
        shared.fail("échec".into());
        assert_eq!(shared.state(), SourceState::Failed);
    }

    #[test]
    fn protected_task_is_restarted_then_given_up() {
        let shared = Shared::new();
        let mut runs = 0;
        run_protected(&shared, "essai", "la capture", || {
            runs += 1;
            if runs < 3 {
                panic!("erreur simulée");
            }
        });
        assert_eq!(runs, 3);
        assert!(!shared.has_failed());
        assert!(shared.error().is_some_and(|e| e.contains("relancée")));

        let shared = Shared::new();
        let mut runs = 0;
        run_protected(&shared, "essai", "la capture", || {
            runs += 1;
            panic!("erreur simulée");
        });
        assert_eq!(runs, MAX_TASK_RESTARTS + 1);
        assert!(shared.has_failed());
    }

    #[test]
    fn peak_is_taken_once() {
        let shared = Shared::new();
        shared.record_peak(0.2);
        shared.record_peak(0.7);
        shared.record_peak(0.4);
        assert_eq!(shared.take_peak(), 0.7);
        assert_eq!(shared.take_peak(), 0.0);
    }
}
