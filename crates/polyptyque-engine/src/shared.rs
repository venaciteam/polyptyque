//! État partagé entre les fils d'une source et l'interface.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use polyptyque_core::config::{CHANNELS, SAMPLE_RATE};
use polyptyque_core::platform::{WindowHandle, WindowInfo};

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
    /// Octets de son mis en file depuis le départ : moins `audio_position`,
    /// le son en attente d'encodage.
    pub audio_queued: AtomicU64,
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
    /// Taille de toutes les parties, relevée par un fil à part : l'interface
    /// ne touche jamais au disque (dossier réseau, disque qui ne répond plus).
    pub file_size: AtomicU64,
    /// Le fil d'encodage est terminé (fichiers fermés ou abandonnés).
    encoding_finished: AtomicBool,
    /// Taille relevée une dernière fois après la fin de l'encodage.
    sizes_settled: AtomicBool,
    /// Signe de vie du fil d'encodage, en millisecondes depuis `created` :
    /// un fil bloqué sur le disque n'empêche pas l'arrêt de la session.
    created: std::time::Instant,
    encoding_heartbeat_ms: AtomicU64,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(SourceState::Searching.to_u8()),
            frames_total: AtomicU64::new(0),
            frames_new: AtomicU64::new(0),
            frames_dropped: AtomicU64::new(0),
            video_backlog: AtomicU64::new(0),
            audio_queued: AtomicU64::new(0),
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
            file_size: AtomicU64::new(0),
            encoding_finished: AtomicBool::new(false),
            sizes_settled: AtomicBool::new(false),
            created: std::time::Instant::now(),
            encoding_heartbeat_ms: AtomicU64::new(0),
        }
    }

    pub fn mark_encoding_finished(&self) {
        self.encoding_finished.store(true, Ordering::SeqCst);
    }

    pub fn encoding_finished(&self) -> bool {
        self.encoding_finished.load(Ordering::SeqCst)
    }

    pub fn mark_sizes_settled(&self) {
        self.sizes_settled.store(true, Ordering::SeqCst);
    }

    pub fn sizes_settled(&self) -> bool {
        self.sizes_settled.load(Ordering::SeqCst)
    }

    /// Le fil d'encodage est en vie (appelé à chaque tour de ses attentes).
    pub fn encoding_beat(&self) {
        self.encoding_heartbeat_ms.store(self.created.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    /// Temps écoulé depuis le dernier signe de vie du fil d'encodage.
    pub fn encoding_silence(&self) -> std::time::Duration {
        let last = std::time::Duration::from_millis(self.encoding_heartbeat_ms.load(Ordering::Relaxed));
        self.created.elapsed().saturating_sub(last)
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

    /// Son en attente d'encodage, en secondes.
    pub fn audio_backlog_seconds(&self) -> f32 {
        let queued = self.audio_queued.load(Ordering::Relaxed);
        let pending = queued.saturating_sub(self.audio_position.load(Ordering::Relaxed));
        pending as f32 / (CHANNELS as u64 * 4 * SAMPLE_RATE as u64) as f32
    }

    /// Retard de l'encodeur (image ou son) au-delà de [`MAX_BACKLOG_SECONDS`] :
    /// il ne rattrapera pas. Plutôt que d'accumuler sans fin (mémoire, puis
    /// finalisation interminable), la source s'arrête ; ce qui est déjà
    /// capturé est encodé et le fichier fermé proprement. Renvoie vrai si la
    /// source vient de passer en erreur.
    pub fn check_backlog(&self, name: &str, seconds: f32) -> bool {
        if seconds < MAX_BACKLOG_SECONDS as f32 || self.has_failed() || self.stop_at().is_some() {
            return false;
        }
        log::error!("[{name}] l'encodeur ne suit plus : {seconds:.0} s de retard, source arrêtée");
        self.fail(tr::tr!(
            "L'encodeur ne suit plus ({} min de retard) : enregistrement de cette source arrêté, fichier conservé. \
             Réduire la définition, la cadence ou le débit",
            MAX_BACKLOG_SECONDS / 60
        ));
        true
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

/// Fenêtres suivies par les sources d'une session : une fenêtre n'est
/// jamais enregistrée par deux sources à la fois.
#[derive(Default)]
pub(crate) struct WindowClaims {
    /// Par exécution (rang de démarrage dans la session) : identifiant de
    /// la source et fenêtre suivie.
    claims: Mutex<HashMap<usize, (String, WindowHandle)>>,
}

impl WindowClaims {
    /// Fenêtres suivies par les autres sources que `source_id`. Une
    /// exécution précédente de la même source (relancée) ne compte pas.
    pub fn taken_by_others(&self, source_id: &str) -> HashSet<WindowHandle> {
        lock(&self.claims).values().filter(|(id, _)| id != source_id).map(|(_, handle)| *handle).collect()
    }

    /// Réserve `handle` pour l'exécution `run`. Faux si une autre source
    /// vient de la prendre.
    pub fn claim(&self, run: usize, source_id: &str, handle: WindowHandle) -> bool {
        let mut claims = lock(&self.claims);
        if claims.iter().any(|(r, (id, h))| *r != run && id != source_id && *h == handle) {
            return false;
        }
        claims.insert(run, (source_id.to_string(), handle));
        true
    }

    pub fn release(&self, run: usize) {
        lock(&self.claims).remove(&run);
    }
}

/// Retard d'encodage (image ou son) au-delà duquel la source s'arrête.
pub(crate) const MAX_BACKLOG_SECONDS: u64 = 300;

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
    fn endless_backlog_stops_the_source() {
        let second = CHANNELS as u64 * 4 * SAMPLE_RATE as u64;
        let shared = Shared::new();
        shared.audio_queued.store(10 * second, Ordering::Relaxed);
        shared.audio_position.store(4 * second, Ordering::Relaxed);
        assert_eq!(shared.audio_backlog_seconds(), 6.0);
        assert!(!shared.check_backlog("essai", shared.audio_backlog_seconds()));
        // Arrêt demandé : la file se vide, rien ne s'arrête plus.
        let stopping = Shared::new();
        stopping.request_stop(1);
        assert!(!stopping.check_backlog("essai", 1_000.0));
        shared.audio_queued.store((MAX_BACKLOG_SECONDS + 10) * second, Ordering::Relaxed);
        assert!(shared.check_backlog("essai", shared.audio_backlog_seconds()));
        assert!(shared.has_failed());
        assert!(shared.error().is_some_and(|e| e.contains("ne suit plus")));
    }

    #[test]
    fn a_window_is_claimed_by_one_source_only() {
        let claims = WindowClaims::default();
        assert!(claims.claim(0, "a", 7));
        assert!(!claims.claim(1, "b", 7));
        assert_eq!(claims.taken_by_others("b"), HashSet::from([7]));
        assert!(claims.taken_by_others("a").is_empty());
        // La même source relancée (nouvelle exécution) peut la reprendre.
        assert!(claims.claim(2, "a", 7));
        claims.release(0);
        claims.release(2);
        assert!(claims.claim(1, "b", 7));
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
