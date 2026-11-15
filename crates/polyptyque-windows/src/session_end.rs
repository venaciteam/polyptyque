//! Fermeture de la session Windows (arrêt, redémarrage, déconnexion) et mise
//! en veille pendant un enregistrement.
//!
//! Pendant un enregistrement, Polyptyque déclare une raison de blocage :
//! Windows l'affiche dans son écran d'arrêt et laisse choisir d'annuler.
//! Si l'arrêt continue malgré tout, les fichiers sont finalisés avant que
//! Windows ne ferme Polyptyque.
//!
//! Une mise en veille demandée (capot, bouton, menu) ne peut pas être
//! empêchée : Windows prévient quelques secondes avant, le temps d'arrêter
//! proprement l'enregistrement, puis de nouveau au réveil.

use std::cell::RefCell;

use anyhow::{Context, Result};
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Power::RegisterSuspendResumeNotification;
use windows::Win32::System::Recovery::{
    RegisterApplicationRestart, UnregisterApplicationRestart, REGISTER_APPLICATION_RESTART_FLAGS,
};
use windows::Win32::System::Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy};
use windows::Win32::System::Threading::SetProcessShutdownParameters;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    DEVICE_NOTIFY_WINDOW_HANDLE, PBT_APMRESUMEAUTOMATIC, PBT_APMSUSPEND, WM_ENDSESSION, WM_NCDESTROY,
    WM_POWERBROADCAST, WM_QUERYENDSESSION,
};

use crate::windows_list::hwnd;

/// Identifiant de notre traitement des messages de la fenêtre.
const SUBCLASS_ID: usize = 0x5059_5054; // « PYPT »
/// Arrêt critique (batterie épuisée, fermeture forcée…) : il ne peut pas
/// être bloqué.
const ENDSESSION_CRITICAL: u32 = 0x4000_0000;
/// Demande du Gestionnaire de redémarrage (installateur qui doit remplacer
/// un fichier ouvert, maintenance du système) : Polyptyque est prié de se
/// fermer, mais la session Windows continue.
const ENDSESSION_CLOSEAPP: u32 = 0x0000_0001;
/// Déconnexion de l'utilisateur.
const ENDSESSION_LOGOFF: u32 = 0x8000_0000;
/// Plus haut niveau réservé aux applications : prévenu avant les processus
/// de niveau ordinaire (0x280), dont les FFmpeg lancés par Polyptyque.
const SHUTDOWN_LEVEL_FIRST: u32 = 0x3FF;

pub enum SessionEnd {
    /// Windows demande s'il peut fermer la session. Le gestionnaire répond
    /// vrai pour bloquer.
    Query,
    /// La session se ferme : tout doit être finalisé avant de répondre.
    Ending,
    /// L'ordinateur va se mettre en veille, dans environ deux secondes.
    Suspend,
    /// L'ordinateur sort de veille.
    Resume,
}

type Handler = Box<dyn Fn(SessionEnd) -> bool>;

thread_local! {
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
}

/// Surveille la fermeture de session sur la fenêtre principale `window`.
/// À appeler sur le fil qui a créé la fenêtre (celui de l'interface).
pub fn watch(window: isize, handler: impl Fn(SessionEnd) -> bool + 'static) -> Result<()> {
    HANDLER.with(|h| *h.borrow_mut() = Some(Box::new(handler)));
    // Polyptyque doit être prévenu avant ses encodeurs : s'ils étaient
    // fermés d'abord, leurs fichiers seraient coupés avant la finalisation.
    if let Err(e) = unsafe { SetProcessShutdownParameters(SHUTDOWN_LEVEL_FIRST, 0) } {
        log::warn!("ordre de fermeture non modifiable : {e}");
    }
    // Sur les PC en veille moderne, seules les fenêtres inscrites sont
    // prévenues de la mise en veille.
    if let Err(e) = unsafe { RegisterSuspendResumeNotification(HANDLE(hwnd(window).0), DEVICE_NOTIFY_WINDOW_HANDLE) } {
        log::warn!("avertissement de mise en veille indisponible : {e}");
    }
    unsafe { SetWindowSubclass(hwnd(window), Some(window_proc), SUBCLASS_ID, 0) }
        .ok()
        .context("surveillance de la fermeture de session")
}

/// Pendant un enregistrement : demande à Windows de relancer Polyptyque
/// s'il est fermé (redémarrage pour une mise à jour, plantage). Le bilan de
/// l'enregistrement interrompu est alors proposé. Windows ne le fait que si
/// la relance des applications à la connexion est activée dans ses
/// paramètres (désactivée par défaut), et si Polyptyque tournait depuis au
/// moins une minute.
pub fn restart_after_interruption(on: bool) {
    let result = unsafe {
        if on {
            RegisterApplicationRestart(w!("--apres-interruption"), REGISTER_APPLICATION_RESTART_FLAGS(0))
        } else {
            UnregisterApplicationRestart()
        }
    };
    if let Err(e) = result {
        log::warn!("relance par Windows : {e}");
    }
}

/// Raison affichée par Windows si l'on tente d'éteindre ou de se
/// déconnecter. Même fil que [`watch`].
pub fn block(window: isize, reason: &str) -> Result<()> {
    unsafe { ShutdownBlockReasonCreate(hwnd(window), &HSTRING::from(reason)) }
        .context("déclaration de l'enregistrement en cours à Windows")
}

pub fn unblock(window: isize) {
    let _ = unsafe { ShutdownBlockReasonDestroy(hwnd(window)) };
}

/// Une panique ne doit jamais remonter jusqu'à Windows (procédure de
/// fenêtre) : le processus s'arrêterait net, en pleine fermeture de session.
fn notify(event: SessionEnd) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        HANDLER
            .try_with(|h| h.try_borrow().ok().and_then(|h| h.as_ref().map(|handler| handler(event))))
            .ok()
            .flatten()
            .unwrap_or(false)
    }))
    .unwrap_or(false)
}

/// Origine d'une demande de fermeture, d'après le `lParam` (un masque de
/// bits) de `WM_QUERYENDSESSION` et `WM_ENDSESSION`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Request {
    /// Arrêt, redémarrage ou déconnexion : la session Windows se ferme.
    Session,
    /// Gestionnaire de redémarrage : seul Polyptyque est prié de se fermer,
    /// Windows continue.
    CloseApp,
}

fn request(lparam: LPARAM) -> Request {
    let flags = lparam.0 as u32;
    if flags & ENDSESSION_CLOSEAPP != 0 && flags & ENDSESSION_LOGOFF == 0 {
        Request::CloseApp
    } else {
        Request::Session
    }
}

fn critical(lparam: LPARAM) -> bool {
    lparam.0 as u32 & ENDSESSION_CRITICAL != 0
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    match message {
        // Gestionnaire de redémarrage pendant un enregistrement : refus,
        // comme pour un arrêt. Selon Microsoft, l'installateur renonce alors
        // (ou demande de fermer Polyptyque, ou de redémarrer plus tard),
        // sauf fermeture forcée : Polyptyque est alors arrêté quoi qu'il
        // réponde, et les FFmpeg de l'enregistrement, processus à part,
        // terminent leurs fichiers seuls. Une demande marquée critique n'est
        // jamais bloquée.
        WM_QUERYENDSESSION => {
            if notify(SessionEnd::Query) && !critical(lparam) {
                match request(lparam) {
                    Request::Session => {
                        log::warn!("fermeture de session demandée pendant un enregistrement : blocage")
                    }
                    Request::CloseApp => log::warn!(
                        "fermeture de Polyptyque demandée par le Gestionnaire de redémarrage (installateur) pendant un enregistrement : refus"
                    ),
                }
                return LRESULT(0);
            }
        }
        WM_ENDSESSION if wparam.0 != 0 => {
            // Gestionnaire de redémarrage non forcé : Windows continue, et
            // Polyptyque aussi. Le traiter comme une fin de session
            // arrêterait l'enregistrement et marquerait la fin du programme
            // comme normale alors qu'il reste ouvert (plus aucun bilan après
            // un plantage). La demande n'a été acceptée que hors
            // enregistrement ; la suite passe par la fermeture ordinaire
            // (WM_CLOSE envoyé ensuite par le Gestionnaire), avec ses
            // confirmations. Marquée critique (fermeture imposée) : comme
            // une fin de session, les fichiers sont finalisés d'abord.

            if request(lparam) == Request::CloseApp && !critical(lparam) {
                log::info!("fermeture demandée par le Gestionnaire de redémarrage : la session Windows continue");
                return LRESULT(0);
            }
            log::warn!("fermeture de session : finalisation des fichiers");
            notify(SessionEnd::Ending);
            return LRESULT(0);
        }
        WM_POWERBROADCAST => match wparam.0 as u32 {
            PBT_APMSUSPEND => {
                notify(SessionEnd::Suspend);
            }
            PBT_APMRESUMEAUTOMATIC => {
                notify(SessionEnd::Resume);
            }
            _ => {}
        },
        WM_NCDESTROY => {
            let _ = unsafe { RemoveWindowSubclass(window, Some(window_proc), SUBCLASS_ID) };
        }
        _ => {}
    }
    unsafe { DefSubclassProc(window, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origine_de_la_demande() {
        // Arrêt ou redémarrage : lParam nul.
        assert_eq!(request(LPARAM(0)), Request::Session);
        assert_eq!(request(LPARAM(0x8000_0000_u32 as isize)), Request::Session);
        assert_eq!(request(LPARAM(0x4000_0000)), Request::Session);
        // Gestionnaire de redémarrage, forcé ou non.
        assert_eq!(request(LPARAM(0x1)), Request::CloseApp);
        assert_eq!(request(LPARAM(0x4000_0001)), Request::CloseApp);
        assert!(critical(LPARAM(0x4000_0001)));
        assert!(!critical(LPARAM(0x1)));
        // Déconnexion signalée en même temps : la session se ferme bien.
        assert_eq!(request(LPARAM(0x8000_0001_u32 as isize)), Request::Session);
    }
}
