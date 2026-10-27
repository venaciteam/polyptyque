//! Cœur de Polyptyque.
//!
//! Tout ce qui ne dépend pas du système d'exploitation : modèle de données
//! (scènes, sources, réglages), nommage des fichiers, calculs de
//! synchronisation et construction des commandes FFmpeg. Les parties propres
//! à un système (capture, fenêtres, tubes) sont décrites par le trait
//! [`platform::Platform`] et implémentées ailleurs.

pub mod browsers;
pub mod clock;
pub mod config;
pub mod control;
pub mod encoder;
pub mod hotkeys;
pub mod i18n;
pub mod launch;
pub mod level;
pub mod naming;
pub mod paths;
pub mod platform;
pub mod recovery;
pub mod report;
pub mod schedule;
pub mod storage;
pub mod timeline;
pub mod video;
