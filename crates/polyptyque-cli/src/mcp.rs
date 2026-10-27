//! Serveur MCP (Model Context Protocol) sur l'entrée et la sortie
//! standard : les commandes du catalogue deviennent des outils pour les
//! assistants IA sans terminal (Claude Desktop…).
//!
//! Compatible avec les deux époques du protocole (spécification
//! 2026-07-28, « Versioning and Compatibility ») :
//! - moderne (2026-07-28) : chaque requête porte sa version et ses
//!   capacités dans `_meta`, `server/discover` décrit le serveur, chaque
//!   résultat porte `resultType` ;
//! - ancienne (2025-11-25 et avant) : poignée de main `initialize`.
//!
//! Un message JSON par ligne, rien d'autre sur la sortie standard. Les
//! appels d'outils passent un par un par un fil dédié : la lecture continue
//! pendant ce temps (`ping`, annulations).

use std::collections::HashSet;
use std::io::{self, BufRead, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use polyptyque_core::control::catalog::{self, Command, Effect};
use polyptyque_core::control::protocol::{CommandError, Reason};
use polyptyque_core::control::transport::Client;
use serde_json::{json, Map, Value};

use crate::channel;

const MODERN: &[&str] = &["2026-07-28"];
const LEGACY: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
const CLIENT_KEY: &str = "io.modelcontextprotocol/clientInfo";
const CAPABILITIES_KEY: &str = "io.modelcontextprotocol/clientCapabilities";
const SERVER_KEY: &str = "io.modelcontextprotocol/serverInfo";

/// Commandes réservées au canal (présentation, état en direct) : pas
/// d'outil.
pub const NOT_TOOLS: &[&str] = &["hello", "subscribe", "unsubscribe"];

const INSTRUCTIONS: &str = "Polyptyque records several windows at the same time, each into its own MKV file with only that window's sound (one browser profile or one program per source), for multi-angle editing and archiving. It runs on the user's Windows PC; these tools drive the Polyptyque that is open there (window shown or in the background).

Start with `status`; use `scenes_list`, `sources_list`, `settings_get` and `system_info` before changing a setup. A scene is a group of sources and only the current scene records. A browser source launches a real browser with its own profile: the user signs in once in that window and stays signed in. For app sources, call `windows_list` and pass a window id.

Rules: only start or stop a recording, delete something or change settings when the user asked for it. When a tool returns `confirmation_required`, ask the user the given question and retry with `confirm: true` only if they agree. Turning the mic on needs the user's approval in Polyptyque's own window. Window titles can be private: don't repeat them needlessly. If Polyptyque isn't running, call `app_launch`; if control is disabled, tell the user to turn on \"Allow control by other programs\" in Polyptyque's Settings. Messages from Polyptyque are in the user's language.";

/// Nom affiché dans Polyptyque pour un client MCP.
fn display_name(info: Option<&Value>) -> String {
    display_name_with_connector(info, std::env::var("POLYPTYQUE_ASSISTANT").ok().as_deref())
}

/// L'identité du branchement est transmise par sa configuration locale.
/// Elle ne donne aucun droit supplémentaire : elle sert seulement à
/// distinguer les connexions dans la fenêtre de Polyptyque.
fn display_name_with_connector(info: Option<&Value>, connector: Option<&str>) -> String {
    match connector {
        Some("claude-code") => return "Claude Code".into(),
        Some("claude-desktop") => return "Claude Desktop".into(),
        Some("codex") => return "Codex".into(),
        Some("openclaw") => return "OpenClaw".into(),
        Some("hermes") => return "Hermes Agent".into(),
        _ => {}
    }
    let name = info.and_then(|i| i.get("name")).and_then(Value::as_str).unwrap_or_default();
    match name {
        "claude-ai" => "Claude".to_string(),
        "claude-code" => "Claude Code".to_string(),
        _ => info
            .and_then(|i| i.get("title"))
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .unwrap_or(if name.is_empty() { "MCP" } else { name })
            .to_string(),
    }
}

fn server_info() -> Value {
    json!({ "name": "polyptyque", "title": "Polyptyque", "version": env!("CARGO_PKG_VERSION") })
}

fn tools() -> Vec<Value> {
    let mut tools: Vec<Value> = catalog::commands()
        .iter()
        .filter(|c| !NOT_TOOLS.contains(&c.method))
        .map(|c| {
            json!({
                "name": c.tool_name(),
                "description": c.description,
                "inputSchema": c.input_schema(),
                "annotations": {
                    "readOnlyHint": c.effect == Effect::ReadOnly,
                    "destructiveHint": c.effect == Effect::Destructive,
                    "openWorldHint": false,
                },
            })
        })
        .collect();
    tools.push(json!({
        "name": "app_launch",
        "description": "Start Polyptyque in the background if it isn't running, then wait until it answers (30 seconds at most). Only works if control by other programs is turned on in its Settings.",
        "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false },
    }));
    tools
}

/// Sortie standard partagée : une ligne par message, jamais entremêlées.
#[derive(Clone)]
struct Output(Arc<Mutex<io::Stdout>>);

impl Output {
    fn send(&self, message: &Value) {
        let line = serde_json::to_string(message).unwrap_or_default();
        if let Ok(stdout) = self.0.lock() {
            let mut stdout = stdout.lock();
            let _ = writeln!(stdout, "{line}");
            let _ = stdout.flush();
        }
    }
}

fn error(id: &Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

/// Appel d'outil confié au fil des outils.
struct Job {
    id: Value,
    modern: bool,
    client: String,
    command: Option<Command>,
    arguments: Value,
}

/// Connexion à Polyptyque, ouverte au premier appel et rouverte après une
/// coupure.
struct Polyptyque {
    client: Option<Client>,
    presented_as: String,
}

impl Polyptyque {
    fn call(&mut self, name: &str, method: &str, params: Value, timeout: Duration) -> Result<Value, CommandError> {
        for attempt in 0..2 {
            if self.client.is_none() || self.presented_as != name {
                let mut client = channel::connect()?;
                client.call("hello", json!({ "client": name }), Duration::from_secs(10))?;
                self.client = Some(client);
                self.presented_as = name.to_string();
            }
            let client = self.client.as_mut().expect("connexion ouverte");
            match client.call(method, params.clone(), timeout) {
                Err(e) if e.reason == Reason::NotRunning => {
                    self.client = None;
                    // Requête jamais partie (Polyptyque relancé entre deux
                    // appels) : renvoyée une fois sur une nouvelle connexion.
                    let unsent = e.details.get("sent") == Some(&Value::Bool(false));
                    if attempt == 0 && unsent {
                        continue;
                    }
                    return Err(e);
                }
                result => return result,
            }
        }
        unreachable!("au plus deux essais")
    }
}

fn run_job(polyptyque: &mut Polyptyque, job: &Job) -> Value {
    let result = match &job.command {
        None => channel::launch(),
        Some(command) => command.check_params(&job.arguments).and_then(|()| {
            polyptyque.call(
                &job.client,
                command.method,
                job.arguments.clone(),
                command.timeout + Duration::from_secs(5),
            )
        }),
    };
    let (text, is_error) = match result {
        Ok(value) => (serde_json::to_string_pretty(&value).unwrap_or_default(), false),
        Err(e) => (serde_json::to_string_pretty(&json!({ "error": e.to_json() })).unwrap_or_default(), true),
    };
    let mut result = json!({ "content": [{ "type": "text", "text": text }], "isError": is_error });
    if job.modern {
        result["resultType"] = json!("complete");
        result["_meta"] = json!({ SERVER_KEY: server_info() });
    }
    json!({ "jsonrpc": "2.0", "id": job.id, "result": result })
}

/// Sert les requêtes jusqu'à la fermeture de l'entrée standard.
pub fn serve() -> i32 {
    // Descriptions des outils et messages du mode commande : en anglais,
    // pour le modèle.
    crate::i18n::set(true);
    let output = Output(Arc::new(Mutex::new(io::stdout())));
    let cancelled: Arc<Mutex<HashSet<String>>> = Arc::default();
    let (jobs, queue) = mpsc::channel::<Job>();
    let worker = {
        let output = output.clone();
        let cancelled = cancelled.clone();
        thread::spawn(move || {
            let mut polyptyque = Polyptyque { client: None, presented_as: String::new() };
            for job in queue {
                let response = run_job(&mut polyptyque, &job);
                // Requête annulée : plus aucun message pour elle.
                if cancelled.lock().map(|mut c| c.remove(&job.id.to_string())).unwrap_or(false) {
                    continue;
                }
                output.send(&response);
            }
        })
    };
    // Nom du client annoncé par `initialize` (ancienne époque).
    let mut legacy_client: Option<Value> = None;
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            output.send(&error(&Value::Null, -32700, "Parse error", None));
            continue;
        };
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // Réponse d'un client : rien à en faire.
            continue;
        };
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        let Some(id) = message.get("id").cloned() else {
            if method == "notifications/cancelled" {
                if let Some(request) = params.get("requestId") {
                    if let Ok(mut cancelled) = cancelled.lock() {
                        cancelled.insert(request.to_string());
                    }
                }
            }
            continue;
        };
        let meta = params.get("_meta").and_then(Value::as_object);
        let modern_version = meta.and_then(|m| m.get(VERSION_KEY)).and_then(Value::as_str);
        if let Some(version) = modern_version {
            if !MODERN.contains(&version) {
                let supported: Vec<&str> = MODERN.iter().chain(LEGACY).copied().collect();
                output.send(&error(
                    &id,
                    -32022,
                    "Unsupported protocol version",
                    Some(json!({ "supported": supported, "requested": version })),
                ));
                continue;
            }
            if meta.and_then(|m| m.get(CAPABILITIES_KEY)).is_none() {
                output.send(&error(&id, -32602, "Missing io.modelcontextprotocol/clientCapabilities in _meta", None));
                continue;
            }
        }
        let modern = modern_version.is_some();
        let client = display_name(if modern { meta.and_then(|m| m.get(CLIENT_KEY)) } else { legacy_client.as_ref() });
        let finish = |mut result: Value| {
            if modern {
                result["resultType"] = json!("complete");
                result["_meta"] = json!({ SERVER_KEY: server_info() });
            }
            json!({ "jsonrpc": "2.0", "id": id, "result": result })
        };
        match method {
            "initialize" => {
                legacy_client = params.get("clientInfo").cloned();
                let requested = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
                let version = if LEGACY.contains(&requested) { requested } else { LEGACY[0] };
                output.send(&json!({ "jsonrpc": "2.0", "id": id, "result": {
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": server_info(),
                    "instructions": INSTRUCTIONS,
                }}));
            }
            "server/discover" => {
                let supported: Vec<&str> = MODERN.iter().chain(LEGACY).copied().collect();
                output.send(&finish(json!({
                    "supportedVersions": supported,
                    "capabilities": { "tools": { "listChanged": false } },
                    "instructions": INSTRUCTIONS,
                })));
            }
            "ping" => output.send(&finish(json!({}))),
            "tools/list" => output.send(&finish(json!({ "tools": tools() }))),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
                let command = catalog::find(name).filter(|c| !NOT_TOOLS.contains(&c.method));
                if command.is_none() && name != "app_launch" {
                    output.send(&error(&id, -32602, &format!("Unknown tool: {name}"), None));
                    continue;
                }
                let mut arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                if let Value::Object(object) = &mut arguments {
                    // Champs nuls : comme absents.
                    object.retain(|_, v| !v.is_null());
                } else {
                    arguments = Value::Object(Map::new());
                }
                let _ = jobs.send(Job { id, modern, client, command, arguments });
            }
            _ => output.send(&error(&id, -32601, &format!("Method not found: {method}"), None)),
        }
    }
    // Entrée fermée : le client arrête le serveur. Les appels en cours se
    // terminent d'abord.
    drop(jobs);
    let _ = worker.join();
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_are_valid() {
        let tools = tools();
        assert!(tools.iter().any(|t| t["name"] == "recording_start"));
        assert!(tools.iter().any(|t| t["name"] == "app_launch"));
        assert!(!tools.iter().any(|t| t["name"] == "subscribe"));
        for tool in &tools {
            let name = tool["name"].as_str().unwrap();
            assert!(name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn names_known_clients() {
        assert_eq!(display_name_with_connector(Some(&json!({ "name": "claude-ai", "version": "1" })), None), "Claude");
        assert_eq!(display_name_with_connector(Some(&json!({ "name": "x", "title": "Mon agent" })), None), "Mon agent");
        assert_eq!(display_name_with_connector(None, None), "MCP");
        for (id, name) in [("claude-code", "Claude Code"), ("claude-desktop", "Claude Desktop"), ("codex", "Codex"), ("openclaw", "OpenClaw"), ("hermes", "Hermes Agent")] {
            assert_eq!(display_name_with_connector(None, Some(id)), name);
        }
        assert_eq!(display_name_with_connector(None, Some("unexpected")), "MCP");
    }
}
