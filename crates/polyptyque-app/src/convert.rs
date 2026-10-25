//! File de conversion MKV → MP4 et de réparation des MKV interrompus,
//! traitée un fichier à la fois sur un fil dédié : l'interface reste libre,
//! et un enregistrement peut démarrer pendant une conversion.

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use polyptyque_core::platform::Platform;
use polyptyque_engine::{probe, remux};
use tr::tr;

/// Marge d'espace disque gardée libre en plus de la taille du fichier.
const DISK_MARGIN: u64 = 1_000_000_000;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// Ce qu'il faut faire d'un fichier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    /// Copie MP4 (ou M4A) à côté du MKV.
    Convert,
    /// Reconstruction de la durée et de l'index d'un MKV interrompu, sur place.
    Repair,
}

pub enum ConversionEvent {
    Started {
        input: PathBuf,
        kind: JobKind,
    },
    Progress(f32),
    Finished {
        input: PathBuf,
        result: Result<PathBuf, String>,
    },
    /// File vide, y compris après une annulation.
    Idle,
}

pub struct Converter {
    jobs: Option<Sender<(u64, PathBuf, JobKind)>>,
    /// Numéro du dernier fichier demandé.
    last_job: Cell<u64>,
    control: Arc<CancelControl>,
    worker: Option<JoinHandle<()>>,
}

/// Annulation : tous les fichiers demandés jusqu'au numéro `up_to` sont
/// abandonnés ; ceux demandés ensuite sont convertis normalement.
#[derive(Default)]
struct CancelControl {
    up_to: Mutex<u64>,
    /// Interrompt la conversion en cours.
    current: AtomicBool,
}

impl CancelControl {
    /// Prépare la conversion du fichier `job` ; faux s'il est annulé.
    fn begin(&self, job: u64) -> bool {
        let up_to = lock(&self.up_to);
        if job <= *up_to {
            return false;
        }
        self.current.store(false, Ordering::SeqCst);
        true
    }

    fn cancel(&self, up_to: u64) {
        let mut limit = lock(&self.up_to);
        *limit = (*limit).max(up_to);
        self.current.store(true, Ordering::SeqCst);
    }
}

impl Converter {
    pub fn start(platform: Arc<dyn Platform>, on_event: impl Fn(ConversionEvent) + Send + 'static) -> Self {
        let (jobs, queue) = mpsc::channel();
        let control = Arc::new(CancelControl::default());
        let worker_control = control.clone();
        let worker = thread::Builder::new()
            .name("conversion-mp4".into())
            .spawn(move || run(platform, queue, worker_control, on_event))
            .map_err(|e| log::error!("conversion MP4 indisponible : {e}"))
            .ok();
        Self { jobs: Some(jobs), last_job: Cell::new(0), control, worker }
    }

    pub fn enqueue(&self, file: PathBuf, kind: JobKind) {
        if let Some(jobs) = &self.jobs {
            let job = self.last_job.get() + 1;
            self.last_job.set(job);
            let _ = jobs.send((job, file, kind));
        }
    }

    /// Interrompt le fichier en cours (le fichier partiel est effacé) et
    /// abandonne ceux qui attendent. Les fichiers demandés ensuite ne sont
    /// pas concernés.
    pub fn cancel_all(&self) {
        self.control.cancel(self.last_job.get());
    }
}

impl Drop for Converter {
    fn drop(&mut self) {
        self.control.cancel(u64::MAX);
        self.jobs.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run(
    platform: Arc<dyn Platform>,
    queue: Receiver<(u64, PathBuf, JobKind)>,
    control: Arc<CancelControl>,
    on_event: impl Fn(ConversionEvent),
) {
    while let Ok(first) = queue.recv() {
        let mut next = Some(first);
        while let Some((job, input, kind)) = next {
            if control.begin(job) {
                on_event(ConversionEvent::Started { input: input.clone(), kind });
                let result = convert_one(platform.as_ref(), &input, kind, &control.current, &on_event)
                    .map_err(|e| format!("{e:#}"));
                on_event(ConversionEvent::Finished { input, result });
            }
            next = queue.try_recv().ok();
        }
        on_event(ConversionEvent::Idle);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn convert_one(
    platform: &dyn Platform,
    input: &Path,
    kind: JobKind,
    cancel: &AtomicBool,
    on_event: &dyn Fn(ConversionEvent),
) -> Result<PathBuf> {
    let ffmpeg = probe::locate_ffmpeg()
        .ok_or_else(|| anyhow!(tr!("FFmpeg introuvable : ffmpeg.exe doit se trouver à côté de polyptyque.exe")))?;
    let size = fs::metadata(input).map_err(|e| anyhow!(tr!("fichier inaccessible : {}", e)))?.len();
    let dir = input.parent().unwrap_or(Path::new("."));
    if let Some(free) = platform.free_disk_space(dir) {
        if free < size.saturating_add(DISK_MARGIN) {
            bail!(tr!("espace disque insuffisant pour la copie"));
        }
    }
    let mut last = Instant::now() - PROGRESS_INTERVAL;
    let mut progress = |progress: f32| {
        if last.elapsed() >= PROGRESS_INTERVAL || progress >= 1.0 {
            last = Instant::now();
            on_event(ConversionEvent::Progress(progress));
        }
    };
    match kind {
        JobKind::Convert => remux::convert(platform, &ffmpeg, input, &mut progress, cancel),
        JobKind::Repair => remux::repair(platform, &ffmpeg, input, &mut progress, cancel),
    }
}
