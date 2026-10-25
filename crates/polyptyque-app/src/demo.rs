//! Plateforme de démonstration, hors Windows : fausses fenêtres animées et
//! faux son. Elle permet de développer et de vérifier l'interface (et même
//! d'enregistrer, avec FFmpeg) sans machine Windows.

use std::f32::consts::TAU;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use polyptyque_core::config::{BrowserFamily, BrowserRef, Config, Scene, Source};
use polyptyque_core::platform::*;

struct DemoWindow {
    handle: WindowHandle,
    /// Titre en français et en anglais.
    title: (&'static str, &'static str),
    class: &'static str,
    exe: &'static str,
    /// Teinte de l'image (chrominance U, V).
    tint: (u8, u8),
}

const CHROME: WindowHandle = 2;
const DISCORD: WindowHandle = 3;

const WINDOWS: [DemoWindow; 4] = [
    DemoWindow {
        handle: 1,
        title: ("Concert en direct — Mozilla Firefox", "Live concert — Mozilla Firefox"),
        class: "MozillaWindowClass",
        exe: "/demo/firefox.exe",
        tint: (96, 168),
    },
    DemoWindow {
        handle: 2,
        title: ("Conférence - Google Chrome", "Conference - Google Chrome"),
        class: "Chrome_WidgetWin_1",
        exe: "/demo/chrome.exe",
        tint: (168, 150),
    },
    DemoWindow {
        handle: 3,
        title: ("#général | Discord", "#general | Discord"),
        class: "Chrome_WidgetWin_1",
        exe: "/demo/Discord.exe",
        tint: (176, 112),
    },
    DemoWindow { handle: 4, title: ("Notes", "Notes"), class: "Notepad", exe: "/demo/notepad.exe", tint: (128, 128) },
];

impl DemoWindow {
    fn title(&self) -> &'static str {
        demo_text(self.title.0, self.title.1)
    }
}

/// Données d'exemple dans la langue du système au lancement, pour des
/// captures d'écran cohérentes. Hors catalogue : la démonstration n'est pas
/// livrée.
fn demo_text(french: &'static str, english: &'static str) -> &'static str {
    match crate::i18n::Language::current() {
        crate::i18n::Language::French => french,
        crate::i18n::Language::English => english,
    }
}

pub struct DemoPlatform {
    start: Instant,
    root: PathBuf,
    /// Fenêtre de Discord réduite (`POLYPTYQUE_DEMO_REDUITE`) jusqu'à ce
    /// qu'elle soit rétablie.
    discord_minimized: AtomicBool,
    /// Fenêtre de Chrome fermée (`POLYPTYQUE_DEMO_PERDUE`).
    chrome_closed: bool,
}

impl DemoPlatform {
    pub fn new(root: &Path) -> Self {
        let _ = fs::create_dir_all(root.join("tubes"));
        Self {
            start: Instant::now(),
            root: root.to_path_buf(),
            discord_minimized: AtomicBool::new(std::env::var_os("POLYPTYQUE_DEMO_REDUITE").is_some()),
            chrome_closed: std::env::var_os("POLYPTYQUE_DEMO_PERDUE").is_some(),
        }
    }

    fn profile(&self, name: &str) -> PathBuf {
        self.root.join("profils").join(name)
    }

    /// Configuration de démonstration : une scène avec trois sources.
    pub fn seed_config(&self) -> Config {
        let mut config = Config::default();
        config.settings.output_dir = self.root.join(demo_text("enregistrements", "recordings"));
        let firefox =
            BrowserRef { name: "Firefox".into(), exe_path: "/demo/firefox.exe".into(), family: BrowserFamily::Gecko };
        let chrome = BrowserRef {
            name: "Google Chrome".into(),
            exe_path: "/demo/chrome.exe".into(),
            family: BrowserFamily::Chromium,
        };
        let mut concert = Source::browser("Concert", firefox, None, &self.root.join("profils"));
        let mut conference =
            Source::browser(demo_text("Conférence", "Conference"), chrome, None, &self.root.join("profils"));
        set_profile(&mut concert, self.profile("concert"));
        set_profile(&mut conference, self.profile("conference"));
        let discord = WindowInfo {
            handle: 3,
            pid: 103,
            title: WINDOWS[2].title().into(),
            class: WINDOWS[2].class.into(),
            exe: WINDOWS[2].exe.into(),
        };
        config.active_scene_mut().name = "Podcast".into();
        for source in [concert, conference, Source::application("Discord", &discord, false, true)] {
            config.add_source(source);
        }
        config.scenes.push(Scene::new("Archive"));
        config
    }
}

fn set_profile(source: &mut Source, dir: PathBuf) {
    if let polyptyque_core::config::SourceKind::Browser { profile_dir, .. } = &mut source.kind {
        *profile_dir = dir;
    }
}

impl Platform for DemoPlatform {
    fn list_windows(&self) -> Vec<WindowInfo> {
        WINDOWS
            .iter()
            .filter(|w| !(self.chrome_closed && w.handle == CHROME))
            .map(|w| WindowInfo {
                handle: w.handle,
                pid: 100 + w.handle as u32,
                title: w.title().into(),
                class: w.class.into(),
                exe: w.exe.into(),
            })
            .collect()
    }

    fn is_window_alive(&self, handle: WindowHandle) -> bool {
        !(self.chrome_closed && handle == CHROME)
    }

    fn is_window_minimized(&self, handle: WindowHandle) -> bool {
        handle == DISCORD && self.discord_minimized.load(Ordering::Relaxed)
    }

    fn restore_window(&self, handle: WindowHandle) -> Result<()> {
        if handle == DISCORD {
            self.discord_minimized.store(false, Ordering::Relaxed);
        }
        Ok(())
    }

    fn process_command_line(&self, pid: u32) -> Option<String> {
        match pid {
            101 => Some(format!("firefox.exe -profile \"{}\" -no-remote", self.profile("concert").display())),
            102 => Some(format!("chrome.exe --user-data-dir={}", self.profile("conference").display())),
            _ => None,
        }
    }

    fn detect_browsers(&self) -> Vec<BrowserInstall> {
        vec![
            BrowserInstall {
                name: "Firefox".into(),
                exe_path: "/demo/firefox.exe".into(),
                family: BrowserFamily::Gecko,
            },
            BrowserInstall {
                name: "Google Chrome".into(),
                exe_path: "/demo/chrome.exe".into(),
                family: BrowserFamily::Chromium,
            },
        ]
    }

    fn now_hns(&self) -> u64 {
        self.start.elapsed().as_nanos() as u64 / 100
    }

    fn open_video_capture(
        &self,
        handle: WindowHandle,
        width: u32,
        height: u32,
        _: bool,
    ) -> Result<Box<dyn VideoCapture>> {
        let tint = WINDOWS.iter().find(|w| w.handle == handle).map_or((128, 128), |w| w.tint);
        Ok(Box::new(DemoVideo { start: self.start, width, height, tint }))
    }

    fn open_audio_capture(&self, pid: u32, sample_rate: u32, channels: u16) -> Result<Box<dyn AudioCapture>> {
        Ok(Box::new(DemoAudio {
            start: self.start,
            sample_rate,
            channels,
            pitch: 220.0 * pid.saturating_sub(100).max(1) as f32,
            next_frame: None,
            phase: 0.0,
        }))
    }

    fn list_microphones(&self) -> Vec<AudioDevice> {
        vec![
            AudioDevice {
                id: "demo-usb".into(), name: demo_text("Micro USB de démonstration", "Demo USB mic").into()
            },
            AudioDevice { id: "demo-casque".into(), name: demo_text("Micro du casque", "Headset mic").into() },
        ]
    }

    fn open_microphone_capture(
        &self,
        _: Option<&str>,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Box<dyn AudioCapture>> {
        Ok(Box::new(DemoAudio { start: self.start, sample_rate, channels, pitch: 330.0, next_frame: None, phase: 0.0 }))
    }

    fn create_stream_pipe(&self, name: &str) -> Result<Box<dyn StreamPipe>> {
        let path = self.root.join("tubes").join(name);
        let status = Command::new("mkfifo").arg(&path).status()?;
        anyhow::ensure!(status.success(), "mkfifo a échoué");
        Ok(Box::new(Fifo { path }))
    }

    fn prepare_encoder_process(&self, _: &mut Command) {}
    fn begin_recording_session(&self) {}
    fn end_recording_session(&self) {}

    fn free_disk_space(&self, _: &Path) -> Option<u64> {
        Some(412_300_000_000)
    }

    fn os_description(&self) -> String {
        format!("{} ({})", std::env::consts::OS, demo_text("démonstration", "demo"))
    }

    fn graphics_adapters(&self) -> Vec<String> {
        vec![demo_text("Carte graphique de démonstration", "Demo graphics card").into()]
    }

    /// Démonstration : rien ne s'ouvre, l'action est seulement affichée.
    fn shell_open(&self, target: &str) -> Result<()> {
        log::info!("démo : ouverture de {target}");
        println!("démo : ouverture de {target}");
        Ok(())
    }

    fn reveal_file(&self, path: &Path) -> Result<()> {
        log::info!("démo : fichier montré : {}", path.display());
        println!("démo : fichier montré : {}", path.display());
        Ok(())
    }
}

/// Image animée : dégradé teinté et barre lumineuse qui défile.
struct DemoVideo {
    start: Instant,
    width: u32,
    height: u32,
    tint: (u8, u8),
}

impl VideoCapture for DemoVideo {
    fn poll(&mut self) -> Result<Option<Nv12Frame>> {
        let (w, h) = (self.width as usize, self.height as usize);
        let t = self.start.elapsed().as_secs_f32();
        let bar = ((t * 0.25).fract() * w as f32) as usize;
        let mut frame = vec![0u8; w * h * 3 / 2];
        for y in 0..h {
            for x in 0..w {
                let base = 40.0 + 70.0 * (y as f32 / h as f32) + 25.0 * ((x as f32 / w as f32) * TAU + t).sin();
                let glow = if x.abs_diff(bar) < w / 24 { 90.0 } else { 0.0 };
                frame[y * w + x] = (base + glow).clamp(16.0, 235.0) as u8;
            }
        }
        for pair in frame[w * h..].chunks_exact_mut(2) {
            pair[0] = self.tint.0;
            pair[1] = self.tint.1;
        }
        Ok(Some(Arc::new(frame)))
    }
}

/// Son continu dont le volume ondule, daté par l'horloge commune.
struct DemoAudio {
    start: Instant,
    sample_rate: u32,
    channels: u16,
    pitch: f32,
    next_frame: Option<u64>,
    phase: f32,
}

impl AudioCapture for DemoAudio {
    fn read(&mut self, timeout_ms: u32, out: &mut Vec<AudioPacket>) -> Result<()> {
        std::thread::sleep(Duration::from_millis(timeout_ms.min(10) as u64));
        let now_frames = self.start.elapsed().as_nanos() as u64 * self.sample_rate as u64 / 1_000_000_000;
        let mut next = self.next_frame.unwrap_or(now_frames);
        while next + 480 <= now_frames {
            let volume = 0.35 + 0.3 * ((next as f32 / self.sample_rate as f32) * 0.7).sin();
            let mut data = Vec::with_capacity(480 * self.channels as usize * 4);
            for _ in 0..480 {
                let sample = (self.phase * TAU).sin() * volume;
                self.phase = (self.phase + self.pitch / self.sample_rate as f32).fract();
                for _ in 0..self.channels {
                    data.extend_from_slice(&sample.to_le_bytes());
                }
            }
            let timestamp = next * 10_000_000 / self.sample_rate as u64;
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
        Ok(Box::new(fs::OpenOptions::new().write(true).open(&self.path)?))
    }
}
