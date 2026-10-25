//! Choix de l'encodeur et construction des commandes FFmpeg.
//!
//! Chaque source est encodée par un processus `ffmpeg` qui reçoit l'image
//! (NV12 brut, cadence constante) et le son (flottant 32 bits entrelacé) par
//! deux tubes, et écrit un fichier Matroska au fil de l'eau.

use std::path::Path;

use crate::config::{Codec, HardwarePreference};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderKind {
    Nvenc,
    Amf,
    Qsv,
    Software,
}

impl EncoderKind {
    pub fn label(self) -> String {
        match self {
            EncoderKind::Nvenc => "NVIDIA NVENC".into(),
            EncoderKind::Amf => "AMD AMF".into(),
            EncoderKind::Qsv => "Intel Quick Sync".into(),
            EncoderKind::Software => tr::tr!("Processeur"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderChoice {
    pub kind: EncoderKind,
    pub codec: Codec,
}

impl EncoderChoice {
    pub fn ffmpeg_codec(self) -> &'static str {
        use Codec::*;
        use EncoderKind::*;
        match (self.kind, self.codec) {
            (Nvenc, H264) => "h264_nvenc",
            (Nvenc, Hevc) => "hevc_nvenc",
            (Amf, H264) => "h264_amf",
            (Amf, Hevc) => "hevc_amf",
            (Qsv, H264) => "h264_qsv",
            (Qsv, Hevc) => "hevc_qsv",
            (Software, H264) => "libx264",
            (Software, Hevc) => "libx265",
        }
    }

    pub fn label(self) -> String {
        format!("{} ({})", self.kind.label(), self.codec.label())
    }
}

/// Encodeurs à essayer, dans l'ordre de préférence.
pub fn candidates(preference: HardwarePreference, codec: Codec) -> Vec<EncoderChoice> {
    let gpu = [EncoderKind::Nvenc, EncoderKind::Amf, EncoderKind::Qsv];
    let kinds: Vec<EncoderKind> = match preference {
        HardwarePreference::Auto => gpu.into_iter().chain([EncoderKind::Software]).collect(),
        HardwarePreference::Gpu => gpu.to_vec(),
        HardwarePreference::Cpu => vec![EncoderKind::Software],
    };
    kinds.into_iter().map(|kind| EncoderChoice { kind, codec }).collect()
}

/// Arguments d'un essai d'encodage rapide sur une image de synthèse, pour
/// vérifier qu'un encodeur fonctionne réellement sur la machine.
pub fn probe_args(choice: EncoderChoice) -> Vec<String> {
    strings(&[
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=black:s=1280x720:r=30",
        "-frames:v",
        "10",
        "-c:v",
        choice.ffmpeg_codec(),
        "-f",
        "null",
        "-",
    ])
}

/// Propriétés colorimétriques de l'image produite par la capture.
const COLOR_PARAMS: &str = "setparams=range=tv:colorspace=bt709:color_primaries=bt709:color_trc=bt709";

pub struct RecordingSpec<'a> {
    pub video_input: &'a str,
    pub audio_input: &'a str,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub sample_rate: u32,
    pub channels: u16,
    pub encoder: EncoderChoice,
    pub video_bitrate_kbps: u32,
    pub audio_bitrate_kbps: u32,
    pub title: &'a str,
    pub output: &'a Path,
}

/// Arguments complets d'un enregistrement.
pub fn recording_args(spec: &RecordingSpec) -> Vec<String> {
    let mut args = strings(&["-hide_banner", "-nostdin", "-loglevel", "warning"]);

    // Image brute NV12 à cadence constante.
    args.extend(strings(&["-f", "rawvideo", "-pix_fmt", "nv12", "-video_size"]));
    args.push(format!("{}x{}", spec.width, spec.height));
    args.push("-framerate".into());
    args.push(spec.fps.to_string());
    args.extend(raw_input_tuning());
    args.push("-i".into());
    args.push(spec.video_input.into());

    // Son brut, flottant 32 bits entrelacé.
    args.extend(raw_audio_format(spec.sample_rate, spec.channels));
    args.extend(raw_input_tuning());
    args.push("-i".into());
    args.push(spec.audio_input.into());

    args.extend(strings(&["-map", "0:v:0", "-map", "1:a:0"]));
    // L'image arrive déjà en BT.709 « TV » : on le déclare pour que FFmpeg
    // n'insère pas de conversion (couleurs faussées et coût processeur).
    args.extend(strings(&["-vf", COLOR_PARAMS]));
    args.extend(video_encoder_args(spec.encoder, spec.video_bitrate_kbps));
    // Une image clé toutes les deux secondes : bon compromis pour le montage.
    args.push("-g".into());
    args.push((spec.fps * 2).to_string());
    // Étiquettes du fichier, cohérentes avec la conversion faite par la carte graphique.
    args.extend(strings(&[
        "-color_range",
        "tv",
        "-colorspace",
        "bt709",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
    ]));
    args.extend(strings(&["-c:a", "aac", "-b:a"]));
    args.push(format!("{}k", spec.audio_bitrate_kbps));
    args.push("-metadata".into());
    args.push(format!("title={}", spec.title));
    args.extend(matroska_output());
    args.push(spec.output.to_string_lossy().into_owned());
    args
}

/// Sortie Matroska résistante aux plantages : les données sont écrites au
/// fil de l'eau, par blocs d'une seconde au plus. Si FFmpeg s'arrête
/// brutalement, le fichier reste lisible et perd au plus une seconde
/// environ (contre cinq par défaut, voire tout le fichier à faible débit).
fn matroska_output() -> Vec<String> {
    strings(&["-cluster_time_limit", "1000", "-flush_packets", "1", "-f", "matroska"])
}

pub struct AudioRecordingSpec<'a> {
    pub input: &'a str,
    pub sample_rate: u32,
    pub channels: u16,
    pub bitrate_kbps: u32,
    pub title: &'a str,
    pub output: &'a Path,
}

/// Arguments d'un enregistrement de son seul (micro), en Matroska audio.
pub fn audio_recording_args(spec: &AudioRecordingSpec) -> Vec<String> {
    let mut args = strings(&["-hide_banner", "-nostdin", "-loglevel", "warning"]);
    args.extend(raw_audio_format(spec.sample_rate, spec.channels));
    args.extend(raw_input_tuning());
    args.push("-i".into());
    args.push(spec.input.into());
    args.extend(strings(&["-map", "0:a:0", "-c:a", "aac", "-b:a"]));
    args.push(format!("{}k", spec.bitrate_kbps));
    args.push("-metadata".into());
    args.push(format!("title={}", spec.title));
    args.extend(matroska_output());
    args.push(spec.output.to_string_lossy().into_owned());
    args
}

/// Format d'un son brut flottant 32 bits entrelacé. La disposition des
/// canaux est donnée explicitement : sinon FFmpeg la devine et le signale
/// dans le journal.
fn raw_audio_format(sample_rate: u32, channels: u16) -> Vec<String> {
    let mut args = strings(&["-f", "f32le", "-ar"]);
    args.push(sample_rate.to_string());
    args.push("-ac".into());
    args.push(channels.to_string());
    let layout = match channels {
        1 => Some("mono"),
        2 => Some("stereo"),
        _ => None,
    };
    if let Some(layout) = layout {
        args.push("-ch_layout".into());
        args.push(layout.into());
    }
    args
}

fn raw_input_tuning() -> Vec<String> {
    // Pas d'analyse préalable : le format est entièrement décrit par les
    // options, et une grande file évite de bloquer la lecture des tubes.
    strings(&["-probesize", "32", "-analyzeduration", "0", "-thread_queue_size", "1024"])
}

fn video_encoder_args(choice: EncoderChoice, kbps: u32) -> Vec<String> {
    let rate = format!("{kbps}k");
    let buffer = format!("{}k", kbps.saturating_mul(2));
    let mut args = vec!["-c:v".to_string(), choice.ffmpeg_codec().to_string()];
    let tuning: &[&str] = match choice.kind {
        // p4 : bonne qualité en laissant de la marge pour plusieurs sources
        // sur l'unique encodeur matériel de la carte.
        EncoderKind::Nvenc => &["-preset", "p4", "-tune", "hq", "-rc", "cbr"],
        EncoderKind::Amf => &["-usage", "transcoding", "-quality", "balanced", "-rc", "cbr"],
        EncoderKind::Qsv => &["-preset", "medium"],
        EncoderKind::Software => match choice.codec {
            Codec::H264 => &["-preset", "veryfast"],
            Codec::Hevc => &["-preset", "ultrafast"],
        },
    };
    args.extend(strings(tuning));
    args.extend(["-b:v".to_string(), rate.clone(), "-maxrate".to_string(), rate]);
    args.extend(["-bufsize".to_string(), buffer]);
    args
}

/// Taille estimée d'une heure d'enregistrement pour une source, en octets
/// (débit vidéo et audio, plus environ 1 % pour le conteneur).
pub fn bytes_per_hour(video_kbps: u32, audio_kbps: u32) -> u64 {
    let bits_per_second = (video_kbps as u64 + audio_kbps as u64) * 1000;
    bits_per_second / 8 * 3600 * 101 / 100
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn candidate_order() {
        let auto = candidates(HardwarePreference::Auto, Codec::H264);
        assert_eq!(auto.len(), 4);
        assert_eq!(auto[0].kind, EncoderKind::Nvenc);
        assert_eq!(auto[3].kind, EncoderKind::Software);
        assert_eq!(candidates(HardwarePreference::Cpu, Codec::Hevc)[0].ffmpeg_codec(), "libx265");
        assert!(candidates(HardwarePreference::Gpu, Codec::H264).iter().all(|c| c.kind != EncoderKind::Software));
    }

    #[test]
    fn recording_args_shape() {
        let output = PathBuf::from("sortie.mkv");
        let spec = RecordingSpec {
            video_input: r"\\.\pipe\v",
            audio_input: r"\\.\pipe\a",
            width: 1920,
            height: 1080,
            fps: 60,
            sample_rate: 48_000,
            channels: 2,
            encoder: EncoderChoice { kind: EncoderKind::Nvenc, codec: Codec::H264 },
            video_bitrate_kbps: 12_000,
            audio_bitrate_kbps: 192,
            title: "Firefox 1",
            output: &output,
        };
        let args = recording_args(&spec);
        let joined = args.join(" ");
        assert!(joined.contains("-f rawvideo -pix_fmt nv12 -video_size 1920x1080 -framerate 60"));
        assert!(joined.contains("-f f32le -ar 48000 -ac 2 -ch_layout stereo"));
        assert!(joined.contains("-c:v h264_nvenc"));
        assert!(joined.contains("-b:v 12000k -maxrate 12000k -bufsize 24000k"));
        assert!(joined.contains("-g 120"));
        assert!(joined.contains("-vf setparams=range=tv:colorspace=bt709"));
        assert!(joined.contains("-preset p4"));
        assert!(joined.contains("-cluster_time_limit 1000 -flush_packets 1 -f matroska sortie.mkv"));
        assert!(args.contains(&"title=Firefox 1".to_string()));
        assert_eq!(args.last().unwrap(), "sortie.mkv");
        // Les deux entrées précèdent la sélection des flux.
        let first_input = args.iter().position(|a| a == "-i").unwrap();
        let map = args.iter().position(|a| a == "-map").unwrap();
        assert!(first_input < map);
    }

    #[test]
    fn audio_only_args_shape() {
        let output = PathBuf::from("micro.mka");
        let args = audio_recording_args(&AudioRecordingSpec {
            input: "tube",
            sample_rate: 48_000,
            channels: 2,
            bitrate_kbps: 192,
            title: "Micro",
            output: &output,
        });
        let joined = args.join(" ");
        assert!(joined.contains("-f f32le -ar 48000 -ac 2 -ch_layout stereo"));
        assert!(joined.contains("-i tube -map 0:a:0 -c:a aac -b:a 192k"));
        assert!(!joined.contains("rawvideo"));
        assert!(joined.ends_with("-f matroska micro.mka"));
    }

    #[test]
    fn size_estimate() {
        // 12 000 + 192 kb/s ≈ 5,4 Go par heure (+1 %).
        let bytes = bytes_per_hour(12_000, 192);
        assert!(bytes > 5_400_000_000 && bytes < 5_600_000_000, "{bytes}");
    }
}
