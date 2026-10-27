//! Arguments d'une commande : `polyptyque-cli recording start --scene
//! Podcast --auto-stop`. Les types viennent du catalogue : un booléen seul
//! vaut `true`, une liste à valeurs fixes se donne séparée par des
//! virgules (`--days mon,fri`), un champ d'objet par son chemin
//! (`--video.bitrate-kbps 8000`), et `--json '{…}'` donne tout ou partie
//! des paramètres d'un coup.

use polyptyque_core::control::catalog::{self, Command, Param, ParamType};
use polyptyque_core::control::protocol::CommandError;
use serde_json::{Map, Value};
use tr::tr;

/// Commande désignée par les premiers mots (« recording start »,
/// « recording.start », « recording_start », « status »). Renvoie la
/// commande et le nombre de mots utilisés.
pub fn resolve(words: &[String]) -> Option<(Command, usize)> {
    // « open-windows » comme « open_windows ».
    let words: Vec<String> = words.iter().take(2).map(|w| w.replace('-', "_")).collect();
    if words.len() >= 2 {
        if let Some(command) = catalog::find(&format!("{}.{}", words[0], words[1])) {
            return Some((command, 2));
        }
    }
    let first = words.first()?;
    catalog::find(first).map(|command| (command, 1))
}

fn boolean(text: &str) -> Option<bool> {
    match text.to_ascii_lowercase().as_str() {
        "true" | "yes" | "oui" | "on" | "1" => Some(true),
        "false" | "no" | "non" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// Paramètre (ou champ d'objet) désigné par son chemin : `video.codec`.
fn lookup<'a>(params: &'a [Param], path: &[&str]) -> Option<&'a Param> {
    let (first, rest) = path.split_first()?;
    let param = params.iter().find(|p| p.name == *first)?;
    if rest.is_empty() {
        Some(param)
    } else {
        lookup(&param.fields, rest)
    }
}

/// Place `value` au chemin `path` dans `object`, en créant les objets
/// intermédiaires.
fn insert(object: &mut Map<String, Value>, path: &[&str], value: Value, append: bool) {
    let (first, rest) = path.split_first().expect("chemin non vide");
    if rest.is_empty() {
        match (object.get_mut(*first), value) {
            (Some(Value::Array(items)), Value::Array(more)) if append => items.extend(more),
            (Some(Value::Object(existing)), Value::Object(more)) => existing.extend(more),
            (_, value) => {
                object.insert((*first).to_string(), value);
            }
        }
        return;
    }
    let child = object.entry((*first).to_string()).or_insert_with(|| Value::Object(Map::new()));
    if !child.is_object() {
        *child = Value::Object(Map::new());
    }
    if let Value::Object(child) = child {
        insert(child, rest, value, append);
    }
}

fn parse_json_object(text: &str, name: &str) -> Result<Map<String, Value>, CommandError> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(object)) => Ok(object),
        _ => Err(CommandError::invalid(tr!("« --{} » : un objet JSON est attendu ({{…}}).", name))),
    }
}

/// Paramètres de `command`, lus dans `options` (ce qui suit les mots de la
/// commande).
pub fn params(command: &Command, options: &[String]) -> Result<Value, CommandError> {
    let mut params = Map::new();
    let mut i = 0;
    while i < options.len() {
        let option = &options[i];
        i += 1;
        let Some(name) = option.strip_prefix("--") else {
            return Err(CommandError::invalid(tr!(
                "Argument inattendu : « {} » (les paramètres s'écrivent --nom valeur).",
                option
            )));
        };
        let (name, inline) = match name.split_once('=') {
            Some((name, value)) => (name, Some(value.to_string())),
            None => (name, None),
        };
        let key = name.replace('-', "_");
        if key == "json" {
            let text = inline.or_else(|| options.get(i).cloned().inspect(|_| i += 1)).unwrap_or_default();
            for (key, value) in parse_json_object(&text, "json")? {
                params.insert(key, value);
            }
            continue;
        }
        let path: Vec<&str> = key.split('.').collect();
        let param = lookup(&command.params, &path);
        let ty = param.map_or(ParamType::String, |p| p.ty);
        // Un booléen n'avale la suite que si c'est bien un booléen.
        let mut take_value = || -> Option<String> {
            if inline.is_some() {
                return inline.clone();
            }
            let next = options.get(i)?;
            if ty == ParamType::Boolean && boolean(next).is_none() {
                return None;
            }
            i += 1;
            Some(next.clone())
        };
        let value = match ty {
            ParamType::Boolean => match take_value() {
                None => Value::Bool(true),
                Some(text) => Value::Bool(
                    boolean(&text)
                        .ok_or_else(|| CommandError::invalid(tr!("« --{} » : true ou false attendu.", name)))?,
                ),
            },
            ParamType::Integer => {
                let text = take_value()
                    .ok_or_else(|| CommandError::invalid(tr!("« --{} » : une valeur est attendue.", name)))?;
                let digits: String =
                    text.chars().filter(|c| !matches!(c, ' ' | '_' | ',' | '\u{a0}' | '\u{202f}')).collect();
                Value::from(
                    digits
                        .parse::<i64>()
                        .map_err(|_| CommandError::invalid(tr!("« --{} » : un nombre entier est attendu.", name)))?,
                )
            }
            ParamType::Strings => {
                let text = take_value()
                    .ok_or_else(|| CommandError::invalid(tr!("« --{} » : une valeur est attendue.", name)))?;
                // Valeurs fixes (jours) : séparées par des virgules. Sinon
                // (chemins de fichiers), une valeur par --nom.
                let items: Vec<Value> = if param.is_some_and(|p| !p.choices.is_empty()) {
                    text.split(',').map(str::trim).filter(|t| !t.is_empty()).map(Value::from).collect()
                } else {
                    vec![Value::from(text)]
                };
                insert(&mut params, &path, Value::Array(items), true);
                continue;
            }
            ParamType::Object => {
                let text = take_value()
                    .ok_or_else(|| CommandError::invalid(tr!("« --{} » : une valeur est attendue.", name)))?;
                Value::Object(parse_json_object(&text, name)?)
            }
            ParamType::String => Value::from(
                take_value().ok_or_else(|| CommandError::invalid(tr!("« --{} » : une valeur est attendue.", name)))?,
            ),
        };
        insert(&mut params, &path, value, false);
    }
    let params = Value::Object(params);
    command.check_params(&params)?;
    Ok(params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn finds_commands_by_words() {
        assert_eq!(
            resolve(&args(&["recording", "start", "--scene", "x"])).map(|(c, n)| (c.method, n)),
            Some(("recording.start", 2))
        );
        assert_eq!(resolve(&args(&["recording.start"])).map(|(c, n)| (c.method, n)), Some(("recording.start", 1)));
        assert_eq!(resolve(&args(&["status"])).map(|(c, n)| (c.method, n)), Some(("status", 1)));
        assert!(resolve(&args(&["record"])).is_none());
        assert_eq!(resolve(&args(&["scenes", "open-windows"])).map(|(c, _)| c.method), Some("scenes.open_windows"));
    }

    #[test]
    fn reads_typed_params() {
        let start = catalog::find("recording.start").unwrap();
        let params = params(&start, &args(&["--scene", "Podcast", "--auto-stop"])).unwrap();
        assert_eq!(params, json!({ "scene": "Podcast", "auto_stop": true }));
        let params = super::params(&start, &args(&["--auto-stop", "false", "--source=Discord"])).unwrap();
        assert_eq!(params, json!({ "auto_stop": false, "source": "Discord" }));

        let create = catalog::find("schedules.create").unwrap();
        let params = super::params(
            &create,
            &args(&["--scene", "Podcast", "--start", "21:00", "--end", "22:30", "--days", "mon,fri", "--days", "sun"]),
        )
        .unwrap();
        assert_eq!(params["days"], json!(["mon", "fri", "sun"]));

        let settings = catalog::find("settings.set").unwrap();
        let params = super::params(
            &settings,
            &args(&["--video.bitrate-kbps", "8 000", "--video.codec", "hevc", "--json", r#"{"previews":false}"#]),
        )
        .unwrap();
        assert_eq!(params, json!({ "video": { "bitrate_kbps": 8000, "codec": "hevc" }, "previews": false }));

        let convert = catalog::find("conversion.start").unwrap();
        let params = super::params(&convert, &args(&["--files", r"C:\a, b.mkv", "--files", "c.mkv"])).unwrap();
        assert_eq!(params["files"], json!([r"C:\a, b.mkv", "c.mkv"]));
    }

    #[test]
    fn rejects_bad_params() {
        let start = catalog::find("recording.start").unwrap();
        assert!(params(&start, &args(&["--scenes", "x"])).is_err());
        assert!(params(&start, &args(&["Podcast"])).is_err());
        let move_ = catalog::find("sources.move").unwrap();
        assert!(params(&move_, &args(&["--source", "x", "--position", "deux"])).is_err());
    }
}
