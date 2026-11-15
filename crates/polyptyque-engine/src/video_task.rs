//! Fil vidéo d'une source : recherche et capture de la fenêtre, émission des
//! images à cadence constante sur l'horloge commune.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossbeam_channel::Sender;
use polyptyque_core::config::{Source, SAMPLE_RATE};
use polyptyque_core::platform::{Nv12Frame, Platform, VideoCapture, WindowHandle};
use polyptyque_core::timeline::{cut_resume_point, frame_time_hns, frames_due, CLOCK_JUMP_HNS, HNS_PER_SEC};
use polyptyque_core::video::black_nv12;

use crate::resolve;
use crate::shared::{run_protected, Shared, SourceState, WindowClaims};
use crate::writer::video_cut_marker;

/// Intervalle de recherche d'une fenêtre absente.
const SEARCH_INTERVAL_HNS: u64 = HNS_PER_SEC;
const MINIMIZED_CHECK_HNS: u64 = HNS_PER_SEC / 2;

pub(crate) struct VideoTask {
    pub platform: Arc<dyn Platform>,
    pub source: Source,
    pub shared: Arc<Shared>,
    pub queue: Sender<Nv12Frame>,
    pub t0_hns: u64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Fenêtres suivies par les sources de la session.
    pub claims: Arc<WindowClaims>,
    /// Rang de cette exécution dans la session (clé de sa réservation).
    pub run: usize,
}

/// Repère les sauts de l'horloge commune dus à une vraie mise en veille
/// (voir `Platform::suspended_hns`), vus par un fil de capture.
#[derive(Default)]
pub(crate) struct JumpWatch {
    /// Instant du dernier relevé, et temps de veille à cet instant.
    last: Option<(u64, Option<u64>)>,
}

impl JumpWatch {
    /// Relevé à l'instant `now` : renvoie l'instant du relevé précédent si
    /// l'horloge a avancé de plus de [`CLOCK_JUMP_HNS`] depuis, et si ce
    /// temps a été passé en veille quand le système sait le mesurer. Un fil
    /// bloqué (capture lente, pilote, périphérique qui se réinitialise)
    /// n'est pas une veille : ce temps est comblé comme d'habitude.
    pub fn check(&mut self, now: u64, suspended: Option<u64>) -> Option<u64> {
        let (prev, prev_suspended) = self.last.replace((now, suspended))?;
        if now.saturating_sub(prev) <= CLOCK_JUMP_HNS {
            return None;
        }
        match (prev_suspended, suspended) {
            (Some(before), Some(after)) => (after.saturating_sub(before) >= CLOCK_JUMP_HNS).then_some(prev),
            _ => Some(prev),
        }
    }

    /// Fin d'une étape qui peut être lente (recherche de la fenêtre,
    /// ouverture d'une capture). Sans mesure de la veille, son temps n'est
    /// pas compté dans le prochain saut ; avec, rien à faire : une veille
    /// pendant l'étape reste vue.
    pub fn slow_step_done(&mut self, now: u64) {
        if let Some((prev, None)) = &mut self.last {
            *prev = now;
        }
    }
}

/// Libère la fenêtre suivie et la réservation de la source à la sortie du
/// fil, même sur une erreur interne.
struct ClaimGuard<'a>(&'a WindowClaims, usize);

impl Drop for ClaimGuard<'_> {
    fn drop(&mut self) {
        self.0.unregister(self.1);
    }
}

impl VideoTask {
    pub fn run(self) {
        // Images déjà émises : une relance après une erreur interne reprend
        // au même rang, image et son restent calés.
        let mut sent = 0u64;
        let what = tr::tr!("la capture de l'image");
        run_protected(&self.shared, &self.source.name, &what, || self.capture(&mut sent));
    }

    fn capture(&self, sent: &mut u64) {
        let (platform, source, shared, queue) = (&self.platform, &self.source, &self.shared, &self.queue);
        let (t0_hns, width, height, fps) = (self.t0_hns, self.width, self.height, self.fps);
        let (claims, run) = (self.claims.as_ref(), self.run);
        claims.register(run, source);
        let _claim = ClaimGuard(claims, run);
        let black: Nv12Frame = Arc::new(black_nv12(width, height));
        let mut latest = black.clone();
        let mut last_sent = black.clone();
        let mut capture: Option<(Box<dyn VideoCapture>, WindowHandle)> = None;
        let mut next_index = *sent;
        let mut next_search = 0u64;
        let mut next_minimized_check = 0u64;
        let mut minimized = false;
        // Au-delà d'une seconde d'avance, l'encodeur ne suit plus : on
        // répète l'image précédente plutôt que d'accumuler de la mémoire.
        let max_backlog = fps as usize;
        let mut jumps = JumpWatch::default();
        // Une relance après une erreur interne ne refait pas une coupure
        // déjà traitée.
        let mut seen_cut = shared.cut_generation();

        loop {
            let now = platform.now_hns();
            let stop_at = shared.stop_at();

            // Saut de l'horloge pendant une veille non signalée : pas de
            // rattrapage en images figées, la suite part dans une nouvelle
            // partie.
            let jumped = jumps.check(now, platform.suspended_hns()).filter(|_| stop_at.is_none());
            if let Some(prev) = jumped {
                let seconds = (now - prev) / HNS_PER_SEC;
                log::warn!("[{}] horloge en avance de {seconds} s d'un coup : coupure", source.name);
                shared.request_cut(prev, now, t0_hns);
            }
            if let Some(cut) = shared.pending_cut(seen_cut) {
                seen_cut = cut.generation;
                if queue.send(video_cut_marker()).is_err() {
                    return;
                }
                // Image et son repartent ensemble de la même image.
                next_index = cut_resume_point(t0_hns, cut.to_hns, Some(fps), SAMPLE_RATE).0;
                *sent = next_index;
            }

            if capture.is_none() && stop_at.is_none() && now >= next_search {
                next_search = now + SEARCH_INTERVAL_HNS;
                let windows = platform.list_windows();
                // Jamais la fenêtre d'une autre source de la session (deux
                // sources d'un même programme).
                let exclusions = claims.exclusions(&source.id, &windows);
                let found = resolve::find_window_excluding(platform.as_ref(), source, &windows, &exclusions)
                    // Prise entre-temps par une autre source : nouvel essai au tour suivant.
                    .filter(|window| claims.claim(run, &source.id, window.handle));
                if let Some(window) = found {
                    match platform.open_video_capture(window.handle, width, height, source.capture_cursor, source.crop) {
                        Ok(c) => {
                            log::info!(
                                "[{}] fenêtre rattachée : « {} » ({})",
                                source.name,
                                window.title,
                                window.exe.display()
                            );
                            capture = Some((c, window.handle));
                            shared.clear_note();
                            shared.set_target(Some(window));
                            shared.set_state(SourceState::Connected);
                            next_minimized_check = 0;
                            minimized = false;
                        }
                        Err(e) => {
                            claims.release(run);
                            log::warn!("[{}] capture de la fenêtre impossible : {e:#}", source.name);
                            shared.note_error(tr::tr!("capture de la fenêtre impossible : {}", format!("{e:#}")));
                        }
                    }
                }
                // Recherche ou ouverture de la capture lente (programmes qui
                // ne répondent pas, pilote) : ce temps n'est pas une veille.
                jumps.slow_step_done(platform.now_hns());
            }

            let mut lost = false;
            if let Some((c, handle)) = capture.as_mut() {
                if !platform.is_window_alive(*handle) {
                    lost = true;
                } else {
                    match c.poll() {
                        Ok(Some(frame)) => {
                            latest = frame;
                            shared.frames_new.fetch_add(1, Ordering::Relaxed);
                        }
                        Ok(None) => {}
                        Err(e) => {
                            log::warn!("[{}] capture interrompue : {e:#}", source.name);
                            lost = true;
                        }
                    }
                    if !lost && now >= next_minimized_check {
                        next_minimized_check = now + MINIMIZED_CHECK_HNS;
                        let now_minimized = platform.is_window_minimized(*handle);
                        if now_minimized != minimized {
                            minimized = now_minimized;
                            if minimized {
                                log::info!("[{}] fenêtre réduite : image figée jusqu'à son retour", source.name);
                            } else {
                                log::info!("[{}] fenêtre rétablie", source.name);
                            }
                        }
                        shared.set_state(if minimized { SourceState::Minimized } else { SourceState::Connected });
                    }
                }
            }
            if lost {
                log::info!("[{}] fenêtre perdue, image noire jusqu'à son retour", source.name);
                capture = None;
                claims.release(run);
                latest = black.clone();
                shared.set_target(None);
                shared.set_state(SourceState::Lost);
                next_search = now + SEARCH_INTERVAL_HNS;
            }

            // Émission des images dues depuis le départ commun.
            let horizon = stop_at.map_or(now, |stop| stop.min(now));
            let due = if horizon >= t0_hns { frames_due(horizon - t0_hns, fps) } else { 0 };
            while next_index < due {
                let frame = if queue.len() >= max_backlog {
                    shared.frames_dropped.fetch_add(1, Ordering::Relaxed);
                    last_sent.clone()
                } else {
                    latest.clone()
                };
                if queue.send(frame.clone()).is_err() {
                    // L'écriture a cessé : l'erreur est déjà signalée.
                    return;
                }
                last_sent = frame;
                next_index += 1;
                *sent = next_index;
                shared.frames_total.fetch_add(1, Ordering::Relaxed);
            }
            shared.video_backlog.store(queue.len() as u64, Ordering::Relaxed);
            shared.check_backlog(&source.name, queue.len() as f32 / fps.max(1) as f32);

            if stop_at.is_some_and(|stop| now >= stop) || shared.has_failed() {
                break;
            }

            let next_time = t0_hns + frame_time_hns(next_index, fps);
            let now = platform.now_hns();
            if next_time > now {
                thread::sleep(Duration::from_nanos((next_time - now) * 100));
            }
        }
        shared.set_state(SourceState::Finished);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = HNS_PER_SEC;

    #[test]
    fn only_a_real_sleep_is_a_jump() {
        // Le système mesure la veille.
        let mut watch = JumpWatch::default();
        assert_eq!(watch.check(100 * S, Some(0)), None);
        // Fil bloqué 8 s sans veille : pas un saut.
        assert_eq!(watch.check(108 * S, Some(0)), None);
        // Une heure de veille.
        assert_eq!(watch.check(3_708 * S, Some(3_600 * S)), Some(108 * S));
        // Étape lente : une veille pendant celle-ci reste vue.
        watch.slow_step_done(3_709 * S);
        assert_eq!(watch.check(3_720 * S, Some(3_610 * S)), Some(3_708 * S));

        // Sans mesure de la veille : tout saut compte, hors étape lente.
        let mut watch = JumpWatch::default();
        assert_eq!(watch.check(100 * S, None), None);
        assert_eq!(watch.check(104 * S, None), None);
        assert_eq!(watch.check(110 * S, None), Some(104 * S));
        watch.slow_step_done(117 * S);
        assert_eq!(watch.check(118 * S, None), None);
    }
}
