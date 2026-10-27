//! Sortie du mode commande : JSON sur la sortie standard, réponse comme
//! erreur.

use std::io::Write;

use polyptyque_core::control::protocol::CommandError;
use serde_json::{json, Value};

/// Caractères non ASCII écrits `\uXXXX` : le JSON reste lisible quel que
/// soit l'encodage de l'outil qui le reçoit (PowerShell lit la sortie d'un
/// programme dans la page de code de la console, pas en UTF-8).
pub fn ascii(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

pub fn print(value: &Value, pretty: bool) {
    let text = if pretty { serde_json::to_string_pretty(value) } else { serde_json::to_string(value) }
        .unwrap_or_else(|_| "null".into());
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{}", ascii(&text));
    let _ = stdout.flush();
}

/// Écrit l'erreur et renvoie le code de retour qui lui correspond.
pub fn fail(error: &CommandError, pretty: bool) -> i32 {
    print(&json!({ "error": error.to_json() }), pretty);
    error.reason.exit_code()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_non_ascii() {
        assert_eq!(ascii(r#"{"a":"Scène 🎙"}"#), r#"{"a":"Sc\u00e8ne \ud83c\udf99"}"#);
        let value: Value = serde_json::from_str(&ascii(r#"{"a":"Scène 🎙"}"#)).unwrap();
        assert_eq!(value["a"], "Scène 🎙");
    }
}
