//! Fenêtres montrées par les aperçus et écoutées par les vumètres.
//!
//! Mêmes règles que l'enregistrement (voir
//! [`resolve::find_window_excluding`]) : deux sources d'un même programme
//! (deux VLC, deux applications Chrome) ne montrent jamais la même fenêtre,
//! ni l'une celle que l'autre attend. Pendant un enregistrement, une source
//! montre la fenêtre que le moteur suit pour elle, et jamais celle qu'il
//! suit pour une autre.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use polyptyque_core::config::Source;
use polyptyque_core::platform::{Platform, WindowHandle, WindowInfo};
use polyptyque_engine::resolve::{self, Exclusions};

/// Fenêtres suivies par l'enregistrement en cours, par identifiant de
/// source, tenues à jour par l'interface (vide hors enregistrement).
pub type TrackedWindows = Arc<Mutex<HashMap<String, WindowHandle>>>;

/// Fenêtres prises par un groupe d'aperçus (ou de vumètres) d'une scène.
pub struct WindowClaims {
    /// Sources de la scène, pour les fenêtres qu'attend chacune.
    sources: Vec<Source>,
    tracked: TrackedWindows,
    /// Par fil (numéro unique) : identifiant de la source et fenêtre prise.
    /// Un fil remplacé (aperçu relancé) ne compte pas pour sa propre source.
    held: Mutex<HashMap<u64, (String, WindowHandle)>>,
}

/// Numéro unique d'un fil d'aperçu ou de vumètre.
pub fn next_run() -> u64 {
    static RUNS: AtomicU64 = AtomicU64::new(1);
    RUNS.fetch_add(1, Ordering::Relaxed)
}

impl WindowClaims {
    pub fn new(sources: &[Source], tracked: TrackedWindows) -> Arc<Self> {
        Arc::new(Self { sources: sources.to_vec(), tracked, held: Mutex::new(HashMap::new()) })
    }

    /// Fenêtre à montrer pour `source` (fil `run`), prise jusqu'à
    /// [`WindowClaims::release`] : celle que l'enregistrement suit pour
    /// elle, sinon la meilleure parmi celles que ni l'enregistrement ni les
    /// autres sources n'ont prises.
    pub fn find(
        &self,
        platform: &dyn Platform,
        run: u64,
        source: &Source,
        windows: &[WindowInfo],
    ) -> Option<WindowInfo> {
        self.release(run);
        let tracked = lock(&self.tracked).clone();
        let found = match tracked.get(&source.id) {
            Some(handle) => windows.iter().find(|w| w.handle == *handle).cloned(),
            None => {
                let mut taken: std::collections::HashSet<WindowHandle> =
                    tracked.iter().filter(|(id, _)| **id != source.id).map(|(_, h)| *h).collect();
                taken.extend(self.held_by_others(&source.id));
                let exclusions = Exclusions::among(source, &self.sources, taken, windows);
                resolve::find_window_excluding(platform, source, windows, &exclusions)
            }
        }?;
        // Prise entre-temps par une autre source : nouvel essai au tour suivant.
        let mut held = lock(&self.held);
        if held.iter().any(|(r, (id, h))| *r != run && *id != source.id && *h == found.handle) {
            return None;
        }
        held.insert(run, (source.id.clone(), found.handle));
        Some(found)
    }

    /// La fenêtre `handle`, montrée pour `source`, ne lui revient plus :
    /// l'enregistrement en suit une autre pour elle, ou la suit pour une
    /// autre source. Il faut alors chercher de nouveau.
    pub fn should_switch(&self, source_id: &str, handle: WindowHandle) -> bool {
        let tracked = lock(&self.tracked);
        match tracked.get(source_id) {
            Some(own) => *own != handle,
            None => tracked.iter().any(|(id, h)| id != source_id && *h == handle),
        }
    }

    /// Le fil `run` ne montre plus de fenêtre.
    pub fn release(&self, run: u64) {
        lock(&self.held).remove(&run);
    }

    fn held_by_others(&self, source_id: &str) -> Vec<WindowHandle> {
        lock(&self.held).values().filter(|(id, _)| id != source_id).map(|(_, h)| *h).collect()
    }
}

/// Libère la fenêtre d'un fil à la fin de sa portée, même s'il s'arrête sur
/// une erreur.
pub struct Release(pub Arc<WindowClaims>, pub u64);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.release(self.1);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use polyptyque_core::config::{Source, SourceKind};
    use polyptyque_core::platform::*;

    use super::*;

    struct NoPlatform;

    impl Platform for NoPlatform {
        fn list_windows(&self) -> Vec<WindowInfo> {
            vec![]
        }
        fn is_window_alive(&self, _: WindowHandle) -> bool {
            true
        }
        fn is_window_minimized(&self, _: WindowHandle) -> bool {
            false
        }
        fn process_command_line(&self, _: u32) -> Option<String> {
            None
        }
        fn detect_browsers(&self) -> Vec<BrowserInstall> {
            vec![]
        }
        fn now_hns(&self) -> u64 {
            0
        }
        fn open_video_capture(
            &self,
            _: WindowHandle,
            _: u32,
            _: u32,
            _: bool,
            _: polyptyque_core::crop::Crop,
        ) -> anyhow::Result<Box<dyn VideoCapture>> {
            unimplemented!()
        }
        fn open_audio_capture(&self, _: u32, _: u32, _: u16) -> anyhow::Result<Box<dyn AudioCapture>> {
            unimplemented!()
        }
        fn list_microphones(&self) -> Vec<AudioDevice> {
            vec![]
        }
        fn open_microphone_capture(&self, _: Option<&str>, _: u32, _: u16) -> anyhow::Result<Box<dyn AudioCapture>> {
            unimplemented!()
        }
        fn create_stream_pipe(&self, _: &str) -> anyhow::Result<Box<dyn StreamPipe>> {
            unimplemented!()
        }
        fn prepare_encoder_process(&self, _: &mut Command) {}
        fn begin_recording_session(&self) {}
        fn end_recording_session(&self) {}
        fn free_disk_space(&self, _: &Path) -> Option<u64> {
            None
        }
    }

    fn vlc(handle: WindowHandle, title: &str) -> WindowInfo {
        WindowInfo {
            handle,
            pid: handle as u32,
            title: title.into(),
            class: "Qt5QWindowIcon".into(),
            exe: PathBuf::from(r"C:\VLC\vlc.exe"),
        }
    }

    fn source(id: &str, title: &str) -> Source {
        Source {
            id: id.into(),
            name: id.into(),
            capture_cursor: true,
            relaunch: false,
            crop: Default::default(),
            kind: SourceKind::Application {
                exe_path: PathBuf::from(r"C:\VLC\vlc.exe"),
                window_class: "Qt5QWindowIcon".into(),
                window_title: title.into(),
                launch_args: vec![],
            },
        }
    }

    #[test]
    fn deux_sources_d_un_meme_programme_ne_montrent_pas_la_meme_fenetre() {
        let (a, b) = (source("a", "Film A"), source("b", "Film B"));
        let claims = WindowClaims::new(&[a.clone(), b.clone()], TrackedWindows::default());
        // Titres changés : une seule fenêtre de la bonne classe pour deux sources.
        let windows = vec![vlc(1, "Autre")];
        assert_eq!(claims.find(&NoPlatform, 1, &a, &windows).map(|w| w.handle), Some(1));
        assert_eq!(claims.find(&NoPlatform, 2, &b, &windows), None);
        // Libérée, elle peut revenir à l'autre source.
        claims.release(1);
        assert_eq!(claims.find(&NoPlatform, 2, &b, &windows).map(|w| w.handle), Some(1));
        // Un aperçu relancé de la même source n'est pas gêné par l'ancien.
        assert_eq!(claims.find(&NoPlatform, 3, &b, &windows).map(|w| w.handle), Some(1));
    }

    #[test]
    fn fenetre_exacte_d_une_autre_source_jamais_prise_a_defaut() {
        let (a, b) = (source("a", "Film A"), source("b", "Film B"));
        let claims = WindowClaims::new(&[a.clone(), b.clone()], TrackedWindows::default());
        let windows = vec![vlc(1, "Film B"), vlc(2, "Film A")];
        assert_eq!(claims.find(&NoPlatform, 1, &b, &windows).map(|w| w.handle), Some(1));
        assert_eq!(claims.find(&NoPlatform, 2, &a, &windows).map(|w| w.handle), Some(2));
        let only_b = vec![vlc(1, "Film B")];
        let claims = WindowClaims::new(&[a.clone(), b], TrackedWindows::default());
        assert_eq!(claims.find(&NoPlatform, 1, &a, &only_b), None);
    }

    #[test]
    fn pendant_l_enregistrement_la_fenetre_suivie_par_le_moteur() {
        let (a, b) = (source("a", "Film A"), source("b", "Film B"));
        let tracked = TrackedWindows::default();
        let claims = WindowClaims::new(&[a.clone(), b.clone()], tracked.clone());
        let windows = vec![vlc(1, "Autre"), vlc(2, "Autre")];
        lock(&tracked).insert("a".into(), 2);
        assert_eq!(claims.find(&NoPlatform, 1, &a, &windows).map(|w| w.handle), Some(2));
        // La fenêtre suivie pour « a » n'est jamais montrée pour « b ».
        assert_eq!(claims.find(&NoPlatform, 2, &b, &windows).map(|w| w.handle), Some(1));
        assert!(!claims.should_switch("a", 2));
        assert!(claims.should_switch("b", 2));
        lock(&tracked).insert("a".into(), 1);
        assert!(claims.should_switch("a", 2));
        assert!(claims.should_switch("b", 1));
    }
}
