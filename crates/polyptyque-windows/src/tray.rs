//! Icône de Polyptyque dans la zone de notification (près de l'horloge),
//! tant que sa fenêtre est cachée : un clic la fait revenir, un clic droit
//! propose « Afficher Polyptyque » et « Quitter Polyptyque ».
//!
//! Les messages de l'icône arrivent sur la fenêtre principale, par le même
//! procédé que [`crate::session_end`] : la fenêtre cachée existe toujours.
//! Une autre copie de Polyptyque lancée pendant ce temps demande à celle-ci
//! de s'afficher par un message enregistré ([`show_request_message`]).

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use tr::tr;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP,
    NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, GetSystemMetrics, IsIconic, LoadIconW, LoadImageW,
    PostMessageW, RegisterWindowMessageW, SetForegroundWindow, ShowWindow, TrackPopupMenu, HICON, IDI_APPLICATION,
    IMAGE_ICON, LR_DEFAULTCOLOR, LR_SHARED, MF_SEPARATOR, MF_STRING, SM_CXSMICON, SM_CYSMICON, SW_RESTORE,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CONTEXTMENU, WM_NCDESTROY, WM_NULL,
};

use crate::windows_list::hwnd;

/// Identifiant de notre traitement des messages de la fenêtre (distinct de
/// celui de `session_end`).
const SUBCLASS_ID: usize = 0x5059_5449; // « PYTI »
/// Identifiant de l'icône (une seule).
const ICON_ID: u32 = 1;
/// Message envoyé par Windows quand on agit sur l'icône.
const CALLBACK_MESSAGE: u32 = WM_APP + 0x50;
/// Commandes du menu.
const COMMAND_SHOW: usize = 1;
const COMMAND_QUIT: usize = 2;
/// Icône choisie au clavier (`NIN_SELECT | NINF_KEY`, absent du crate
/// `windows`).
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
/// Longueur maximale de l'info-bulle, zéro final compris.
const TIP_LEN: usize = 128;

pub enum TrayEvent {
    /// Clic sur l'icône, « Afficher Polyptyque », ou autre copie lancée.
    Show,
    /// « Quitter Polyptyque ».
    Quit,
}

type Handler = Box<dyn Fn(TrayEvent)>;

thread_local! {
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
    /// Icône affichée, et son info-bulle (à reposer si l'Explorateur
    /// redémarre).
    static SHOWN: Cell<bool> = const { Cell::new(false) };
    static TIP: RefCell<String> = const { RefCell::new(String::new()) };
    static ICON: Cell<Option<HICON>> = const { Cell::new(None) };
}

/// Message par lequel une autre copie de Polyptyque demande à celle-ci de
/// s'afficher.
pub(crate) fn show_request_message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    *MESSAGE.get_or_init(|| unsafe { RegisterWindowMessageW(w!("Polyptyque.Afficher")) })
}

/// L'Explorateur a redémarré (plantage, mise à jour) : les icônes sont à
/// reposer.
fn taskbar_created_message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    *MESSAGE.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

/// Reçoit les actions sur l'icône de la fenêtre principale `window`. À
/// appeler une fois, sur le fil qui a créé la fenêtre (celui de
/// l'interface). Le gestionnaire est appelé pendant le traitement d'un
/// message de Windows : il doit seulement planifier la suite.
pub fn watch(window: isize, handler: impl Fn(TrayEvent) + 'static) -> Result<()> {
    HANDLER.with(|h| *h.borrow_mut() = Some(Box::new(handler)));
    unsafe { SetWindowSubclass(hwnd(window), Some(window_proc), SUBCLASS_ID, 0) }
        .ok()
        .context("icône de la zone de notification")
}

/// Affiche l'icône, avec son info-bulle.
pub fn show(window: isize, tip: &str) {
    TIP.with(|t| *t.borrow_mut() = tip.to_string());
    SHOWN.set(true);
    add(window);
}

/// Change l'info-bulle de l'icône affichée.
pub fn set_tip(window: isize, tip: &str) {
    TIP.with(|t| *t.borrow_mut() = tip.to_string());
    if !SHOWN.get() {
        return;
    }
    let mut data = icon_data(window);
    data.uFlags = NIF_TIP | NIF_SHOWTIP;
    if !unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) }.as_bool() {
        log::warn!("info-bulle de l'icône de la zone de notification non modifiée");
    }
}

/// Retire l'icône.
pub fn hide(window: isize) {
    if !SHOWN.replace(false) {
        return;
    }
    let data = icon_data(window);
    let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
}

/// Fenêtre de nouveau affichée : la met au premier plan (et la restaure si
/// elle était réduite).
pub fn bring_to_front(window: isize) {
    let window = hwnd(window);
    unsafe {
        if IsIconic(window).as_bool() {
            let _ = ShowWindow(window, SW_RESTORE);
        }
        let _ = SetForegroundWindow(window);
    }
}

fn add(window: isize) {
    let mut data = icon_data(window);
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
        // Déjà présente (Explorateur pas encore prêt à l'ouverture de
        // session, puis redémarré) : on la met à jour.
        if !unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) }.as_bool() {
            log::warn!("icône de la zone de notification non ajoutée");
            return;
        }
    }
    // Comportement actuel de Windows : un clic donne `NIN_SELECT`, un clic
    // droit `WM_CONTEXTMENU`.
    data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    let _ = unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) };
}

fn icon_data(window: isize) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd(window),
        uID: ICON_ID,
        uCallbackMessage: CALLBACK_MESSAGE,
        hIcon: icon(),
        ..Default::default()
    };
    TIP.with(|tip| {
        let tip: Vec<u16> = tip.borrow().encode_utf16().take(TIP_LEN - 1).collect();
        data.szTip[..tip.len()].copy_from_slice(&tip);
    });
    data
}

/// Icône de l'exécutable (ressource 1, voir `packaging/ressources_windows.rs`),
/// à la taille des petites icônes, chargée une fois (partagée : gérée par
/// Windows, jamais détruite).
fn icon() -> HICON {
    if let Some(icon) = ICON.get() {
        return icon;
    }
    let loaded = unsafe {
        GetModuleHandleW(None).ok().and_then(|module| {
            LoadImageW(
                Some(module.into()),
                PCWSTR(1 as *const u16),
                IMAGE_ICON,
                GetSystemMetrics(SM_CXSMICON),
                GetSystemMetrics(SM_CYSMICON),
                LR_DEFAULTCOLOR | LR_SHARED,
            )
            .ok()
        })
    };
    let icon = match loaded {
        Some(handle) => HICON(handle.0),
        None => {
            log::warn!("icône de Polyptyque introuvable dans l'exécutable : icône générique");
            unsafe { LoadIconW(None, IDI_APPLICATION) }.unwrap_or_default()
        }
    };
    ICON.set(Some(icon));
    icon
}

/// Menu du clic droit. Renvoie la commande choisie (0 : aucune).
fn context_menu(window: HWND) -> usize {
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
        return 0;
    };
    let show = HSTRING::from(tr!("Afficher Polyptyque"));
    let quit = HSTRING::from(tr!("Quitter Polyptyque"));
    let mut point = POINT::default();
    let command = unsafe {
        let _ = AppendMenuW(menu, MF_STRING, COMMAND_SHOW, &show);
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, COMMAND_QUIT, &quit);
        let _ = GetCursorPos(&mut point);
        // Sans cela, le menu ne se ferme pas quand on clique ailleurs
        // (documentation de `TrackPopupMenu`).
        let _ = SetForegroundWindow(window);
        let command =
            TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, point.x, point.y, None, window, None);
        let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        command
    };
    command.0 as usize
}

/// Une panique ne doit jamais remonter jusqu'à Windows (procédure de
/// fenêtre).
fn notify(event: TrayEvent) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = HANDLER.try_with(|h| {
            if let Ok(handler) = h.try_borrow() {
                if let Some(handler) = handler.as_ref() {
                    handler(event);
                }
            }
        });
    }));
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    if message == CALLBACK_MESSAGE {
        // Version 4 : l'événement dans le mot bas, l'icône dans le mot haut.
        match lparam.0 as u32 & 0xFFFF {
            NIN_SELECT | NIN_KEYSELECT => notify(TrayEvent::Show),
            WM_CONTEXTMENU => match context_menu(window) {
                COMMAND_SHOW => notify(TrayEvent::Show),
                COMMAND_QUIT => notify(TrayEvent::Quit),
                _ => {}
            },
            _ => {}
        }
        return LRESULT(0);
    }
    if message == show_request_message() {
        notify(TrayEvent::Show);
        return LRESULT(0);
    }
    if message == taskbar_created_message() && SHOWN.get() {
        add(window.0 as isize);
    }
    if message == WM_NCDESTROY {
        if SHOWN.replace(false) {
            let data = icon_data(window.0 as isize);
            let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
        }
        let _ = unsafe { RemoveWindowSubclass(window, Some(window_proc), SUBCLASS_ID) };
    }
    unsafe { DefSubclassProc(window, message, wparam, lparam) }
}
