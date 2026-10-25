//! Lancement de Polyptyque à l'ouverture de la session Windows, pour que
//! les programmations survivent à un redémarrage (mise à jour de Windows).
//!
//! Polyptyque s'inscrit dans la liste « Applications au démarrage » de
//! l'utilisateur (`HKCU\…\CurrentVersion\Run`), sans droits administrateur.
//! L'utilisateur peut l'y désactiver depuis les Paramètres de Windows
//! (Applications > Démarrage) ou le Gestionnaire des tâches : Windows le
//! note alors à part, dans `…\Explorer\StartupApproved\Run`, sans toucher à
//! l'inscription. Le format de cette note n'est pas documenté par
//! Microsoft ; constaté : premier octet 02 (ou 06) pour activé, 03 (ou 07)
//! pour désactivé, valeur absente pour activé.

use std::ffi::c_void;
use std::path::Path;

use anyhow::{bail, Result};
use windows::core::HSTRING;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const VALUE: &str = "Polyptyque";
/// Argument donné à Polyptyque quand Windows le lance à l'ouverture de
/// session : il démarre réduit, sans ouvrir les fenêtres de la scène.
pub const ARG: &str = "--demarrage-windows";

/// Inscrit `exe` au démarrage de Windows, ou met à jour son emplacement
/// (version portable déplacée). Avec `override_windows`, lève aussi une
/// désactivation faite dans les Paramètres de Windows : à réserver à une
/// demande explicite de l'utilisateur dans Polyptyque.
pub fn enable(exe: &Path, override_windows: bool) -> Result<()> {
    let command = format!("\"{}\" {ARG}", exe.display());
    if registered().as_deref() != Some(command.as_str()) {
        let mut data: Vec<u16> = command.encode_utf16().collect();
        data.push(0);
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(RUN_KEY),
                &HSTRING::from(VALUE),
                REG_SZ.0,
                Some(data.as_ptr() as *const c_void),
                (data.len() * 2) as u32,
            )
        };
        if status != ERROR_SUCCESS {
            bail!(tr::tr!("inscription au démarrage de Windows impossible (code {})", status.0));
        }
    }
    if override_windows && blocked_by_windows() {
        delete(APPROVED_KEY)?;
    }
    Ok(())
}

/// Retire Polyptyque du démarrage de Windows.
pub fn disable() -> Result<()> {
    delete(RUN_KEY)
}

/// Retire l'inscription si elle lance ce programme-ci (désinstallation) :
/// une autre copie de Polyptyque (portable) garde la sienne.
pub fn disable_for(exe: &Path) {
    let ours =
        registered().is_some_and(|command| command.to_lowercase().contains(&exe.display().to_string().to_lowercase()));
    if ours {
        let _ = disable();
    }
}

/// Commande inscrite au démarrage de Windows, s'il y en a une.
pub fn registered() -> Option<String> {
    let mut size = 0u32;
    let key = HSTRING::from(RUN_KEY);
    let value = HSTRING::from(VALUE);
    unsafe {
        if RegGetValueW(HKEY_CURRENT_USER, &key, &value, RRF_RT_REG_SZ, None, None, Some(&mut size)) != ERROR_SUCCESS {
            return None;
        }
        let mut buffer = vec![0u16; size as usize / 2 + 1];
        let mut size = (buffer.len() * 2) as u32;
        let status = RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            &value,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr() as *mut c_void),
            Some(&mut size),
        );
        if status != ERROR_SUCCESS {
            return None;
        }
        let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..len]))
    }
}

/// L'utilisateur a désactivé Polyptyque dans les applications au
/// démarrage de Windows : l'inscription reste, mais Windows ne le lance pas.
pub fn blocked_by_windows() -> bool {
    let mut data = [0u8; 16];
    let mut size = data.len() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(APPROVED_KEY),
            &HSTRING::from(VALUE),
            RRF_RT_REG_BINARY,
            None,
            Some(data.as_mut_ptr() as *mut c_void),
            Some(&mut size),
        )
    };
    status == ERROR_SUCCESS && size > 0 && data[0] & 1 == 1
}

fn delete(key: &str) -> Result<()> {
    let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(VALUE)) };
    if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
        bail!(tr::tr!("modification du démarrage de Windows impossible (code {})", status.0));
    }
    Ok(())
}
