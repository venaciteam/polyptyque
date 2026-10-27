//! Messages échangés sur le canal de commande : JSON-RPC 2.0, un message
//! JSON par ligne (UTF-8, sans retour à la ligne à l'intérieur).
//!
//! Chaque erreur porte une raison stable ([`Reason`]), lue par les
//! programmes, et un message dans la langue de Polyptyque, lu par les
//! personnes (et par les assistants, qui le leur transmettent).

use std::fmt;

use serde_json::{json, Map, Value};
use tr::tr;

/// Version du protocole, annoncée par `hello`. Elle ne change que si une
/// commande existante change de sens : en ajouter n'en change pas.
pub const PROTOCOL_VERSION: u32 = 1;

/// Plus long message accepté, en octets.
pub const MAX_LINE: usize = 1 << 20;

/// Raison d'une erreur, stable d'une version à l'autre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reason {
    /// Erreur interne ou inattendue.
    Internal,
    /// Commande inconnue, paramètre manquant, de mauvais type ou inconnu.
    InvalidRequest,
    /// Polyptyque n'est pas ouvert (côté client seulement).
    NotRunning,
    /// Pilotage désactivé dans les réglages (côté client seulement).
    ControlDisabled,
    /// Interdit pendant un enregistrement (scènes, sources, réglages).
    Locked,
    /// Opération en cours, ou fenêtre ouverte dans Polyptyque : à réessayer
    /// plus tard.
    Busy,
    NotFound,
    /// Le nom donné désigne plusieurs éléments : utiliser l'identifiant.
    Ambiguous,
    /// L'interface aurait demandé confirmation : à faire confirmer par la
    /// personne, puis à relancer avec `confirm: true`.
    ConfirmationRequired,
    /// Valeur invalide ou conflit (chevauchement, doublon…).
    Refused,
    /// Refus de la personne devant l'écran.
    Declined,
    /// Délai dépassé.
    Timeout,
    /// Impossible sur ce système ou dans cet état.
    Unavailable,
}

impl Reason {
    pub const ALL: [Reason; 13] = [
        Reason::Internal,
        Reason::InvalidRequest,
        Reason::NotRunning,
        Reason::ControlDisabled,
        Reason::Locked,
        Reason::Busy,
        Reason::NotFound,
        Reason::Ambiguous,
        Reason::ConfirmationRequired,
        Reason::Refused,
        Reason::Declined,
        Reason::Timeout,
        Reason::Unavailable,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Internal => "internal",
            Reason::InvalidRequest => "invalid_request",
            Reason::NotRunning => "not_running",
            Reason::ControlDisabled => "control_disabled",
            Reason::Locked => "locked",
            Reason::Busy => "busy",
            Reason::NotFound => "not_found",
            Reason::Ambiguous => "ambiguous",
            Reason::ConfirmationRequired => "confirmation_required",
            Reason::Refused => "refused",
            Reason::Declined => "declined",
            Reason::Timeout => "timeout",
            Reason::Unavailable => "unavailable",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == text)
    }

    /// Code de retour du mode commande (voir le cahier des charges).
    pub fn exit_code(self) -> i32 {
        match self {
            Reason::Internal => 1,
            Reason::InvalidRequest => 2,
            Reason::NotRunning => 3,
            Reason::ControlDisabled => 4,
            Reason::Locked | Reason::Busy => 5,
            Reason::NotFound | Reason::Ambiguous => 6,
            Reason::ConfirmationRequired => 7,
            Reason::Refused | Reason::Declined | Reason::Timeout => 8,
            Reason::Unavailable => 9,
        }
    }

    /// Code d'erreur JSON-RPC : ceux de la norme quand ils conviennent,
    /// sinon la plage réservée aux applications (-32000 à -32099).
    pub fn rpc_code(self) -> i64 {
        match self {
            Reason::Internal => -32603,
            Reason::InvalidRequest => -32602,
            Reason::NotRunning => -32010,
            Reason::ControlDisabled => -32011,
            Reason::Locked => -32001,
            Reason::Busy => -32002,
            Reason::NotFound => -32003,
            Reason::Ambiguous => -32004,
            Reason::ConfirmationRequired => -32005,
            Reason::Refused => -32006,
            Reason::Declined => -32007,
            Reason::Timeout => -32008,
            Reason::Unavailable => -32009,
        }
    }
}

/// Erreur d'une commande.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandError {
    pub reason: Reason,
    pub message: String,
    /// Précisions pour les programmes (texte de la confirmation attendue,
    /// candidats d'un nom ambigu…).
    pub details: Map<String, Value>,
}

impl CommandError {
    pub fn new(reason: Reason, message: impl Into<String>) -> Self {
        Self { reason, message: message.into(), details: Map::new() }
    }

    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.details.insert(key.to_string(), value.into());
        self
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(Reason::Internal, message)
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(Reason::InvalidRequest, message)
    }

    /// `{"reason": …, "message": …, …précisions}` : forme écrite par le mode
    /// commande et rendue par le serveur MCP.
    pub fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("reason".into(), self.reason.as_str().into());
        object.insert("message".into(), self.message.clone().into());
        for (key, value) in &self.details {
            object.insert(key.clone(), value.clone());
        }
        Value::Object(object)
    }

    /// Relit l'objet `error` d'une réponse JSON-RPC.
    pub fn from_rpc(error: &Value) -> Self {
        let message = error.get("message").and_then(Value::as_str).unwrap_or_default().to_string();
        let mut details = error.get("data").and_then(Value::as_object).cloned().unwrap_or_default();
        let reason = details.remove("reason").and_then(|r| r.as_str().and_then(Reason::parse)).unwrap_or(
            match error.get("code").and_then(Value::as_i64) {
                Some(-32700 | -32600 | -32601 | -32602) => Reason::InvalidRequest,
                _ => Reason::Internal,
            },
        );
        Self { reason, message, details }
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CommandError {}

pub type CommandResult = Result<Value, CommandError>;

/// Requête reçue par Polyptyque.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// Absent : notification, à exécuter sans répondre.
    pub id: Option<Value>,
    pub method: String,
    /// Toujours un objet (paramètres nommés), vide si absent.
    pub params: Value,
}

/// Ligne reçue par Polyptyque : une requête, ou un message invalide à
/// signaler (avec l'identifiant de la requête s'il a pu être lu).
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Request(Request),
    Invalid { id: Option<Value>, error: CommandError },
}

pub fn parse_request(line: &str) -> Incoming {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(e) => {
            log::warn!("pilotage : message illisible : {e}");
            return Incoming::Invalid {
                id: None,
                error: CommandError::invalid(tr!("Message illisible : un objet JSON par ligne est attendu.")),
            };
        }
    };
    let Value::Object(mut object) = value else {
        return Incoming::Invalid { id: None, error: CommandError::invalid(tr!("Un objet JSON est attendu.")) };
    };
    let id = object.remove("id");
    if id.as_ref().is_some_and(|id| !(id.is_string() || id.is_number() || id.is_null())) {
        return Incoming::Invalid {
            id: None,
            error: CommandError::invalid(tr!("Identifiant de requête invalide : un nombre ou un texte est attendu.")),
        };
    }
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Incoming::Invalid { id, error: CommandError::invalid(tr!("« jsonrpc »: \"2.0\" est attendu.")) };
    }
    let Some(method) = object.get("method").and_then(Value::as_str).map(str::to_string) else {
        return Incoming::Invalid {
            id,
            error: CommandError::invalid(tr!("Nom de la commande (« method ») manquant.")),
        };
    };
    let params = match object.remove("params") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(params @ Value::Object(_)) => params,
        Some(_) => {
            return Incoming::Invalid {
                id,
                error: CommandError::invalid(tr!(
                    "Les paramètres (« params ») sont attendus sous forme d'objet nommé."
                )),
            }
        }
    };
    Incoming::Request(Request { id, method, params })
}

/// Réponse réussie.
pub fn success(id: &Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

/// Réponse en erreur (identifiant `null` s'il n'a pas pu être lu).
pub fn failure(id: Option<&Value>, error: &CommandError) -> String {
    let mut data = Map::new();
    data.insert("reason".into(), error.reason.as_str().into());
    for (key, value) in &error.details {
        data.insert(key.clone(), value.clone());
    }
    json!({
        "jsonrpc": "2.0",
        "id": id.cloned().unwrap_or(Value::Null),
        "error": { "code": error.reason.rpc_code(), "message": error.message, "data": data },
    })
    .to_string()
}

/// Notification envoyée par Polyptyque (état, message affiché).
pub fn notification(method: &str, params: Value) -> String {
    json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string()
}

/// Requête envoyée par un client.
pub fn request(id: u64, method: &str, params: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
}

/// Ligne reçue par un client.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientMessage {
    Response { id: Value, result: CommandResult },
    Notification { method: String, params: Value },
}

pub fn parse_client_message(line: &str) -> Result<ClientMessage, CommandError> {
    let value: Value = serde_json::from_str(line)
        .map_err(|e| CommandError::internal(tr!("Réponse illisible de Polyptyque : {}", e.to_string())))?;
    if let Some(method) = value.get("method").and_then(Value::as_str) {
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        return Ok(ClientMessage::Notification { method: method.to_string(), params });
    }
    let id = value.get("id").cloned().unwrap_or(Value::Null);
    let result = match (value.get("result"), value.get("error")) {
        (_, Some(error)) => Err(CommandError::from_rpc(error)),
        (Some(result), None) => Ok(result.clone()),
        (None, None) => Err(CommandError::internal(tr!("Réponse incomplète de Polyptyque."))),
    };
    Ok(ClientMessage::Response { id, result })
}

/// Découpe en lignes ce qui arrive sur le canal, par morceaux quelconques.
#[derive(Default)]
pub struct LineBuffer {
    data: Vec<u8>,
}

impl LineBuffer {
    pub fn push(&mut self, bytes: &[u8]) {
        self.data.extend_from_slice(bytes);
    }

    /// Ligne complète suivante, sans sa fin de ligne ; les lignes vides sont
    /// sautées. Une erreur (ligne trop longue, texte invalide) rend le canal
    /// inutilisable : il faut le fermer.
    pub fn next_line(&mut self) -> Option<Result<String, CommandError>> {
        loop {
            let Some(end) = self.data.iter().position(|&b| b == b'\n') else {
                if self.data.len() > MAX_LINE {
                    self.data.clear();
                    return Some(Err(CommandError::invalid(tr!("Message trop long."))));
                }
                return None;
            };
            let line: Vec<u8> = self.data.drain(..=end).collect();
            let line = &line[..line.len() - 1];
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.len() > MAX_LINE {
                return Some(Err(CommandError::invalid(tr!("Message trop long."))));
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            return Some(
                String::from_utf8(line.to_vec())
                    .map_err(|_| CommandError::invalid(tr!("Message invalide : texte UTF-8 attendu."))),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_requests() {
        let Incoming::Request(request) = parse_request(r#"{"jsonrpc":"2.0","id":7,"method":"status"}"#) else {
            panic!("requête attendue");
        };
        assert_eq!(request.id, Some(json!(7)));
        assert_eq!(request.method, "status");
        assert_eq!(request.params, json!({}));

        let Incoming::Request(request) =
            parse_request(r#"{"jsonrpc":"2.0","method":"scenes.activate","params":{"scene":"Podcast"}}"#)
        else {
            panic!("notification attendue");
        };
        assert_eq!(request.id, None);
        assert_eq!(request.params["scene"], "Podcast");
    }

    #[test]
    fn reports_invalid_messages() {
        for line in ["pas du json", "[1,2]", r#"{"jsonrpc":"2.0","id":{"a":1},"method":"x"}"#] {
            assert!(matches!(parse_request(line), Incoming::Invalid { id: None, .. }), "{line}");
        }
        // L'identifiant lu est rendu, pour que le client reconnaisse sa requête.
        for line in [
            r#"{"id":3,"method":"status"}"#,
            r#"{"jsonrpc":"2.0","id":3}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"status","params":[1]}"#,
        ] {
            let Incoming::Invalid { id, error } = parse_request(line) else { panic!("{line}") };
            assert_eq!(id, Some(json!(3)));
            assert_eq!(error.reason, Reason::InvalidRequest);
        }
    }

    #[test]
    fn errors_survive_the_round_trip() {
        let error = CommandError::new(Reason::ConfirmationRequired, "Supprimer ?").with("confirm_text", "La scène…");
        let line = failure(Some(&json!(4)), &error);
        let ClientMessage::Response { id, result } = parse_client_message(&line).unwrap() else {
            panic!("réponse attendue");
        };
        assert_eq!(id, json!(4));
        assert_eq!(result, Err(error));
        // Code de la norme sans raison : demande invalide.
        let message = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}"#;
        let ClientMessage::Response { result: Err(error), .. } = parse_client_message(message).unwrap() else {
            panic!("erreur attendue");
        };
        assert_eq!(error.reason, Reason::InvalidRequest);
    }

    #[test]
    fn reasons_are_stable() {
        for reason in Reason::ALL {
            assert_eq!(Reason::parse(reason.as_str()), Some(reason));
            assert!(reason.exit_code() > 0);
        }
        assert_eq!(Reason::ConfirmationRequired.exit_code(), 7);
        assert_eq!(Reason::NotRunning.exit_code(), 3);
    }

    #[test]
    fn splits_lines() {
        let mut lines = LineBuffer::default();
        lines.push(b"{\"a\":1}\r\n\n{\"b\"");
        assert_eq!(lines.next_line(), Some(Ok("{\"a\":1}".to_string())));
        assert_eq!(lines.next_line(), None);
        lines.push(b":2}\n");
        assert_eq!(lines.next_line(), Some(Ok("{\"b\":2}".to_string())));
        lines.push(&vec![b'x'; MAX_LINE + 1]);
        assert!(matches!(lines.next_line(), Some(Err(_))));
        assert_eq!(lines.next_line(), None);
    }
}
