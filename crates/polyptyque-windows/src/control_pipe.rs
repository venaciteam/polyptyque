//! Canal de commande sous Windows (voir « Pilotage par d'autres
//! programmes » dans le cahier des charges) : tube nommé
//! `\\.\pipe\polyptyque-<SID>`, réservé au compte qui a lancé Polyptyque.
//!
//! - Autorisations du tube : ce compte seul, propriétaire compris (celles
//!   que Windows donne par défaut ouvrent la lecture à tous les comptes,
//!   anonyme compris).
//! - Connexions depuis une autre machine refusées par le système.
//! - Polyptyque crée la première instance du tube, ou renonce : un autre
//!   programme qui l'aurait créée avant lui ne reçoit rien. Côté client, le
//!   propriétaire du tube est vérifié avant tout envoi.
//!
//! Entrées-sorties asynchrones (OVERLAPPED) : une connexion lit et écrit
//! depuis le même fil, sans que l'écriture attende la fin de la lecture en
//! cours. Avec une poignée synchrone, Windows exécute les deux l'une après
//! l'autre : les notifications de l'état en direct resteraient bloquées
//! derrière une lecture.

use std::ffi::c_void;
use std::io;
use std::time::Duration;

use polyptyque_core::control::transport::{ConnectError, Connection, Listener, Peer};
use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, ERROR_SUCCESS, GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
    SE_KERNEL_OBJECT,
};
use windows::Win32::Security::{
    GetTokenInformation, TokenUser, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
    TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE,
    OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, WaitNamedPipeW, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Threading::{CreateEventW, GetCurrentProcess, OpenProcessToken, WaitForSingleObject};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

use crate::process;

/// Tampons du tube, de chaque côté.
const BUFFER: u32 = 64 * 1024;
/// Au-delà, l'autre côté ne lit plus : la connexion est abandonnée.
const WRITE_TIMEOUT_MS: u32 = 5_000;

fn is(error: &windows::core::Error, code: windows::Win32::Foundation::WIN32_ERROR) -> bool {
    error.code() == code.to_hresult()
}

fn millis(timeout: Duration) -> u32 {
    timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32
}

/// Poignée fermée automatiquement.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// Texte alloué par Windows (`LocalAlloc`), libéré automatiquement.
struct LocalText(PWSTR);

impl LocalText {
    fn to_string(&self) -> io::Result<String> {
        unsafe { self.0.to_string() }.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

impl Drop for LocalText {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.0 .0 as *mut c_void)));
            }
        }
    }
}

fn sid_text(sid: PSID) -> io::Result<String> {
    let mut text = PWSTR::null();
    unsafe { ConvertSidToStringSidW(sid, &mut text)? };
    LocalText(text).to_string()
}

/// SID du compte qui exécute ce programme (« S-1-5-21-… »).
pub fn user_sid() -> io::Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let token = Owned(token);
        let mut needed = 0u32;
        let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut needed);
        // Tampon aligné : il commence par une structure qui contient un
        // pointeur.
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8).max(1)];
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buffer.as_mut_ptr() as *mut c_void),
            (buffer.len() * 8) as u32,
            &mut needed,
        )?;
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        sid_text(user.User.Sid)
    }
}

/// Chemin du tube de ce compte.
pub fn pipe_path() -> io::Result<String> {
    Ok(format!(r"\\.\pipe\polyptyque-{}", user_sid()?))
}

/// Autorisations du tube : propriétaire et accès complet pour ce compte,
/// rien pour les autres (« P » : aucune autorisation héritée ajoutée).
struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl SecurityDescriptor {
    fn for_user(sid: &str) -> io::Result<Self> {
        let sddl = HSTRING::from(format!("O:{sid}D:P(A;;GA;;;{sid})"));
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(&sddl, SDDL_REVISION_1, &mut descriptor, None)? };
        Ok(Self(descriptor))
    }

    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0 .0,
            bInheritHandle: false.into(),
        }
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.0 .0)));
        }
    }
}

/// Instance du tube qui attend un client.
struct Waiting {
    pipe: Option<HANDLE>,
    /// Adresse stable : Windows y écrit jusqu'à la fin de l'attente.
    overlapped: Box<OVERLAPPED>,
    event: Owned,
    connected: bool,
}

impl Waiting {
    /// La connexion est établie : l'instance passe à la connexion.
    fn into_pipe(mut self) -> HANDLE {
        self.pipe.take().unwrap_or_default()
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        let Some(pipe) = self.pipe.take() else {
            return;
        };
        unsafe {
            if !self.connected {
                let _ = CancelIoEx(pipe, Some(&*self.overlapped));
                let mut transferred = 0;
                let _ = GetOverlappedResult(pipe, &*self.overlapped, &mut transferred, true);
            }
            let _ = CloseHandle(pipe);
        }
    }
}

/// Côté Polyptyque : le tube et l'instance qui attend le client suivant.
pub struct PipeListener {
    path: HSTRING,
    security: SecurityDescriptor,
    waiting: Option<Waiting>,
}

// Utilisé par un seul fil à la fois (celui qui attend les clients).
unsafe impl Send for PipeListener {}

impl PipeListener {
    /// Crée le tube. Échoue si un autre programme l'a déjà créé.
    pub fn bind() -> io::Result<Self> {
        let sid = user_sid()?;
        let path = HSTRING::from(format!(r"\\.\pipe\polyptyque-{sid}"));
        let security = SecurityDescriptor::for_user(&sid)?;
        let mut listener = Self { path, security, waiting: None };
        listener.waiting = Some(listener.listen(true)?);
        Ok(listener)
    }

    /// Nouvelle instance du tube, en attente d'un client. `first` : la
    /// toute première, qui échoue si le nom est déjà pris.
    fn listen(&self, first: bool) -> io::Result<Waiting> {
        let attributes = self.security.attributes();
        let mut mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
        if first {
            mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        let pipe = unsafe {
            CreateNamedPipeW(
                &self.path,
                mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                BUFFER,
                BUFFER,
                0,
                Some(&attributes),
            )
        };
        if pipe.is_invalid() {
            return Err(io::Error::last_os_error());
        }
        let event = match unsafe { CreateEventW(None, true, false, PCWSTR::null()) } {
            Ok(event) => Owned(event),
            Err(e) => {
                unsafe {
                    let _ = CloseHandle(pipe);
                }
                return Err(e.into());
            }
        };
        let overlapped = Box::new(OVERLAPPED { hEvent: event.0, ..Default::default() });
        let mut waiting = Waiting { pipe: Some(pipe), overlapped, event, connected: false };
        match unsafe { ConnectNamedPipe(pipe, Some(&mut *waiting.overlapped)) } {
            // Un client arrivé entre la création et l'attente.
            Ok(()) => waiting.connected = true,
            Err(e) if is(&e, ERROR_PIPE_CONNECTED) => waiting.connected = true,
            Err(e) if is(&e, ERROR_IO_PENDING) => {}
            Err(e) => {
                // Aucune attente à annuler : l'instance est seulement fermée.
                waiting.connected = true;
                return Err(e.into());
            }
        }
        Ok(waiting)
    }
}

impl Listener for PipeListener {
    fn accept(&mut self, timeout: Duration) -> io::Result<Option<(Box<dyn Connection>, Peer)>> {
        if self.waiting.is_none() {
            self.waiting = Some(self.listen(false)?);
        }
        let waiting = self.waiting.as_mut().expect("instance en attente");
        if !waiting.connected {
            match unsafe { WaitForSingleObject(waiting.event.0, millis(timeout)) } {
                WAIT_OBJECT_0 => {}
                WAIT_TIMEOUT => return Ok(None),
                _ => return Err(io::Error::last_os_error()),
            }
            let pipe = waiting.pipe.unwrap_or_default();
            let mut transferred = 0;
            let result = unsafe { GetOverlappedResult(pipe, &*waiting.overlapped, &mut transferred, false) };
            waiting.connected = true;
            if let Err(e) = result {
                // Client reparti aussitôt : l'instance est remplacée.
                log::warn!("pilotage : connexion avortée : {e}");
                self.waiting = None;
                return Ok(None);
            }
        }
        let pipe = self.waiting.take().expect("instance connectée").into_pipe();
        // Instance suivante tout de suite, pour les prochains clients (une
        // erreur sera retentée à l'appel suivant).
        match self.listen(false) {
            Ok(next) => self.waiting = Some(next),
            Err(e) => log::warn!("pilotage : nouvelle instance du tube impossible : {e}"),
        }
        let mut pid = 0u32;
        let peer = if unsafe { GetNamedPipeClientProcessId(pipe, &mut pid) }.is_ok() {
            Peer {
                pid: Some(pid),
                program: process::image_path(pid).and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())),
            }
        } else {
            Peer::default()
        };
        let connection = PipeConnection::new(pipe)?;
        Ok(Some((Box::new(connection), peer)))
    }
}

/// Une connexion, côté Polyptyque comme côté client.
pub struct PipeConnection {
    pipe: Owned,
    read: Box<OVERLAPPED>,
    read_event: Owned,
    /// Lecture en cours : Windows écrit dans `read_buffer` jusqu'à sa fin.
    read_pending: bool,
    read_buffer: Box<[u8]>,
    /// Lu mais pas encore rendu (tampon de l'appelant trop petit).
    leftover: Vec<u8>,
    write: Box<OVERLAPPED>,
    write_event: Owned,
}

// La poignée n'est utilisée que par un seul fil à la fois.
unsafe impl Send for PipeConnection {}

impl PipeConnection {
    fn new(pipe: HANDLE) -> io::Result<Self> {
        let pipe = Owned(pipe);
        let read_event = Owned(unsafe { CreateEventW(None, true, false, PCWSTR::null())? });
        let write_event = Owned(unsafe { CreateEventW(None, true, false, PCWSTR::null())? });
        Ok(Self {
            pipe,
            read: Box::default(),
            read_event,
            read_pending: false,
            read_buffer: vec![0u8; BUFFER as usize].into_boxed_slice(),
            leftover: Vec::new(),
            write: Box::default(),
            write_event,
        })
    }

    fn take_leftover(&mut self, buf: &mut [u8]) -> usize {
        let n = self.leftover.len().min(buf.len());
        buf[..n].copy_from_slice(&self.leftover[..n]);
        self.leftover.drain(..n);
        n
    }
}

fn closed(error: &windows::core::Error) -> bool {
    is(error, ERROR_BROKEN_PIPE) || is(error, ERROR_PIPE_NOT_CONNECTED) || is(error, ERROR_NO_DATA)
}

impl Connection for PipeConnection {
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<Option<usize>> {
        if !self.leftover.is_empty() {
            return Ok(Some(self.take_leftover(buf)));
        }
        if !self.read_pending {
            *self.read = OVERLAPPED { hEvent: self.read_event.0, ..Default::default() };
            match unsafe { ReadFile(self.pipe.0, Some(&mut self.read_buffer), None, Some(&mut *self.read)) } {
                // Terminée tout de suite : le résultat se lit comme une
                // lecture asynchrone (l'événement est signalé).
                Ok(()) => {}
                Err(e) if is(&e, ERROR_IO_PENDING) => {}
                Err(e) if closed(&e) => return Ok(Some(0)),
                Err(e) => return Err(e.into()),
            }
            self.read_pending = true;
        }
        match unsafe { WaitForSingleObject(self.read_event.0, millis(timeout)) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => return Ok(None),
            _ => return Err(io::Error::last_os_error()),
        }
        let mut n = 0u32;
        let result = unsafe { GetOverlappedResult(self.pipe.0, &*self.read, &mut n, false) };
        self.read_pending = false;
        match result {
            Ok(()) if n == 0 => Ok(None),
            Ok(()) => {
                self.leftover.extend_from_slice(&self.read_buffer[..n as usize]);
                Ok(Some(self.take_leftover(buf)))
            }
            Err(e) if closed(&e) => Ok(Some(0)),
            Err(e) => Err(e.into()),
        }
    }

    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        let mut offset = 0;
        while offset < data.len() {
            *self.write = OVERLAPPED { hEvent: self.write_event.0, ..Default::default() };
            match unsafe { WriteFile(self.pipe.0, Some(&data[offset..]), None, Some(&mut *self.write)) } {
                Ok(()) => {}
                Err(e) if is(&e, ERROR_IO_PENDING) => {}
                Err(e) if closed(&e) => return Err(io::ErrorKind::BrokenPipe.into()),
                Err(e) => return Err(e.into()),
            }
            let mut n = 0u32;
            if unsafe { WaitForSingleObject(self.write_event.0, WRITE_TIMEOUT_MS) } != WAIT_OBJECT_0 {
                unsafe {
                    let _ = CancelIoEx(self.pipe.0, Some(&*self.write));
                    let _ = GetOverlappedResult(self.pipe.0, &*self.write, &mut n, true);
                }
                return Err(io::ErrorKind::TimedOut.into());
            }
            match unsafe { GetOverlappedResult(self.pipe.0, &*self.write, &mut n, false) } {
                Ok(()) => {}
                Err(e) if closed(&e) => return Err(io::ErrorKind::BrokenPipe.into()),
                Err(e) => return Err(e.into()),
            }
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            offset += n as usize;
        }
        Ok(())
    }
}

impl Drop for PipeConnection {
    fn drop(&mut self) {
        // Windows ne doit plus écrire dans le tampon une fois libéré.
        if self.read_pending {
            unsafe {
                let _ = CancelIoEx(self.pipe.0, Some(&*self.read));
                let mut n = 0;
                let _ = GetOverlappedResult(self.pipe.0, &*self.read, &mut n, true);
            }
        }
    }
}

/// Vrai si le tube appartient au compte `sid` : un tube créé par un autre
/// compte n'est pas celui de Polyptyque.
fn owned_by(pipe: HANDLE, sid: &str) -> bool {
    let mut owner = PSID::default();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let status = unsafe {
        GetSecurityInfo(
            pipe,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            Some(&mut descriptor),
        )
    };
    if status != ERROR_SUCCESS {
        log::warn!("pilotage : propriétaire du tube illisible ({})", status.0);
        return false;
    }
    let same = sid_text(owner).is_ok_and(|owner| owner.eq_ignore_ascii_case(sid));
    unsafe {
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
    }
    same
}

/// Connexion d'un client au Polyptyque ouvert par ce compte.
pub fn connect() -> Result<PipeConnection, ConnectError> {
    let sid = user_sid().map_err(ConnectError::Io)?;
    let path = HSTRING::from(format!(r"\\.\pipe\polyptyque-{sid}"));
    for _ in 0..5 {
        let opened = unsafe {
            CreateFileW(
                &path,
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                None,
            )
        };
        match opened {
            Ok(pipe) => {
                if !owned_by(pipe, &sid) {
                    unsafe {
                        let _ = CloseHandle(pipe);
                    }
                    return Err(ConnectError::NotOurs);
                }
                return PipeConnection::new(pipe).map_err(ConnectError::Io);
            }
            Err(e) if is(&e, ERROR_FILE_NOT_FOUND) => return Err(ConnectError::NotListening),
            // Toutes les instances occupées : on attend la suivante.
            Err(e) if is(&e, ERROR_PIPE_BUSY) => unsafe {
                let _ = WaitNamedPipeW(&path, 2_000);
            },
            Err(e) => return Err(ConnectError::Io(e.into())),
        }
    }
    Err(ConnectError::Io(io::ErrorKind::TimedOut.into()))
}
