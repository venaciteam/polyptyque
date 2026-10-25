//! Description du système pour un rapport de problème, et ouverture
//! d'adresses et de fichiers.

use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Result};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

const CURRENT_VERSION: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

/// Format régional de Windows (Paramètres > Heure et langue > Langue et
/// région) : heure courte sur 12 h (« h:mm tt ») ou 24 h (« HH:mm »).
pub fn uses_twelve_hour_clock() -> bool {
    const INTERNATIONAL: &str = r"Control Panel\International";
    if let Some(format) = registry_string_in(HKEY_CURRENT_USER, INTERNATIONAL, "sShortTime") {
        return format.contains('h') && !format.contains('H');
    }
    // Ancienne valeur : « 0 » pour 12 h, « 1 » pour 24 h.
    registry_string_in(HKEY_CURRENT_USER, INTERNATIONAL, "iTime").is_some_and(|v| v.trim() == "0")
}

/// « Windows 11 Pro 24H2 (build 26100.2033) ».
pub fn os_description() -> String {
    let product = registry_string(CURRENT_VERSION, "ProductName").unwrap_or_else(|| "Windows".into());
    let build: u32 = registry_string(CURRENT_VERSION, "CurrentBuildNumber").and_then(|b| b.parse().ok()).unwrap_or(0);
    // Windows 11 garde « Windows 10 » dans ProductName : seul le numéro de
    // build les distingue.
    let mut text = if build >= 22000 { product.replacen("Windows 10", "Windows 11", 1) } else { product };
    if let Some(display) = registry_string(CURRENT_VERSION, "DisplayVersion") {
        text.push(' ');
        text.push_str(&display);
    }
    if build > 0 {
        let revision = registry_dword(CURRENT_VERSION, "UBR").map(|r| format!(".{r}")).unwrap_or_default();
        text.push_str(&format!(" (build {build}{revision})"));
    }
    text
}

/// Noms des cartes graphiques (adaptateurs matériels, sans le pilote
/// logiciel de Windows).
pub fn graphics_adapters() -> Vec<String> {
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return Vec::new();
    };
    let mut names: Vec<String> = Vec::new();
    for index in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
        let name = String::from_utf16_lossy(&desc.Description[..len]).trim().to_string();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// Adresse web (navigateur par défaut) ou fichier (programme habituel).
pub fn shell_open(target: &str) -> Result<()> {
    let result = unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(target), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL)
    };
    // Une valeur de 32 ou moins signale une erreur.
    if result.0 as isize <= 32 {
        bail!(tr::tr!("ouverture impossible (code {})", result.0 as isize));
    }
    Ok(())
}

/// Ouvre l'Explorateur sur le dossier du fichier, le fichier sélectionné.
pub fn reveal_file(path: &Path) -> Result<()> {
    Command::new("explorer").raw_arg(format!("/select,\"{}\"", path.display())).spawn()?;
    Ok(())
}

fn registry_string(path: &str, name: &str) -> Option<String> {
    registry_string_in(HKEY_LOCAL_MACHINE, path, name)
}

fn registry_string_in(root: HKEY, path: &str, name: &str) -> Option<String> {
    let (path, name) = (HSTRING::from(path), HSTRING::from(name));
    let mut size = 0u32;
    unsafe {
        if RegGetValueW(root, &path, &name, RRF_RT_REG_SZ, None, None, Some(&mut size)) != ERROR_SUCCESS {
            return None;
        }
        let mut buffer = vec![0u16; size as usize / 2 + 1];
        let mut size = (buffer.len() * 2) as u32;
        if RegGetValueW(
            root,
            &path,
            &name,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr() as *mut c_void),
            Some(&mut size),
        ) != ERROR_SUCCESS
        {
            return None;
        }
        let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..len]))
    }
}

fn registry_dword(path: &str, name: &str) -> Option<u32> {
    let (path, name) = (HSTRING::from(path), HSTRING::from(name));
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &path,
            &name,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(value)
}
