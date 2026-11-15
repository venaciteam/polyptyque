//! Informations sur les processus : chemin de l'exécutable et ligne de
//! commande (qui permet de reconnaître le profil d'un navigateur), FFmpeg
//! auxiliaires arrêtés avec Polyptyque.

use std::ffi::{c_void, OsString};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::process::Child;
use std::sync::OnceLock;

use windows::core::{PCWSTR, PWSTR};
use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessCommandLineInformation};
use windows::Win32::Foundation::{CloseHandle, HANDLE, UNICODE_STRING};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// Poignée de processus fermée automatiquement.
struct ProcessHandle(HANDLE);

impl ProcessHandle {
    fn open(pid: u32) -> Option<Self> {
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok().map(ProcessHandle) }
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub fn image_path(pid: u32) -> Option<PathBuf> {
    let process = ProcessHandle::open(pid)?;
    let mut buffer = vec![0u16; 1024];
    let mut len = buffer.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(process.0, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len).ok()?;
    }
    Some(PathBuf::from(OsString::from_wide(&buffer[..len as usize])))
}

pub fn command_line(pid: u32) -> Option<String> {
    let process = ProcessHandle::open(pid)?;
    // Tampon aligné sur 8 octets : le résultat commence par une structure
    // UNICODE_STRING qui pointe vers le texte placé juste après.
    let mut buffer: Vec<u64> = vec![0; 1024];
    for _ in 0..2 {
        let mut needed = 0u32;
        let status = unsafe {
            NtQueryInformationProcess(
                process.0,
                ProcessCommandLineInformation,
                buffer.as_mut_ptr() as *mut c_void,
                (buffer.len() * 8) as u32,
                &mut needed,
            )
        };
        if status.0 >= 0 {
            let text = unsafe {
                let header = &*(buffer.as_ptr() as *const UNICODE_STRING);
                if header.Buffer.is_null() || header.Length == 0 {
                    return Some(String::new());
                }
                std::slice::from_raw_parts(header.Buffer.0, header.Length as usize / 2)
            };
            return Some(String::from_utf16_lossy(text));
        }
        if needed as usize > buffer.len() * 8 {
            buffer = vec![0; needed as usize / 8 + 1];
        } else {
            return None;
        }
    }
    None
}

/// Objet tâche de Windows : ses processus sont arrêtés quand il est fermé.
struct Job(HANDLE);

// Une poignée d'objet tâche s'utilise depuis n'importe quel fil.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Job {
    /// Processus arrêtés à la fermeture de la dernière poignée, donc aussi
    /// quand Polyptyque se termine, même brutalement (Windows ferme alors
    /// ses poignées).
    fn kill_on_close() -> windows::core::Result<Self> {
        let job = Job(unsafe { CreateJobObjectW(None, PCWSTR::null())? });
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )?;
        }
        Ok(job)
    }

    fn assign(&self, child: &Child) -> windows::core::Result<()> {
        unsafe { AssignProcessToJobObject(self.0, HANDLE(child.as_raw_handle())) }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Rattache un FFmpeg auxiliaire (conversion, réparation, lecture des
/// pistes, essai d'encodeur) à Polyptyque : s'il reste figé, il est arrêté
/// avec Polyptyque, plantage compris, au lieu de lui survivre.
///
/// Jamais pour les FFmpeg d'un enregistrement : ils doivent pouvoir
/// terminer leurs fichiers seuls si Polyptyque s'arrête net.
///
/// À appeler juste après le lancement. FFmpeg ne lance aucun autre
/// programme : rien ne lui échappe entre son lancement et son rattachement.
pub fn end_with_polyptyque(child: &Child) {
    if let Some(job) = helpers() {
        if let Err(e) = job.assign(child) {
            log::warn!("FFmpeg (processus {}) non rattaché à Polyptyque : {e}", child.id());
        }
    }
}

/// Objet tâche des FFmpeg auxiliaires, créé au premier besoin et gardé
/// ouvert jusqu'à la fin de Polyptyque.
fn helpers() -> Option<&'static Job> {
    static HELPERS: OnceLock<Option<Job>> = OnceLock::new();
    HELPERS
        .get_or_init(|| {
            Job::kill_on_close().map_err(|e| log::warn!("objet tâche des FFmpeg auxiliaires indisponible : {e}")).ok()
        })
        .as_ref()
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use windows::Win32::System::JobObjects::IsProcessInJob;

    use super::*;

    fn long_process() -> Child {
        Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("lancement de ping")
    }

    fn ends_within(child: &mut Child, delay: Duration) -> bool {
        let deadline = Instant::now() + delay;
        while Instant::now() < deadline {
            if child.try_wait().expect("état du processus").is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn processus_arrete_a_la_fermeture() {
        let job = Job::kill_on_close().unwrap();
        let mut child = long_process();
        job.assign(&child).unwrap();
        assert!(!ends_within(&mut child, Duration::from_millis(200)));
        drop(job);
        assert!(ends_within(&mut child, Duration::from_secs(5)), "processus encore en vie");
    }

    #[test]
    fn rattachement() {
        let mut child = long_process();
        end_with_polyptyque(&child);
        let mut inside = windows::core::BOOL::default();
        unsafe { IsProcessInJob(HANDLE(child.as_raw_handle()), Some(helpers().unwrap().0), &mut inside).unwrap() };
        assert!(inside.as_bool());
        let _ = child.kill();
        let _ = child.wait();
    }
}
