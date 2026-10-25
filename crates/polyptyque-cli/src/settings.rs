//! Réglages d'encodage et de sortie.

use std::path::PathBuf;

use anyhow::Result;
use dialoguer::{Input, Select};
use polyptyque_core::config::{Codec, Config, HardwarePreference};
use polyptyque_core::naming::TEMPLATE_HELP;
use polyptyque_core::platform::Platform;

use crate::ui;

const RESOLUTIONS: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)];
const FRAME_RATES: [u32; 5] = [24, 25, 30, 50, 60];
const AUDIO_BITRATES: [u32; 5] = [128, 160, 192, 256, 320];

pub fn menu(platform: &dyn Platform, config: &mut Config) -> Result<()> {
    let theme = ui::theme();
    loop {
        let s = &config.settings;
        let items = [
            format!("Résolution : {}x{}", s.video.width, s.video.height),
            format!("Images par seconde : {}", s.video.fps),
            format!("Débit vidéo : {} kb/s", ui::thousands(s.video.bitrate_kbps as u64)),
            format!("Format vidéo : {}", s.video.codec.label()),
            format!("Matériel d'encodage : {}", s.video.hardware.label()),
            format!("Débit audio : {} kb/s", s.audio.bitrate_kbps),
            format!("Dossier des enregistrements : {}", s.output_dir.display()),
            format!("Modèle de nom de fichier : {}", s.filename_template),
            format!("Conversion MP4 à la fin de l'enregistrement : {}", if s.convert_to_mp4 { "oui" } else { "non" }),
            format!(
                "Micro (fichier séparé) : {}",
                if s.microphone.enabled { s.microphone.label().to_string() } else { "désactivé".into() }
            ),
            "Retour".to_string(),
        ];
        let choice = Select::with_theme(&theme)
            .with_prompt("Réglages (communs à toutes les sources)")
            .items(&items)
            .default(items.len() - 1)
            .interact()?;
        let settings = &mut config.settings;
        match choice {
            0 => {
                let mut options: Vec<String> = RESOLUTIONS.iter().map(|(w, h)| format!("{w}x{h}")).collect();
                options.push("Personnalisée".into());
                let i = Select::with_theme(&theme)
                    .with_prompt("Résolution des fichiers")
                    .items(&options)
                    .default(1)
                    .interact()?;
                if let Some(&(w, h)) = RESOLUTIONS.get(i) {
                    settings.video.width = w;
                    settings.video.height = h;
                } else {
                    let text: String = Input::with_theme(&theme)
                        .with_prompt("Largeur x hauteur (par exemple 1600x900)")
                        .interact_text()?;
                    match parse_size(&text) {
                        Some((w, h)) => {
                            settings.video.width = w;
                            settings.video.height = h;
                        }
                        None => {
                            ui::failure("Format non reconnu.");
                            ui::pause();
                        }
                    }
                }
            }
            1 => {
                let mut options: Vec<String> = FRAME_RATES.iter().map(|f| format!("{f} i/s")).collect();
                options.push("Personnalisé".into());
                let i = Select::with_theme(&theme)
                    .with_prompt("Images par seconde")
                    .items(&options)
                    .default(4)
                    .interact()?;
                settings.video.fps = match FRAME_RATES.get(i) {
                    Some(&fps) => fps,
                    None => Input::with_theme(&theme)
                        .with_prompt("Images par seconde (1 à 240)")
                        .default(settings.video.fps)
                        .interact_text()?,
                };
            }
            2 => {
                println!("Repères en 1080p à 60 i/s : 6 000 (léger), 12 000 (bon), 20 000 (très bon).");
                settings.video.bitrate_kbps = Input::with_theme(&theme)
                    .with_prompt("Débit vidéo en kb/s")
                    .default(settings.video.bitrate_kbps)
                    .interact_text()?;
            }
            3 => {
                let options = [
                    "H.264 (compatibilité maximale, recommandé pour le montage)",
                    "HEVC (fichiers plus légers, moins compatible)",
                ];
                let i = Select::with_theme(&theme).with_prompt("Format vidéo").items(options).default(0).interact()?;
                settings.video.codec = if i == 0 { Codec::H264 } else { Codec::Hevc };
            }
            4 => {
                let prefs = [HardwarePreference::Auto, HardwarePreference::Gpu, HardwarePreference::Cpu];
                let labels: Vec<String> = prefs.iter().map(|p| p.label()).collect();
                let i = Select::with_theme(&theme)
                    .with_prompt("Matériel d'encodage")
                    .items(&labels)
                    .default(0)
                    .interact()?;
                settings.video.hardware = prefs[i];
            }
            5 => {
                let labels: Vec<String> = AUDIO_BITRATES.iter().map(|b| format!("{b} kb/s")).collect();
                let i = Select::with_theme(&theme).with_prompt("Débit audio").items(&labels).default(2).interact()?;
                settings.audio.bitrate_kbps = AUDIO_BITRATES[i];
            }
            6 => {
                let current = settings.output_dir.display().to_string();
                let text: String = Input::with_theme(&theme)
                    .with_prompt("Dossier des enregistrements")
                    .default(current)
                    .interact_text()?;
                settings.output_dir = PathBuf::from(text.trim().trim_matches('"'));
            }
            7 => {
                println!("{TEMPLATE_HELP}");
                let text: String = Input::with_theme(&theme)
                    .with_prompt("Modèle de nom")
                    .default(settings.filename_template.clone())
                    .interact_text()?;
                settings.filename_template = text.trim().to_string();
            }
            8 => settings.convert_to_mp4 = !settings.convert_to_mp4,
            9 => {
                let devices = platform.list_microphones();
                let mut options = vec!["Désactivé".to_string(), "Micro par défaut de Windows".to_string()];
                options.extend(devices.iter().map(|d| d.name.clone()));
                let i = Select::with_theme(&theme)
                    .with_prompt("Micro à enregistrer")
                    .items(&options)
                    .default(0)
                    .interact()?;
                let micro = &mut settings.microphone;
                micro.enabled = i > 0;
                if i == 1 {
                    micro.device_id.clear();
                    micro.device_name.clear();
                } else if let Some(device) = devices.get(i.saturating_sub(2)).filter(|_| i >= 2) {
                    micro.device_id = device.id.clone();
                    micro.device_name = device.name.clone();
                }
                if micro.enabled {
                    println!("Windows doit autoriser l'accès au micro pour les applications de bureau (Paramètres > Confidentialité et sécurité > Microphone).");
                    ui::pause();
                }
            }
            _ => return Ok(()),
        }
        config.normalize();
    }
}

fn parse_size(text: &str) -> Option<(u32, u32)> {
    let (w, h) =
        text.trim().to_lowercase().split_once('x').map(|(w, h)| (w.trim().to_string(), h.trim().to_string()))?;
    Some((w.parse().ok()?, h.parse().ok()?))
}
