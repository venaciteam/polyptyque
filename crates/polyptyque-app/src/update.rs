//! Mises à jour : recherche d'une nouvelle version publiée sur GitHub,
//! téléchargement, puis redémarrage sur la nouvelle version (Velopack).
//!
//! Rien n'est installé sans l'accord de l'utilisateur, et jamais pendant
//! un enregistrement. Hors d'une installation Velopack (version compilée à
//! la main), la recherche est simplement sans effet.

use anyhow::Result;

/// Dépôt où sont publiées les versions.
#[cfg_attr(not(windows), allow(dead_code))]
pub const REPOSITORY: &str = "https://github.com/venaciteam/Polyptyque";

/// Nouvelle version disponible.
#[derive(Clone)]
pub struct Available {
    pub version: String,
    /// Taille du téléchargement, en octets.
    pub size: u64,
    /// Trouvée en cherchant aussi les bêtas : le téléchargement doit les
    /// chercher de même.
    #[cfg_attr(not(windows), allow(dead_code))]
    beta: bool,
    #[cfg(windows)]
    info: velopack::UpdateInfo,
}

#[cfg(windows)]
mod imp {
    use std::sync::mpsc;
    use std::thread;

    use anyhow::{anyhow, Result};
    use velopack::sources::GithubSource;
    use velopack::{UpdateCheck, UpdateManager};

    use super::{Available, REPOSITORY};

    /// Les bêtas sont publiées comme préversions GitHub, que la source
    /// ignore sauf si `beta`.
    fn manager(beta: bool) -> Result<UpdateManager> {
        UpdateManager::new(GithubSource::new(REPOSITORY, None, beta), None, None)
            .map_err(|e| anyhow!(tr::tr!("Polyptyque n'a pas été installé par son installateur ({})", e)))
    }

    pub fn check(beta: bool) -> Result<Option<Available>> {
        match manager(beta)?.check_for_updates().map_err(|e| anyhow!("{e}"))? {
            UpdateCheck::UpdateAvailable(info) => Ok(Some(Available {
                version: info.TargetFullRelease.Version.clone(),
                size: info.TargetFullRelease.Size,
                beta,
                info: *info,
            })),
            UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty => Ok(None),
        }
    }

    pub fn download(update: &Available, progress: &dyn Fn(f32)) -> Result<()> {
        let manager = manager(update.beta)?;
        let info = update.info.clone();
        let (sender, receiver) = mpsc::channel::<i16>();
        let worker = thread::spawn(move || manager.download_updates(&info, Some(sender)));
        for percent in receiver {
            progress(f32::from(percent.clamp(0, 100)) / 100.0);
        }
        worker
            .join()
            .map_err(|_| anyhow!(tr::tr!("erreur interne pendant le téléchargement")))?
            .map_err(|e| anyhow!("{e}"))
    }

    /// Quitte Polyptyque, installe la nouvelle version et la relance. Ne
    /// revient qu'en cas d'échec.
    pub fn apply(update: &Available) -> Result<()> {
        manager(update.beta)?.apply_updates_and_restart(&update.info).map_err(|e| anyhow!("{e}"))
    }
}

/// Mode démonstration : une mise à jour fictive, si la variable
/// `POLYPTYQUE_DEMO_MISE_A_JOUR` est définie.
#[cfg(not(windows))]
mod imp {
    use std::thread;
    use std::time::Duration;

    use anyhow::{bail, Result};

    use super::Available;

    pub fn check(beta: bool) -> Result<Option<Available>> {
        Ok(std::env::var_os("POLYPTYQUE_DEMO_MISE_A_JOUR").map(|_| Available {
            version: if beta { "1.1.0-beta.1" } else { "1.1.0" }.into(),
            size: 74_000_000,
            beta,
        }))
    }

    pub fn download(_: &Available, progress: &dyn Fn(f32)) -> Result<()> {
        for step in 0..=20 {
            progress(step as f32 / 20.0);
            thread::sleep(Duration::from_millis(150));
        }
        Ok(())
    }

    pub fn apply(_: &Available) -> Result<()> {
        bail!("mode démonstration : pas de redémarrage")
    }
}

/// Recherche une nouvelle version, bêtas comprises si `beta` (accès
/// réseau : à appeler hors du fil de l'interface).
pub fn check(beta: bool) -> Result<Option<Available>> {
    imp::check(beta)
}

/// Télécharge la nouvelle version ; `progress` reçoit l'avancement (0 à 1).
pub fn download(update: &Available, progress: &dyn Fn(f32)) -> Result<()> {
    imp::download(update, progress)
}

/// Installe la version téléchargée et redémarre Polyptyque.
pub fn apply(update: &Available) -> Result<()> {
    imp::apply(update)
}
