//! Niveau sonore hors enregistrement.
//!
//! - Sources : chaque source écoute le son du programme de sa fenêtre, pour
//!   que sa carte montre un niveau même sans enregistrer. Dès que le moteur
//!   écoute la source (enregistrement, préparation, finalisation), son écoute
//!   est suspendue : la carte affiche alors le niveau du moteur, et un même
//!   programme n'est pas écouté deux fois pendant tout un enregistrement.
//! - Micro : écouté seulement pendant que les réglages sont ouverts, pour
//!   vérifier le micro choisi.
//!
//! Même capture que l'enregistrement : si le niveau bouge ici, il bougera
//! pendant l'enregistrement. Rien n'est écrit ni encodé, seule la crête est
//! gardée.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use polyptyque_core::config::{Source, CHANNELS, SAMPLE_RATE};
use polyptyque_core::level::{peak_of, RECENT_SOUND_HNS, SOUND_THRESHOLD};
use polyptyque_core::platform::{AudioCapture, AudioPacket, Platform, WindowHandle};
use polyptyque_engine::resolve;

/// Attente maximale d'une lecture : le fil vérifie au moins aussi souvent
/// s'il doit s'arrêter.
const READ_TIMEOUT_MS: u32 = 50;
const IDLE: Duration = Duration::from_millis(READ_TIMEOUT_MS as u64);
/// Délai avant un nouvel essai (fenêtre introuvable, capture impossible), et
/// entre deux vérifications de la fenêtre écoutée.
const RETRY: Duration = Duration::from_secs(2);

/// Niveau mesuré, partagé entre le fil d'écoute et l'interface.
#[derive(Default)]
struct Level {
    peak_bits: AtomicU32,
    last_sound_hns: AtomicU64,
    /// Dernière erreur d'ouverture, effacée dès que la capture fonctionne.
    error: Mutex<Option<String>>,
}

impl Level {
    fn record(&self, packets: &[AudioPacket], now_hns: u64) {
        let peak = packets.iter().filter(|p| !p.silent).map(|p| peak_of(&p.data)).fold(0.0, f32::max);
        if peak > SOUND_THRESHOLD {
            self.last_sound_hns.store(now_hns, Ordering::Relaxed);
        }
        if peak.is_finite() && peak > 0.0 {
            // Flottants positifs : l'ordre de leurs bits est celui des valeurs.
            self.peak_bits.fetch_max(peak.to_bits(), Ordering::Relaxed);
        }
    }

    /// Crête depuis l'appel précédent (0 à 1), et son entendu récemment.
    fn take(&self, now_hns: u64) -> (f32, bool) {
        let peak = f32::from_bits(self.peak_bits.swap(0, Ordering::Relaxed));
        let last = self.last_sound_hns.load(Ordering::Relaxed);
        (peak, last != 0 && now_hns.saturating_sub(last) < RECENT_SOUND_HNS)
    }

    fn set_error(&self, error: Option<String>) {
        *self.error.lock().unwrap_or_else(|e| e.into_inner()) = error;
    }

    fn error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

// ----- Sources ---------------------------------------------------------------

pub struct SourceMeter {
    pub source_id: String,
    level: Arc<Level>,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl SourceMeter {
    /// Suspend l'écoute (le moteur écoute la source) ou la reprend.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub fn take(&self, now_hns: u64) -> (f32, bool) {
        self.level.take(now_hns)
    }
}

impl Drop for SourceMeter {
    fn drop(&mut self) {
        // Le fil s'arrête de lui-même au prochain tour, sans bloquer l'interface.
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub struct SourceMeters {
    meters: Vec<SourceMeter>,
}

impl SourceMeters {
    /// Une écoute par source ; celles que `engine_listens` désigne démarrent
    /// suspendues.
    pub fn start(platform: &Arc<dyn Platform>, sources: &[Source], engine_listens: impl Fn(&str) -> bool) -> Self {
        let meters = sources
            .iter()
            .filter_map(|source| {
                let level = Arc::new(Level::default());
                let paused = Arc::new(AtomicBool::new(engine_listens(&source.id)));
                let stop = Arc::new(AtomicBool::new(false));
                let (platform, source_clone, level_clone, paused_clone, stop_clone) =
                    (platform.clone(), source.clone(), level.clone(), paused.clone(), stop.clone());
                thread::Builder::new()
                    .name(format!("{}-niveau", source.name))
                    .spawn(move || run_source(platform, source_clone, level_clone, paused_clone, stop_clone))
                    .map_err(|e| log::warn!("niveau sonore de {} indisponible : {e}", source.name))
                    .ok()
                    .map(|_| SourceMeter { source_id: source.id.clone(), level, paused, stop })
            })
            .collect();
        Self { meters }
    }

    pub fn meters(&self) -> &[SourceMeter] {
        &self.meters
    }

    pub fn pause(&self, source_id: &str) {
        for meter in self.meters.iter().filter(|m| m.source_id == source_id) {
            meter.set_paused(true);
        }
    }

    pub fn stop(&mut self) {
        self.meters.clear();
    }
}

fn run_source(
    platform: Arc<dyn Platform>,
    source: Source,
    level: Arc<Level>,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
) {
    // Capture en cours, avec la fenêtre et le processus écoutés.
    let mut capture: Option<(Box<dyn AudioCapture>, WindowHandle, u32)> = None;
    let mut next_check = Instant::now();
    let mut warned_pid: Option<u32> = None;
    let mut packets = Vec::new();

    while !stop.load(Ordering::Relaxed) {
        if paused.load(Ordering::Relaxed) {
            capture = None;
            next_check = Instant::now();
            thread::sleep(IDLE);
            continue;
        }
        let now = Instant::now();
        if now >= next_check {
            next_check = now + RETRY;
            // Fenêtre fermée : son programme peut continuer sans elle, ou
            // revenir dans un autre processus. On cherche de nouveau.
            if capture.as_ref().is_some_and(|(_, handle, _)| !platform.is_window_alive(*handle)) {
                capture = None;
            }
            if capture.is_none() {
                let windows = platform.list_windows();
                if let Some(window) = resolve::find_window(platform.as_ref(), &source, &windows) {
                    match platform.open_audio_capture(window.pid, SAMPLE_RATE, CHANNELS) {
                        Ok(c) => capture = Some((c, window.handle, window.pid)),
                        Err(e) => {
                            if warned_pid != Some(window.pid) {
                                log::warn!(
                                    "[{}] niveau sonore du processus {} indisponible : {e:#}",
                                    source.name,
                                    window.pid
                                );
                                warned_pid = Some(window.pid);
                            }
                        }
                    }
                }
            }
        }

        let Some((c, _, pid)) = capture.as_mut() else {
            thread::sleep(IDLE);
            continue;
        };
        packets.clear();
        match c.read(READ_TIMEOUT_MS, &mut packets) {
            Ok(()) => {
                if !packets.is_empty() {
                    level.record(&packets, platform.now_hns());
                }
            }
            Err(e) => {
                log::debug!("[{}] écoute du processus {pid} interrompue : {e:#}", source.name);
                capture = None;
                next_check = Instant::now() + RETRY;
            }
        }
    }
}

// ----- Micro (test dans les réglages) ------------------------------------------

pub struct MicMeter {
    /// `None` : le micro par défaut de Windows.
    device: Option<String>,
    level: Arc<Level>,
    stop: Arc<AtomicBool>,
}

impl MicMeter {
    pub fn start(platform: &Arc<dyn Platform>, device: Option<String>) -> Self {
        let level = Arc::new(Level::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (platform, device_clone, level_clone, stop_clone) =
            (platform.clone(), device.clone(), level.clone(), stop.clone());
        if let Err(e) = thread::Builder::new()
            .name("micro-test".into())
            .spawn(move || run_microphone(platform, device_clone, level_clone, stop_clone))
        {
            log::warn!("test du micro indisponible : {e}");
            level.set_error(Some(tr::tr!("Test du micro indisponible.")));
        }
        Self { device, level, stop }
    }

    pub fn device(&self) -> Option<&str> {
        self.device.as_deref()
    }

    pub fn take(&self, now_hns: u64) -> (f32, bool) {
        self.level.take(now_hns)
    }

    /// Pourquoi le micro ne peut pas être écouté (accès refusé par Windows,
    /// micro débranché…).
    pub fn error(&self) -> Option<String> {
        self.level.error()
    }
}

impl Drop for MicMeter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn run_microphone(platform: Arc<dyn Platform>, device: Option<String>, level: Arc<Level>, stop: Arc<AtomicBool>) {
    let mut capture: Option<Box<dyn AudioCapture>> = None;
    let mut retry_at = Instant::now();
    let mut warned = false;
    let mut packets = Vec::new();

    while !stop.load(Ordering::Relaxed) {
        if capture.is_none() && Instant::now() >= retry_at {
            match platform.open_microphone_capture(device.as_deref(), SAMPLE_RATE, CHANNELS) {
                Ok(c) => {
                    capture = Some(c);
                    level.set_error(None);
                }
                Err(e) => {
                    if !warned {
                        log::warn!("test du micro : {e:#}");
                        warned = true;
                    }
                    level.set_error(Some(e.to_string()));
                    retry_at = Instant::now() + RETRY;
                }
            }
        }
        let Some(c) = capture.as_mut() else {
            thread::sleep(IDLE);
            continue;
        };
        packets.clear();
        match c.read(READ_TIMEOUT_MS, &mut packets) {
            Ok(()) => {
                if !packets.is_empty() {
                    level.record(&packets, platform.now_hns());
                }
            }
            Err(e) => {
                log::warn!("test du micro interrompu : {e:#}");
                level.set_error(Some(e.to_string()));
                capture = None;
                retry_at = Instant::now() + RETRY;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(samples: &[f32], silent: bool) -> AudioPacket {
        AudioPacket { data: samples.iter().flat_map(|s| s.to_le_bytes()).collect(), timestamp_hns: None, silent }
    }

    #[test]
    fn level_keeps_the_peak_until_taken() {
        let level = Level::default();
        level.record(&[packet(&[0.1, -0.4], false), packet(&[0.9], true)], 1_000);
        level.record(&[packet(&[0.2], false)], 2_000);
        // Les paquets déclarés silencieux ne comptent pas.
        assert_eq!(level.take(2_000), (0.4, true));
        assert_eq!(level.take(2_000).0, 0.0);
        // Plus de son depuis plus de deux secondes : « Muet ».
        assert!(!level.take(2_000 + RECENT_SOUND_HNS).1);
    }
}
