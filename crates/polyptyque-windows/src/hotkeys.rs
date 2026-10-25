//! Raccourcis clavier globaux, actifs même quand Polyptyque est en
//! arrière-plan. Ils sont enregistrés sur un fil dédié qui possède sa propre
//! file de messages Windows.

use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use anyhow::{anyhow, Result};
use polyptyque_core::hotkeys::Hotkey;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, VK_F1,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, PeekMessageW, PostThreadMessageW, MSG, PM_NOREMOVE, WM_HOTKEY, WM_QUIT,
};

pub struct HotkeyListener {
    thread_id: u32,
    handle: Option<JoinHandle<()>>,
}

impl HotkeyListener {
    /// Enregistre les raccourcis. `on_press(i)` est appelé, depuis le fil
    /// d'écoute, quand le raccourci `keys[i]` est pressé. Renvoie aussi les
    /// raccourcis déjà pris par un autre programme.
    pub fn start(keys: Vec<Hotkey>, on_press: impl Fn(usize) + Send + 'static) -> Result<(Self, Vec<String>)> {
        let (ready_tx, ready_rx) = mpsc::channel();
        let handle = thread::Builder::new().name("raccourcis".into()).spawn(move || {
            let mut msg = MSG::default();
            unsafe {
                // Crée la file de messages du fil avant toute chose.
                let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
            }
            let thread_id = unsafe { GetCurrentThreadId() };
            let failed: Vec<String> = keys
                .iter()
                .enumerate()
                .filter(|(i, key)| unsafe { RegisterHotKey(None, *i as i32 + 1, modifiers(key), vk(key)) }.is_err())
                .map(|(_, key)| describe(key))
                .collect();
            let _ = ready_tx.send((thread_id, failed));

            loop {
                // 0 : WM_QUIT reçu ; -1 : erreur.
                let result = unsafe { GetMessageW(&mut msg, None, 0, 0) };
                if result.0 <= 0 {
                    break;
                }
                if msg.message == WM_HOTKEY && msg.wParam.0 >= 1 {
                    on_press(msg.wParam.0 - 1);
                }
            }
            for i in 0..keys.len() {
                let _ = unsafe { UnregisterHotKey(None, i as i32 + 1) };
            }
        })?;
        let (thread_id, failed) = ready_rx.recv().map_err(|_| anyhow!("le fil des raccourcis s'est arrêté"))?;
        Ok((Self { thread_id, handle: Some(handle) }, failed))
    }
}

impl Drop for HotkeyListener {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn modifiers(key: &Hotkey) -> HOT_KEY_MODIFIERS {
    let mut modifiers = MOD_NOREPEAT;
    if key.ctrl {
        modifiers |= MOD_CONTROL;
    }
    if key.shift {
        modifiers |= MOD_SHIFT;
    }
    if key.alt {
        modifiers |= MOD_ALT;
    }
    modifiers
}

fn vk(key: &Hotkey) -> u32 {
    VK_F1.0 as u32 + key.function_key as u32 - 1
}

fn describe(key: &Hotkey) -> String {
    let mut parts = Vec::new();
    if key.ctrl {
        parts.push("Ctrl".to_string());
    }
    if key.shift {
        parts.push(tr::tr!("Maj"));
    }
    if key.alt {
        parts.push("Alt".to_string());
    }
    parts.push(format!("F{}", key.function_key));
    parts.join("+")
}
