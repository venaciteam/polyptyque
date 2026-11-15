//! Synchronisation : cadence constante de l'image et placement du son sur
//! l'horloge commune.
//!
//! Toutes les sources partagent un instant de départ `t0`. L'image n est
//! émise à `t0 + n / fps` ; le son est placé d'après l'horodatage de chaque
//! paquet. Ainsi, quelle que soit la durée, tous les fichiers restent calés
//! les uns sur les autres et l'image reste calée sur le son.

/// Centaines de nanosecondes par seconde.
pub const HNS_PER_SEC: u64 = 10_000_000;

/// Nombre d'images qui doivent avoir été émises après `elapsed_hns` (la
/// première image est émise à l'instant 0).
pub fn frames_due(elapsed_hns: u64, fps: u32) -> u64 {
    (elapsed_hns as u128 * fps as u128 / HNS_PER_SEC as u128) as u64 + 1
}

/// Instant, relatif à `t0`, de l'image `index`.
pub fn frame_time_hns(index: u64, fps: u32) -> u64 {
    (index as u128 * HNS_PER_SEC as u128 / fps as u128) as u64
}

/// Saut de l'horloge entre deux tours d'un fil de capture au-delà duquel
/// l'interruption (mise en veille non signalée, programme ou machine
/// virtuelle figés) n'est pas comblée d'images figées et de silence : la
/// suite part dans une nouvelle partie, comme après une mise en veille
/// annoncée (B3 dans l'analyse des risques).
pub const CLOCK_JUMP_HNS: u64 = 5 * HNS_PER_SEC;

/// Silence ajouté en une fois au plus (10 s, moins de 4 Mo) : un trou plus
/// long n'est jamais alloué d'un bloc. Le reste est ajouté aux appels
/// suivants.
const MAX_SILENCE_MS: u64 = 10_000;

/// Point de reprise après une coupure à `resume_hns` : rang de la première
/// image à partir de cet instant et échantillon de son correspondant (calé
/// sur cette image quand la source a une image, `fps`).
pub fn cut_resume_point(t0_hns: u64, resume_hns: u64, fps: Option<u32>, sample_rate: u32) -> (u64, u64) {
    let elapsed = resume_hns.saturating_sub(t0_hns) as u128;
    match fps {
        Some(fps) => {
            let fps = fps.max(1) as u128;
            let frame = (elapsed * fps).div_ceil(HNS_PER_SEC as u128);
            (frame as u64, (frame * sample_rate as u128).div_ceil(fps) as u64)
        }
        None => (0, (elapsed * sample_rate as u128 / HNS_PER_SEC as u128) as u64),
    }
}

/// Écart au-delà duquel le son est recalé d'un coup (silence inséré ou
/// échantillons sautés).
const HARD_THRESHOLD_MS: u64 = 40;
/// Écart toléré sans correction.
const SOFT_THRESHOLD_MS: u64 = 2;
/// Écarts plus larges quand l'horodatage n'est qu'une estimation.
const APPROX_HARD_THRESHOLD_MS: u64 = 80;
const APPROX_SOFT_THRESHOLD_MS: u64 = 15;
/// Correction douce : un échantillon dupliqué ou retiré tous les N, soit
/// 0,2 % de variation de vitesse, inaudible.
const STUFF_INTERVAL: u32 = 500;
/// Sans paquet depuis ce délai, la source est considérée silencieuse.
const IDLE_AFTER_MS: u64 = 100;
/// Pendant un silence, le son est complété jusqu'à « maintenant » moins
/// cette marge, pour laisser la place aux paquets en retard.
const IDLE_MARGIN_MS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketTime {
    /// Horodatage fourni par le système.
    Exact(u64),
    /// Horodatage estimé à partir de l'heure de réception.
    Approx(u64),
}

#[derive(Debug, Default, Clone, Copy)]
pub struct AudioTimelineStats {
    /// Silence ajouté (source muette, perdue, ou trou dans le flux).
    pub silence_frames: u64,
    /// Échantillons sautés lors d'un recalage brusque.
    pub skipped_frames: u64,
    /// Échantillons dupliqués ou retirés par la correction douce.
    pub stuffed_frames: u64,
}

/// Place le son sur l'horloge commune. Produit un flux continu, sans trou,
/// dont la position suit l'horloge à quelques millisecondes près.
pub struct AudioTimeline {
    t0_hns: u64,
    sample_rate: u32,
    frame_bytes: usize,
    written: u64,
    last_packet_hns: Option<u64>,
    stuff_counter: u32,
    stats: AudioTimelineStats,
}

impl AudioTimeline {
    pub fn new(t0_hns: u64, sample_rate: u32, channels: u16) -> Self {
        Self {
            t0_hns,
            sample_rate,
            frame_bytes: channels as usize * 4,
            written: 0,
            last_packet_hns: None,
            stuff_counter: 0,
            stats: AudioTimelineStats::default(),
        }
    }

    /// Reprend un flux dont `frames` échantillons ont déjà été transmis
    /// (fil de capture relancé après une erreur interne).
    pub fn resumed(mut self, frames: u64) -> Self {
        self.written = frames;
        self
    }

    pub fn frame_bytes(&self) -> usize {
        self.frame_bytes
    }

    pub fn written_frames(&self) -> u64 {
        self.written
    }

    pub fn stats(&self) -> AudioTimelineStats {
        self.stats
    }

    /// Position (en échantillons) correspondant à un instant de l'horloge.
    fn position(&self, time_hns: u64) -> i64 {
        ((time_hns as i128 - self.t0_hns as i128) * self.sample_rate as i128 / HNS_PER_SEC as i128) as i64
    }

    fn ms_to_frames(&self, ms: u64) -> i64 {
        (self.sample_rate as u64 * ms / 1000) as i64
    }

    /// Ajoute un paquet reçu à `arrival_hns`, en écrivant dans `out` les
    /// octets à envoyer à l'encodeur.
    pub fn place(&mut self, arrival_hns: u64, time: PacketTime, data: &[u8], silent: bool, out: &mut Vec<u8>) {
        let frame_bytes = self.frame_bytes;
        let frames = data.len() / frame_bytes;
        if frames == 0 {
            return;
        }
        let was_idle = self.is_idle(arrival_hns);
        self.last_packet_hns = Some(arrival_hns);

        let (time_hns, hard, soft) = match time {
            PacketTime::Exact(t) => (t, HARD_THRESHOLD_MS, SOFT_THRESHOLD_MS),
            PacketTime::Approx(t) => (t, APPROX_HARD_THRESHOLD_MS, APPROX_SOFT_THRESHOLD_MS),
        };
        let mut start = 0usize;
        let mut target = self.position(time_hns);
        if target + frames as i64 <= 0 {
            // Entièrement antérieur au départ.
            return;
        }
        if target < 0 {
            start = (-target) as usize;
            target = 0;
        }
        let diff = target - self.written as i64;
        let hard = self.ms_to_frames(hard);
        let soft = self.ms_to_frames(soft);
        // Après un silence, on peut recaler d'un coup sans que cela s'entende.
        let hard = if was_idle { soft } else { hard };

        let mut stuff = 0i64; // > 0 : dupliquer, < 0 : retirer
        if diff > hard {
            if !self.push_silence(diff as u64, out) {
                // Trou démesuré (le fil de capture traite normalement un tel
                // saut comme une coupure) : il est comblé par blocs aux
                // appels suivants, ce paquet est écarté.
                return;
            }
        } else if diff < -hard {
            let skip = (-diff) as usize;
            let skipped = skip.min(frames - start);
            self.stats.skipped_frames += skipped as u64;
            start += skipped;
        } else if diff > soft {
            stuff = diff - soft;
        } else if diff < -soft {
            stuff = diff + soft;
        }
        if start >= frames {
            return;
        }

        let payload = &data[start * frame_bytes..frames * frame_bytes];
        if stuff == 0 {
            self.push_samples(payload, silent, out);
            return;
        }
        // Correction douce, échantillon par échantillon.
        let mut remaining = stuff.unsigned_abs();
        for frame in payload.chunks_exact(frame_bytes) {
            if remaining > 0 {
                self.stuff_counter += 1;
                if self.stuff_counter >= STUFF_INTERVAL {
                    self.stuff_counter = 0;
                    remaining -= 1;
                    self.stats.stuffed_frames += 1;
                    if stuff > 0 {
                        self.push_samples(frame, silent, out);
                    } else {
                        continue;
                    }
                }
            }
            self.push_samples(frame, silent, out);
        }
    }

    /// À appeler régulièrement : si la source ne produit plus rien (silence,
    /// fenêtre perdue), complète avec du silence pour que le flux suive
    /// l'horloge.
    pub fn pad_idle(&mut self, now_hns: u64, out: &mut Vec<u8>) {
        if !self.is_idle(now_hns) {
            return;
        }
        let target = self.position(now_hns) - self.ms_to_frames(IDLE_MARGIN_MS);
        if target > self.written as i64 {
            self.push_silence((target - self.written as i64) as u64, out);
        }
    }

    /// Termine le flux exactement à l'instant d'arrêt. Renvoie vrai s'il
    /// reste du silence à ajouter : envoyer `out`, puis rappeler.
    pub fn finish(&mut self, stop_hns: u64, out: &mut Vec<u8>) -> bool {
        let target = self.position(stop_hns);
        target > self.written as i64 && !self.push_silence((target - self.written as i64) as u64, out)
    }

    /// Reprise après une coupure : le flux continue à l'échantillon `frames`
    /// (voir [`cut_resume_point`]), sans combler l'interruption.
    pub fn jump_to(&mut self, frames: u64) {
        self.written = frames;
        self.last_packet_hns = None;
        self.stuff_counter = 0;
    }

    fn is_idle(&self, now_hns: u64) -> bool {
        let reference = self.last_packet_hns.unwrap_or(self.t0_hns);
        now_hns.saturating_sub(reference) >= IDLE_AFTER_MS * HNS_PER_SEC / 1000 || self.last_packet_hns.is_none()
    }

    fn push_samples(&mut self, bytes: &[u8], silent: bool, out: &mut Vec<u8>) {
        if silent {
            out.resize(out.len() + bytes.len(), 0);
        } else {
            out.extend_from_slice(bytes);
        }
        self.written += (bytes.len() / self.frame_bytes) as u64;
    }

    /// Ajoute du silence, au plus [`MAX_SILENCE_MS`] d'un coup. Renvoie faux
    /// si tout n'a pas pu être ajouté.
    fn push_silence(&mut self, frames: u64, out: &mut Vec<u8>) -> bool {
        let added = frames.min(self.ms_to_frames(MAX_SILENCE_MS) as u64);
        out.resize(out.len() + added as usize * self.frame_bytes, 0);
        self.written += added;
        self.stats.silence_frames += added;
        added == frames
    }
}

/// Horodatages estimés pour une rafale de paquets lus ensemble : le dernier
/// échantillon est supposé reçu à `arrival_hns`.
pub fn approximate_times(arrival_hns: u64, frame_counts: &[usize], sample_rate: u32) -> Vec<u64> {
    let total: usize = frame_counts.iter().sum();
    let mut remaining = total as u64;
    frame_counts
        .iter()
        .map(|&frames| {
            let offset = remaining * HNS_PER_SEC / sample_rate as u64;
            remaining -= frames as u64;
            arrival_hns.saturating_sub(offset)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;
    const MS: u64 = 10_000; // une milliseconde en hns

    fn packet(frames: usize, value: f32) -> Vec<u8> {
        let mut data = Vec::with_capacity(frames * 8);
        for _ in 0..frames * 2 {
            data.extend_from_slice(&value.to_le_bytes());
        }
        data
    }

    fn frames_of(out: &[u8]) -> u64 {
        (out.len() / 8) as u64
    }

    #[test]
    fn frame_schedule() {
        assert_eq!(frames_due(0, 60), 1);
        assert_eq!(frames_due(HNS_PER_SEC - 1, 60), 60);
        assert_eq!(frames_due(HNS_PER_SEC, 60), 61);
        assert_eq!(frame_time_hns(60, 60), HNS_PER_SEC);
        // 12 h à 60 i/s : aucune dérive due aux arrondis.
        let twelve_hours = 12 * 3600 * HNS_PER_SEC;
        assert_eq!(frames_due(twelve_hours, 60), 12 * 3600 * 60 + 1);
        assert_eq!(frame_time_hns(12 * 3600 * 60, 60), twelve_hours);
    }

    #[test]
    fn contiguous_packets_are_written_as_is() {
        let t0 = 1_000 * MS;
        let mut tl = AudioTimeline::new(t0, SR, 2);
        let mut out = Vec::new();
        for i in 0..100u64 {
            let time = t0 + i * 10 * MS; // 480 échantillons = 10 ms
            tl.place(time + 5 * MS, PacketTime::Exact(time), &packet(480, 0.5), false, &mut out);
        }
        assert_eq!(tl.written_frames(), 48_000);
        assert_eq!(frames_of(&out), 48_000);
        let stats = tl.stats();
        assert_eq!(stats.silence_frames, 0);
        assert_eq!(stats.stuffed_frames, 0);
    }

    #[test]
    fn gap_is_filled_with_silence() {
        let t0 = 0;
        let mut tl = AudioTimeline::new(t0, SR, 2);
        let mut out = Vec::new();
        tl.place(5 * MS, PacketTime::Exact(0), &packet(480, 0.5), false, &mut out);
        // Paquet suivant une demi-seconde plus tard.
        tl.place(505 * MS, PacketTime::Exact(500 * MS), &packet(480, 0.5), false, &mut out);
        assert_eq!(tl.written_frames(), 24_000 + 480);
        // Le paquet est bien placé à 500 ms.
        let index = 24_000usize * 8;
        assert_eq!(f32::from_le_bytes(out[index..index + 4].try_into().unwrap()), 0.5);
        assert_eq!(f32::from_le_bytes(out[index - 8..index - 4].try_into().unwrap()), 0.0);
    }

    #[test]
    fn overlap_is_skipped() {
        let mut tl = AudioTimeline::new(0, SR, 2);
        let mut out = Vec::new();
        tl.place(0, PacketTime::Exact(0), &packet(4_800, 0.5), false, &mut out); // 100 ms
                                                                                 // Paquet daté de 0 à nouveau : entièrement en recouvrement.
        tl.place(MS, PacketTime::Exact(0), &packet(4_800, 0.5), false, &mut out);
        assert_eq!(tl.written_frames(), 4_800);
        assert_eq!(tl.stats().skipped_frames, 4_800);
    }

    #[test]
    fn slow_drift_is_corrected_smoothly() {
        // Horloge audio 0,01 % plus lente que l'horloge commune pendant 10 min.
        let mut tl = AudioTimeline::new(0, SR, 2);
        let mut out = Vec::new();
        let packets = 60_000; // 10 ms chacun
        for i in 0..packets as u64 {
            let exact = i * 10 * MS;
            let drifted = exact + exact / 10_000;
            tl.place(drifted, PacketTime::Exact(drifted), &packet(480, 0.25), false, &mut out);
            out.clear();
        }
        let end_time = (packets as u64 * 10 * MS) + (packets as u64 * 10 * MS) / 10_000;
        let expected = (end_time as u128 * SR as u128 / HNS_PER_SEC as u128) as i64;
        let error = (tl.written_frames() as i64 - expected).abs();
        // Écart final inférieur à 3 ms, sans aucun silence inséré.
        assert!(error <= 144, "écart de {error} échantillons");
        assert_eq!(tl.stats().silence_frames, 0);
        assert!(tl.stats().stuffed_frames > 0);
    }

    #[test]
    fn idle_source_is_padded_and_finished_exactly() {
        let mut tl = AudioTimeline::new(0, SR, 2);
        let mut out = Vec::new();
        tl.pad_idle(1_000 * MS, &mut out);
        // Complété jusqu'à 1 s - 60 ms.
        assert_eq!(tl.written_frames(), 48_000 - 2_880);
        tl.finish(2_000 * MS, &mut out);
        assert_eq!(tl.written_frames(), 96_000);
        assert_eq!(frames_of(&out), 96_000);
    }

    #[test]
    fn packet_after_silence_is_resynced_immediately() {
        let mut tl = AudioTimeline::new(0, SR, 2);
        let mut out = Vec::new();
        tl.pad_idle(1_000 * MS, &mut out); // écrit jusqu'à 940 ms
                                           // Le son reprend à 970 ms : 30 ms d'écart, recalé tout de suite.
        tl.place(980 * MS, PacketTime::Exact(970 * MS), &packet(480, 0.5), false, &mut out);
        assert_eq!(tl.written_frames(), 46_560 + 480);
        assert_eq!(tl.stats().stuffed_frames, 0);
    }

    #[test]
    fn silent_packets_write_zeros() {
        let mut tl = AudioTimeline::new(0, SR, 2);
        let mut out = Vec::new();
        tl.place(0, PacketTime::Exact(0), &packet(480, 0.9), true, &mut out);
        assert!(out.iter().all(|&b| b == 0));
        assert_eq!(tl.written_frames(), 480);
    }

    #[test]
    fn packets_before_start_are_trimmed() {
        let mut tl = AudioTimeline::new(100 * MS, SR, 2);
        let mut out = Vec::new();
        // Paquet de 20 ms commençant 10 ms avant le départ.
        tl.place(110 * MS, PacketTime::Exact(90 * MS), &packet(960, 0.5), false, &mut out);
        assert_eq!(tl.written_frames(), 480);
    }

    #[test]
    fn long_gap_is_filled_by_bounded_blocks() {
        // Une heure sans nouvelles (veille non signalée) : jamais plus de
        // 10 s de silence alloué d'un coup.
        let block = 10 * 48_000 * 8;
        let mut tl = AudioTimeline::new(0, SR, 2);
        let mut out = Vec::new();
        tl.pad_idle(3_600_000 * MS, &mut out);
        assert_eq!(out.len(), block);
        out.clear();
        let mut calls = 1;
        while tl.finish(3_600_000 * MS, &mut out) {
            assert!(out.len() <= block);
            out.clear();
            calls += 1;
        }
        assert_eq!(tl.written_frames(), 3_600 * 48_000);
        assert!(calls > 300, "{calls}");
        // Un paquet très en avance : un bloc de silence, le paquet est écarté.
        let mut tl = AudioTimeline::new(0, SR, 2);
        let mut out = Vec::new();
        tl.place(0, PacketTime::Exact(0), &packet(480, 0.5), false, &mut out);
        out.clear();
        tl.place(60_000 * MS, PacketTime::Exact(60_000 * MS), &packet(480, 0.5), false, &mut out);
        assert_eq!(out.len(), block);
        assert_eq!(tl.written_frames(), 480 + 480_000);
    }

    #[test]
    fn cut_resumes_on_a_frame_without_filling() {
        // 1,0083 s après le départ : image 61 (à 1,0167 s), son calé dessus.
        assert_eq!(cut_resume_point(500, 500 + 10_083 * MS / 10, Some(60), SR), (61, 61 * 800));
        assert_eq!(cut_resume_point(0, HNS_PER_SEC, Some(30), SR), (30, 48_000));
        // Son seul : l'échantillon de l'instant de reprise.
        assert_eq!(cut_resume_point(0, 1_500 * MS, None, SR), (0, 72_000));
        assert_eq!(cut_resume_point(10, 0, Some(60), SR), (0, 0));

        let mut tl = AudioTimeline::new(0, SR, 2);
        let mut out = Vec::new();
        tl.place(5 * MS, PacketTime::Exact(0), &packet(480, 0.5), false, &mut out);
        out.clear();
        // Reprise une heure plus tard : rien n'est comblé.
        let (_, sample) = cut_resume_point(0, 3_600_000 * MS, Some(60), SR);
        tl.jump_to(sample);
        tl.place(3_600_010 * MS, PacketTime::Exact(3_600_000 * MS), &packet(480, 0.5), false, &mut out);
        assert_eq!(frames_of(&out), 480);
        assert_eq!(tl.written_frames(), sample + 480);
        // Paquet resté d'avant la coupure : écarté.
        out.clear();
        tl.place(3_600_020 * MS, PacketTime::Exact(10 * MS), &packet(480, 0.5), false, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn approximate_times_for_a_burst() {
        let times = approximate_times(1_000 * MS, &[480, 480], SR);
        assert_eq!(times, vec![980 * MS, 990 * MS]);
    }
}
