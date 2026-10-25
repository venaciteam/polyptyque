//! Polyptyque, mode commande : pilote le Polyptyque ouvert sur cet
//! ordinateur par son canal de commande (voir « Pilotage par d'autres
//! programmes » dans le cahier des charges).
//!
//! Chaque commande écrit sa réponse en JSON sur la sortie standard, et se
//! termine par un code de retour stable (`Reason::exit_code`). `mcp`
//! transforme les mêmes commandes en outils pour les assistants IA.

mod args;
mod channel;
#[cfg(test)]
mod doc;
mod help;
mod i18n;
mod mcp;
mod output;

use std::io::Write;
use std::time::Duration;

use polyptyque_core::config::LanguageChoice;
use polyptyque_core::control::catalog;
use polyptyque_core::control::protocol::{CommandError, PROTOCOL_VERSION};
use serde_json::{json, Value};
use tr::tr;

fn main() {
    let language = channel::read_config().map_or(LanguageChoice::Auto, |c| c.settings.language);
    i18n::set(i18n::english(language));
    std::process::exit(run(std::env::args().skip(1).collect()));
}

fn run(mut args: Vec<String>) -> i32 {
    let pretty = args.iter().any(|a| a == "--pretty");
    args.retain(|a| a != "--pretty");
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] | ["help" | "--help" | "-h"] => {
            print!("{}", help::general());
            0
        }
        ["help" | "--help" | "-h", rest @ ..] | [rest @ .., "--help" | "-h"] => {
            let rest: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
            match args::resolve(&rest) {
                Some((command, _)) => {
                    print!("{}", help::command(&command));
                    0
                }
                None => unknown(&rest.join(" "), pretty),
            }
        }
        ["version" | "--version" | "-V"] => {
            let mut value = json!({ "cli": env!("CARGO_PKG_VERSION"), "protocol": PROTOCOL_VERSION });
            if let Ok(app) = channel::connect().and_then(|mut c| c.call("hello", json!({}), Duration::from_secs(10))) {
                value["app"] = app;
            }
            output::print(&value, pretty);
            0
        }
        ["commands"] => {
            // Pour les modèles : descriptions en anglais.
            i18n::set(true);
            let commands: Vec<Value> = catalog::commands().iter().map(|c| c.to_json()).collect();
            output::print(&json!({ "protocol": PROTOCOL_VERSION, "commands": commands }), pretty);
            0
        }
        ["app", "launch"] => match channel::launch() {
            Ok(value) => {
                output::print(&value, pretty);
                0
            }
            Err(e) => output::fail(&e, pretty),
        },
        ["mcp"] => mcp::serve(),
        ["events"] => events(pretty),
        ["call", method] => call(method, "{}", pretty),
        ["call", method, params] => call(method, params, pretty),
        _ => {
            let Some((command, used)) = args::resolve(&args) else {
                return unknown(
                    &args.iter().take_while(|a| !a.starts_with("--")).cloned().collect::<Vec<_>>().join(" "),
                    pretty,
                );
            };
            match args::params(&command, &args[used..]) {
                Ok(params) => execute(command.method, params, command.timeout, pretty),
                Err(e) => output::fail(&e, pretty),
            }
        }
    }
}

fn unknown(words: &str, pretty: bool) -> i32 {
    output::fail(
        &CommandError::invalid(tr!("Commande inconnue : « {} ». La liste : polyptyque-cli help.", words)),
        pretty,
    )
}

/// Envoie une commande et écrit sa réponse.
fn execute(method: &str, params: Value, timeout: Duration, pretty: bool) -> i32 {
    let result = channel::connect().and_then(|mut client| {
        client.call("hello", json!({ "client": "polyptyque-cli" }), Duration::from_secs(10))?;
        client.call(method, params, timeout + Duration::from_secs(5))
    });
    match result {
        Ok(value) => {
            output::print(&value, pretty);
            0
        }
        Err(e) => output::fail(&e, pretty),
    }
}

/// `call <méthode> '<json>'` : n'importe quelle commande, paramètres en
/// JSON.
fn call(method: &str, params: &str, pretty: bool) -> i32 {
    let Some(command) = catalog::find(method) else {
        return unknown(method, pretty);
    };
    let params: Value = match serde_json::from_str(params) {
        Ok(params @ Value::Object(_)) => params,
        _ => {
            return output::fail(&CommandError::invalid(tr!("Paramètres : un objet JSON est attendu ({{…}}).")), pretty)
        }
    };
    if let Err(e) = command.check_params(&params) {
        return output::fail(&e, pretty);
    }
    execute(command.method, params, command.timeout, pretty)
}

/// État en direct : une ligne JSON par notification, jusqu'à la coupure.
fn events(pretty: bool) -> i32 {
    let mut client = match channel::connect() {
        Ok(client) => client,
        Err(e) => return output::fail(&e, pretty),
    };
    let subscribed = client
        .call("hello", json!({ "client": "polyptyque-cli events" }), Duration::from_secs(10))
        .and_then(|_| client.call("subscribe", json!({}), Duration::from_secs(10)));
    match subscribed {
        Ok(result) => output::print(&json!({ "event": "state", "data": result["state"] }), pretty),
        Err(e) => return output::fail(&e, pretty),
    }
    loop {
        match client.next_notification(Duration::from_secs(3600)) {
            Ok(Some((method, params))) => {
                output::print(&json!({ "event": method, "data": params }), pretty);
                let _ = std::io::stdout().flush();
            }
            Ok(None) => {}
            Err(e) => return output::fail(&e, pretty),
        }
    }
}
