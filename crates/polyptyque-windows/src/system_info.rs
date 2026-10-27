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

/// Pilotes graphiques suivis dans le journal Système : NVIDIA, AMD, Intel,
/// et « Display » (pilote réinitialisé par Windows, événement 4101).
const DRIVER_PROVIDERS: [&str; 9] =
    ["nvlddmkm", "amdkmdag", "amdwddmg", "igfx", "igfxn", "igfxnd", "igfxncd", "Display", "Microsoft-Windows-DxgKrnl"];
/// Journal Application : application fermée par le pilote OpenGL de NVIDIA
/// (l'interface de Polyptyque dessine en OpenGL), plantages et blocages.
const APPLICATION_PROVIDERS: [&str; 3] = ["NVIDIA OpenGL Driver", "Application Error", "Application Hang"];
/// Plantages et blocages repris : ceux de Polyptyque et de ses encodeurs.
const OUR_PROGRAMS: [&str; 3] = ["polyptyque.exe", "polyptyque-cli.exe", "ffmpeg.exe"];

/// Erreurs et avertissements récents des pilotes graphiques, et plantages de
/// Polyptyque, lus dans les journaux de Windows (`wevtutil`, présent sur
/// tout Windows et lisible sans droits d'administrateur).
pub fn graphics_driver_events() -> Option<Vec<String>> {
    use polyptyque_core::report::{self, DRIVER_EVENTS_DAYS, DRIVER_EVENTS_MAX};
    let system = query_events("System", &DRIVER_PROVIDERS)?;
    let application = query_events("Application", &APPLICATION_PROVIDERS)?;
    let mut lines: Vec<String> = report::parse_events(&system).iter().map(|e| e.line("Système")).collect();
    lines.extend(
        report::parse_events(&application)
            .iter()
            // Le pilote OpenGL de NVIDIA ne ferme que des programmes qu'il
            // fait dessiner : toujours repris.
            .filter(|e| e.provider == "NVIDIA OpenGL Driver" || e.mentions_any(&OUR_PROGRAMS))
            .map(|e| e.line("Application")),
    );
    // Lignes commençant par l'heure : du plus ancien au plus récent.
    lines.sort();
    let skip = lines.len().saturating_sub(DRIVER_EVENTS_MAX);
    log::debug!("rapport : {} événement(s) graphique(s) sur {DRIVER_EVENTS_DAYS} jours", lines.len());
    Some(lines.split_off(skip))
}

/// Export XML des erreurs et avertissements (niveaux 1 à 3) de `providers`
/// dans le journal `log`, sur la période du rapport, les plus récents d'abord.
fn query_events(log: &str, providers: &[&str]) -> Option<String> {
    use polyptyque_core::report::{DRIVER_EVENTS_DAYS, DRIVER_EVENTS_MAX};
    let names = providers.iter().map(|p| format!("@Name='{p}'")).collect::<Vec<_>>().join(" or ");
    let period_ms = u64::from(DRIVER_EVENTS_DAYS) * 24 * 3600 * 1000;
    let query = format!(
        "*[System[Provider[{names}] and (Level=1 or Level=2 or Level=3) and TimeCreated[timediff(@SystemTime) <= {period_ms}]]]"
    );
    let output = Command::new("wevtutil")
        .args(["qe", log])
        .raw_arg(format!("/q:\"{query}\""))
        .args(["/rd:true", &format!("/c:{}", DRIVER_EVENTS_MAX * 2), "/f:xml"])
        .creation_flags(crate::CREATE_NO_WINDOW)
        .output();
    match output {
        Ok(output) if output.status.success() => Some(String::from_utf8_lossy(&output.stdout).into_owned()),
        Ok(output) => {
            log::warn!("journal {log} illisible : {}", String::from_utf8_lossy(&output.stderr).trim());
            None
        }
        Err(e) => {
            log::warn!("journal {log} illisible : {e}");
            None
        }
    }
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

#[cfg(test)]
mod tests {
    /// Lit les vrais journaux de ce PC : `cargo test -p polyptyque-windows -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn lists_this_pc_graphics_events() {
        let events = super::graphics_driver_events().expect("journaux de Windows lisibles");
        for line in &events {
            println!("{line}");
        }
    }
}
