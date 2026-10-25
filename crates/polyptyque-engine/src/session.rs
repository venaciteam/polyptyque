//! Session d'enregistrement : départ commun des sources d'une scène sur une
//! même horloge, ajout ou arrêt d'une source en cours de route, arrêt propre.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use crossbeam_channel::unbounded;
use polyptyque_core::config::{new_id, Settings, Source, CHANNELS, SAMPLE_RATE};
use polyptyque_core::encoder::{bytes_per_hour, EncoderChoice};
use polyptyque_core::level::RECENT_SOUND_HNS;
use polyptyque_core::naming::{self, NameParts};
use polyptyque_core::platform::{Nv12Frame, Platform};
use tr::tr;

use crate::audio_task::{AudioInput, AudioTask};
use crate::output::{clock, OutputPlan, OutputTask};
use crate::shared::{Shared, SourceState};
use crate::video_task::VideoTask;

/// À l'arrêt, délai accordé aux fils de capture pour se terminer.
const CAPTURE_JOIN_TIMEOUT: Duration = Duration::from_secs(10);

pub struct SessionPlan {
    pub scene_name: String,
    /// Sources démarrées ensemble, au même instant.
    pub sources: Vec<Source>,
    pub settings: Settings,
    pub encoder: EncoderChoice,
    pub ffmpeg: PathBuf,
    /// Dossier des journaux de cette session.
    pub log_dir: PathBuf,
    /// Dossier de secours si le dossier d'enregistrement devient
    /// inaccessible en cours de route.
    pub fallback_dir: PathBuf,
    /// Micro enregistré dans un fichier séparé, pendant toute la session.
    pub microphone: Option<MicrophonePlan>,
}

pub struct MicrophonePlan {
    /// Périphérique (`None` : le micro par défaut de Windows).
    pub device_id: Option<String>,
}

/// Identifiant de la piste du micro dans les états et les bilans.
pub const MICROPHONE_ID: &str = "micro";

/// Ce que capture une exécution.
enum Capture<'a> {
    /// Une fenêtre et le son de son programme.
    Window(&'a Source),
    /// Un micro seul.
    Microphone(Option<String>),
}

#[derive(Debug, Clone)]
pub struct SourceStatus {
    pub id: String,
    pub name: String,
    pub state: SourceState,
    pub frames_total: u64,
    pub frames_new: u64,
    pub frames_dropped: u64,
    /// Niveau crête depuis le dernier appel (0 à 1).
    pub audio_level: f32,
    pub audio_recent: bool,
    /// Retard de l'encodeur, en secondes d'images en attente.
    pub backlog_seconds: f32,
    /// Partie de fichier en cours : plus de 1 après une relance de
    /// l'encodeur.
    pub part: u32,
    /// Fichier en cours d'écriture.
    pub output: PathBuf,
    /// Taille de toutes les parties.
    pub file_size: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SourceSummary {
    pub id: String,
    pub name: String,
    /// Fichiers produits, dans l'ordre (plusieurs si l'encodeur a été relancé).
    pub outputs: Vec<PathBuf>,
    pub file_size: u64,
    pub frames_total: u64,
    pub frames_dropped: u64,
    pub error: Option<String>,
    pub log: PathBuf,
}

struct SourceRuntime {
    /// Ordre de démarrage dans la session.
    order: usize,
    id: String,
    name: String,
    log: PathBuf,
    shared: Arc<Shared>,
    capture_threads: Vec<JoinHandle<()>>,
    output_thread: Option<JoinHandle<()>>,
    /// Fil qui finalise la source une fois son arrêt demandé.
    finisher: Option<JoinHandle<()>>,
}

impl SourceRuntime {
    /// La source enregistre (ni arrêtée, ni en échec définitif).
    fn recording(&self) -> bool {
        self.finisher.is_none() && !self.shared.is_done() && !self.shared.has_failed()
    }

    /// Demande l'arrêt et confie la finalisation à un fil séparé : rien ne
    /// bloque l'appelant.
    fn begin_stop(&mut self, stop_at: u64) {
        if self.finisher.is_some() {
            return;
        }
        self.shared.request_stop(stop_at);
        self.shared.mark_finishing();
        let captures = std::mem::take(&mut self.capture_threads);
        let output = self.output_thread.take();
        let shared = self.shared.clone();
        let name = self.name.clone();
        let finish = move || {
            let deadline = Instant::now() + CAPTURE_JOIN_TIMEOUT;
            for handle in captures {
                if !join_until(handle, deadline) {
                    // Fil bloqué (pilote, périphérique audio…) : on ferme le
                    // fichier sans lui plutôt que de rester suspendu.
                    log::warn!("[{name}] capture bloquée à l'arrêt : fichier fermé sans elle");
                    shared.abandon_capture();
                }
            }
            if let Some(output) = output {
                let _ = output.join();
            }
            shared.mark_done();
            log::info!("[{name}] fichier(s) finalisé(s) ; {}", summary_line(&shared));
        };
        match thread::Builder::new().name(format!("{}-finalisation", self.name)).spawn(finish) {
            Ok(handle) => self.finisher = Some(handle),
            Err(e) => log::error!("[{}] finalisation impossible : {e}", self.name),
        }
    }

    fn status(&self, now: u64, fps: f32) -> SourceStatus {
        let shared = &self.shared;
        let outputs = shared.outputs();
        let last_sound = shared.audio_last_sound_hns.load(Ordering::Relaxed);
        SourceStatus {
            id: self.id.clone(),
            name: self.name.clone(),
            state: shared.state(),
            frames_total: shared.frames_total.load(Ordering::Relaxed),
            frames_new: shared.frames_new.load(Ordering::Relaxed),
            frames_dropped: shared.frames_dropped.load(Ordering::Relaxed),
            audio_level: shared.take_peak(),
            audio_recent: last_sound != 0 && now.saturating_sub(last_sound) < RECENT_SOUND_HNS,
            backlog_seconds: shared.video_backlog.load(Ordering::Relaxed) as f32 / fps,
            part: shared.part.load(Ordering::Relaxed),
            output: outputs.last().cloned().unwrap_or_default(),
            file_size: total_size(&outputs),
            error: shared.error(),
        }
    }

    fn summary(&self) -> SourceSummary {
        let outputs = self.shared.outputs();
        SourceSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            file_size: total_size(&outputs),
            outputs,
            frames_total: self.shared.frames_total.load(Ordering::Relaxed),
            frames_dropped: self.shared.frames_dropped.load(Ordering::Relaxed),
            error: self.shared.error(),
            log: self.log.clone(),
        }
    }
}

pub struct Session {
    platform: Arc<dyn Platform>,
    scene_name: String,
    settings: Settings,
    encoder: EncoderChoice,
    ffmpeg: PathBuf,
    log_dir: PathBuf,
    fallback_dir: PathBuf,
    /// Préfixe des tubes nommés, unique sur la machine.
    tag: String,
    /// Source par source, la dernière exécution (une source arrêtée puis
    /// relancée en a plusieurs : les précédentes sont dans `retired`).
    sources: Vec<SourceRuntime>,
    retired: Vec<SourceRuntime>,
    microphone: Option<SourceRuntime>,
    started_count: usize,
    taken: HashSet<PathBuf>,
    t0_hns: u64,
    started: Instant,
}

impl Session {
    pub fn start(platform: Arc<dyn Platform>, plan: SessionPlan) -> Result<Session> {
        let settings = &plan.settings;
        fs::create_dir_all(&settings.output_dir)
            .with_context(|| tr!("création du dossier {}", settings.output_dir.display()))?;
        fs::create_dir_all(&plan.log_dir).with_context(|| tr!("création du dossier {}", plan.log_dir.display()))?;
        platform.begin_recording_session();

        let t0_hns = platform.now_hns();
        let mut session = Session {
            tag: format!("polyptyque-{}-{}", std::process::id(), new_id()),
            platform,
            scene_name: plan.scene_name,
            settings: plan.settings,
            encoder: plan.encoder,
            ffmpeg: plan.ffmpeg,
            log_dir: plan.log_dir,
            fallback_dir: plan.fallback_dir,
            sources: Vec::new(),
            retired: Vec::new(),
            microphone: None,
            started_count: 0,
            taken: HashSet::new(),
            t0_hns,
            started: Instant::now(),
        };
        // Départ commun : toutes les sources partagent la même origine.
        let (date, time) = naming::file_name_parts();
        for source in &plan.sources {
            session.launch(source, t0_hns, &date, &time);
        }
        if let Some(microphone) = plan.microphone {
            let runtime = session.spawn_runtime(
                MICROPHONE_ID,
                &tr!("Micro"),
                Capture::Microphone(microphone.device_id),
                t0_hns,
                &date,
                &time,
            );
            session.microphone = Some(runtime);
        }
        Ok(session)
    }

    /// Démarre une source de plus pendant la session (son fichier commence
    /// maintenant). Refusé si elle enregistre déjà.
    pub fn add_source(&mut self, source: &Source) -> Result<()> {
        if self.is_recording(&source.id) {
            bail!(tr!("« {} » enregistre déjà", source.name));
        }
        let (date, time) = naming::file_name_parts();
        let origin = self.platform.now_hns();
        self.launch(source, origin, &date, &time);
        Ok(())
    }

    /// Arrête une seule source ; son fichier est finalisé en arrière-plan.
    /// Renvoie faux si elle n'enregistrait pas.
    pub fn stop_source(&mut self, id: &str) -> bool {
        let stop_at = self.platform.now_hns();
        match self.sources.iter_mut().find(|s| s.id == id && s.recording()) {
            Some(source) => {
                source.begin_stop(stop_at);
                true
            }
            None => false,
        }
    }

    /// Fenêtre que suit la source `id` pendant son enregistrement.
    pub fn tracked_window(&self, id: &str) -> Option<polyptyque_core::platform::WindowHandle> {
        self.sources.iter().find(|s| s.id == id && s.recording()).and_then(|s| s.shared.target_handle())
    }

    pub fn is_recording(&self, id: &str) -> bool {
        self.sources.iter().any(|s| s.id == id && s.recording())
    }

    /// Fichiers encore en cours d'écriture (micro compris).
    pub fn files_in_progress(&self) -> Vec<PathBuf> {
        self.sources
            .iter()
            .chain(self.retired.iter())
            .chain(self.microphone.iter())
            .filter(|s| !s.shared.is_done())
            .flat_map(|s| s.shared.outputs())
            .collect()
    }

    /// Tous les fichiers de la session, dans l'ordre de démarrage des
    /// sources (parties, sources arrêtées en cours de route et micro compris).
    pub fn outputs(&self) -> Vec<PathBuf> {
        let mut all: Vec<&SourceRuntime> =
            self.sources.iter().chain(self.retired.iter()).chain(self.microphone.iter()).collect();
        all.sort_by_key(|s| s.order);
        all.into_iter().flat_map(|s| s.shared.outputs()).collect()
    }

    /// Nombre de sources en train d'enregistrer.
    pub fn recording_count(&self) -> usize {
        self.sources.iter().filter(|s| s.recording()).count()
    }

    fn launch(&mut self, source: &Source, origin_hns: u64, date: &str, time: &str) {
        let runtime = self.spawn_runtime(&source.id, &source.name, Capture::Window(source), origin_hns, date, time);
        if let Some(index) = self.sources.iter().position(|s| s.id == runtime.id) {
            let mut previous = std::mem::replace(&mut self.sources[index], runtime);
            // Exécution précédente en échec : ses fils sont finalisés aussi.
            previous.begin_stop(origin_hns);
            self.retired.push(previous);
        } else {
            self.sources.push(runtime);
        }
    }

    /// Démarre la capture, l'encodage et le fichier d'une source (ou du
    /// micro), avec `origin_hns` pour origine des temps.
    fn spawn_runtime(
        &mut self,
        id: &str,
        name: &str,
        capture: Capture,
        origin_hns: u64,
        date: &str,
        time: &str,
    ) -> SourceRuntime {
        let settings = &self.settings;
        let video = &settings.video;
        let has_video = matches!(capture, Capture::Window(_));
        let order = self.started_count;
        self.started_count += 1;
        let stem = naming::render(
            &settings.filename_template,
            &NameParts { date, time, scene: &self.scene_name, source: name },
        );
        let output =
            naming::unique_path(&settings.output_dir, &stem, if has_video { "mkv" } else { "mka" }, &self.taken);
        self.taken.insert(output.clone());
        let log = self.log_dir.join(format!("{:02}-{}.ffmpeg.log", order + 1, naming::sanitize(name)));
        let shared = Arc::new(Shared::new());
        let (video_tx, video_rx) = unbounded::<Nv12Frame>();
        let (audio_tx, audio_rx) = unbounded::<Vec<u8>>();

        let output_task = OutputTask {
            platform: self.platform.clone(),
            plan: OutputPlan {
                video: has_video,
                ffmpeg: self.ffmpeg.clone(),
                encoder: self.encoder,
                width: video.width,
                height: video.height,
                fps: video.fps,
                video_bitrate_kbps: video.bitrate_kbps,
                audio_bitrate_kbps: settings.audio.bitrate_kbps,
                title: name.to_string(),
                first_output: output.clone(),
                fallback_dir: self.fallback_dir.clone(),
                log: log.clone(),
                pipe_prefix: format!("{}-{order}", self.tag),
            },
            shared: shared.clone(),
            video: has_video.then_some(video_rx),
            audio: audio_rx,
        };
        let mut runtime = SourceRuntime {
            order,
            id: id.to_string(),
            name: name.to_string(),
            log,
            shared: shared.clone(),
            capture_threads: Vec::new(),
            output_thread: None,
            finisher: None,
        };
        let output_shared = shared.clone();
        let output_name = name.to_string();
        let encode = move || {
            // L'écriture ne se relance pas d'elle-même : une erreur interne
            // arrête la source, visiblement, sans toucher aux autres.
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| output_task.run())).is_err() {
                output_shared.fail(format!("[{output_name}] erreur interne de l'encodage (voir le journal)"));
            }
        };
        match thread::Builder::new().name(format!("{name}-encodage")).spawn(encode) {
            Ok(handle) => runtime.output_thread = Some(handle),
            Err(e) => shared.fail(tr!("fil d'encodage : {}", e)),
        }
        if !shared.has_failed() {
            let input = match capture {
                Capture::Window(source) => {
                    let video_task = VideoTask {
                        platform: self.platform.clone(),
                        source: source.clone(),
                        shared: shared.clone(),
                        queue: video_tx,
                        t0_hns: origin_hns,
                        width: video.width,
                        height: video.height,
                        fps: video.fps,
                    };
                    match thread::Builder::new().name(format!("{name}-video")).spawn(move || video_task.run()) {
                        Ok(handle) => runtime.capture_threads.push(handle),
                        Err(e) => shared.fail(tr!("fil vidéo : {}", e)),
                    }
                    AudioInput::Window
                }
                Capture::Microphone(device) => AudioInput::Microphone(device),
            };
            let audio_task = AudioTask {
                platform: self.platform.clone(),
                name: name.to_string(),
                input,
                shared: shared.clone(),
                queue: audio_tx,
                t0_hns: origin_hns,
            };
            match thread::Builder::new().name(format!("{name}-audio")).spawn(move || audio_task.run()) {
                Ok(handle) => runtime.capture_threads.push(handle),
                Err(e) => shared.fail(tr!("fil son : {}", e)),
            }
        }
        log::info!("[{name}] enregistrement vers {}", output.display());
        runtime
    }

    pub fn elapsed(&self) -> Duration {
        Duration::from_nanos(self.platform.now_hns().saturating_sub(self.t0_hns) * 100)
    }

    pub fn started_at(&self) -> Instant {
        self.started
    }

    pub fn output_dir(&self) -> &Path {
        &self.settings.output_dir
    }

    /// Débit total estimé des sources qui enregistrent (micro compris), en
    /// octets par heure.
    pub fn bytes_per_hour(&self) -> u64 {
        let audio = self.settings.audio.bitrate_kbps;
        let microphone = if self.microphone.is_some() { bytes_per_hour(0, audio) } else { 0 };
        bytes_per_hour(self.settings.video.bitrate_kbps, audio) * self.recording_count() as u64 + microphone
    }

    /// Dossier où s'écrivent les fichiers : le dossier choisi, ou celui de
    /// secours s'il est devenu inaccessible.
    pub fn current_output_dir(&self) -> PathBuf {
        self.files_in_progress()
            .last()
            .and_then(|f| f.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| self.settings.output_dir.clone())
    }

    pub fn free_space(&self) -> Option<u64> {
        self.platform.free_disk_space(&self.current_output_dir())
    }

    pub fn status(&mut self) -> Vec<SourceStatus> {
        let now = self.platform.now_hns();
        let fps = self.settings.video.fps.max(1) as f32;
        self.sources.iter().map(|source| source.status(now, fps)).collect()
    }

    /// État du micro, s'il est enregistré.
    pub fn microphone_status(&self) -> Option<SourceStatus> {
        let now = self.platform.now_hns();
        self.microphone.as_ref().map(|micro| micro.status(now, 1.0))
    }

    /// Arrête toutes les sources au même instant et attend que chaque
    /// fichier soit correctement fermé (y compris ceux des sources arrêtées
    /// une à une auparavant).
    pub fn stop(mut self) -> Vec<SourceSummary> {
        let stop_at = self.platform.now_hns();
        for source in self.sources.iter_mut().chain(self.microphone.iter_mut()) {
            source.begin_stop(stop_at);
        }
        for source in self.sources.iter_mut().chain(self.retired.iter_mut()).chain(self.microphone.iter_mut()) {
            if let Some(finisher) = source.finisher.take() {
                let _ = finisher.join();
            }
        }
        self.platform.end_recording_session();

        let mut all: Vec<&SourceRuntime> =
            self.sources.iter().chain(self.retired.iter()).chain(self.microphone.iter()).collect();
        all.sort_by_key(|s| s.order);
        all.into_iter().map(SourceRuntime::summary).collect()
    }
}

/// Bilan d'une source pour le journal : durée, fichiers, taille, images.
fn summary_line(shared: &Shared) -> String {
    let outputs = shared.outputs();
    let bytes_per_second = CHANNELS as u64 * 4 * SAMPLE_RATE as u64;
    let seconds = shared.audio_position.load(Ordering::SeqCst) / bytes_per_second;
    let bytes = total_size(&outputs) as f64;
    let size = if bytes >= 1e9 { format!("{:.1} Go", bytes / 1e9) } else { format!("{:.0} Mo", bytes / 1e6) };
    let size = size.replace('.', ",");
    let mut line = format!("bilan : {} enregistrées, {} fichier(s), {size}", clock(seconds), outputs.len());
    let frames = shared.frames_total.load(Ordering::Relaxed);
    if frames > 0 {
        line.push_str(&format!(
            ", images : {frames} écrites, {} perdues, {} reçues de la fenêtre",
            shared.frames_dropped.load(Ordering::Relaxed),
            shared.frames_new.load(Ordering::Relaxed)
        ));
    }
    if let Some(error) = shared.error() {
        line.push_str(&format!(" ; dernier message : {error}"));
    }
    line
}

fn total_size(outputs: &[PathBuf]) -> u64 {
    outputs.iter().filter_map(|p| fs::metadata(p).ok()).map(|m| m.len()).sum()
}

/// Attend un fil jusqu'à l'échéance. Renvoie faux s'il ne s'est pas
/// terminé (il est alors abandonné).
fn join_until(handle: JoinHandle<()>, deadline: Instant) -> bool {
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let _ = handle.join();
    true
}
