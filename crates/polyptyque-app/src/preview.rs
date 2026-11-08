//! Aperçus en direct des sources.
//!
//! Chaque source a sa propre capture en définition réduite, indépendante de
//! l'enregistrement : la carte graphique réduit l'image avant la lecture,
//! ce qui rend les aperçus peu coûteux. Ils fonctionnent aussi bien hors
//! enregistrement que pendant.
//!
//! La définition suit la taille réelle des cartes à l'écran (voir
//! [`size_for_card`]) : assez fine pour rester nette, sans calcul inutile.
//! Les cartes montrent la partie gardée de chaque fenêtre (rognage) ;
//! l'éditeur de rognage a son propre aperçu de la fenêtre entière
//! ([`start_whole`]).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use polyptyque_core::config::Source;
use polyptyque_core::crop::Crop;
use polyptyque_core::platform::{Platform, VideoCapture, WindowHandle};
use polyptyque_core::video::{fit_rect, nv12_to_rgba};
use polyptyque_engine::resolve;
use slint::{Rgba8Pixel, SharedPixelBuffer};

/// Définitions possibles des aperçus (16:9), de la plus légère à la plus fine.
const PREVIEW_SIZES: [(u32, u32); 3] = [(640, 360), (960, 540), (1280, 720)];
/// Environ 10 images par seconde : fluide à l'œil, négligeable en calcul.
const FRAME_INTERVAL: Duration = Duration::from_millis(100);
const SEARCH_INTERVAL: Duration = Duration::from_secs(2);
/// Définition de l'aperçu de l'éditeur de rognage (fenêtre entière).
const WHOLE_SIZE: (u32, u32) = (1280, 720);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewState {
    Searching,
    Live,
    Minimized,
    /// Fenêtre introuvable : elle peut être ouverte depuis l'aperçu.
    Missing,
    Failed(String),
}

struct Slot {
    frame: Option<SharedPixelBuffer<Rgba8Pixel>>,
    state: PreviewState,
    state_changed: bool,
    /// Taille de la fenêtre (aperçu de la fenêtre entière seulement).
    content: Option<(u32, u32)>,
}

pub struct PreviewWorker {
    pub source_id: String,
    slot: Arc<Mutex<Slot>>,
    stop: Arc<AtomicBool>,
}

impl PreviewWorker {
    /// Nouvelle image et nouvel état depuis le dernier appel.
    pub fn take_update(&self) -> (Option<SharedPixelBuffer<Rgba8Pixel>>, Option<PreviewState>) {
        let mut slot = lock(&self.slot);
        let state = std::mem::take(&mut slot.state_changed).then(|| slot.state.clone());
        (slot.frame.take(), state)
    }

    /// Taille de la fenêtre en pixels, connue après la première image de
    /// l'aperçu de la fenêtre entière.
    pub fn content_size(&self) -> Option<(u32, u32)> {
        lock(&self.slot).content
    }
}

/// Aperçu de la fenêtre entière, sans rognage, pour l'éditeur de rognage :
/// l'image a les proportions de la fenêtre (sans bandes noires).
pub fn start_whole(platform: &Arc<dyn Platform>, source: &Source) -> Option<PreviewWorker> {
    spawn(platform, source, Arc::new(AtomicU64::new(pack(WHOLE_SIZE))), true)
}

fn spawn(platform: &Arc<dyn Platform>, source: &Source, size: Arc<AtomicU64>, whole: bool) -> Option<PreviewWorker> {
    let slot =
        Arc::new(Mutex::new(Slot { frame: None, state: PreviewState::Searching, state_changed: true, content: None }));
    let stop = Arc::new(AtomicBool::new(false));
    let (platform, source_clone, slot_clone, stop_clone) =
        (platform.clone(), source.clone(), slot.clone(), stop.clone());
    thread::Builder::new()
        .name(format!("{}-apercu", source.name))
        .spawn(move || run(platform, source_clone, slot_clone, stop_clone, size, whole))
        .map_err(|e| log::warn!("aperçu de {} indisponible : {e}", source.name))
        .ok()
        .map(|_| PreviewWorker { source_id: source.id.clone(), slot, stop })
}

impl Drop for PreviewWorker {
    fn drop(&mut self) {
        // Le fil s'arrête de lui-même au prochain tour, sans bloquer l'interface.
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Définition d'aperçu adaptée à une carte large de `card_width` pixels
/// physiques : la plus petite qui couvre la carte (à 10 % près), 640×360
/// au minimum.
pub fn size_for_card(card_width: f32) -> (u32, u32) {
    let last = PREVIEW_SIZES[PREVIEW_SIZES.len() - 1];
    if !card_width.is_finite() {
        return PREVIEW_SIZES[0];
    }
    PREVIEW_SIZES.iter().copied().find(|&(w, _)| w as f32 >= card_width * 0.9).unwrap_or(last)
}

pub fn default_size() -> (u32, u32) {
    PREVIEW_SIZES[0]
}

fn pack(size: (u32, u32)) -> u64 {
    (size.0 as u64) << 32 | size.1 as u64
}

fn unpack(value: u64) -> (u32, u32) {
    ((value >> 32) as u32, value as u32)
}

#[derive(Default)]
pub struct PreviewManager {
    workers: Vec<PreviewWorker>,
    /// Définition demandée, commune à tous les aperçus.
    size: Arc<AtomicU64>,
}

impl PreviewManager {
    pub fn start(platform: &Arc<dyn Platform>, sources: &[Source], size: (u32, u32)) -> Self {
        let wanted = Arc::new(AtomicU64::new(pack(size)));
        let workers = sources.iter().filter_map(|source| spawn(platform, source, wanted.clone(), false)).collect();
        Self { workers, size: wanted }
    }

    pub fn workers(&self) -> &[PreviewWorker] {
        &self.workers
    }

    /// Change la définition : chaque aperçu se recrée à la nouvelle taille
    /// en gardant sa dernière image affichée en attendant.
    pub fn set_size(&self, size: (u32, u32)) {
        self.size.store(pack(size), Ordering::Relaxed);
    }

    pub fn stop(&mut self) {
        self.workers.clear();
    }
}

fn run(
    platform: Arc<dyn Platform>,
    source: Source,
    slot: Arc<Mutex<Slot>>,
    stop: Arc<AtomicBool>,
    size: Arc<AtomicU64>,
    whole: bool,
) {
    let set_state = |state: PreviewState| {
        let mut slot = lock(&slot);
        if slot.state != state {
            slot.state = state;
            slot.state_changed = true;
        }
    };
    let mut capture: Option<(Box<dyn VideoCapture>, WindowHandle)> = None;
    let mut capture_size = unpack(size.load(Ordering::Relaxed));
    let mut next_search = Instant::now();
    let mut next_minimized_check = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        let wanted = unpack(size.load(Ordering::Relaxed));
        if wanted != capture_size {
            // Nouvelle définition : la capture est recréée tout de suite.
            capture_size = wanted;
            capture = None;
            next_search = now;
        }
        if capture.is_none() && now >= next_search {
            next_search = now + SEARCH_INTERVAL;
            let windows = platform.list_windows();
            match resolve::find_window(platform.as_ref(), &source, &windows) {
                Some(window) => {
                    let (width, height) = capture_size;
                    let crop = if whole { Crop::default() } else { source.crop };
                    match platform.open_video_capture(window.handle, width, height, source.capture_cursor, crop) {
                        Ok(c) => {
                            capture = Some((c, window.handle));
                            set_state(PreviewState::Live);
                        }
                        Err(e) => set_state(PreviewState::Failed(format!("{e:#}"))),
                    }
                }
                None => set_state(PreviewState::Missing),
            }
        }

        let mut lost = false;
        if let Some((c, handle)) = capture.as_mut() {
            if !platform.is_window_alive(*handle) {
                lost = true;
            } else {
                if now >= next_minimized_check {
                    next_minimized_check = now + Duration::from_millis(500);
                    let minimized = platform.is_window_minimized(*handle);
                    set_state(if minimized { PreviewState::Minimized } else { PreviewState::Live });
                }
                match c.poll() {
                    Ok(Some(frame)) => {
                        let (width, height) = capture_size;
                        let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
                        nv12_to_rgba(&frame, width as usize, height as usize, buffer.make_mut_bytes());
                        let content = if whole { c.content_size() } else { None };
                        if let Some((window_w, window_h)) = content {
                            // Sans les bandes noires : l'image a la forme de la fenêtre.
                            buffer = extract(&buffer, fit_rect(window_w, window_h, width, height));
                        }
                        let mut slot = lock(&slot);
                        slot.frame = Some(buffer);
                        if content.is_some() {
                            slot.content = content;
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        log::debug!("aperçu de {} interrompu : {e:#}", source.name);
                        lost = true;
                    }
                }
            }
        }
        if lost {
            capture = None;
            set_state(PreviewState::Missing);
            next_search = now + SEARCH_INTERVAL;
        }
        thread::sleep(FRAME_INTERVAL);
    }
}

/// Copie d'une partie `(x, y, largeur, hauteur)` d'une image.
fn extract(image: &SharedPixelBuffer<Rgba8Pixel>, area: (u32, u32, u32, u32)) -> SharedPixelBuffer<Rgba8Pixel> {
    let (x, y, w, h) = area;
    let mut out = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    let stride = image.width() as usize * 4;
    let (row_len, left) = (w as usize * 4, x as usize * 4);
    let source = image.as_bytes();
    for (row, target) in out.make_mut_bytes().chunks_exact_mut(row_len).enumerate() {
        let start = (y as usize + row) * stride + left;
        target.copy_from_slice(&source[start..start + row_len]);
    }
    out
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_size_follows_card_width() {
        assert_eq!(size_for_card(300.0), (640, 360));
        assert_eq!(size_for_card(700.0), (640, 360));
        assert_eq!(size_for_card(800.0), (960, 540));
        assert_eq!(size_for_card(1100.0), (1280, 720));
        assert_eq!(size_for_card(2500.0), (1280, 720));
        assert_eq!(size_for_card(f32::NAN), (640, 360));
    }

    #[test]
    fn extract_keeps_the_middle() {
        let mut image = SharedPixelBuffer::<Rgba8Pixel>::new(4, 2);
        for (i, byte) in image.make_mut_bytes().iter_mut().enumerate() {
            *byte = (i / 4) as u8;
        }
        let part = extract(&image, (1, 1, 2, 1));
        assert_eq!((part.width(), part.height()), (2, 1));
        assert_eq!(part.as_bytes()[0], 5);
        assert_eq!(part.as_bytes()[4], 6);
    }

    #[test]
    fn size_packing_round_trips() {
        assert_eq!(unpack(pack((1280, 720))), (1280, 720));
    }
}
