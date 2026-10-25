//! Énumération des fenêtres principales.

use std::ffi::c_void;

use anyhow::{bail, Result};
use polyptyque_core::platform::{WindowHandle, WindowInfo};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, TRUE, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONULL, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindow, GetWindowLongW, GetWindowPlacement, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, PostMessageW, SetWindowPos, ShowWindow,
    GWL_EXSTYLE, GW_OWNER, SWP_NOACTIVATE, SWP_NOZORDER, SW_SHOWMAXIMIZED, SW_SHOWNOACTIVATE, WINDOWPLACEMENT,
    WM_CLOSE, WPF_RESTORETOMAXIMIZED, WS_EX_TOOLWINDOW,
};

use crate::process;

/// Fenêtres du bureau et de la barre des tâches, jamais utiles à capturer.
const IGNORED_CLASSES: &[&str] = &["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"];

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
    handles.into_iter().filter_map(describe).filter(|w| w.pid != own_pid).collect()
}

unsafe extern "system" fn collect(window: HWND, lparam: LPARAM) -> BOOL {
    let handles = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    handles.push(window);
    TRUE
}

fn describe(window: HWND) -> Option<WindowInfo> {
    unsafe {
        if !IsWindowVisible(window).as_bool() || is_cloaked(window) {
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
        let exe = process::image_path(pid).unwrap_or_default();
        Some(WindowInfo { handle: window.0 as WindowHandle, pid, title, class, exe })
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
