//! Capture du son par WASAPI :
//! - d'un programme (et de ses sous-programmes) en « process loopback »,
//!   disponible depuis Windows 10 version 2004 ;
//! - d'un micro.

use anyhow::{anyhow, Result};
use polyptyque_core::platform::{AudioCapture, AudioDevice, AudioPacket};
use wasapi::{
    AudioCaptureClient, AudioClient, DeviceEnumerator, Direction, Handle, SampleType, StreamMode, WasapiError,
    WaveFormat,
};

/// Accès refusé : réglage de confidentialité de Windows pour le micro.
const E_ACCESSDENIED: i32 = 0x8007_0005_u32 as i32;

pub struct WasapiCapture {
    // Gardé vivant tant que la capture tourne.
    client: AudioClient,
    capture: AudioCaptureClient,
    event: Handle,
    frame_bytes: usize,
    buffer: Vec<u8>,
}

impl WasapiCapture {
    /// Son produit par le processus `pid` et ses sous-processus.
    pub fn process(pid: u32, sample_rate: u32, channels: u16) -> Result<Self> {
        let _ = wasapi::initialize_mta();
        let client = AudioClient::new_application_loopback_client(pid, true)
            .map_err(|e| hidden(tr::tr!("activation de la capture audio du processus {}", pid), e))?;
        Self::start(client, sample_rate, channels).map_err(|e| hidden(tr::tr!("initialisation de la capture audio"), e))
    }

    /// Son d'un micro (`None` : le micro par défaut de Windows).
    pub fn microphone(device_id: Option<&str>, sample_rate: u32, channels: u16) -> Result<Self> {
        let _ = wasapi::initialize_mta();
        let enumerator = DeviceEnumerator::new().map_err(|e| hidden(tr::tr!("liste des périphériques audio"), e))?;
        let device = match device_id {
            Some(id) => enumerator.get_device(id).map_err(|e| hidden(tr::tr!("micro introuvable (débranché ?)"), e))?,
            None => enumerator
                .get_default_device(&Direction::Capture)
                .map_err(|e| hidden(tr::tr!("aucun micro par défaut dans Windows"), e))?,
        };
        let client = device.get_iaudioclient().map_err(|e| hidden(tr::tr!("ouverture du micro"), e))?;
        Self::start(client, sample_rate, channels).map_err(|e| {
            if access_denied(&e) {
                anyhow!(tr::tr!(
                    "Windows refuse l'accès au micro. L'autoriser dans Paramètres > Confidentialité et sécurité > Microphone (accès au micro, et accès pour les applications de bureau)."
                ))
            } else {
                hidden(tr::tr!("micro inutilisable"), e)
            }
        })
    }

    fn start(mut client: AudioClient, sample_rate: u32, channels: u16) -> Result<Self, WasapiError> {
        // Flottant 32 bits : Windows convertit si besoin depuis le format de
        // la source capturée.
        let format = WaveFormat::new(32, 32, &SampleType::Float, sample_rate as usize, channels as usize, None);
        let mode = StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 };
        client.initialize_client(&format, &Direction::Capture, &mode)?;
        let event = client.set_get_eventhandle()?;
        let capture = client.get_audiocaptureclient()?;
        client.start_stream()?;
        Ok(Self { client, capture, event, frame_bytes: channels as usize * 4, buffer: Vec::new() })
    }
}

/// Erreur de la bibliothèque audio, en anglais : le détail va au journal, le
/// message affiché (dans la langue de l'interface) y renvoie.
fn hidden(what: String, detail: impl std::fmt::Display) -> anyhow::Error {
    log::warn!("{what} : {detail}");
    anyhow!(tr::tr!("{} (détail dans le journal)", what))
}

fn access_denied(error: &WasapiError) -> bool {
    matches!(error, WasapiError::Windows(e) if e.code().0 == E_ACCESSDENIED)
}

impl AudioCapture for WasapiCapture {
    fn read(&mut self, timeout_ms: u32, out: &mut Vec<AudioPacket>) -> Result<()> {
        // Un délai dépassé n'est pas une erreur : la source est muette.
        let _ = self.event.wait_for_event(timeout_ms);
        loop {
            let frames = self.capture.get_next_packet_size()?.unwrap_or(0) as usize;
            if frames == 0 {
                return Ok(());
            }
            let bytes = frames * self.frame_bytes;
            if self.buffer.len() < bytes {
                self.buffer.resize(bytes, 0);
            }
            let (read, info) = self.capture.read_from_device(&mut self.buffer[..bytes])?;
            let read_bytes = read as usize * self.frame_bytes;
            let timestamp = (!info.flags.timestamp_error && info.timestamp != 0).then_some(info.timestamp);
            out.push(AudioPacket {
                data: self.buffer[..read_bytes].to_vec(),
                timestamp_hns: timestamp,
                silent: info.flags.silent,
            });
        }
    }
}

impl Drop for WasapiCapture {
    fn drop(&mut self) {
        let _ = self.client.stop_stream();
    }
}

/// Micros branchés et activés.
pub fn microphones() -> Vec<AudioDevice> {
    let _ = wasapi::initialize_mta();
    let list = || -> Result<Vec<AudioDevice>, WasapiError> {
        let collection = DeviceEnumerator::new()?.get_device_collection(&Direction::Capture)?;
        let mut devices = Vec::new();
        for device in &collection {
            let device = device?;
            devices.push(AudioDevice { id: device.get_id()?, name: device.get_friendlyname()? });
        }
        Ok(devices)
    };
    list().unwrap_or_else(|e| {
        log::warn!("liste des micros indisponible : {e}");
        Vec::new()
    })
}
