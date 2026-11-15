//! Fil audio d'une source : capture du son du programme de la fenêtre (ou
//! du micro) et placement sur l'horloge commune (silence quand il n'y a
//! rien).

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossbeam_channel::Sender;
use polyptyque_core::config::{CHANNELS, SAMPLE_RATE};
use polyptyque_core::level::{peak_of, SOUND_THRESHOLD};
use polyptyque_core::platform::{AudioCapture, AudioPacket, Platform};
use polyptyque_core::timeline::{
    approximate_times, cut_resume_point, AudioTimeline, PacketTime, CLOCK_JUMP_HNS, HNS_PER_SEC,
};

use crate::shared::{run_protected, Shared, SourceState};

const READ_TIMEOUT_MS: u32 = 20;
const RETRY_HNS: u64 = 2 * HNS_PER_SEC;
/// Un horodatage qui s'écarte de plus d'une seconde de l'heure de
/// réception n'est pas dans la base de temps attendue : on l'ignore.
const MAX_TIMESTAMP_SKEW_HNS: u64 = HNS_PER_SEC;

/// Origine du son.
pub(crate) enum AudioInput {
    /// Programme de la fenêtre suivie par le fil vidéo.
    Window,
    /// Micro (`None` : celui choisi par défaut dans Windows).
    Microphone(Option<String>),
}

pub(crate) struct AudioTask {
    pub platform: Arc<dyn Platform>,
    pub name: String,
    pub input: AudioInput,
    pub shared: Arc<Shared>,
    pub queue: Sender<Vec<u8>>,
    pub t0_hns: u64,
    /// Cadence de l'image de la source : après une coupure, le son reprend
    /// calé sur l'image de reprise (`None` pour le micro).
    pub fps: Option<u32>,
}

impl AudioTask {
    pub fn run(self) {
        // Échantillons déjà transmis : une relance après une erreur interne
        // reprend au même point de l'horloge.
        let mut sent = 0u64;
        let what = match self.input {
            AudioInput::Window => tr::tr!("la capture du son"),
            AudioInput::Microphone(_) => tr::tr!("la capture du micro"),
        };
        run_protected(&self.shared, &self.name, &what, || self.capture(&mut sent));
    }

    /// Met `out` en file (s'il n'est pas vide). Faux si l'écriture a cessé.
    fn send(&self, out: &mut Vec<u8>) -> bool {
        if out.is_empty() {
            return true;
        }
        self.shared.audio_queued.fetch_add(out.len() as u64, Ordering::Relaxed);
        self.queue.send(std::mem::take(out)).is_ok()
    }

    fn capture(&self, sent: &mut u64) {
        let (platform, name, input, shared, queue) =
            (&self.platform, &self.name, &self.input, &self.shared, &self.queue);
        let mut timeline = AudioTimeline::new(self.t0_hns, SAMPLE_RATE, CHANNELS).resumed(*sent);
        let mut capture: Option<Box<dyn AudioCapture>> = None;
        let mut generation = u64::MAX;
        let mut retry_at = 0u64;
        let mut warned_pid: Option<u32> = None;
        let mut warned_microphone = false;
        let mut warned_skew = false;
        let mut packets: Vec<AudioPacket> = Vec::new();
        let mut out: Vec<u8> = Vec::new();
        let mut last_arrival: Option<u64> = None;
        // Une relance après une erreur interne ne refait pas une coupure
        // déjà traitée.
        let mut seen_cut = shared.cut_generation();

        loop {
            // Encodeur arrêté : inutile d'accumuler du son qui ne sera jamais lu.
            if shared.has_failed() {
                break;
            }
            let now = platform.now_hns();
            if let Some(stop) = shared.stop_at() {
                if now >= stop {
                    // Fin exacte à l'instant d'arrêt, par blocs bornés.
                    loop {
                        let more = timeline.finish(stop, &mut out);
                        *sent = timeline.written_frames();
                        if !self.send(&mut out) || !more {
                            break;
                        }
                    }
                    break;
                }
            }

            // La fenêtre a changé (rattachée, perdue, retrouvée) : on suit
            // son processus.
            let current = shared.target_generation();
            if matches!(input, AudioInput::Window) && current != generation {
                generation = current;
                capture = None;
                retry_at = 0;
            }
            if capture.is_none() && now >= retry_at {
                if let AudioInput::Microphone(device) = &input {
                    match platform.open_microphone_capture(device.as_deref(), SAMPLE_RATE, CHANNELS) {
                        Ok(c) => {
                            log::info!("[{name}] micro capturé");
                            capture = Some(c);
                            warned_microphone = false;
                            shared.clear_note();
                            shared.set_state(SourceState::Connected);
                        }
                        Err(e) => {
                            if !warned_microphone {
                                log::warn!("[{name}] micro indisponible : {e:#}");
                                shared.note_error(format!("{e:#}"));
                                warned_microphone = true;
                            }
                            shared.set_state(SourceState::Lost);
                            retry_at = now + RETRY_HNS;
                        }
                    }
                } else if let Some(pid) = shared.target_pid() {
                    match platform.open_audio_capture(pid, SAMPLE_RATE, CHANNELS) {
                        Ok(c) => {
                            log::info!("[{name}] son du processus {pid} capturé");
                            capture = Some(c);
                        }
                        Err(e) => {
                            if warned_pid != Some(pid) {
                                log::warn!("[{name}] son du processus {pid} indisponible : {e:#}");
                                shared.note_error(tr::tr!("son indisponible : {}", format!("{e:#}")));
                                warned_pid = Some(pid);
                            }
                            retry_at = now + RETRY_HNS;
                        }
                    }
                }
            }

            packets.clear();
            match capture.as_mut() {
                Some(c) => {
                    if let Err(e) = c.read(READ_TIMEOUT_MS, &mut packets) {
                        log::warn!("[{name}] capture du son interrompue : {e:#}");
                        capture = None;
                        retry_at = now + RETRY_HNS;
                        if matches!(input, AudioInput::Microphone(_)) {
                            shared.set_state(SourceState::Lost);
                        }
                    }
                }
                None => thread::sleep(Duration::from_millis(READ_TIMEOUT_MS as u64)),
            }

            let arrival = platform.now_hns();
            // Saut de l'horloge (veille non signalée, programme figé), le plus
            // souvent pendant la lecture : vu avant de placer les paquets.
            // L'interruption n'est pas comblée de silence, la suite part dans
            // une nouvelle partie.
            let jumped = last_arrival
                .filter(|&prev| shared.stop_at().is_none() && arrival.saturating_sub(prev) > CLOCK_JUMP_HNS);
            if let Some(prev) = jumped {
                log::warn!("[{name}] horloge en avance de {} s d'un coup : coupure", (arrival - prev) / HNS_PER_SEC);
                shared.request_cut(prev, arrival, self.t0_hns);
            }
            last_arrival = Some(arrival);
            if let Some(cut) = shared.pending_cut(seen_cut) {
                seen_cut = cut.generation;
                // Son dû jusqu'au début de l'interruption, puis la marque de
                // coupure, puis reprise calée sur l'image de reprise.
                loop {
                    let more = timeline.finish(cut.from_hns, &mut out);
                    *sent = timeline.written_frames();
                    if !self.send(&mut out) {
                        return;
                    }
                    if !more {
                        break;
                    }
                }
                if queue.send(Vec::new()).is_err() {
                    return;
                }
                let (_, sample) = cut_resume_point(self.t0_hns, cut.to_hns, self.fps, SAMPLE_RATE);
                timeline.jump_to(sample);
                *sent = sample;
            }
            if !packets.is_empty() {
                let frame_bytes = timeline.frame_bytes();
                let counts: Vec<usize> = packets.iter().map(|p| p.data.len() / frame_bytes).collect();
                let estimates = approximate_times(arrival, &counts, SAMPLE_RATE);
                let mut peak = 0f32;
                for (packet, estimate) in packets.iter().zip(estimates) {
                    let time = match packet.timestamp_hns {
                        Some(t) if t.abs_diff(arrival) <= MAX_TIMESTAMP_SKEW_HNS => PacketTime::Exact(t),
                        Some(t) => {
                            if !warned_skew {
                                log::warn!(
                                    "[{name}] horodatage audio incohérent ({t} contre {arrival}), estimation utilisée"
                                );
                                warned_skew = true;
                            }
                            PacketTime::Approx(estimate)
                        }
                        None => PacketTime::Approx(estimate),
                    };
                    timeline.place(arrival, time, &packet.data, packet.silent, &mut out);
                    if !packet.silent {
                        peak = peak.max(peak_of(&packet.data));
                    }
                }
                if peak > SOUND_THRESHOLD {
                    shared.audio_last_sound_hns.store(arrival, Ordering::Relaxed);
                }
                shared.record_peak(peak);
            }
            timeline.pad_idle(arrival, &mut out);

            if !out.is_empty() {
                *sent = timeline.written_frames();
                if !self.send(&mut out) {
                    return;
                }
                shared.check_backlog(name, shared.audio_backlog_seconds());
            }
        }

        let stats = timeline.stats();
        log::info!(
            "[{name}] son : {} échantillons écrits, {} de silence ajouté, {} sautés, {} corrigés",
            timeline.written_frames(),
            stats.silence_frames,
            stats.skipped_frames,
            stats.stuffed_frames
        );
    }
}
