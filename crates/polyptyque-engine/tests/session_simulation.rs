//! Session complète simulée : fausses fenêtres et faux son, vrai FFmpeg.
//!
//! Vérifie de bout en bout ce qui ne dépend pas de Windows : tubes, cadence
//! constante, placement du son, fenêtres absentes ou tardives, arrêt propre
//! et fichiers MKV lisibles aux bonnes durées. Ignoré si FFmpeg n'est pas
//! installé ; ne fonctionne que sous Unix (tubes nommés par `mkfifo`).

#![cfg(unix)]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use polyptyque_core::config::{new_id, Codec, HardwarePreference, Settings, Source, SourceKind, VideoSettings};
use polyptyque_core::encoder::{EncoderChoice, EncoderKind};
use polyptyque_core::platform::*;
use polyptyque_engine::{remux, MicrophonePlan, Session, SessionPlan, SourceState};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;
const FPS: u32 = 30;

struct SimPlatform {
    start: Instant,
    pipes_dir: PathBuf,
}

impl SimPlatform {
    fn elapsed_hns(&self) -> u64 {
        self.start.elapsed().as_nanos() as u64 / 100
    }
}

fn window(handle: isize, title: &str) -> WindowInfo {
    WindowInfo {
        handle,
        pid: handle as u32,
        title: title.into(),
        class: "Classe".into(),
        exe: PathBuf::from(format!("/sim/{title}")),
    }
}

impl Platform for SimPlatform {
    fn list_windows(&self) -> Vec<WindowInfo> {
        let mut windows = vec![window(1, "immediate")];
        // La deuxième fenêtre n'apparaît qu'après une seconde.
        if self.start.elapsed() > Duration::from_secs(1) {
            windows.push(window(2, "tardive"));
        }
        windows
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
        // Décalage arbitraire : l'horloge ne commence pas à zéro.
        5_000_000_000 + self.elapsed_hns()
    }
    fn open_video_capture(
        &self,
        _: WindowHandle,
        w: u32,
        h: u32,
        _: bool,
        _: polyptyque_core::crop::Crop,
    ) -> Result<Box<dyn VideoCapture>> {
        Ok(Box::new(SimVideo { width: w, height: h, counter: 0 }))
    }
    fn open_audio_capture(&self, _: u32, sample_rate: u32, channels: u16) -> Result<Box<dyn AudioCapture>> {
        Ok(Box::new(SimAudio { start: self.start, sample_rate, channels, next_frame: None, phase: 0.0 }))
    }
    fn list_microphones(&self) -> Vec<AudioDevice> {
        vec![AudioDevice { id: "sim".into(), name: "Micro simulé".into() }]
    }
    fn open_microphone_capture(
        &self,
        _: Option<&str>,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Box<dyn AudioCapture>> {
        Ok(Box::new(SimAudio { start: self.start, sample_rate, channels, next_frame: None, phase: 0.0 }))
    }
    fn create_stream_pipe(&self, name: &str) -> Result<Box<dyn StreamPipe>> {
        let path = self.pipes_dir.join(name);
        let status = Command::new("mkfifo").arg(&path).status()?;
        anyhow::ensure!(status.success(), "mkfifo a échoué");
        Ok(Box::new(Fifo { path }))
    }
    fn prepare_encoder_process(&self, _: &mut Command) {}
    fn begin_recording_session(&self) {}
    fn end_recording_session(&self) {}
    fn free_disk_space(&self, _: &Path) -> Option<u64> {
        Some(u64::MAX)
    }
}

struct SimVideo {
    width: u32,
    height: u32,
    counter: u8,
}

impl VideoCapture for SimVideo {
    fn poll(&mut self) -> Result<Option<Nv12Frame>> {
        self.counter = self.counter.wrapping_add(1);
        let luma = (self.width * self.height) as usize;
        let mut frame = vec![self.counter; luma + luma / 2];
        frame[luma..].fill(128);
        Ok(Some(Arc::new(frame)))
    }
}

/// Son continu (sinus 440 Hz) daté par l'horloge commune, livré par
/// paquets de 10 ms comme le ferait WASAPI.
struct SimAudio {
    start: Instant,
    sample_rate: u32,
    channels: u16,
    next_frame: Option<u64>,
    phase: f32,
}

impl AudioCapture for SimAudio {
    fn read(&mut self, timeout_ms: u32, out: &mut Vec<AudioPacket>) -> Result<()> {
        thread::sleep(Duration::from_millis(timeout_ms.min(10) as u64));
        let now_frames = self.start.elapsed().as_nanos() as u64 * self.sample_rate as u64 / 1_000_000_000;
        let mut next = self.next_frame.unwrap_or(now_frames);
        while next + 480 <= now_frames {
            let mut data = Vec::with_capacity(480 * self.channels as usize * 4);
            for _ in 0..480 {
                let sample = (self.phase * std::f32::consts::TAU).sin() * 0.5;
                self.phase = (self.phase + 440.0 / self.sample_rate as f32).fract();
                for _ in 0..self.channels {
                    data.extend_from_slice(&sample.to_le_bytes());
                }
            }
            let timestamp = 5_000_000_000 + next * 10_000_000 / self.sample_rate as u64;
            out.push(AudioPacket { data, timestamp_hns: Some(timestamp), silent: false });
            next += 480;
        }
        self.next_frame = Some(next);
        Ok(())
    }
}

struct Fifo {
    path: PathBuf,
}

impl StreamPipe for Fifo {
    fn ffmpeg_path(&self) -> String {
        self.path.display().to_string()
    }
    fn connect(self: Box<Self>) -> Result<Box<dyn Write + Send>> {
        Ok(Box::new(OpenOptions::new().write(true).open(&self.path)?))
    }
}

fn application(name: &str, title: &str) -> Source {
    Source {
        id: new_id(),
        name: name.into(),
        capture_cursor: false,
        relaunch: false,
        crop: Default::default(),
        kind: SourceKind::Application {
            exe_path: PathBuf::from(format!("/sim/{title}")),
            window_class: "Classe".into(),
            window_title: title.into(),
            launch_args: vec![],
        },
    }
}

fn ffmpeg_available() -> bool {
    ["ffmpeg", "ffprobe"]
        .iter()
        .all(|tool| Command::new(tool).arg("-version").output().map(|o| o.status.success()).unwrap_or(false))
}

/// Durées (secondes) de la vidéo et du son d'un fichier, en comptant les
/// images et les échantillons réellement décodés.
fn measure(path: &Path) -> (f64, f64) {
    let probe = |stream: &str, entry: &str| -> String {
        let output = Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", stream, "-count_frames", "-show_entries"])
            .arg(format!("stream={entry}"))
            .args(["-of", "default=noprint_wrappers=1:nokey=1"])
            .arg(path)
            .output()
            .expect("ffprobe");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    let video_frames: f64 = probe("v:0", "nb_read_frames").parse().expect("images");
    // Échantillons audio : décodage complet en PCM brut.
    let pcm = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-f", "s16le", "-ac", "1", "-ar", "48000", "-"])
        .output()
        .expect("décodage audio");
    let audio_samples = pcm.stdout.len() as f64 / 2.0;
    (video_frames / FPS as f64, audio_samples / 48_000.0)
}

/// Durée (secondes) du son décodé d'un fichier.
fn audio_seconds(path: &Path) -> f64 {
    let pcm = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-f", "s16le", "-ac", "1", "-ar", "48000", "-"])
        .output()
        .expect("décodage audio");
    pcm.stdout.len() as f64 / 2.0 / 48_000.0
}

/// Étiquettes colorimétriques du flux vidéo.
fn color_tags(path: &Path) -> String {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
        .arg("stream=color_range,color_space,color_transfer,color_primaries")
        .args(["-of", "compact=nk=0:p=0"])
        .arg(path)
        .output()
        .expect("ffprobe");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

struct Sim {
    root: PathBuf,
    platform: Arc<SimPlatform>,
}

fn sim() -> Sim {
    let root = std::env::temp_dir().join(format!("polyptyque-sim-{}", new_id()));
    let pipes_dir = root.join("tubes");
    fs::create_dir_all(&pipes_dir).unwrap();
    Sim { platform: Arc::new(SimPlatform { start: Instant::now(), pipes_dir }), root }
}

fn plan(root: &Path, sources: Vec<Source>) -> SessionPlan {
    SessionPlan {
        scene_name: "Simulation".into(),
        sources,
        settings: Settings {
            output_dir: root.join("sorties"),
            filename_template: "{scène}_{source}".into(),
            video: VideoSettings {
                width: WIDTH,
                height: HEIGHT,
                fps: FPS,
                codec: Codec::H264,
                hardware: HardwarePreference::Cpu,
                bitrate_kbps: 500,
            },
            ..Default::default()
        },
        encoder: EncoderChoice { kind: EncoderKind::Software, codec: Codec::H264 },
        ffmpeg: PathBuf::from("ffmpeg"),
        log_dir: root.join("journaux"),
        fallback_dir: root.join("secours"),
        microphone: None,
    }
}

/// Image et son de chaque fichier calés l'un sur l'autre ; renvoie la
/// durée totale de la vidéo.
fn check_parts(name: &str, outputs: &[PathBuf]) -> f64 {
    let mut total = 0.0;
    for output in outputs {
        let (video, audio) = measure(output);
        eprintln!("{name} · {} : vidéo {video:.3} s, son {audio:.3} s", output.file_name().unwrap().to_string_lossy());
        assert!((audio - video).abs() < 0.1, "{} : son {audio} s, vidéo {video} s", output.display());
        total += video;
    }
    total
}

/// Motif `pkill -f` qui désigne littéralement `text`.
fn regex_escape(text: &str) -> String {
    text.chars().flat_map(|c| if "\\.^$|?*+()[]{}".contains(c) { vec!['\\', c] } else { vec![c] }).collect()
}

#[test]
fn crashed_encoder_resumes_in_a_new_part() {
    let pkill = Command::new("pkill").arg("--version").output().is_ok_and(|o| o.status.success());
    if !ffmpeg_available() || !pkill {
        eprintln!("FFmpeg ou pkill absent : test ignoré");
        return;
    }
    let Sim { root, platform } = sim();
    let plan = plan(&root, vec![application("Plantée", "immediate"), application("Témoin", "immediate")]);
    let mut session = Session::start(platform, plan).expect("démarrage");
    thread::sleep(Duration::from_secs(3));
    // Plantage brutal du FFmpeg de la première source.
    let pattern = format!("{}/.*-0-p1-video", regex_escape(&root.join("tubes").display().to_string()));
    let killed = Command::new("pkill").args(["-KILL", "-f", &pattern]).status().unwrap();
    assert!(killed.success(), "FFmpeg de la première source introuvable");
    thread::sleep(Duration::from_secs(4));
    let statuses = session.status();
    assert_eq!(statuses[0].part, 2, "{:?}", statuses[0]);
    assert_eq!(statuses[0].state, SourceState::Connected, "{:?}", statuses[0]);
    assert_eq!(statuses[1].part, 1);
    let elapsed = session.elapsed().as_secs_f64();
    let summaries = session.stop();

    let crashed = &summaries[0];
    assert_eq!(crashed.outputs.len(), 2, "{:?}", crashed.outputs);
    assert!(crashed.outputs[1].ends_with("Simulation_Plantée_partie2.mkv"));
    let total = check_parts(&crashed.name, &crashed.outputs);
    // Seule la reprise (environ une seconde) manque.
    assert!(total > elapsed - 2.0 && total <= elapsed + 0.25, "{total} s pour {elapsed} s");
    let witness = &summaries[1];
    assert_eq!(witness.outputs.len(), 1);
    assert!((check_parts(&witness.name, &witness.outputs) - elapsed).abs() < 0.25);

    // La première partie, coupée net, n'a ni durée ni index : la réparation
    // les reconstruit sans rien perdre, et remplace le fichier sur place.
    let sim_platform = SimPlatform { start: Instant::now(), pipes_dir: root.clone() };
    let ffmpeg = Path::new("ffmpeg");
    let cut = &crashed.outputs[0];
    let before = measure(cut);
    assert!(remux::needs_repair(&remux::inspect(&sim_platform, ffmpeg, cut).unwrap()));
    let mut progress = Vec::new();
    let repaired = remux::repair(&sim_platform, ffmpeg, cut, &mut |p| progress.push(p), &AtomicBool::new(false))
        .expect("réparation");
    assert_eq!(&repaired, cut);
    let info = remux::inspect(&sim_platform, ffmpeg, cut).unwrap();
    assert!(!remux::needs_repair(&info), "{info:?}");
    assert_eq!(progress.last(), Some(&1.0));
    let after = measure(cut);
    assert!((after.0 - before.0).abs() < 0.05 && (after.1 - before.1).abs() < 0.05, "{before:?} -> {after:?}");
    let leftovers: Vec<_> = fs::read_dir(cut.parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".reparation") || n.contains(".avant-reparation") || n.contains(".partiel"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lost_output_folder_continues_in_fallback() {
    let pkill = Command::new("pkill").arg("--version").output().is_ok_and(|o| o.status.success());
    if !ffmpeg_available() || !pkill {
        eprintln!("FFmpeg ou pkill absent : test ignoré");
        return;
    }
    let Sim { root, platform } = sim();
    let plan = plan(&root, vec![application("Débranchée", "immediate")]);
    let mut session = Session::start(platform, plan).expect("démarrage");
    thread::sleep(Duration::from_secs(3));
    // Disque débranché : le dossier disparaît (remplacé par un fichier, pour
    // qu'il ne puisse pas être recréé) et l'encodeur tombe en erreur.
    let outputs = root.join("sorties");
    fs::rename(&outputs, root.join("sorties-debranchees")).unwrap();
    fs::write(&outputs, b"").unwrap();
    let pattern = format!("{}/.*-0-p1-video", regex_escape(&root.join("tubes").display().to_string()));
    assert!(Command::new("pkill").args(["-KILL", "-f", &pattern]).status().unwrap().success());
    thread::sleep(Duration::from_secs(4));
    let status = &session.status()[0];
    assert_eq!(status.part, 2, "{status:?}");
    assert!(status.output.starts_with(root.join("secours")), "{status:?}");
    assert!(status.error.as_deref().is_some_and(|e| e.contains("inaccessible")), "{status:?}");
    let summaries = session.stop();
    let part2 = &summaries[0].outputs[1];
    assert!(part2.ends_with("secours/Simulation_Débranchée_partie2.mkv"), "{part2:?}");
    assert!(measure(part2).0 > 1.0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn refused_gpu_encoder_falls_back_to_the_processor() {
    if !ffmpeg_available() {
        eprintln!("FFmpeg absent : test ignoré");
        return;
    }
    let Sim { root, platform } = sim();
    let mut plan = plan(&root, vec![application("Carte", "immediate")]);
    // Sans carte NVIDIA, NVENC refuse de démarrer à chaque essai.
    plan.encoder = EncoderChoice { kind: EncoderKind::Nvenc, codec: Codec::H264 };
    let mut session = Session::start(platform, plan).expect("démarrage");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let status = &session.status()[0];
        if status.error.as_deref().is_some_and(|e| e.contains("passe sur le processeur")) {
            break;
        }
        assert!(Instant::now() < deadline, "pas de bascule sur le processeur : {status:?}");
        assert_ne!(status.state, SourceState::Failed, "{status:?}");
        thread::sleep(Duration::from_millis(200));
    }
    thread::sleep(Duration::from_secs(3));
    let summaries = session.stop();
    let summary = &summaries[0];
    assert!(summary.error.as_deref().is_some_and(|e| e.contains("processeur")), "{summary:?}");
    // Les essais ratés n'ont laissé aucun fichier ; la suite est bien écrite.
    assert_eq!(summary.outputs.len(), 1, "{:?}", summary.outputs);
    assert!(measure(&summary.outputs[0]).0 > 2.0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn frozen_encoder_is_detected_and_replaced() {
    let pkill = Command::new("pkill").arg("--version").output().is_ok_and(|o| o.status.success());
    if !ffmpeg_available() || !pkill {
        eprintln!("FFmpeg ou pkill absent : test ignoré");
        return;
    }
    let Sim { root, platform } = sim();
    let mut session =
        Session::start(platform, plan(&root, vec![application("Figée", "immediate")])).expect("démarrage");
    thread::sleep(Duration::from_secs(3));
    // FFmpeg suspendu : il ne lit plus ses tubes, sans pour autant s'arrêter.
    let pattern = format!("{}/.*-0-p1-video", regex_escape(&root.join("tubes").display().to_string()));
    let frozen = Command::new("pkill").args(["-STOP", "-f", &pattern]).status().unwrap();
    assert!(frozen.success(), "FFmpeg introuvable");
    let deadline = Instant::now() + Duration::from_secs(45);
    while session.status()[0].part < 2 {
        assert!(Instant::now() < deadline, "encodeur figé non détecté : {:?}", session.status()[0]);
        thread::sleep(Duration::from_millis(250));
    }
    thread::sleep(Duration::from_secs(2));
    assert_eq!(session.status()[0].state, SourceState::Connected);
    let elapsed = session.elapsed().as_secs_f64();
    let summaries = session.stop();
    let outputs = &summaries[0].outputs;
    assert_eq!(outputs.len(), 2, "{outputs:?}");
    // Rien de ce qui attendait pendant le blocage n'est perdu, hormis ce
    // que FFmpeg gardait en mémoire.
    let total = check_parts("Figée", outputs);
    assert!(total > elapsed - 2.0 && total <= elapsed + 0.25, "{total} s pour {elapsed} s");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn sources_start_and_stop_individually() {
    if !ffmpeg_available() {
        eprintln!("FFmpeg absent : test ignoré");
        return;
    }
    let Sim { root, platform } = sim();
    let first = application("Première", "immediate");
    let second = application("Seconde", "immediate");
    let mut session = Session::start(platform, plan(&root, vec![first.clone()])).expect("démarrage");
    thread::sleep(Duration::from_secs(1));
    session.add_source(&second).expect("ajout");
    assert!(session.add_source(&second).is_err(), "une source ne peut pas enregistrer deux fois");
    assert_eq!(session.recording_count(), 2);
    thread::sleep(Duration::from_secs(2));
    assert!(session.stop_source(&second.id));
    assert!(!session.is_recording(&second.id));
    assert_eq!(session.recording_count(), 1);
    // La finalisation se fait en arrière-plan.
    let deadline = Instant::now() + Duration::from_secs(10);
    while session.status()[1].state != SourceState::Finished {
        assert!(Instant::now() < deadline, "finalisation trop longue");
        thread::sleep(Duration::from_millis(100));
    }
    thread::sleep(Duration::from_secs(1));
    // Reprise : un nouveau fichier.
    session.add_source(&second).expect("reprise");
    thread::sleep(Duration::from_secs(1));
    let elapsed = session.elapsed().as_secs_f64();
    let summaries = session.stop();
    let names: Vec<&str> = summaries.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Première", "Seconde", "Seconde"]);
    let durations: Vec<f64> = summaries.iter().map(|s| check_parts(&s.name, &s.outputs)).collect();
    assert!((durations[0] - elapsed).abs() < 0.25, "{durations:?} pour {elapsed} s");
    assert!((durations[1] - 2.0).abs() < 0.3, "{durations:?}");
    assert!((durations[2] - 1.0).abs() < 0.3, "{durations:?}");
    assert!(summaries[2].outputs[0].ends_with("Simulation_Seconde (2).mkv"), "{:?}", summaries[2].outputs);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn simulated_session_produces_synchronised_files() {
    if !ffmpeg_available() {
        eprintln!("FFmpeg absent : test ignoré");
        return;
    }
    let Sim { root, platform } = sim();
    let mut plan = plan(
        &root,
        vec![
            application("Immédiate", "immediate"),
            application("Tardive", "tardive"),
            application("Absente", "absente"),
        ],
    );
    plan.microphone = Some(MicrophonePlan { device_id: None });

    let mut session = Session::start(platform.clone(), plan).expect("démarrage");
    let recording = Duration::from_secs(4);
    let started = Instant::now();
    while started.elapsed() < recording {
        thread::sleep(Duration::from_millis(250));
        for status in session.status() {
            assert!(status.error.is_none() || status.state != SourceState::Failed, "{:?}", status);
        }
    }
    let statuses = session.status();
    assert_eq!(statuses[0].state, SourceState::Connected);
    assert_eq!(statuses[1].state, SourceState::Connected);
    assert_eq!(statuses[2].state, SourceState::Searching);
    assert_eq!(session.microphone_status().map(|m| m.state), Some(SourceState::Connected));
    let elapsed = session.elapsed().as_secs_f64();
    let mut summaries = session.stop();

    // Micro : fichier de son seul, sur la même horloge.
    let micro = summaries.pop().expect("bilan du micro");
    assert_eq!(micro.name, "Micro");
    assert!(micro.outputs[0].ends_with("Simulation_Micro.mka"), "{:?}", micro.outputs);
    let micro_seconds = audio_seconds(&micro.outputs[0]);
    eprintln!("Micro : son {micro_seconds:.3} s, session {elapsed:.3} s");
    assert!((micro_seconds - elapsed).abs() < 0.25, "micro {micro_seconds} s pour {elapsed} s");
    let m4a =
        remux::convert(platform.as_ref(), Path::new("ffmpeg"), &micro.outputs[0], &mut |_| {}, &AtomicBool::new(false))
            .expect("conversion du micro");
    assert!(m4a.ends_with("Simulation_Micro.m4a"));
    assert!((audio_seconds(&m4a) - micro_seconds).abs() < 0.03);

    assert_eq!(summaries.len(), 3);
    for summary in &summaries {
        assert!(summary.error.is_none(), "{} : {:?}", summary.name, summary.error);
        assert_eq!(summary.outputs.len(), 1, "{}", summary.name);
        let output = &summary.outputs[0];
        assert!(output.exists(), "fichier manquant : {}", output.display());
        assert_eq!(
            color_tags(output),
            "color_range=tv|color_space=bt709|color_transfer=bt709|color_primaries=bt709",
            "{}",
            summary.name
        );
        let (video, audio) = measure(output);
        eprintln!("{} : vidéo {video:.3} s, son {audio:.3} s, session {elapsed:.3} s", summary.name);
        // Toutes les sources ont la durée de la session, image et son calés.
        assert!((video - elapsed).abs() < 0.25, "{} : vidéo {video} s pour {elapsed} s", summary.name);
        assert!((audio - video).abs() < 0.1, "{} : son {audio} s, vidéo {video} s", summary.name);
    }
    assert!(summaries[0].outputs[0].ends_with("Simulation_Immédiate.mkv"));

    // Conversion sans perte en MP4 : mêmes durées, mêmes couleurs, aucune
    // erreur de décodage, fichier d'origine conservé.
    let mkv = &summaries[0].outputs[0];
    let mut progress = Vec::new();
    let mp4 = remux::convert(
        &SimPlatform { start: Instant::now(), pipes_dir: root.clone() },
        Path::new("ffmpeg"),
        mkv,
        &mut |p| progress.push(p),
        &AtomicBool::new(false),
    )
    .expect("conversion");
    assert!(mp4.ends_with("Simulation_Immédiate.mp4"));
    assert!(mkv.exists());
    assert_eq!(progress.last(), Some(&1.0));
    let (mkv_video, mkv_audio) = measure(mkv);
    let (mp4_video, mp4_audio) = measure(&mp4);
    eprintln!("MP4 : vidéo {mp4_video:.3} s, son {mp4_audio:.3} s");
    assert!((mp4_video - mkv_video).abs() < 0.001, "vidéo {mp4_video} s au lieu de {mkv_video} s");
    assert!((mp4_audio - mkv_audio).abs() < 0.03, "son {mp4_audio} s au lieu de {mkv_audio} s");
    assert!(color_tags(&mp4).contains("color_space=bt709"), "{}", color_tags(&mp4));
    let decode =
        Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(&mp4).args(["-f", "null", "-"]).output().unwrap();
    assert!(decode.stderr.is_empty(), "{}", String::from_utf8_lossy(&decode.stderr));
    // Une deuxième conversion n'écrase pas la première.
    let again = remux::convert(
        &SimPlatform { start: Instant::now(), pipes_dir: root.clone() },
        Path::new("ffmpeg"),
        mkv,
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .expect("deuxième conversion");
    assert!(again.ends_with("Simulation_Immédiate (2).mp4"));
    // Une conversion annulée ne laisse rien derrière elle.
    let cancelled = remux::convert(
        &SimPlatform { start: Instant::now(), pipes_dir: root.clone() },
        Path::new("ffmpeg"),
        mkv,
        &mut |_| {},
        &AtomicBool::new(true),
    );
    assert!(cancelled.is_err());
    assert!(!mkv.with_file_name("Simulation_Immédiate (3).mp4").exists());
    fs::remove_dir_all(root).unwrap();
}
