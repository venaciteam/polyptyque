//! Canal de commande, indépendamment du système : ce que doivent fournir le
//! tube nommé de Windows et la socket Unix, et le client qui s'en sert
//! (mode commande, serveur MCP).

use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

use serde_json::Value;
use tr::tr;

use super::protocol::{self, ClientMessage, CommandError, CommandResult, LineBuffer, Reason};

/// Une connexion établie, dans un sens comme dans l'autre.
pub trait Connection: Send {
    /// Lit ce qui est arrivé, en attendant au plus `timeout`. `Ok(None)` :
    /// rien dans ce délai ; `Ok(Some(0))` : connexion fermée par l'autre
    /// côté.
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<Option<usize>>;

    /// Écrit tout, ou échoue (autre côté fermé, ou qui ne lit plus).
    fn write_all(&mut self, data: &[u8]) -> io::Result<()>;
}

/// Programme à l'autre bout d'une connexion, quand le système le dit.
#[derive(Debug, Clone, Default)]
pub struct Peer {
    pub pid: Option<u32>,
    /// Nom de son exécutable (« claude.exe »).
    pub program: Option<String>,
}

/// Côté Polyptyque : attend les connexions.
pub trait Listener: Send {
    /// Connexion suivante, en attendant au plus `timeout`.
    fn accept(&mut self, timeout: Duration) -> io::Result<Option<(Box<dyn Connection>, Peer)>>;
}

/// Échec de la connexion d'un client à Polyptyque.
#[derive(Debug)]
pub enum ConnectError {
    /// Personne n'écoute : Polyptyque fermé, ou pilotage désactivé.
    NotListening,
    /// Le canal existe mais n'appartient pas à ce compte : il n'est pas
    /// tenu par Polyptyque.
    NotOurs,
    Io(io::Error),
}

/// Client du canal : envoie des commandes et reçoit leurs réponses ; les
/// notifications reçues entre-temps sont gardées.
pub struct Client {
    connection: Box<dyn Connection>,
    lines: LineBuffer,
    next_id: u64,
    notifications: VecDeque<(String, Value)>,
}

impl Client {
    pub fn new(connection: Box<dyn Connection>) -> Self {
        Self { connection, lines: LineBuffer::default(), next_id: 1, notifications: VecDeque::new() }
    }

    /// Envoie une commande et attend sa réponse au plus `timeout`.
    pub fn call(&mut self, method: &str, params: Value, timeout: Duration) -> CommandResult {
        let id = self.next_id;
        self.next_id += 1;
        let mut line = protocol::request(id, method, params);
        line.push('\n');
        // Pas envoyée : la renvoyer sur une nouvelle connexion est sans
        // risque (`sent: false`).
        self.connection.write_all(line.as_bytes()).map_err(|e| lost(e).with("sent", false))?;
        let deadline = Instant::now() + timeout;
        loop {
            match self.read_message(deadline)? {
                Some(ClientMessage::Response { id: got, result }) if got == id => return result,
                // Réponse à une requête abandonnée (délai dépassé) : ignorée.
                Some(ClientMessage::Response { .. }) => {}
                Some(ClientMessage::Notification { method, params }) => self.notifications.push_back((method, params)),
                None => {
                    return Err(CommandError::new(
                        Reason::Timeout,
                        tr!("Polyptyque n'a pas répondu à temps à « {} ».", method),
                    ))
                }
            }
        }
    }

    /// Notification suivante (état, message), en attendant au plus
    /// `timeout`.
    pub fn next_notification(&mut self, timeout: Duration) -> Result<Option<(String, Value)>, CommandError> {
        if let Some(notification) = self.notifications.pop_front() {
            return Ok(Some(notification));
        }
        let deadline = Instant::now() + timeout;
        loop {
            match self.read_message(deadline)? {
                Some(ClientMessage::Notification { method, params }) => return Ok(Some((method, params))),
                Some(ClientMessage::Response { .. }) => {}
                None => return Ok(None),
            }
        }
    }

    /// Message suivant, `None` une fois `deadline` passée.
    fn read_message(&mut self, deadline: Instant) -> Result<Option<ClientMessage>, CommandError> {
        let mut buf = [0u8; 16 * 1024];
        loop {
            if let Some(line) = self.lines.next_line() {
                let line = line?;
                return protocol::parse_client_message(&line).map(Some);
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            match self.connection.read(&mut buf, (deadline - now).min(Duration::from_millis(250))) {
                Ok(Some(0)) => return Err(lost(io::ErrorKind::UnexpectedEof.into())),
                Ok(Some(n)) => self.lines.push(&buf[..n]),
                Ok(None) => {}
                Err(e) => return Err(lost(e)),
            }
        }
    }
}

/// Connexion perdue en cours d'échange (Polyptyque fermé entre-temps,
/// pilotage désactivé).
fn lost(error: io::Error) -> CommandError {
    log::warn!("pilotage : connexion perdue : {error}");
    CommandError::new(
        Reason::NotRunning,
        tr!("La connexion avec Polyptyque a été coupée (fermé, ou pilotage désactivé)."),
    )
}
