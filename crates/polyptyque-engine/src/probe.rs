//! Recherche de FFmpeg et des encodeurs réellement utilisables.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use polyptyque_core::config::{Codec, HardwarePreference};
use polyptyque_core::encoder::{candidates, probe_args, EncoderChoice};
use polyptyque_core::platform::Platform;

const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

fn executable_name() -> &'static str {
    if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

/// FFmpeg livré à côté de Polyptyque, sinon celui du système.
pub fn locate_ffmpeg() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        let bundled = exe.with_file_name(executable_name());
        if bundled.exists() {
            return Some(bundled);
        }
    }
    let system = PathBuf::from(executable_name());
    Command::new(&system)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()
        .filter(|status| status.success())
        .map(|_| system)
}

/// Encode quelques images de test pour vérifier qu'un encodeur fonctionne
/// sur cette machine (pilote présent, carte compatible…).
pub fn probe(platform: &dyn Platform, ffmpeg: &Path, choice: EncoderChoice) -> bool {
    let mut command = Command::new(ffmpeg);
    command.args(probe_args(choice)).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    platform.prepare_encoder_process(&mut command);
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    platform.end_helper_with_polyptyque(&child);
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                log::info!("essai de l'encodeur {} : {status}", choice.ffmpeg_codec());
                return status.success();
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                log::warn!("essai de l'encodeur {} : délai dépassé", choice.ffmpeg_codec());
                return false;
            }
        }
    }
}

/// Premier encodeur fonctionnel selon la préférence de l'utilisateur.
pub fn select_encoder(
    platform: &dyn Platform,
    ffmpeg: &Path,
    preference: HardwarePreference,
    codec: Codec,
) -> Option<EncoderChoice> {
    candidates(preference, codec).into_iter().find(|&choice| probe(platform, ffmpeg, choice))
}
