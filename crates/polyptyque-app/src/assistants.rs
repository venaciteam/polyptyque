//! Connexions MCP des assistants installés sur ce compte Windows.
//!
//! Les fichiers restent ceux des clients : seule l'entrée `polyptyque` est
//! ajoutée, réparée ou retirée. Une sauvegarde et une vérification de
//! concurrence précèdent chaque remplacement. Aucun assistant n'est lancé.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use polyptyque_core::{config::new_id, storage};
use serde_json::{json, Value};
use toml_edit::{DocumentMut, Item};
use tr::tr;

const SERVER: &str = "polyptyque";
pub const CLIENT_ENV: &str = "POLYPTYQUE_ASSISTANT";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Assistant {
    ClaudeCode,
    ClaudeDesktop,
    Codex,
    OpenClaw,
    Hermes,
}

impl Assistant {
    pub const ALL: [Self; 5] = [Self::ClaudeCode, Self::ClaudeDesktop, Self::Codex, Self::OpenClaw, Self::Hermes];

    pub fn id(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::ClaudeDesktop => "claude-desktop",
            Self::Codex => "codex",
            Self::OpenClaw => "openclaw",
            Self::Hermes => "hermes",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::ClaudeDesktop => "Claude Desktop",
            Self::Codex => "Codex",
            Self::OpenClaw => "OpenClaw",
            Self::Hermes => "Hermes Agent",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }

    pub fn next_step(self) -> String {
        match self {
            Self::ClaudeDesktop => tr!("1. Quitter Claude Desktop depuis son icône près de l'horloge, puis le rouvrir.\n2. Choisir le mode Claude et ouvrir une nouvelle conversation.\n3. Cliquer sur +, puis Connecteurs : activer Polyptyque s'il est désactivé.\n4. Envoyer « Liste mes scènes dans Polyptyque. » et accepter la demande d'autorisation si elle apparaît."),
            Self::ClaudeCode => tr!("1. Fermer la session Claude Code en cours, puis en ouvrir une nouvelle.\n2. Saisir /mcp dans cette session et vérifier que polyptyque est disponible.\n3. Envoyer « Liste mes scènes dans Polyptyque. » et accepter la demande d'utilisation des outils si elle apparaît."),
            Self::Codex => tr!("1. Dans les paramètres de Codex, ouvrir Serveurs MCP et vérifier que polyptyque est activé. Relancer Codex si la connexion n'est pas chargée.\n2. Ouvrir une nouvelle conversation exécutée sur cet ordinateur.\n3. Envoyer « Liste mes scènes dans Polyptyque. » et accepter la demande d'utilisation des outils si elle apparaît."),
            Self::OpenClaw => tr!("1. Relancer la passerelle OpenClaw qui tourne sur ce PC pour charger la configuration.\n2. Dans son interface de contrôle, ouvrir Settings > MCP et vérifier la présence de polyptyque.\n3. Dans la conversation, ouvrir + > Plugins > Tool access et vérifier que les outils Polyptyque ne sont pas bloqués.\n4. Envoyer « Liste mes scènes dans Polyptyque. »."),
            Self::Hermes => tr!("1. Dans la conversation Hermes Agent, saisir /reload-mcp pour charger la connexion, ou ouvrir une nouvelle session.\n2. Envoyer « Liste mes scènes dans Polyptyque. ».\n3. Si Hermes demande l'accord pour utiliser un outil Polyptyque, accepter cette demande. Si le rechargement signale une erreur, la connexion n'est pas encore utilisable."),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Missing,
    Available,
    Configured,
    Repair,
    Conflict,
    Error,
}

#[derive(Clone, Debug)]
pub struct Target {
    pub assistant: Assistant,
    pub path: PathBuf,
    pub detected: bool,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub target: Target,
    pub state: State,
    pub detail: String,
}

/// Variables héritées du processus uniquement ; aucun secret n'est lu.
pub fn targets() -> Vec<Target> {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
    let roaming = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| home.join("AppData/Roaming"));
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| home.join("AppData/Local"));
    let override_dir = |key: &str, default: PathBuf| {
        std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or(default)
    };
    let codex = override_dir("CODEX_HOME", home.join(".codex"));
    let claude = override_dir("CLAUDE_CONFIG_DIR", home.join(".claude"));
    let claw = override_dir("OPENCLAW_STATE_DIR", home.join(".openclaw"));
    let claw_file = override_dir("OPENCLAW_CONFIG_PATH", claw.join("openclaw.json"));
    let hermes = override_dir("HERMES_HOME", local.join("hermes"));
    let claude_file = if std::env::var_os("CLAUDE_CONFIG_DIR").is_some() {
        claude.join(".claude.json")
    } else {
        home.join(".claude.json")
    };
    let definitions = [
        (Assistant::ClaudeCode, claude_file, claude.is_dir() || executable("claude", &home, &roaming)),
        (
            Assistant::ClaudeDesktop,
            roaming.join("Claude/claude_desktop_config.json"),
            roaming.join("Claude").is_dir() || local.join("AnthropicClaude").is_dir(),
        ),
        (Assistant::Codex, codex.join("config.toml"), codex.is_dir() || executable("codex", &home, &roaming)),
        (Assistant::OpenClaw, claw_file, claw.is_dir() || executable("openclaw", &home, &roaming)),
        (Assistant::Hermes, hermes.join("config.yaml"), hermes.is_dir() || executable("hermes", &home, &roaming)),
    ];
    definitions
        .into_iter()
        .map(|(assistant, path, detected)| Target { assistant, detected: detected || path.is_file(), path })
        .collect()
}

fn executable(name: &str, home: &Path, roaming: &Path) -> bool {
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    dirs.extend([home.join(".local/bin"), roaming.join("npm"), home.join(".cargo/bin")]);
    dirs.iter().any(|dir| ["exe", "cmd", "ps1"].iter().any(|ext| dir.join(format!("{name}.{ext}")).is_file()))
}

pub fn cli_path() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let cli = exe.with_file_name(if cfg!(windows) { "polyptyque-cli.exe" } else { "polyptyque-cli" });
    if !cli.is_file() {
        bail!(tr!("Le composant de connexion manque. Réinstaller Polyptyque, puis réessayer."));
    }
    Ok(cli)
}

fn read(path: &Path) -> Result<Option<String>> {
    if cfg!(windows) && (!path.is_absolute() || path.to_string_lossy().starts_with("\\\\")) {
        bail!(tr!("Choisir une configuration sur un disque Windows local. Les chemins réseau et WSL ne sont pas pris en charge automatiquement."));
    }
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| tr!("Impossible de lire la configuration de l'assistant.")),
    }
}

enum Document {
    Json(Value),
    Toml(DocumentMut),
    Yaml(serde_yaml_ng::Value),
}

impl Document {
    fn parse(assistant: Assistant, text: &str) -> Result<Self> {
        let text = text.trim_start_matches('\u{feff}');
        let invalid = || tr!("La configuration de l'assistant est invalide. Aucun fichier n'a été modifié.");
        let document = match assistant {
            Assistant::Codex => Self::Toml(text.parse().with_context(invalid)?),
            Assistant::Hermes => {
                let value = if text.trim().is_empty() {
                    serde_yaml_ng::Value::Mapping(Default::default())
                } else {
                    serde_yaml_ng::from_str(text).with_context(invalid)?
                };
                Self::Yaml(value)
            }
            Assistant::OpenClaw => {
                let value: Value =
                    json5::from_str(if text.trim().is_empty() { "{}" } else { text }).with_context(invalid)?;
                // Les inclusions peuvent masquer une autre définition : pas de
                // modification automatique d'une configuration distribuée.
                if contains_include(&value) {
                    bail!(tr!("Cette configuration OpenClaw utilise des fichiers inclus. La connexion automatique ne peut pas la modifier."));
                }
                Self::Json(value)
            }
            _ => Self::Json(
                serde_json::from_str(if text.trim().is_empty() { "{}" } else { text }).with_context(invalid)?,
            ),
        };
        // Vérifie les conteneurs même si l'entrée n'existe pas encore.
        document.entry(assistant)?;
        Ok(document)
    }

    fn entry(&self, assistant: Assistant) -> Result<Option<Value>> {
        let malformed = || {
            anyhow::anyhow!(tr!("La liste des serveurs de l'assistant est invalide. Aucun fichier n'a été modifié."))
        };
        match self {
            Self::Json(root) => {
                let root = root.as_object().ok_or_else(malformed)?;
                let parent = if assistant == Assistant::OpenClaw {
                    match root.get("mcp") {
                        None => return Ok(None),
                        Some(v) => v.as_object().ok_or_else(malformed)?,
                    }
                } else {
                    root
                };
                let key = if assistant == Assistant::OpenClaw { "servers" } else { "mcpServers" };
                match parent.get(key) {
                    None => Ok(None),
                    Some(v) => Ok(v.as_object().ok_or_else(malformed)?.get(SERVER).cloned()),
                }
            }
            Self::Toml(root) => match root.get("mcp_servers") {
                None => Ok(None),
                Some(v) => {
                    let servers = v.as_table_like().ok_or_else(malformed)?;
                    // toml_edit préserve les commentaires, même hors de cette table.
                    let Some(entry) = servers.get(SERVER) else { return Ok(None) };
                    let mut wrapper = DocumentMut::new();
                    wrapper[SERVER] = entry.clone();
                    let value: Value = toml_to_json(&wrapper.to_string())?;
                    Ok(value.get(SERVER).cloned())
                }
            },
            Self::Yaml(root) => {
                let root = root.as_mapping().ok_or_else(malformed)?;
                let key = serde_yaml_ng::Value::String("mcp_servers".into());
                match root.get(&key) {
                    None => Ok(None),
                    Some(v) => {
                        let servers = v.as_mapping().ok_or_else(malformed)?;
                        servers
                            .get(serde_yaml_ng::Value::String(SERVER.into()))
                            .map(serde_json::to_value)
                            .transpose()
                            .with_context(malformed)
                    }
                }
            }
        }
    }

    fn set(&mut self, assistant: Assistant, entry: Option<Value>) -> Result<()> {
        match self {
            Self::Json(root) => {
                let parent = if assistant == Assistant::OpenClaw {
                    root.as_object_mut().unwrap().entry("mcp").or_insert_with(|| json!({}))
                } else {
                    root
                };
                let key = if assistant == Assistant::OpenClaw { "servers" } else { "mcpServers" };
                let servers =
                    parent.as_object_mut().unwrap().entry(key).or_insert_with(|| json!({})).as_object_mut().unwrap();
                if let Some(entry) = entry {
                    servers.insert(SERVER.into(), entry);
                } else {
                    servers.remove(SERVER);
                }
            }
            Self::Toml(root) => {
                if root.get("mcp_servers").is_none() {
                    root["mcp_servers"] = Item::Table(Default::default());
                }
                let servers = root["mcp_servers"].as_table_like_mut().unwrap();
                if let Some(entry) = entry {
                    // Seules les clés de connexion sont changées ; filtres
                    // d'outils et politiques restent à la main du client.
                    if servers.get(SERVER).is_none() {
                        servers.insert(SERVER, Item::Table(Default::default()));
                    }
                    let target = servers.get_mut(SERVER).unwrap().as_table_like_mut().ok_or_else(|| {
                        anyhow::anyhow!(tr!("L'entrée Polyptyque ne peut pas être modifiée automatiquement."))
                    })?;
                    target.insert("command", toml_edit::value(entry["command"].as_str().unwrap()));
                    let mut args = toml_edit::Array::new();
                    args.push("mcp");
                    target.insert("args", toml_edit::value(args));
                    target.insert("enabled", toml_edit::value(true));
                    if target.get("env").is_none() {
                        target.insert("env", Item::Table(Default::default()));
                    }
                    target
                        .get_mut("env")
                        .unwrap()
                        .as_table_like_mut()
                        .ok_or_else(|| anyhow::anyhow!(tr!("L'environnement du serveur Polyptyque est invalide.")))?
                        .insert(CLIENT_ENV, toml_edit::value(assistant.id()));
                } else {
                    servers.remove(SERVER);
                }
            }
            Self::Yaml(root) => {
                let key = serde_yaml_ng::Value::String("mcp_servers".into());
                let root = root.as_mapping_mut().unwrap();
                let servers = root
                    .entry(key)
                    .or_insert_with(|| serde_yaml_ng::Value::Mapping(Default::default()))
                    .as_mapping_mut()
                    .unwrap();
                let key = serde_yaml_ng::Value::String(SERVER.into());
                if let Some(entry) = entry {
                    servers.insert(key, serde_yaml_ng::to_value(entry)?);
                } else {
                    servers.remove(&key);
                }
            }
        }
        Ok(())
    }

    fn render(&self) -> Result<String> {
        Ok(match self {
            Self::Json(value) => format!("{}\n", serde_json::to_string_pretty(value)?),
            Self::Toml(value) => value.to_string(),
            Self::Yaml(value) => serde_yaml_ng::to_string(value)?,
        })
    }
}

fn toml_to_json(text: &str) -> Result<Value> {
    let value: toml::Value = toml::from_str(text)?;
    Ok(serde_json::to_value(value)?)
}

fn contains_include(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.contains_key("$include") || map.values().any(contains_include),
        Value::Array(values) => values.iter().any(contains_include),
        _ => false,
    }
}

/// Une ancienne configuration manuelle est reconnue seulement avec le
/// nom exact de notre programme et l'unique argument `mcp`.
fn ours(entry: &Value) -> bool {
    let command = entry.get("command").and_then(Value::as_str).unwrap_or_default().replace('\\', "/");
    let name = command.rsplit('/').next().unwrap_or_default();
    (name.eq_ignore_ascii_case("polyptyque-cli.exe") || name == "polyptyque-cli")
        && entry.get("args") == Some(&json!(["mcp"]))
        && entry.get("url").is_none()
}

fn desired(assistant: Assistant, cli: &Path, previous: Option<Value>) -> Result<Value> {
    let mut entry = previous.unwrap_or_else(|| json!({}));
    let map = entry.as_object_mut().ok_or_else(|| anyhow::anyhow!(tr!("L'entrée Polyptyque est invalide.")))?;
    map.insert("command".into(), json!(cli));
    map.insert("args".into(), json!(["mcp"]));
    if matches!(assistant, Assistant::Codex | Assistant::OpenClaw | Assistant::Hermes) {
        map.insert("enabled".into(), json!(true));
    }
    let env = map
        .entry("env")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!(tr!("L'environnement du serveur Polyptyque est invalide.")))?;
    env.insert(CLIENT_ENV.into(), json!(assistant.id()));
    Ok(entry)
}

pub fn inspect(target: Target, cli: &Path) -> Snapshot {
    let result = (|| -> Result<(State, String)> {
        let text = read(&target.path)?;
        let doc = Document::parse(target.assistant, text.as_deref().unwrap_or(""))?;
        let entry = doc.entry(target.assistant)?;
        Ok(match entry {
            Some(entry) if !ours(&entry) => (State::Conflict, tr!("Un autre serveur porte déjà le nom Polyptyque. Il est conservé ; le renommer dans l'assistant avant de continuer.")),
            Some(entry) => {
                let expected = desired(target.assistant, cli, Some(entry.clone()))?;
                if entry == expected { (State::Configured, target.assistant.next_step()) }
                else { (State::Repair, tr!("La connexion existe, mais son chemin ou ses réglages doivent être actualisés.")) }
            }
            None if target.detected => (State::Available, tr!("Prêt à configurer. Seule la connexion Polyptyque sera ajoutée à cet assistant.")),
            None => (State::Missing, tr!("Installation Windows non détectée. Ouvrir l'assistant une première fois, puis actualiser, ou choisir sa configuration.")),
        })
    })();
    let (state, detail) = result.unwrap_or_else(|e| (State::Error, format!("{e:#}")));
    Snapshot { target, state, detail }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Connect,
    Disconnect,
}

/// Préserve une copie exacte du fichier précédent avant l'écriture.
/// Les sauvegardes peuvent contenir les secrets du client : elles restent
/// à côté de sa configuration et ne sont jamais ajoutées aux rapports.
pub fn configure(target: &Target, cli: &Path, action: Action) -> Result<()> {
    if action == Action::Connect && !cli.is_file() {
        bail!(tr!("Le composant de connexion manque. Réinstaller Polyptyque, puis réessayer."));
    }
    let before = read(&target.path)?;
    let mut doc = Document::parse(target.assistant, before.as_deref().unwrap_or(""))?;
    let previous = doc.entry(target.assistant)?;
    if previous.as_ref().is_some_and(|v| !ours(v)) {
        bail!(tr!("Un autre serveur porte déjà le nom Polyptyque. Aucun fichier n'a été modifié."));
    }
    if action == Action::Disconnect && previous.is_none() {
        return Ok(());
    }
    let entry = if action == Action::Connect { Some(desired(target.assistant, cli, previous)?) } else { None };
    doc.set(target.assistant, entry.clone())?;
    let after = doc.render()?;
    // Vérifie aussi le format effectivement écrit, avant de toucher au disque.
    let parsed = Document::parse(target.assistant, &after)?;
    if parsed.entry(target.assistant)? != entry {
        bail!(tr!("La configuration préparée n'a pas pu être vérifiée."));
    }
    if before.as_deref() == Some(&after) {
        return Ok(());
    }
    replace(&target.path, before.as_deref(), &after)?;
    Ok(())
}

fn replace(path: &Path, before: Option<&str>, after: &str) -> Result<()> {
    // Ne pas remplacer silencieusement une modification concurrente du client.
    if read(path)?.as_deref() != before {
        bail!(tr!("L'assistant a modifié sa configuration entre-temps. Actualiser, puis réessayer."));
    }
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        bail!(tr!("La configuration est un lien symbolique. Choisir son fichier réel avant de continuer."));
    }
    if let Some(text) = before {
        let backup = storage::sibling(path, &format!(".polyptyque-{}.bak", new_id()));
        // Copie pour garder les permissions ; aucun contenu dans les journaux.
        fs::copy(path, &backup)
            .with_context(|| tr!("Impossible de sauvegarder la configuration. Aucun fichier n'a été modifié."))?;
        if fs::read_to_string(&backup)?.as_str() != text {
            bail!(tr!("L'assistant a modifié sa configuration entre-temps. Actualiser, puis réessayer."));
        }
    }
    if read(path)?.as_deref() != before {
        bail!(tr!("L'assistant a modifié sa configuration entre-temps. Actualiser, puis réessayer."));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Nom unique : le client peut lui-même écrire son fichier `.tmp`.
    let tmp = storage::sibling(path, &format!(".polyptyque-{}.tmp", new_id()));
    let result = (|| -> Result<()> {
        let file = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        drop(file);
        // Conserve aussi les permissions du fichier client (y compris les
        // propriétés de sécurité copiées par Windows), avant remplacement.
        if before.is_some() {
            fs::copy(path, &tmp)?;
        }
        let mut file = fs::OpenOptions::new().write(true).truncate(true).open(&tmp)?;
        file.write_all(after.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if read(path)?.as_deref() != before {
            bail!(tr!("L'assistant a modifié sa configuration entre-temps. Actualiser, puis réessayer."));
        }
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.with_context(|| tr!("Impossible d'enregistrer la connexion de l'assistant."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_other_servers_settings_and_disconnects_only_polyptyque() {
        for (assistant, initial) in [
            (Assistant::ClaudeCode, r#"{"theme":"dark","mcpServers":{"other":{"command":"other"}}}"#),
            (Assistant::ClaudeDesktop, r#"{"preferences":{"a":true},"mcpServers":{"other":{"command":"other"}}}"#),
            (Assistant::Codex, "# conserver\nmodel = 'example'\n[mcp_servers.other]\ncommand = 'other'\n"),
            (
                Assistant::OpenClaw,
                "{ // commentaire\n agents: { defaults: {} }, mcp: { servers: { other: { command: 'other' } } } }",
            ),
            (Assistant::Hermes, "model: example\nmcp_servers:\n  other:\n    command: other\n"),
        ] {
            let mut doc = Document::parse(assistant, initial).unwrap();
            let entry =
                desired(assistant, Path::new("C:/Program Files/Polyptyque/current/polyptyque-cli.exe"), None).unwrap();
            doc.set(assistant, Some(entry.clone())).unwrap();
            let rendered = doc.render().unwrap();
            let mut parsed = Document::parse(assistant, &rendered).unwrap();
            assert_eq!(parsed.entry(assistant).unwrap(), Some(entry));
            assert!(rendered.contains("other"));
            if assistant == Assistant::Codex {
                assert!(rendered.contains("# conserver"));
            }
            parsed.set(assistant, None).unwrap();
            assert_eq!(parsed.entry(assistant).unwrap(), None);
            assert!(parsed.render().unwrap().contains("other"));
        }
    }

    #[test]
    fn corrupt_configurations_and_includes_are_rejected() {
        for (assistant, text) in [
            (Assistant::ClaudeDesktop, "["),
            (Assistant::ClaudeCode, "null"),
            (Assistant::Codex, "mcp_servers = 2"),
            (Assistant::Hermes, "mcp_servers: []"),
            (Assistant::OpenClaw, "{mcp: {$include: 'other.json'}}"),
            (Assistant::OpenClaw, "{mcp: {servers: false}}"),
        ] {
            assert!(Document::parse(assistant, text).is_err(), "{assistant:?}");
        }
    }

    #[test]
    fn conflict_backup_repair_and_concurrent_changes() {
        let dir = std::env::temp_dir().join(format!("polyptyque-assistants-{}", new_id()));
        fs::create_dir_all(&dir).unwrap();
        let cli = dir.join("polyptyque-cli.exe");
        fs::write(&cli, "").unwrap();
        let target = Target { assistant: Assistant::ClaudeDesktop, path: dir.join("client.json"), detected: true };
        let initial = r#"{"mcpServers":{"polyptyque":{"command":"someone-else","args":["mcp"]}}}"#;
        fs::write(&target.path, initial).unwrap();
        assert_eq!(inspect(target.clone(), &cli).state, State::Conflict);
        assert!(configure(&target, &cli, Action::Connect).is_err());
        assert!(configure(&target, &cli, Action::Disconnect).is_err());
        assert_eq!(fs::read_to_string(&target.path).unwrap(), initial);
        fs::write(&target.path, "{\"custom\":42}").unwrap();
        configure(&target, &cli, Action::Connect).unwrap();
        assert_eq!(inspect(target.clone(), &cli).state, State::Configured);
        assert!(fs::read_dir(&dir).unwrap().any(|e| e.unwrap().path().extension().is_some_and(|e| e == "bak")));
        let other_cli = dir.join("moved/polyptyque-cli.exe");
        fs::create_dir_all(other_cli.parent().unwrap()).unwrap();
        fs::write(&other_cli, "").unwrap();
        assert_eq!(inspect(target.clone(), &other_cli).state, State::Repair);
        configure(&target, &other_cli, Action::Connect).unwrap();
        assert_eq!(inspect(target.clone(), &other_cli).state, State::Configured);
        assert!(replace(&target.path, Some("stale"), "{}").is_err());
        configure(&target, &cli, Action::Disconnect).unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(&target.path).unwrap()).unwrap();
        assert_eq!(value["custom"], 42);
        assert_eq!(inspect(target, &cli).state, State::Available);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn repair_preserves_env_filters_and_policy() {
        let previous = json!({"command":"C:/old/polyptyque-cli.exe","args":["mcp"],"enabled":false,"env":{"KEEP":"yes"},"disabled_tools":["app_quit"]});
        assert!(ours(&previous));
        let expected = desired(Assistant::Codex, Path::new("C:/new/polyptyque-cli.exe"), Some(previous)).unwrap();
        assert_eq!(expected["env"]["KEEP"], "yes");
        assert_eq!(expected["disabled_tools"], json!(["app_quit"]));
        assert_eq!(expected["enabled"], true);
    }
}
