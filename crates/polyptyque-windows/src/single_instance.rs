//! Une seule copie de Polyptyque ouverte à la fois : deux copies se
//! disputeraient les réglages, les raccourcis clavier et les noms de
//! fichiers.

use std::path::Path;

use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, LPARAM, WPARAM};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, IsIconic, IsWindowVisible, PostMessageW, SetForegroundWindow, ShowWindowAsync,
    SW_RESTORE, SW_SHOW,
};

use crate::{tray, windows_list};

/// Verrou tenu tant que Polyptyque est ouvert (absent si le système n'a pas
/// pu le créer : mieux vaut alors deux copies qu'aucune).
pub struct InstanceLock(Option<HANDLE>);

impl Drop for InstanceLock {
    fn drop(&mut self) {
        if let Some(handle) = self.0 {
            unsafe {
                let _ = CloseHandle(handle);
            }
        }
    }
}

pub enum Instance {
    /// Première copie ouverte dans cette session Windows.
    First(InstanceLock),
    /// Polyptyque était déjà ouvert : sa fenêtre a été ramenée au premier
    /// plan (sauf demande contraire).
    AlreadyOpen,
}

/// Prend le verrou de cette session Windows, ou ramène au premier plan la
/// copie déjà ouverte si `bring_to_front` (pas quand Windows relance
/// Polyptyque de lui-même à l'ouverture de session : une copie déjà
/// lancée reste comme elle est).
pub fn acquire(bring_to_front: bool) -> Instance {
    let handle = match unsafe { CreateMutexW(None, false, w!("Local\\Polyptyque-instance")) } {
        Ok(handle) => handle,
        Err(e) => {
            log::warn!("verrou d'instance unique indisponible : {e}");
            return Instance::First(InstanceLock(None));
        }
    };
    if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
        return Instance::First(InstanceLock(Some(handle)));
    }
    unsafe {
        let _ = CloseHandle(handle);
    }
    if bring_to_front {
        show_existing();
    }
    Instance::AlreadyOpen
}

/// Ramène au premier plan la fenêtre de la copie déjà ouverte. Cachée (dans
/// la zone de notification), elle est priée de s'afficher elle-même.
fn show_existing() {
    let own_pid = std::process::id();
    let existing = windows_list::list_including_hidden(own_pid)
        .into_iter()
        .find(|w| is_polyptyque(&w.exe) && w.title.ends_with("Polyptyque"));
    let Some(window) = existing else {
        log::warn!("Polyptyque est déjà ouvert, mais sa fenêtre est introuvable");
        return;
    };
    let hwnd = windows_list::hwnd(window.handle);
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            // Elle pourra passer au premier plan : c'est cette copie-ci,
            // lancée à l'instant, qui en a le droit.
            let _ = AllowSetForegroundWindow(window.pid);
            let _ = PostMessageW(Some(hwnd), tray::show_request_message(), WPARAM(0), LPARAM(0));
            return;
        }
        // Sans attendre : une copie figée ne doit pas bloquer celle-ci.
        let _ = ShowWindowAsync(hwnd, if IsIconic(hwnd).as_bool() { SW_RESTORE } else { SW_SHOW });
        let _ = SetForegroundWindow(hwnd);
    }
}

fn is_polyptyque(exe: &Path) -> bool {
    exe.file_name().is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("polyptyque.exe"))
}
