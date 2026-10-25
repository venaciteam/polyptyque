//! Conversion sans perte des enregistrements Matroska en MP4 (ou en M4A
//! pour un fichier de son seul), pour les logiciels de montage qui gèrent
//! mal le MKV.
//!
//! Les données sont recopiées telles quelles (« remuxage ») : aucune perte
//! de qualité, et la durée ne dépend que de la vitesse du disque. Le
//! fichier d'origine est toujours conservé, et le résultat est vérifié
//! (pistes, durée) avant d'être déclaré réussi.

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use anyhow::{bail, Context, Result};
use polyptyque_core::naming;
use polyptyque_core::platform::Platform;
use polyptyque_core::storage;
use tr::tr;

/// Ce que FFmpeg annonce d'un fichier.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaInfo {
    /// Durée en secondes, si le fichier l'indique (un MKV interrompu
    /// brutalement peut ne pas l'avoir).
    pub duration: Option<f64>,
    pub video_streams: usize,
    pub audio_streams: usize,
    pub video_codec: Option<String>,
}

/// Fichiers que l'on sait convertir.
pub fn is_convertible(path: &Path) -> bool {
    matches!(extension(path).as_deref(), Some("mkv" | "mka"))
}

/// Destination d'une conversion : même dossier, même nom, extension `.mp4`
/// (ou `.m4a` pour du son seul). Un fichier existant n'est jamais écrasé.
pub fn converted_path(input: &Path) -> PathBuf {
    let ext = if extension(input).as_deref() == Some("mka") { "m4a" } else { "mp4" };
    let dir = input.parent().unwrap_or(Path::new("."));
    let stem = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "conversion".into());
    naming::unique_path(dir, &stem, ext, &HashSet::new())
}

/// Lit les pistes et la durée d'un fichier.
pub fn inspect(platform: &dyn Platform, ffmpeg: &Path, path: &Path) -> Result<MediaInfo> {
    let mut command = Command::new(ffmpeg);
    command.args(["-hide_banner", "-nostdin", "-i"]).arg(path);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    platform.prepare_encoder_process(&mut command);
    // Sans fichier de sortie, FFmpeg décrit l'entrée puis s'arrête en erreur :
    // c'est attendu.
    let output = command.output().with_context(|| tr!("lancement de {}", ffmpeg.display()))?;
    let text = String::from_utf8_lossy(&output.stderr);
    let info = parse_media_info(&text);
    if info.video_streams == 0 && info.audio_streams == 0 {
        log::warn!("FFmpeg ne lit pas {} : {}", path.display(), last_line(&text));
        bail!(tr!("FFmpeg ne peut pas le lire (détail dans le journal)"));
    }
    Ok(info)
}

/// Format d'un fichier remuxé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    Mp4,
    Matroska,
}

/// Convertit `input` et renvoie le chemin du fichier créé. `on_progress`
/// reçoit l'avancement (0 à 1). Si `cancel` passe à vrai, la conversion
/// s'arrête et le fichier partiel est effacé.
pub fn convert(
    platform: &dyn Platform,
    ffmpeg: &Path,
    input: &Path,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<PathBuf> {
    if !is_convertible(input) {
        bail!(tr!("seuls les fichiers .mkv et .mka peuvent être convertis"));
    }
    let output = converted_path(input);
    log::info!("conversion : {} -> {}", input.display(), output.display());
    remux_file(platform, ffmpeg, input, &output, Container::Mp4, on_progress, cancel)
        .inspect_err(|e| log::warn!("conversion de {} : {e:#}", input.display()))?;
    log::info!("conversion réussie : {}", output.display());
    Ok(output)
}

/// Un MKV interrompu brutalement (coupure de courant, écran bleu, FFmpeg
/// arrêté de force) n'a ni durée ni index : il se lit, mais le déplacement
/// y est lent et certains logiciels de montage le refusent.
pub fn needs_repair(info: &MediaInfo) -> bool {
    info.duration.is_none()
}

/// Répare un MKV (ou MKA) interrompu en le recopiant sans réencodage : la
/// durée et l'index sont reconstruits. L'original n'est remplacé qu'une fois
/// la copie vérifiée ; en cas d'échec, il reste intact.
pub fn repair(
    platform: &dyn Platform,
    ffmpeg: &Path,
    input: &Path,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<PathBuf> {
    if !is_convertible(input) {
        bail!(tr!("seuls les fichiers .mkv et .mka peuvent être réparés"));
    }
    log::info!("réparation : {}", input.display());
    let repaired = storage::sibling(input, ".reparation");
    let result = remux_file(platform, ffmpeg, input, &repaired, Container::Matroska, on_progress, cancel)
        .and_then(|()| check_repaired_size(input, &repaired))
        .and_then(|()| replace_with(input, &repaired));
    match result {
        Ok(()) => {
            log::info!("réparation réussie : {}", input.display());
            Ok(input.to_path_buf())
        }
        Err(e) => {
            let _ = fs::remove_file(&repaired);
            log::warn!("réparation de {} : {e:#}", input.display());
            Err(e)
        }
    }
}

/// Un fichier interrompu n'a pas de durée : la vérification des pistes ne
/// suffit pas. La copie doit aussi contenir presque toutes les données de
/// l'original (FFmpeg peut s'arrêter sur une zone abîmée sans le signaler) :
/// sinon l'original est gardé.
fn check_repaired_size(original: &Path, repaired: &Path) -> Result<()> {
    let before = fs::metadata(original).map(|m| m.len()).unwrap_or(0);
    let after = fs::metadata(repaired).map(|m| m.len()).unwrap_or(0);
    if after < before / 100 * MIN_REPAIRED_PERCENT {
        bail!(tr!("copie réparée incomplète ({} octets sur {}) : l'original est conservé tel quel", after, before));
    }
    Ok(())
}

/// Part minimale des données de l'original retrouvée dans la copie réparée.
const MIN_REPAIRED_PERCENT: u64 = 95;

/// Remplace `original` par `replacement`, en remettant l'original en place
/// si le second renommage échoue.
fn replace_with(original: &Path, replacement: &Path) -> Result<()> {
    let backup = storage::sibling(original, ".avant-reparation");
    fs::rename(original, &backup).context(tr!("le fichier d'origine est utilisé par un autre programme"))?;
    if let Err(e) = fs::rename(replacement, original) {
        let _ = fs::rename(&backup, original);
        return Err(e).context(tr!("remplacement du fichier d'origine"));
    }
    let _ = fs::remove_file(&backup);
    Ok(())
}

/// Recopie `input` dans `output` sans réencodage, sous un nom temporaire
/// renommé seulement après vérification : un fichier interrompu (plantage,
/// annulation) ne passe jamais pour un fichier complet.
fn remux_file(
    platform: &dyn Platform,
    ffmpeg: &Path,
    input: &Path,
    output: &Path,
    container: Container,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<()> {
    let source = inspect(platform, ffmpeg, input)?;
    let input_size = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
    let partial = storage::sibling(output, ".partiel");
    // Reste d'une conversion interrompue : FFmpeg refuserait de l'écraser.
    let _ = fs::remove_file(&partial);
    let args = remux_args(input, &partial, container, source.video_codec.as_deref() == Some("hevc"));

    let mut command = Command::new(ffmpeg);
    command.args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    platform.prepare_encoder_process(&mut command);
    let mut child = command.spawn().with_context(|| tr!("lancement de {}", ffmpeg.display()))?;

    // Les erreurs sont lues à part pour que FFmpeg ne bloque jamais dessus.
    let stderr = child.stderr.take().map(|mut pipe| {
        thread::spawn(move || {
            let mut text = String::new();
            let _ = pipe.read_to_string(&mut text);
            text
        })
    });
    let mut cancelled = false;
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else {
                break;
            };
            if cancel.load(Ordering::Relaxed) {
                cancelled = true;
                let _ = child.kill();
                break;
            }
            // Avancement d'après la durée, ou d'après la taille écrite pour
            // un fichier interrompu dont la durée est inconnue.
            let fraction = match (source.duration, parse_progress(&line), parse_written(&line)) {
                (Some(total), Some(seconds), _) if total > 0.0 => Some(seconds / total),
                (None, _, Some(bytes)) if input_size > 0 => Some(bytes as f64 / input_size as f64),
                _ => None,
            };
            if let Some(fraction) = fraction {
                on_progress(fraction.clamp(0.0, 1.0) as f32);
            }
        }
    }
    let status = child.wait().context(tr!("attente de FFmpeg"))?;
    let errors = stderr.and_then(|h| h.join().ok()).unwrap_or_default();

    let result = if cancelled || cancel.load(Ordering::Relaxed) {
        Err(anyhow::anyhow!(tr!("annulé")))
    } else if !status.success() {
        log::warn!("échec de FFmpeg ({status}) : {}", last_line(&errors));
        Err(anyhow::anyhow!(tr!("échec de FFmpeg (détail dans le journal)")))
    } else {
        inspect(platform, ffmpeg, &partial)
            .and_then(|copy| verify(&source, &copy))
            .and_then(|()| fs::rename(&partial, output).context(tr!("renommage du fichier terminé")))
    };
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    } else {
        on_progress(1.0);
    }
    result
}

fn remux_args(input: &Path, output: &Path, container: Container, hevc: bool) -> Vec<String> {
    let mut args: Vec<String> =
        ["-hide_banner", "-nostdin", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-i"]
            .iter()
            .map(|s| s.to_string())
            .collect();
    args.push(input.to_string_lossy().into_owned());
    args.extend(["-map", "0", "-c", "copy"].iter().map(|s| s.to_string()));
    match container {
        Container::Mp4 => {
            if hevc {
                // Étiquette attendue par la plupart des logiciels pour le HEVC en MP4.
                args.extend(["-tag:v".to_string(), "hvc1".to_string()]);
            }
            args.extend(["-f".to_string(), "mp4".to_string()]);
        }
        Container::Matroska => args.extend(["-f".to_string(), "matroska".to_string()]),
    }
    args.push(output.to_string_lossy().into_owned());
    args
}

/// Le fichier converti doit avoir les mêmes pistes et la même durée.
fn verify(source: &MediaInfo, converted: &MediaInfo) -> Result<()> {
    if (converted.video_streams, converted.audio_streams) != (source.video_streams, source.audio_streams) {
        bail!(tr!(
            "pistes manquantes après conversion ({} vidéo et {} son au lieu de {} et {})",
            converted.video_streams,
            converted.audio_streams,
            source.video_streams,
            source.audio_streams
        ));
    }
    match (source.duration, converted.duration) {
        (Some(expected), Some(actual)) if (expected - actual).abs() > 1.0 + expected * 0.001 => {
            bail!(tr!(
                "durée différente après conversion ({} s au lieu de {} s)",
                format!("{actual:.1}"),
                format!("{expected:.1}")
            ))
        }
        (_, None) => bail!(tr!("durée illisible après conversion")),
        _ => Ok(()),
    }
}

fn parse_media_info(text: &str) -> MediaInfo {
    let mut info = MediaInfo::default();
    // Seule la description de l'entrée compte.
    for line in text.lines().skip_while(|l| !l.starts_with("Input #0")) {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Duration:") {
            info.duration = rest.split(',').next().and_then(|d| parse_clock(d.trim()));
        } else if line.starts_with("Stream #") {
            if let Some(index) = line.find(": Video: ") {
                info.video_streams += 1;
                if info.video_codec.is_none() {
                    let codec = line[index + 9..].split([' ', ',']).next().unwrap_or("");
                    info.video_codec = Some(codec.to_string());
                }
            } else if line.contains(": Audio: ") {
                info.audio_streams += 1;
            }
        } else if line.starts_with("Output #") {
            break;
        }
    }
    info
}

/// `01:02:03.45` en secondes.
fn parse_clock(text: &str) -> Option<f64> {
    let mut parts = text.split(':');
    let hours: f64 = parts.next()?.parse().ok()?;
    let minutes: f64 = parts.next()?.parse().ok()?;
    let seconds: f64 = parts.next()?.parse().ok()?;
    let total = hours * 3600.0 + minutes * 60.0 + seconds;
    (parts.next().is_none() && total.is_finite() && total >= 0.0).then_some(total)
}

/// Position atteinte, en secondes, d'après une ligne de `-progress`.
fn parse_progress(line: &str) -> Option<f64> {
    // `out_time_ms` est en réalité en microsecondes, comme `out_time_us`.
    let value = line.strip_prefix("out_time_us=").or_else(|| line.strip_prefix("out_time_ms="))?;
    value.trim().parse::<i64>().ok().filter(|&us| us >= 0).map(|us| us as f64 / 1_000_000.0)
}

/// Octets déjà écrits, d'après une ligne de `-progress`.
fn parse_written(line: &str) -> Option<u64> {
    line.strip_prefix("total_size=")?.trim().parse().ok()
}

fn extension(path: &Path) -> Option<String> {
    path.extension().map(|e| e.to_string_lossy().to_lowercase())
}

fn last_line(text: &str) -> String {
    let line = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("aucun détail").trim();
    line.chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESCRIPTION: &str = "\
Input #0, matroska,webm, from 'a.mkv':
  Metadata:
    title           : Firefox 1
  Duration: 01:02:03.50, start: 0.000000, bitrate: 160 kb/s
  Stream #0:0: Video: hevc (Main), yuv420p(tv, bt709), 1920x1080, 60 fps (default)
  Stream #0:1(fre): Audio: aac (LC), 48000 Hz, stereo, fltp (default)
At least one output file must be specified";

    #[test]
    fn reads_media_description() {
        let info = parse_media_info(DESCRIPTION);
        assert_eq!(info.duration, Some(3723.5));
        assert_eq!((info.video_streams, info.audio_streams), (1, 1));
        assert_eq!(info.video_codec.as_deref(), Some("hevc"));
    }

    #[test]
    fn missing_duration_is_tolerated() {
        let info = parse_media_info("Input #0, matroska,webm, from 'a.mka':\n  Duration: N/A, start: 0.0\n  Stream #0:0: Audio: aac, 48000 Hz\n");
        assert_eq!(info.duration, None);
        assert_eq!((info.video_streams, info.audio_streams), (0, 1));
    }

    #[test]
    fn reads_progress() {
        assert_eq!(parse_progress("out_time_us=2500000"), Some(2.5));
        assert_eq!(parse_progress("out_time_ms=1000000"), Some(1.0));
        assert_eq!(parse_progress("out_time_us=N/A"), None);
        assert_eq!(parse_progress("progress=continue"), None);
        assert_eq!(parse_written("total_size=1234"), Some(1234));
        assert_eq!(parse_written("total_size=N/A"), None);
    }

    #[test]
    fn verification_rules() {
        let source = MediaInfo { duration: Some(3600.0), video_streams: 1, audio_streams: 1, video_codec: None };
        let same = MediaInfo { duration: Some(3600.5), ..source.clone() };
        assert!(verify(&source, &same).is_ok());
        let short = MediaInfo { duration: Some(3500.0), ..source.clone() };
        assert!(verify(&source, &short).is_err());
        let no_audio = MediaInfo { audio_streams: 0, ..source.clone() };
        assert!(verify(&source, &no_audio).is_err());
        // Un MKV interrompu sans durée reste convertible.
        let unknown = MediaInfo { duration: None, ..source.clone() };
        assert!(verify(&unknown, &same).is_ok());
    }

    #[test]
    fn truncated_repair_is_rejected() {
        let dir = std::env::temp_dir().join(format!("polyptyque-reparation-{}", polyptyque_core::config::new_id()));
        fs::create_dir_all(&dir).unwrap();
        let original = dir.join("a.mkv");
        let repaired = dir.join("a.mkv.reparation");
        fs::write(&original, vec![0u8; 10_000]).unwrap();
        fs::write(&repaired, vec![0u8; 9_800]).unwrap();
        assert!(check_repaired_size(&original, &repaired).is_ok());
        fs::write(&repaired, vec![0u8; 5_000]).unwrap();
        assert!(check_repaired_size(&original, &repaired).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn destination_names() {
        assert!(is_convertible(Path::new("a/b.MKV")));
        assert!(!is_convertible(Path::new("a/b.mp4")));
        let dir = std::env::temp_dir();
        let stem = format!("polyptyque-conv-{}", polyptyque_core::config::new_id());
        assert_eq!(converted_path(&dir.join(format!("{stem}.mkv"))), dir.join(format!("{stem}.mp4")));
        assert_eq!(converted_path(&dir.join(format!("{stem}.mka"))), dir.join(format!("{stem}.m4a")));
    }

    #[test]
    fn hevc_gets_compatible_tag() {
        let args = remux_args(Path::new("a.mkv"), Path::new("a.mp4"), Container::Mp4, true).join(" ");
        assert!(args.contains("-map 0 -c copy -tag:v hvc1 -f mp4 a.mp4"));
        let args = remux_args(Path::new("a.mkv"), Path::new("a.mp4"), Container::Mp4, false).join(" ");
        assert!(!args.contains("hvc1"));
        let args = remux_args(Path::new("a.mkv"), Path::new("b.mkv"), Container::Matroska, true).join(" ");
        assert!(args.ends_with("-map 0 -c copy -f matroska b.mkv"));
    }
}
