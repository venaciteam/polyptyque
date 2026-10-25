//! Canal de commande hors Windows (mode démonstration, futur portage) :
//! socket Unix dans un dossier réservé au compte (droits 0700).

use std::fs::{self, DirBuilder};
use std::io::{self, ErrorKind, Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use super::transport::{ConnectError, Connection, Listener, Peer};

/// Délai d'écriture : au-delà, l'autre côté ne lit plus.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Dossier de la socket : celui des fichiers d'exécution du compte s'il
/// existe (Linux), sinon le dossier temporaire (propre au compte sous
/// macOS).
fn socket_dir() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let user = std::env::var("USER").or_else(|_| std::env::var("LOGNAME")).unwrap_or_default();
    let user: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
    base.join(if user.is_empty() { "polyptyque".to_string() } else { format!("polyptyque-{user}") })
}

/// Emplacement de la socket, à donner aux clients qui ne passent pas par
/// `polyptyque-cli`.
pub fn socket_path() -> PathBuf {
    socket_dir().join("control.sock")
}

/// Crée le dossier de la socket, réservé au compte ; refuse un dossier
/// existant ouvert aux autres comptes.
fn private_dir() -> io::Result<PathBuf> {
    let dir = socket_dir();
    match DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let mode = fs::metadata(&dir)?.permissions().mode();
    if mode & 0o077 != 0 {
        return Err(io::Error::new(
            ErrorKind::PermissionDenied,
            format!("{} est accessible à d'autres comptes", dir.display()),
        ));
    }
    Ok(dir)
}

pub struct UnixChannelListener {
    listener: UnixListener,
    path: PathBuf,
}

impl UnixChannelListener {
    pub fn bind() -> io::Result<Self> {
        let path = private_dir()?.join("control.sock");
        let listener = match UnixListener::bind(&path) {
            Ok(listener) => listener,
            Err(e) if e.kind() == ErrorKind::AddrInUse => {
                // Socket laissée par un Polyptyque arrêté net : personne ne
                // répond, elle est remplacée.
                if UnixStream::connect(&path).is_ok() {
                    return Err(io::Error::new(ErrorKind::AddrInUse, "un autre Polyptyque tient déjà le canal"));
                }
                fs::remove_file(&path)?;
                UnixListener::bind(&path)?
            }
            Err(e) => return Err(e),
        };
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        Ok(Self { listener, path })
    }
}

impl Listener for UnixChannelListener {
    fn accept(&mut self, timeout: Duration) -> io::Result<Option<(Box<dyn Connection>, Peer)>> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false)?;
                    return Ok(Some((Box::new(UnixConnection(stream)), Peer::default())));
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(e),
            }
        }
    }
}

impl Drop for UnixChannelListener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub struct UnixConnection(UnixStream);

impl Connection for UnixConnection {
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<Option<usize>> {
        // Un délai nul voudrait dire « jamais » pour le système.
        self.0.set_read_timeout(Some(timeout.max(Duration::from_millis(1))))?;
        match self.0.read(buf) {
            Ok(n) => Ok(Some(n)),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        self.0.set_write_timeout(Some(WRITE_TIMEOUT))?;
        self.0.write_all(data)?;
        self.0.flush()
    }
}

/// Connexion d'un client.
pub fn connect() -> Result<UnixConnection, ConnectError> {
    match UnixStream::connect(socket_path()) {
        Ok(stream) => Ok(UnixConnection(stream)),
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
            Err(ConnectError::NotListening)
        }
        Err(e) => Err(ConnectError::Io(e)),
    }
}
