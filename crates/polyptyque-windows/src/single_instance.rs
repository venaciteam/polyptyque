//! Une seule copie de Polyptyque ouverte à la fois : deux copies se
//! disputeraient les réglages, les raccourcis clavier et les noms de
//! fichiers.

use std::path::Path;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, HANDLE, LPARAM, WPARAM,
};
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
    let lock = match lock(w!("Local\\Polyptyque-instance")) {
        Some(lock) => lock,
        None => {
            if bring_to_front {
                show_existing();
            }
            return Instance::AlreadyOpen;
        }
    };
    Instance::First(lock)
}

/// Verrou `name`, ou `None` s'il est déjà tenu.
fn lock(name: PCWSTR) -> Option<InstanceLock> {
    let handle = match unsafe { CreateMutexW(None, false, name) } {
        Ok(handle) => handle,
        // Verrou créé par une copie lancée en administrateur : cette copie-ci
        // n'a pas le droit de l'ouvrir, mais il existe, donc Polyptyque est
        // déjà ouvert (cas documenté de CreateMutex). Ouvrir quand même une
        // seconde copie ferait se disputer réglages, fichiers et canal de
        // commande.
        Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => {
            log::warn!("Polyptyque est déjà ouvert avec d'autres droits (en administrateur ?)");
            return None;
        }
        Err(e) => {
            log::warn!("verrou d'instance unique indisponible : {e}");
            return Some(InstanceLock(None));
        }
    };
    if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
        return Some(InstanceLock(Some(handle)));
    }
    unsafe {
        let _ = CloseHandle(handle);
    }
    None
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

#[cfg(test)]
mod tests {
    use windows::core::HSTRING;
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

    use super::*;

    fn test_name(what: &str) -> HSTRING {
        HSTRING::from(format!("Local\\Polyptyque-test-{what}-{}", std::process::id()))
    }

    #[test]
    fn deuxieme_copie() {
        let name = test_name("copie");
        let first = lock(PCWSTR(name.as_ptr())).expect("premier verrou");
        assert!(first.0.is_some());
        assert!(lock(PCWSTR(name.as_ptr())).is_none());
        drop(first);
        assert!(lock(PCWSTR(name.as_ptr())).is_some());
    }

    /// Verrou d'une copie aux droits plus élevés, que celle-ci ne peut pas
    /// ouvrir (simulé par un verrou sans aucune autorisation) : Polyptyque
    /// est déjà ouvert.
    #[test]
    fn verrou_inaccessible() {
        let name = test_name("administrateur");
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &HSTRING::from("D:P"),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
            .unwrap();
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let other = unsafe { CreateMutexW(Some(&attributes), false, PCWSTR(name.as_ptr())) }.unwrap();
        assert!(lock(PCWSTR(name.as_ptr())).is_none());
        unsafe {
            let _ = CloseHandle(other);
            let _ = windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(descriptor.0)));
        }
    }
}
