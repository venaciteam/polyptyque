//! Pilotage par d'autres programmes, côté interface (voir « Pilotage par
//! d'autres programmes » dans le cahier des charges) : le canal de
//! commande, tenu tant que le réglage l'autorise, les programmes connectés
//! et l'état en direct. Les commandes elles-mêmes sont dans `commands.rs`.
//!
//! Un fil attend les connexions, un fil par connexion lit les requêtes et
//! écrit les réponses ; chaque requête est exécutée sur le fil de
//! l'interface, comme un clic.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use polyptyque_core::control::catalog;
use polyptyque_core::control::protocol::{self, CommandError, CommandResult, Incoming, LineBuffer, Reason};
use polyptyque_core::control::transport::{Connection, Listener, Peer};
use polyptyque_engine::SourceStatus;
use serde_json::{json, Value};
use slint::{Timer, TimerMode};
use tr::tr;

use super::{post, with_app, App};
use crate::preview::PreviewState;

/// Messages affichés gardés pour les réponses (ceux affichés pendant une
/// commande y sont joints).
const KEPT_MESSAGES: usize = 50;
/// Rythme de vérification des commandes longues.
const POLL: Duration = Duration::from_millis(100);
/// Attente maximale d'une lecture par le fil d'une connexion : c'est aussi
/// le plus long délai avant l'envoi d'une réponse.
const READ_SLICE: Duration = Duration::from_millis(40);

/// Vérifie si une commande longue est terminée : son résultat, ou `None`.
pub(super) type Check = Box<dyn Fn(&App) -> Option<CommandResult>>;

/// Réponse d'une commande.
pub(super) enum Reply {
    Now(CommandResult),
    /// Résultat à attendre (démarrage, arrêt…) : `check` est rappelé
    /// toutes les 100 ms, jusqu'à ce qu'il rende un résultat ou que `limit`
    /// soit dépassé.
    Wait {
        check: Check,
        limit: Duration,
    },
}

impl Reply {
    pub(super) fn ok(value: Value) -> Self {
        Reply::Now(Ok(value))
    }

    pub(super) fn err(error: CommandError) -> Self {
        Reply::Now(Err(error))
    }
}

/// Programme connecté.
struct Client {
    /// Nom annoncé par `hello`.
    name: Option<String>,
    peer: Peer,
    outgoing: Sender<String>,
    subscribed: bool,
}

/// Commande longue en attente de son résultat.
struct Pending {
    client: u64,
    id: Option<Value>,
    method: String,
    check: Check,
    deadline: Instant,
    since: u64,
}

struct Server {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Démarrage demandé par un programme.
pub(super) struct StartRequest {
    /// Nom du programme, cité dans le message de démarrage.
    pub client: String,
    /// Pendant un créneau programmé : arrêt automatique choisi par la
    /// commande, à la place de la question.
    pub auto_stop: bool,
}

#[derive(Default)]
pub(super) struct ControlState {
    server: Option<Server>,
    clients: BTreeMap<u64, Client>,
    pending: Vec<Pending>,
    poll_timer: Option<Timer>,
    /// Dernier état envoyé aux abonnés.
    last_state: String,
    messages: VecDeque<(u64, i32, String)>,
    message_seq: u64,
    pub start_request: Option<StartRequest>,
    /// Fenêtres introuvables au dernier démarrage.
    pub last_missing: Vec<String>,
    /// Bilan du dernier arrêt (fichiers, taille, dossier).
    pub last_stop: Option<Value>,
    /// Bilan de la dernière ouverture des fenêtres de la scène.
    pub last_open: Option<Value>,
    /// État des sources de l'enregistrement en cours, au dernier relevé.
    pub statuses: Vec<SourceStatus>,
    /// Fenêtre de chaque source d'après son aperçu.
    pub previews: HashMap<String, PreviewState>,
    /// Résultat du dernier essai des encodeurs (`None` : en cours ou
    /// jamais fait).
    pub encoder_test: Option<Value>,
    pub encoder_test_running: bool,
    /// Réponse à la confirmation du micro demandée par un programme.
    pub mic_answer: Option<bool>,
}

impl ControlState {
    /// Numéro du dernier message affiché.
    pub fn message_seq(&self) -> u64 {
        self.message_seq
    }

    pub fn client_count(&self) -> usize {
        self.clients.len()
    }
}

#[cfg(windows)]
fn bind() -> io::Result<Box<dyn Listener>> {
    Ok(Box::new(polyptyque_windows::control_pipe::PipeListener::bind()?))
}

#[cfg(unix)]
fn bind() -> io::Result<Box<dyn Listener>> {
    Ok(Box::new(polyptyque_core::control::unix::UnixChannelListener::bind()?))
}

/// Attend les connexions jusqu'à l'arrêt du canal.
fn listen(mut listener: Box<dyn Listener>, stop: Arc<AtomicBool>) {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let mut connections: Vec<JoinHandle<()>> = Vec::new();
    while !stop.load(Ordering::SeqCst) {
        match listener.accept(Duration::from_millis(200)) {
            Ok(Some((connection, peer))) => {
                let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                let stop = stop.clone();
                connections.retain(|c| !c.is_finished());
                connections.push(thread::spawn(move || serve(id, connection, peer, stop)));
            }
            Ok(None) => {}
            Err(e) => {
                log::warn!("pilotage : attente des connexions : {e}");
                thread::sleep(Duration::from_millis(500));
            }
        }
    }
    for connection in connections {
        let _ = connection.join();
    }
}

/// Une connexion : requêtes transmises au fil de l'interface, réponses et
/// notifications écrites dès qu'elles arrivent.
fn serve(id: u64, mut connection: Box<dyn Connection>, peer: Peer, stop: Arc<AtomicBool>) {
    let (outgoing, incoming): (Sender<String>, Receiver<String>) = mpsc::channel();
    post(move |app| app.control_connected(id, peer, outgoing));
    let mut lines = LineBuffer::default();
    let mut buf = vec![0u8; 16 * 1024];
    'connection: while !stop.load(Ordering::SeqCst) {
        loop {
            match incoming.try_recv() {
                Ok(mut line) => {
                    line.push('\n');
                    if connection.write_all(line.as_bytes()).is_err() {
                        break 'connection;
                    }
                }
                Err(TryRecvError::Empty) => break,
                // Le programme a été déconnecté par Polyptyque.
                Err(TryRecvError::Disconnected) => break 'connection,
            }
        }
        match connection.read(&mut buf, READ_SLICE) {
            Ok(None) => continue,
            Ok(Some(0)) | Err(_) => break,
            Ok(Some(n)) => lines.push(&buf[..n]),
        }
        while let Some(line) = lines.next_line() {
            match line {
                Ok(line) => post(move |app| app.control_request(id, line)),
                Err(error) => {
                    let mut line = protocol::failure(None, &error);
                    line.push('\n');
                    let _ = connection.write_all(line.as_bytes());
                    break 'connection;
                }
            }
        }
    }
    post(move |app| app.control_disconnected(id));
}

impl App {
    // ----- Canal --------------------------------------------------------------

    /// Ouvre ou ferme le canal selon le réglage (au lancement, et quand le
    /// réglage change).
    pub(super) fn apply_remote_control(&self) {
        let wanted = self.config.borrow().settings.remote_control;
        let running = self.control.borrow().server.is_some();
        if wanted == running {
            return;
        }
        if !wanted {
            self.stop_control();
            log::info!("pilotage désactivé : canal fermé");
            return;
        }
        match bind() {
            Ok(listener) => {
                let stop = Arc::new(AtomicBool::new(false));
                let thread = {
                    let stop = stop.clone();
                    thread::spawn(move || listen(listener, stop))
                };
                let timer = Timer::default();
                timer.start(TimerMode::Repeated, POLL, || {
                    with_app(|app| app.control_poll());
                });
                let mut control = self.control.borrow_mut();
                control.server = Some(Server { stop, thread: Some(thread) });
                control.poll_timer = Some(timer);
                log::info!("pilotage activé : canal ouvert");
            }
            Err(e) => {
                log::error!("pilotage : canal impossible à ouvrir : {e}");
                self.toast(
                    &tr!("Pilotage indisponible : le canal de commande n'a pas pu s'ouvrir (détail dans le journal)."),
                    3,
                );
            }
        }
    }

    /// Ferme le canal et déconnecte les programmes (pilotage désactivé, ou
    /// fin de Polyptyque).
    pub(super) fn stop_control(&self) {
        let (server, pending) = {
            let mut control = self.control.borrow_mut();
            control.clients.clear();
            control.poll_timer = None;
            (control.server.take(), std::mem::take(&mut control.pending))
        };
        drop(pending);
        // Attend la fin des fils (au plus quelques dixièmes de seconde).
        drop(server);
        self.refresh_control_pill();
    }

    fn control_connected(&self, id: u64, peer: Peer, outgoing: Sender<String>) {
        if self.control.borrow().server.is_none() {
            // Canal fermé entre-temps : `outgoing` lâché, le fil s'arrête.
            return;
        }
        log::info!(
            "pilotage : programme connecté ({}, processus {})",
            peer.program.as_deref().unwrap_or("inconnu"),
            peer.pid.map(|p| p.to_string()).unwrap_or_else(|| "?".into())
        );
        self.control.borrow_mut().clients.insert(id, Client { name: None, peer, outgoing, subscribed: false });
        self.refresh_control_pill();
    }

    fn control_disconnected(&self, id: u64) {
        let removed = {
            let mut control = self.control.borrow_mut();
            control.pending.retain(|p| p.client != id);
            control.clients.remove(&id)
        };
        if let Some(client) = removed {
            log::info!("pilotage : programme déconnecté ({})", label(&client));
            self.refresh_control_pill();
        }
    }

    /// Nom du programme connecté, pour les messages et le journal.
    pub(super) fn control_client_name(&self, id: u64) -> String {
        self.control.borrow().clients.get(&id).map(label).unwrap_or_else(|| tr!("un programme"))
    }

    pub(super) fn set_control_client_name(&self, id: u64, name: &str) {
        let name: String = name.trim().chars().filter(|c| !c.is_control()).take(40).collect();
        if let Some(client) = self.control.borrow_mut().clients.get_mut(&id) {
            client.name = Some(name).filter(|n| !n.is_empty());
        }
        self.refresh_control_pill();
    }

    pub(super) fn set_control_subscribed(&self, id: u64, subscribed: bool) {
        if let Some(client) = self.control.borrow_mut().clients.get_mut(&id) {
            client.subscribed = subscribed;
        }
        if subscribed {
            // Le prochain état part, même inchangé.
            self.control.borrow_mut().last_state.clear();
        }
    }

    /// Mode d'emploi du pilotage par un assistant IA : section du README,
    /// dans la langue de l'interface.
    pub(super) fn open_control_help(&self) {
        let page = match crate::i18n::Language::current() {
            crate::i18n::Language::French => "README.fr.md#piloter-polyptyque-avec-un-assistant-ia",
            crate::i18n::Language::English => "README.md#control-polyptyque-with-an-ai-assistant",
        };
        let url = format!("{}/blob/main/{page}", crate::update::REPOSITORY);
        if let Err(e) = self.platform.shell_open(&url) {
            log::warn!("ouverture de {url} : {e:#}");
            self.toast(&tr!("Impossible d'ouvrir la page : {}", url), 3);
        }
    }

    /// Pastille « Pilotage » : les programmes connectés.
    fn refresh_control_pill(&self) {
        let names: Vec<String> = {
            let control = self.control.borrow();
            let mut names: Vec<String> = control.clients.values().map(label).collect();
            names.sort();
            names.dedup();
            names
        };
        let text = match names.as_slice() {
            [] => String::new(),
            names => tr!("Pilotage · {}", names.join(", ")),
        };
        self.with_bridge(|b| b.set_control_text(text.into()));
    }

    // ----- Requêtes -------------------------------------------------------------

    fn control_request(&self, client: u64, line: String) {
        let request = match protocol::parse_request(&line) {
            Incoming::Request(request) => request,
            Incoming::Invalid { id, error } => {
                self.control_send(client, protocol::failure(id.as_ref(), &error));
                return;
            }
        };
        let since = self.control.borrow().message_seq;
        let Some(command) = catalog::commands().into_iter().find(|c| c.method == request.method) else {
            let error = CommandError::invalid(tr!(
                "Commande inconnue : « {} ». La liste est donnée par « polyptyque-cli commands ».",
                request.method
            ));
            self.control_answer(client, request.id, &request.method, Err(error), since);
            return;
        };
        if let Err(error) = command.check_params(&request.params) {
            self.control_answer(client, request.id, &request.method, Err(error), since);
            return;
        }
        if !matches!(request.method.as_str(), "status" | "hello") {
            log::info!("pilotage : {} ({})", request.method, self.control_client_name(client));
        }
        match self.run_command(client, &request.method, &request.params) {
            Reply::Now(result) => self.control_answer(client, request.id, &request.method, result, since),
            Reply::Wait { check, limit } => {
                // Déjà terminé (refus immédiat) : réponse tout de suite.
                if let Some(result) = check(self) {
                    self.control_answer(client, request.id, &request.method, result, since);
                    return;
                }
                self.control.borrow_mut().pending.push(Pending {
                    client,
                    id: request.id,
                    method: request.method,
                    check,
                    deadline: Instant::now() + limit,
                    since,
                });
            }
        }
    }

    /// Toutes les 100 ms : commandes longues terminées ?
    fn control_poll(&self) {
        let pending = std::mem::take(&mut self.control.borrow_mut().pending);
        let mut still = Vec::new();
        for task in pending {
            let result = (task.check)(self).or_else(|| {
                (Instant::now() >= task.deadline).then(|| {
                    Err(CommandError::new(
                        Reason::Timeout,
                        tr!(
                            "« {} » n'a pas abouti dans le délai prévu (voir l'état et les messages de Polyptyque).",
                            task.method
                        ),
                    ))
                })
            });
            match result {
                Some(result) => self.control_answer(task.client, task.id, &task.method, result, task.since),
                None => still.push(task),
            }
        }
        self.control.borrow_mut().pending.extend(still);
    }

    /// Envoie la réponse ; un objet réussi reçoit les messages affichés par
    /// Polyptyque pendant la commande.
    fn control_answer(&self, client: u64, id: Option<Value>, method: &str, result: CommandResult, since: u64) {
        if let Err(error) = &result {
            log::info!("pilotage : {method} refusé ({}) : {}", error.reason.as_str(), error.message);
        }
        let Some(id) = id else {
            return;
        };
        let line = match result {
            Ok(mut value) => {
                let messages = self.control_messages_since(since);
                if let (Value::Object(object), false) = (&mut value, messages.is_empty()) {
                    object.insert("messages".into(), Value::Array(messages));
                }
                protocol::success(&id, value)
            }
            Err(error) => protocol::failure(Some(&id), &error),
        };
        self.control_send(client, line);
    }

    fn control_send(&self, client: u64, line: String) {
        if let Some(client) = self.control.borrow().clients.get(&client) {
            let _ = client.outgoing.send(line);
        }
    }

    // ----- Messages et état en direct -----------------------------------------------

    /// Un message vient d'être affiché : gardé pour les réponses, et envoyé
    /// aux abonnés.
    pub(super) fn control_message(&self, text: &str, tone: i32) {
        let mut control = self.control.borrow_mut();
        if control.server.is_none() {
            return;
        }
        control.message_seq += 1;
        let seq = control.message_seq;
        control.messages.push_back((seq, tone, text.to_string()));
        while control.messages.len() > KEPT_MESSAGES {
            control.messages.pop_front();
        }
        let line = protocol::notification("message", message_json(tone, text));
        for client in control.clients.values().filter(|c| c.subscribed) {
            let _ = client.outgoing.send(line.clone());
        }
    }

    /// Dernier message d'erreur ou d'avertissement affiché depuis `since` :
    /// la raison d'un échec, telle que la personne l'a vue.
    pub(super) fn control_problem_since(&self, since: u64) -> Option<String> {
        let control = self.control.borrow();
        control.messages.iter().rev().find(|(seq, tone, _)| *seq > since && *tone >= 2).map(|(_, _, text)| text.clone())
    }

    fn control_messages_since(&self, since: u64) -> Vec<Value> {
        let control = self.control.borrow();
        control
            .messages
            .iter()
            .filter(|(seq, _, _)| *seq > since)
            .map(|(_, tone, text)| message_json(*tone, text))
            .collect()
    }

    /// Deux fois par seconde : état envoyé aux abonnés s'il a changé.
    pub(super) fn control_tick(&self) {
        let subscribed = self.control.borrow().clients.values().any(|c| c.subscribed);
        if !subscribed {
            return;
        }
        let state = self.control_state();
        let text = state.to_string();
        let mut control = self.control.borrow_mut();
        if control.last_state == text {
            return;
        }
        control.last_state = text;
        let line = protocol::notification("state", state);
        for client in control.clients.values().filter(|c| c.subscribed) {
            let _ = client.outgoing.send(line.clone());
        }
    }
}

fn label(client: &Client) -> String {
    client.name.clone().or_else(|| client.peer.program.clone()).unwrap_or_else(|| tr!("un programme"))
}

/// Niveau d'un message, d'après sa couleur à l'écran.
fn message_json(tone: i32, text: &str) -> Value {
    let level = match tone {
        3 => "error",
        2 => "warning",
        _ => "info",
    };
    json!({ "level": level, "text": text })
}
