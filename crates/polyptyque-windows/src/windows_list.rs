//! Énumération des fenêtres principales.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;

use anyhow::{bail, Result};
use polyptyque_core::platform::{WindowDiagnostic, WindowHandle, WindowInfo};
use windows::core::{w, BOOL, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, TRUE, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONULL, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, GetClassNameW, GetWindow, GetWindowLongW, GetWindowPlacement, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, PostMessageW, SetWindowPos, ShowWindow,
    GWL_EXSTYLE, GW_OWNER, SWP_NOACTIVATE, SWP_NOZORDER, SW_SHOWMAXIMIZED, SW_SHOWNOACTIVATE, WINDOWPLACEMENT,
    WM_CLOSE, WPF_RESTORETOMAXIMIZED, WS_EX_TOOLWINDOW,
};

use crate::process;

/// Fenêtres du bureau et de la barre des tâches, jamais utiles à capturer.
const IGNORED_CLASSES: &[&str] = &["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"];
/// Cadre des applications du Microsoft Store (UWP) : il appartient à
/// ApplicationFrameHost.exe, commun à toutes, et contient la fenêtre de
/// l'application elle-même ([`CORE_WINDOW_CLASS`]), qui appartient à son
/// vrai processus.
const STORE_FRAME_CLASS: &str = "ApplicationFrameWindow";
const CORE_WINDOW_CLASS: PCWSTR = w!("Windows.UI.Core.CoreWindow");

pub fn hwnd(handle: WindowHandle) -> HWND {
    HWND(handle as *mut c_void)
}

/// Fenêtres principales visibles, dans l'ordre d'empilement (la plus
/// récemment utilisée en premier).
pub fn list(own_pid: u32) -> Vec<WindowInfo> {
    let mut handles: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut handles as *mut Vec<HWND> as isize));
    }
    handles.into_iter().filter_map(|w| describe(w, false)).filter(|w| w.pid != own_pid).collect()
}

/// Fenêtres principales, y compris cachées (Polyptyque dans la zone de
/// notification).
pub(crate) fn list_including_hidden(own_pid: u32) -> Vec<WindowInfo> {
    let mut handles: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut handles as *mut Vec<HWND> as isize));
    }
    handles.into_iter().filter_map(|w| describe(w, true)).filter(|w| w.pid != own_pid).collect()
}

/// Fenêtres de premier niveau de la classe `class`, cachées comprises, avec
/// ce qui peut les écarter de [`list`] (journal seulement).
pub fn diagnostics(class: &str) -> Vec<WindowDiagnostic> {
    let mut handles: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut handles as *mut Vec<HWND> as isize));
    }
    handles
        .into_iter()
        .filter(|&w| class_name(w) == class)
        .map(|window| unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(window, Some(&mut pid));
            let ex_style = GetWindowLongW(window, GWL_EXSTYLE) as u32;
            WindowDiagnostic {
                handle: window.0 as WindowHandle,
                pid,
                visible: IsWindowVisible(window).as_bool(),
                cloaked: is_cloaked(window),
                owned: GetWindow(window, GW_OWNER).is_ok_and(|owner| !owner.is_invalid()),
                tool_window: ex_style & WS_EX_TOOLWINDOW.0 != 0,
                has_title: !text(window).trim().is_empty(),
                minimized: IsIconic(window).as_bool(),
            }
        })
        .collect()
}

unsafe extern "system" fn collect(window: HWND, lparam: LPARAM) -> BOOL {
    let handles = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    handles.push(window);
    TRUE
}

fn describe(window: HWND, include_hidden: bool) -> Option<WindowInfo> {
    unsafe {
        if !include_hidden && (!IsWindowVisible(window).as_bool() || is_cloaked(window)) {
            return None;
        }
        let ex_style = GetWindowLongW(window, GWL_EXSTYLE) as u32;
        if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
            return None;
        }
        if let Ok(owner) = GetWindow(window, GW_OWNER) {
            if !owner.is_invalid() {
                return None;
            }
        }
        let title = text(window);
        if title.trim().is_empty() {
            return None;
        }
        let class = class_name(window);
        if IGNORED_CLASSES.contains(&class.as_str()) {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(window, Some(&mut pid));
        if class == STORE_FRAME_CLASS {
            if let Some(app) = store_app_pid(window, pid, &title) {
                pid = app;
            }
        }
        let exe = process::image_path(pid).unwrap_or_default();
        Some(WindowInfo { handle: window.0 as WindowHandle, pid, title, class, exe })
    }
}

fn window_pid(window: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    pid
}

/// Processus d'application du Store vu en dernier dans chaque cadre.
static STORE_APPS: Mutex<Option<HashMap<isize, u32>>> = Mutex::new(None);

/// Vrai processus de l'application du Store affichée dans le cadre `frame`
/// (qui appartient à `host_pid`, ApplicationFrameHost.exe) : c'est lui qui
/// identifie la source et dont le son est capturé, celui du cadre étant
/// commun à toutes les applications du Store, et muet.
///
/// Fenêtre réduite (ou application en pause) : Windows détache la fenêtre
/// de l'application du cadre, qui devient une fenêtre de premier niveau.
/// Par prudence, seul un processus sûr est retenu, dans l'ordre : celui vu
/// dans ce cadre auparavant s'il a toujours sa fenêtre d'application, puis
/// la seule fenêtre d'application détachée de même titre. Sinon `None` : le
/// cadre reste attribué à ApplicationFrameHost.exe (comportement d'avant,
/// son muet), plutôt qu'à une application qui ne serait pas la bonne.
fn store_app_pid(frame: HWND, host_pid: u32, title: &str) -> Option<u32> {
    let mut known = STORE_APPS.lock().unwrap_or_else(|e| e.into_inner());
    let known = known.get_or_insert_with(HashMap::new);
    let key = frame.0 as isize;
    // Fenêtre de l'application dans le cadre.
    let mut child: Option<HWND> = None;
    while let Ok(next) = unsafe { FindWindowExW(Some(frame), child, CORE_WINDOW_CLASS, PCWSTR::null()) } {
        let pid = window_pid(next);
        if pid != 0 && pid != host_pid {
            // Cadres disparus : oubliés (une poignée peut resservir).
            known.retain(|&handle, _| unsafe { IsWindow(Some(HWND(handle as *mut c_void))).as_bool() });
            known.insert(key, pid);
            return Some(pid);
        }
        child = Some(next);
    }
    // Fenêtre d'application détachée (cadre réduit).
    let detached = |name: PCWSTR| -> Vec<u32> {
        let mut pids = Vec::new();
        let mut after: Option<HWND> = None;
        while let Ok(next) = unsafe { FindWindowExW(None, after, CORE_WINDOW_CLASS, name) } {
            let pid = window_pid(next);
            if pid != 0 && pid != host_pid && !pids.contains(&pid) {
                pids.push(pid);
            }
            after = Some(next);
        }
        pids
    };
    if let Some(&pid) = known.get(&key) {
        if detached(PCWSTR::null()).contains(&pid) {
            return Some(pid);
        }
        known.remove(&key);
    }
    let title = HSTRING::from(title);
    match detached(PCWSTR(title.as_ptr()))[..] {
        [pid] => Some(pid),
        _ => None,
    }
}

/// Fenêtres « masquées » par Windows (applications suspendues, bureaux
/// virtuels…) : visibles en apparence mais jamais affichées.
fn is_cloaked(window: HWND) -> bool {
    let mut cloaked: u32 = 0;
    let result = unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut c_void,
            std::mem::size_of::<u32>() as u32,
        )
    };
    result.is_ok() && cloaked != 0
}

fn text(window: HWND) -> String {
    let mut buffer = [0u16; 512];
    let len = unsafe { GetWindowTextW(window, &mut buffer) };
    String::from_utf16_lossy(&buffer[..len.max(0) as usize])
}

fn class_name(window: HWND) -> String {
    let mut buffer = [0u16; 256];
    let len = unsafe { GetClassNameW(window, &mut buffer) };
    String::from_utf16_lossy(&buffer[..len.max(0) as usize])
}

pub fn is_alive(handle: WindowHandle) -> bool {
    unsafe { IsWindow(Some(hwnd(handle))).as_bool() }
}

pub fn is_minimized(handle: WindowHandle) -> bool {
    unsafe { IsIconic(hwnd(handle)).as_bool() }
}

/// Demande à la fenêtre de se fermer (`WM_CLOSE`, comme sa croix), sans
/// attendre : un programme figé ne bloque pas l'appelant.
pub fn close(handle: WindowHandle) -> Result<()> {
    unsafe { PostMessageW(Some(hwnd(handle)), WM_CLOSE, WPARAM(0), LPARAM(0)) }?;
    Ok(())
}

/// Fait revenir une fenêtre réduite sans lui donner le focus, et la replace
/// au centre de l'écran principal si elle n'est plus sur aucun écran (écran
/// débranché, par exemple). Peut attendre le programme de la fenêtre : à
/// appeler hors du fil de l'interface.
pub fn restore(handle: WindowHandle) -> Result<()> {
    let window = hwnd(handle);
    unsafe {
        if !IsWindow(Some(window)).as_bool() {
            bail!(tr::tr!("la fenêtre n'existe plus"));
        }
        if IsIconic(window).as_bool() {
            let mut placement =
                WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
            let maximized =
                GetWindowPlacement(window, &mut placement).is_ok() && placement.flags.0 & WPF_RESTORETOMAXIMIZED.0 != 0;
            // Réduite depuis le plein écran : elle y revient, en prenant le
            // focus (Windows n'agrandit pas une fenêtre sans l'activer). Sinon
            // elle revient à sa taille, sans prendre le focus.
            let _ = ShowWindow(window, if maximized { SW_SHOWMAXIMIZED } else { SW_SHOWNOACTIVATE });
        }
        if !MonitorFromWindow(window, MONITOR_DEFAULTTONULL).is_invalid() {
            return Ok(());
        }
        let mut rect = RECT::default();
        GetWindowRect(window, &mut rect)?;
        let primary = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(primary, &mut info).as_bool() {
            bail!(tr::tr!("écran principal introuvable"));
        }
        let area = info.rcWork;
        let (area_width, area_height) = (area.right - area.left, area.bottom - area.top);
        let width = (rect.right - rect.left).clamp(320.min(area_width), area_width);
        let height = (rect.bottom - rect.top).clamp(240.min(area_height), area_height);
        let x = area.left + (area_width - width) / 2;
        let y = area.top + (area_height - height) / 2;
        SetWindowPos(window, None, x, y, width, height, SWP_NOZORDER | SWP_NOACTIVATE)?;
        log::info!("fenêtre replacée sur l'écran principal");
    }
    Ok(())
}
