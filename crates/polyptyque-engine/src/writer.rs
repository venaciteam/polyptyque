//! Fils d'écriture : vident une file vers un tube lu par FFmpeg.
//!
//! Un fil d'écriture sert une seule partie de fichier. Si l'encodeur
//! s'arrête, le superviseur de la source abandonne la partie et en démarre
//! une nouvelle, dont les fils reprennent les mêmes files là où elles en
//! sont.

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError};
use polyptyque_core::platform::StreamPipe;

use crate::shared::Shared;

const POLL: Duration = Duration::from_millis(500);

/// Données envoyables dans un tube : une image partagée ou un bloc de son.
pub(crate) trait Chunk: Send + 'static {
    /// Taille dans l'unité de position du flux (une image, ou des octets
    /// de son).
    fn units(&self) -> u64;
    /// Données à écrire une fois les `skip` premières unités sautées.
    fn tail(&self, skip: u64) -> &[u8];
    /// Marque de coupure (vide) : fin de ce flux pour cette partie.
    fn is_cut(&self) -> bool;
}

/// Marque de coupure d'une file d'images : une image vide.
pub(crate) fn video_cut_marker() -> Arc<Vec<u8>> {
    Arc::new(Vec::new())
}

/// Bloc de son : la position se compte en octets.
impl Chunk for Vec<u8> {
    fn units(&self) -> u64 {
        self.len() as u64
    }

    fn tail(&self, skip: u64) -> &[u8] {
        &self[(skip as usize).min(self.len())..]
    }

    fn is_cut(&self) -> bool {
        self.is_empty()
    }
}

/// Image : la position se compte en images.
impl Chunk for Arc<Vec<u8>> {
    fn units(&self) -> u64 {
        1
    }

    fn tail(&self, skip: u64) -> &[u8] {
        if skip > 0 {
            &[]
        } else {
            self.as_slice()
        }
    }

    fn is_cut(&self) -> bool {
        self.is_empty()
    }
}

/// Commande partagée par les fils d'écriture d'une même partie.
#[derive(Default)]
pub(crate) struct PartControl {
    /// La partie est abandonnée (encodeur arrêté) : les fils s'arrêtent.
    pub cancel: AtomicBool,
    /// Une écriture a échoué : l'encodeur ne lit plus ce tube.
    pub broken: AtomicBool,
    /// Flux arrivés à une marque de coupure : la partie se termine là.
    pub cuts: AtomicU32,
}

pub(crate) struct WriterJob<T> {
    pub thread_name: String,
    pub pipe: Box<dyn StreamPipe>,
    pub queue: Receiver<T>,
    /// Position du flux, partagée avec les parties précédentes et suivantes.
    pub position: Arc<AtomicU64>,
    /// Les données antérieures à cette position sont sautées (recalage
    /// après une relance de l'encodeur).
    pub start_at: u64,
    /// Octets écrits dans ce tube : le superviseur vérifie que chaque flux
    /// avance.
    pub progress: Arc<AtomicU64>,
    pub label: &'static str,
}

pub(crate) fn spawn<T: Chunk>(
    job: WriterJob<T>,
    shared: Arc<Shared>,
    control: Arc<PartControl>,
) -> io::Result<JoinHandle<()>> {
    let WriterJob { thread_name, pipe, queue, position, start_at, progress, label } = job;
    thread::Builder::new().name(thread_name).spawn(move || {
        let mut output = match pipe.connect() {
            Ok(output) => output,
            Err(e) => {
                log::warn!("{label} : connexion à l'encodeur impossible : {e:#}");
                control.broken.store(true, Ordering::SeqCst);
                return;
            }
        };
        while !control.cancel.load(Ordering::SeqCst) {
            match queue.recv_timeout(POLL) {
                Ok(item) if item.is_cut() => {
                    // Coupure (saut de l'horloge) : ce flux est complet pour
                    // cette partie, la suite ira dans la suivante.
                    control.cuts.fetch_add(1, Ordering::SeqCst);
                    break;
                }
                Ok(item) => {
                    let units = item.units();
                    let at = position.fetch_add(units, Ordering::SeqCst);
                    let data = item.tail(start_at.saturating_sub(at).min(units));
                    if data.is_empty() {
                        continue;
                    }
                    if let Err(e) = output.write_all(data) {
                        if !control.cancel.load(Ordering::SeqCst) {
                            log::warn!("{label} : l'encodeur ne reçoit plus les données ({e})");
                        }
                        control.broken.store(true, Ordering::SeqCst);
                        return;
                    }
                    shared.bytes_sent.fetch_add(data.len() as u64, Ordering::Relaxed);
                    progress.fetch_add(data.len() as u64, Ordering::Relaxed);
                }
                // File vide : on continue, sauf si la capture a été abandonnée
                // à l'arrêt (fil bloqué), auquel cas on ferme le flux.
                Err(RecvTimeoutError::Timeout) if shared.capture_abandoned() => break,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = output.flush();
        // La fermeture du tube signale la fin du flux à FFmpeg.
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_is_cut_at_the_resume_position() {
        let block: Vec<u8> = (0..16).collect();
        assert_eq!(block.units(), 16);
        assert_eq!(block.tail(0).len(), 16);
        assert_eq!(block.tail(8), &block[8..]);
        assert!(block.tail(40).is_empty());
    }

    #[test]
    fn frames_are_skipped_whole() {
        let frame = Arc::new(vec![1u8; 12]);
        assert_eq!(frame.units(), 1);
        assert_eq!(frame.tail(0).len(), 12);
        assert!(frame.tail(1).is_empty());
    }

    #[test]
    fn cut_markers_are_recognised() {
        assert!(video_cut_marker().is_cut());
        assert!(!Arc::new(vec![1u8; 12]).is_cut());
        assert!(Vec::<u8>::new().is_cut());
        assert!(!vec![0u8; 8].is_cut());
    }
}
