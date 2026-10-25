//! Configuration persistante : scènes, sources et réglages.

use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tr::tr;

use crate::browsers::program_name;
use crate::naming;
use crate::paths;
use crate::platform::{BrowserInstall, WindowInfo};
use crate::schedule::Schedule;
use crate::storage;

/// 2 : sources rangées à part, les scènes n'en gardent que les identifiants
/// (1 : chaque scène contenait ses sources).
pub const CONFIG_VERSION: u32 = 2;

/// Fréquence d'échantillonnage utilisée pour tout l'audio.
pub const SAMPLE_RATE: u32 = 48_000;
/// Nombre de canaux audio (stéréo).
pub const CHANNELS: u16 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub active_scene: String,
    pub settings: Settings,
    pub scenes: Vec<Scene>,
    /// Toutes les sources, une seule fois chacune. Les scènes n'en gardent
    /// que les identifiants : une même source (même fenêtre, même profil de
    /// navigateur) peut ainsi figurer dans plusieurs scènes.
    pub sources: Vec<Source>,
    /// Programmations des enregistrements (chacune lance une scène).
    pub schedules: Vec<Schedule>,
}

impl Default for Config {
    fn default() -> Self {
        let scene = Scene::new(&tr!("Scène principale"));
        Self {
            version: CONFIG_VERSION,
            active_scene: scene.id.clone(),
            settings: Settings::default(),
            scenes: vec![scene],
            sources: Vec::new(),
            schedules: Vec::new(),
        }
    }
}

impl Config {
    /// Charge la configuration. Fichier absent : configuration par défaut.
    /// Fichier abîmé ou vide (coupure de courant, disque) : une copie en est
    /// gardée, puis la sauvegarde la plus récente encore valide est
    /// restaurée ; à défaut, configuration par défaut. Le message éventuel
    /// est à montrer à l'utilisateur.
    pub fn load_or_default(path: &Path) -> (Config, Option<String>) {
        let problem = match fs::read_to_string(path) {
            Ok(text) => match parse(&text) {
                Ok(config) => return (config, None),
                Err(e) => {
                    let damaged = storage::sibling(path, ".invalide");
                    let _ = fs::copy(path, &damaged);
                    // Erreur du lecteur de réglages, en anglais : au journal.
                    log::warn!("réglages abîmés : {e:#}");
                    tr!("Réglages abîmés (détail dans le journal). Une copie a été gardée dans {}.", damaged.display())
                }
            },
            Err(e) if e.kind() == ErrorKind::NotFound => return (Config::default(), None),
            Err(e) => tr!("Réglages illisibles ({}).", e),
        };
        for backup in backups(path) {
            let Ok(config) = fs::read_to_string(&backup).map_err(anyhow::Error::from).and_then(|t| parse(&t)) else {
                continue;
            };
            let name = backup.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let _ = config.save(path);
            return (config, Some(format!("{problem} {}", tr!("La sauvegarde « {} » a été restaurée.", name))));
        }
        (Config::default(), Some(format!("{problem} {}", tr!("Aucune sauvegarde valide : réglages par défaut."))))
    }

    /// Enregistre la configuration sans jamais laisser un fichier à moitié
    /// écrit. La version précédente est gardée (`config.toml.precedent`),
    /// ainsi qu'une copie par jour dans `sauvegardes` (les cinq derniers
    /// jours).
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self).context(tr!("sérialisation de la configuration"))?;
        if let Ok(current) = fs::read_to_string(path) {
            // Une version abîmée ne remplace jamais une bonne sauvegarde.
            if parse(&current).is_ok() && current != text {
                let _ = storage::write_atomic(&storage::sibling(path, ".precedent"), current.as_bytes());
                backup_daily(path, &current);
            }
        }
        storage::write_atomic(path, text.as_bytes())
    }

    /// Garantit les invariants : au moins une scène, une scène active valide,
    /// chaque source une seule fois, et au plus une fois par scène, des
    /// programmations qui lancent des scènes existantes (celles d'une
    /// scène supprimée disparaissent avec elle), des réglages dans les
    /// bornes.
    pub fn normalize(&mut self) {
        self.version = CONFIG_VERSION;
        if self.scenes.is_empty() {
            self.scenes.push(Scene::new(&tr!("Scène principale")));
        }
        if !self.scenes.iter().any(|s| s.id == self.active_scene) {
            self.active_scene = self.scenes[0].id.clone();
        }
        let mut known = HashSet::new();
        self.sources.retain(|source| known.insert(source.id.clone()));
        // Deux fois la même source dans une scène enregistrerait deux fois le
        // même programme, avec le même son.
        for scene in &mut self.scenes {
            let mut seen = HashSet::new();
            scene.source_ids.retain(|id| known.contains(id) && seen.insert(id.clone()));
        }
        // Une source qui n'est plus dans aucune scène disparaît ; son profil
        // de navigateur reste sur le disque.
        let used: HashSet<&String> = self.scenes.iter().flat_map(|s| &s.source_ids).collect();
        self.sources.retain(|source| used.contains(&source.id));
        let scenes = &self.scenes;
        self.schedules.retain(|schedule| scenes.iter().any(|scene| scene.id == schedule.scene_id));
        self.settings.video.normalize();
        self.settings.audio.normalize();
        if self.settings.filename_template.trim().is_empty() {
            self.settings.filename_template = naming::default_template();
        }
    }

    /// Vrai si le navigateur d'une source n'est plus à l'emplacement
    /// enregistré (autre PC, navigateur réinstallé ailleurs).
    pub fn has_missing_browser(&self) -> bool {
        self.sources.iter().any(|source| match &source.kind {
            SourceKind::Browser { browser, .. } => !browser.exe_path.exists(),
            SourceKind::Application { .. } => false,
        })
    }

    /// Après un changement de PC ou de dossier (version portable) : chaque
    /// profil de navigateur est cherché dans le dossier de profils actuel,
    /// et un navigateur introuvable est remplacé par le même navigateur
    /// installé ailleurs. Renvoie vrai si quelque chose a changé.
    pub fn relocate(&mut self, profiles_root: &Path, browsers: &[BrowserInstall]) -> bool {
        let mut changed = false;
        for source in &mut self.sources {
            let SourceKind::Browser { profile_dir, browser, .. } = &mut source.kind else {
                continue;
            };
            // Profil créé par Polyptyque : son dossier porte l'identifiant de
            // la source.
            let here = profiles_root.join(&source.id);
            let ours = profile_dir.file_name().is_some_and(|n| n.to_string_lossy() == source.id);
            if ours && *profile_dir != here && (here.exists() || !profile_dir.exists()) {
                *profile_dir = here;
                changed = true;
            }
            if !browser.exe_path.exists() {
                let found = browsers
                    .iter()
                    .find(|b| b.name == browser.name && b.family == browser.family && b.exe_path != browser.exe_path);
                if let Some(found) = found {
                    browser.exe_path = found.exe_path.clone();
                    changed = true;
                }
            }
        }
        changed
    }

    pub fn active_scene(&self) -> &Scene {
        self.scenes.iter().find(|s| s.id == self.active_scene).unwrap_or(&self.scenes[0])
    }

    pub fn scene(&self, id: &str) -> Option<&Scene> {
        self.scenes.iter().find(|s| s.id == id)
    }

    pub fn active_scene_mut(&mut self) -> &mut Scene {
        let index = self.scenes.iter().position(|s| s.id == self.active_scene).unwrap_or(0);
        &mut self.scenes[index]
    }

    pub fn source(&self, id: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.id == id)
    }

    /// Sources d'une scène, dans son ordre.
    pub fn scene_sources(&self, scene: &Scene) -> Vec<Source> {
        scene.source_ids.iter().filter_map(|id| self.source(id)).cloned().collect()
    }

    /// Sources de la scène active, dans son ordre.
    pub fn active_sources(&self) -> Vec<Source> {
        self.scene_sources(self.active_scene())
    }

    /// Scènes qui contiennent cette source.
    pub fn scenes_with(&self, source_id: &str) -> Vec<&Scene> {
        self.scenes.iter().filter(|s| s.contains(source_id)).collect()
    }

    /// Ajoute une nouvelle source à la fin de la scène active.
    pub fn add_source(&mut self, source: Source) {
        self.active_scene_mut().source_ids.push(source.id.clone());
        self.sources.push(source);
    }

    /// Colle une source existante à la fin de la scène active : c'est la
    /// même source (même fenêtre, même profil), pas une copie.
    pub fn paste_source(&mut self, source_id: &str) -> Result<(), PasteError> {
        if self.source(source_id).is_none() {
            return Err(PasteError::Missing);
        }
        if self.active_scene().contains(source_id) {
            return Err(PasteError::AlreadyInScene);
        }
        self.active_scene_mut().source_ids.push(source_id.to_string());
        Ok(())
    }

    /// Déplace la source de la position `from` à la position `to` dans la
    /// scène active ; les autres se décalent. Faux si rien ne change.
    pub fn move_source(&mut self, from: usize, to: usize) -> bool {
        let ids = &mut self.active_scene_mut().source_ids;
        if from == to || from >= ids.len() || to >= ids.len() {
            return false;
        }
        let id = ids.remove(from);
        ids.insert(to, id);
        true
    }

    /// Retire une source de la scène active seulement. Si elle n'est plus
    /// dans aucune scène, elle disparaît (son profil reste sur le disque).
    pub fn remove_source(&mut self, source_id: &str) {
        self.active_scene_mut().source_ids.retain(|id| id != source_id);
        self.normalize();
    }

    /// Nouvelle scène avec les mêmes sources (par référence, dans le même
    /// ordre), qui devient la scène active. Ses programmations ne sont pas
    /// reprises. Renvoie l'identifiant de la nouvelle scène.
    pub fn duplicate_scene(&mut self, scene_id: &str, name: &str) -> Option<String> {
        let source_ids = self.scene(scene_id)?.source_ids.clone();
        let scene = Scene { source_ids, ..Scene::new(name) };
        let id = scene.id.clone();
        self.scenes.push(scene);
        self.active_scene = id.clone();
        Some(id)
    }

    /// Sources de cette scène qui ne sont dans aucune autre : elles
    /// disparaîtraient avec elle.
    pub fn sources_only_in(&self, scene_id: &str) -> Vec<&Source> {
        let Some(scene) = self.scene(scene_id) else {
            return Vec::new();
        };
        scene
            .source_ids
            .iter()
            .filter(|id| !self.scenes.iter().any(|other| other.id != scene_id && other.contains(id)))
            .filter_map(|id| self.source(id))
            .collect()
    }
}

/// Collage d'une source refusé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteError {
    /// La source copiée a été supprimée entre-temps.
    Missing,
    /// La scène la contient déjà.
    AlreadyInScene,
}

/// Jours de sauvegarde quotidienne des réglages conservés.
const BACKUP_DAYS: usize = 5;

fn parse(text: &str) -> Result<Config> {
    // Un fichier vide serait lu comme une configuration par défaut : toutes
    // les scènes disparaîtraient sans un mot.
    if text.trim().is_empty() {
        anyhow::bail!(tr!("fichier vide"));
    }
    let mut table: toml::Table = toml::from_str(text)?;
    lift_scene_sources(&mut table);
    let mut config: Config = table.try_into()?;
    config.normalize();
    Ok(config)
}

/// Version 1 : chaque scène contenait ses sources en entier. Elles passent
/// dans la liste commune (`[[sources]]`), et la scène n'en garde que les
/// identifiants, dans le même ordre. Sans effet sur un fichier récent.
fn lift_scene_sources(table: &mut toml::Table) {
    let mut lifted = Vec::new();
    if let Some(toml::Value::Array(scenes)) = table.get_mut("scenes") {
        for scene in scenes.iter_mut().filter_map(toml::Value::as_table_mut) {
            let Some(toml::Value::Array(entries)) = scene.get_mut("sources") else {
                continue;
            };
            for entry in entries.iter_mut() {
                let id = entry.as_table().and_then(|source| source.get("id")).and_then(toml::Value::as_str);
                if let Some(id) = id.map(str::to_string) {
                    lifted.push(std::mem::replace(entry, toml::Value::String(id)));
                }
            }
        }
    }
    if lifted.is_empty() {
        return;
    }
    match table.entry("sources").or_insert_with(|| toml::Value::Array(Vec::new())) {
        toml::Value::Array(sources) => sources.extend(lifted),
        other => *other = toml::Value::Array(lifted),
    }
}

fn backup_dir(path: &Path) -> PathBuf {
    path.parent().unwrap_or(Path::new(".")).join("sauvegardes")
}

/// Sauvegardes possibles, de la plus récente à la plus ancienne.
fn backups(path: &Path) -> Vec<PathBuf> {
    let mut daily: Vec<PathBuf> = fs::read_dir(backup_dir(path))
        .map(|entries| entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| is_daily_backup(p)).collect())
        .unwrap_or_default();
    daily.sort();
    daily.reverse();
    std::iter::once(storage::sibling(path, ".precedent")).chain(daily).collect()
}

fn is_daily_backup(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    name.starts_with("config_") && name.ends_with(".toml")
}

/// Garde une copie par jour (la première version du jour), et efface les
/// plus anciennes.
fn backup_daily(path: &Path, text: &str) {
    let dir = backup_dir(path);
    let (date, _) = naming::now_parts();
    let today = dir.join(format!("config_{date}.toml"));
    if today.exists() {
        return;
    }
    if storage::write_atomic(&today, text.as_bytes()).is_err() {
        return;
    }
    let mut daily: Vec<PathBuf> = backups(path).into_iter().skip(1).collect();
    for old in daily.drain(BACKUP_DAYS.min(daily.len())..) {
        let _ = fs::remove_file(old);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scene {
    pub id: String,
    pub name: String,
    /// Identifiants des sources de la scène, dans leur ordre (les sources
    /// elles-mêmes sont dans `Config::sources`).
    #[serde(default, rename = "sources")]
    pub source_ids: Vec<String>,
}

impl Scene {
    pub fn new(name: &str) -> Self {
        Self { id: new_id(), name: name.to_string(), source_ids: Vec::new() }
    }

    pub fn contains(&self, source_id: &str) -> bool {
        self.source_ids.iter().any(|id| id == source_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub capture_cursor: bool,
    /// Relancer le programme s'il n'est pas ouvert (toujours vrai pour un
    /// navigateur géré).
    #[serde(default)]
    pub relaunch: bool,
    pub kind: SourceKind,
}

impl Source {
    /// Source navigateur avec un profil dédié, rangé dans `profiles_root`.
    pub fn browser(name: &str, browser: BrowserRef, start_url: Option<&str>, profiles_root: &Path) -> Self {
        let id = new_id();
        Self {
            name: name.trim().to_string(),
            capture_cursor: true,
            relaunch: true,
            kind: SourceKind::Browser {
                profile_dir: profiles_root.join(&id),
                start_url: start_url.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()),
                browser,
            },
            id,
        }
    }

    /// Source application d'après une fenêtre ouverte.
    pub fn application(name: &str, window: &WindowInfo, relaunch: bool, capture_cursor: bool) -> Self {
        Self {
            id: new_id(),
            name: name.trim().to_string(),
            capture_cursor,
            relaunch,
            kind: SourceKind::Application {
                exe_path: window.exe.clone(),
                window_class: window.class.clone(),
                window_title: window.title.clone(),
                launch_args: Vec::new(),
            },
        }
    }

    pub fn should_launch(&self) -> bool {
        match self.kind {
            SourceKind::Browser { .. } => true,
            SourceKind::Application { .. } => self.relaunch,
        }
    }

    /// Description courte : « Navigateur Firefox », « Application Discord ».
    pub fn kind_label(&self) -> String {
        match &self.kind {
            // Contexte : « Navigateur {} » sert aussi de nom par défaut d'une
            // source (« Navigateur 3 »), traduit autrement.
            SourceKind::Browser { browser, .. } => tr!("Type de source" => "Navigateur {}", browser.name),
            SourceKind::Application { exe_path, .. } => {
                tr!("Type de source" => "Application {}", program_name(exe_path).unwrap_or_default())
            }
        }
    }

    pub fn is_browser(&self) -> bool {
        matches!(self.kind, SourceKind::Browser { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceKind {
    /// Navigateur lancé par Polyptyque avec un profil qui lui est dédié.
    Browser {
        profile_dir: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start_url: Option<String>,
        browser: BrowserRef,
    },
    /// Toute autre fenêtre (Discord, jeu, Tor Browser…).
    Application {
        exe_path: PathBuf,
        window_class: String,
        window_title: String,
        #[serde(default)]
        launch_args: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserRef {
    pub name: String,
    pub exe_path: PathBuf,
    pub family: BrowserFamily,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserFamily {
    /// Firefox et ses dérivés.
    Gecko,
    /// Chrome, Edge, Brave, Vivaldi, Opera…
    Chromium,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub output_dir: PathBuf,
    pub filename_template: String,
    /// Aperçus en direct des sources dans l'interface.
    pub previews: bool,
    /// Conversion automatique en MP4 à la fin de chaque enregistrement
    /// (le MKV d'origine est conservé).
    pub convert_to_mp4: bool,
    /// Recherche d'une nouvelle version au démarrage (sur GitHub).
    pub check_updates: bool,
    /// Versions proposées. Absent tant que l'utilisateur n'a rien choisi :
    /// c'est alors celui de la version en cours (voir `UpdateChannel`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_channel: Option<UpdateChannel>,
    /// Au lancement de Polyptyque, ouvrir les fenêtres manquantes de la
    /// scène active (navigateurs, applications dont la relance est activée).
    pub open_windows_at_launch: bool,
    /// En quittant Polyptyque, fermer les fenêtres des navigateurs des
    /// sources (profils dédiés, toutes scènes confondues). Les applications
    /// ne sont jamais fermées.
    pub close_browsers_at_exit: bool,
    /// Lancer Polyptyque à l'ouverture de la session Windows (réduit), pour
    /// que les programmations survivent à un redémarrage.
    pub launch_at_startup: bool,
    /// Fermeture de la fenêtre alors qu'une programmation doit encore
    /// lancer un enregistrement.
    pub close_with_schedules: CloseChoice,
    /// Langue de l'interface (voir « Langues et formats »).
    pub language: LanguageChoice,
    /// Heures affichées et saisies sur 24 h ou 12 h.
    pub clock: ClockChoice,
    pub video: VideoSettings,
    pub audio: AudioSettings,
    pub microphone: MicrophoneSettings,
    pub hotkeys: HotkeySettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            output_dir: paths::default_output_dir(),
            filename_template: naming::default_template(),
            previews: true,
            convert_to_mp4: false,
            check_updates: true,
            update_channel: None,
            open_windows_at_launch: true,
            close_browsers_at_exit: true,
            launch_at_startup: false,
            close_with_schedules: CloseChoice::Ask,
            language: LanguageChoice::Auto,
            clock: ClockChoice::Auto,
            video: VideoSettings::default(),
            audio: AudioSettings::default(),
            microphone: MicrophoneSettings::default(),
            hotkeys: HotkeySettings::default(),
        }
    }
}

impl Settings {
    /// Canal de mises à jour en vigueur pour la version `version` de
    /// Polyptyque.
    pub fn update_channel(&self, version: &str) -> UpdateChannel {
        self.update_channel.unwrap_or_else(|| UpdateChannel::of_version(version))
    }
}

/// Langue de l'interface : celle de Windows (le français si Windows est en
/// français, l'anglais sinon), ou imposée.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LanguageChoice {
    #[default]
    Auto,
    French,
    English,
}

/// Horloge : celle du format régional de Windows, ou imposée.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClockChoice {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "24h")]
    H24,
    #[serde(rename = "12h")]
    H12,
}

/// Fermeture de la fenêtre alors qu'une programmation doit encore lancer
/// un enregistrement : Polyptyque doit rester ouvert pour cela.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CloseChoice {
    /// Poser la question à chaque fois.
    #[default]
    Ask,
    /// Continuer en arrière-plan, dans la zone de notification.
    Background,
    /// Quitter, sans autre avertissement que ceux d'un enregistrement ou
    /// d'une conversion en cours.
    Quit,
}

/// Versions proposées par la recherche de mises à jour.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateChannel {
    /// Versions stables seulement.
    Stable,
    /// Versions stables et bêta (préversions GitHub).
    Beta,
}

impl UpdateChannel {
    /// Canal par défaut : une bêta reçoit les bêtas suivantes, puis la
    /// version stable ; une version stable ne reçoit que des versions
    /// stables. Une bêta se reconnaît au tiret de son numéro (« 1.0.0-beta.1 »).
    pub fn of_version(version: &str) -> Self {
        let without_build = version.split('+').next().unwrap_or(version);
        if without_build.contains('-') {
            Self::Beta
        } else {
            Self::Stable
        }
    }
}

/// Raccourcis clavier globaux, au format « Ctrl+Maj+F9 » (vide : aucun).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HotkeySettings {
    pub toggle_recording: String,
    pub show_window: String,
    pub toggle_previews: String,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self {
            toggle_recording: "Ctrl+Maj+F9".into(),
            show_window: "Ctrl+Maj+F10".into(),
            toggle_previews: "Ctrl+Maj+F11".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: Codec,
    pub hardware: HardwarePreference,
    pub bitrate_kbps: u32,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 60,
            codec: Codec::H264,
            hardware: HardwarePreference::Auto,
            bitrate_kbps: 12_000,
        }
    }
}

impl VideoSettings {
    /// Ramène les valeurs dans des bornes sûres. Le format NV12 impose des
    /// dimensions paires.
    pub fn normalize(&mut self) {
        self.width = (self.width.clamp(64, 7680)) & !1;
        self.height = (self.height.clamp(64, 4320)) & !1;
        self.fps = self.fps.clamp(1, 240);
        self.bitrate_kbps = self.bitrate_kbps.clamp(250, 500_000);
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Codec {
    H264,
    Hevc,
}

impl Codec {
    pub fn label(self) -> &'static str {
        match self {
            Codec::H264 => "H.264",
            Codec::Hevc => "HEVC",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HardwarePreference {
    /// Carte graphique si disponible, sinon processeur.
    Auto,
    /// Carte graphique uniquement.
    Gpu,
    /// Processeur uniquement.
    Cpu,
}

impl HardwarePreference {
    pub fn label(self) -> String {
        match self {
            HardwarePreference::Auto => tr!("Automatique (carte graphique si disponible)"),
            HardwarePreference::Gpu => tr!("Carte graphique"),
            HardwarePreference::Cpu => tr!("Processeur"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    pub bitrate_kbps: u32,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self { bitrate_kbps: 192 }
    }
}

impl AudioSettings {
    pub fn normalize(&mut self) {
        self.bitrate_kbps = self.bitrate_kbps.clamp(64, 512);
    }
}

/// Micro enregistré dans un fichier séparé. Désactivé par défaut : il
/// faut le choisir explicitement.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MicrophoneSettings {
    pub enabled: bool,
    /// Identifiant du périphérique ; vide pour le micro par défaut de Windows.
    pub device_id: String,
    /// Nom affiché du périphérique choisi (pour le reconnaître s'il est
    /// débranché).
    pub device_name: String,
}

impl MicrophoneSettings {
    pub fn device(&self) -> Option<&str> {
        Some(self.device_id.as_str()).filter(|id| !id.is_empty())
    }

    pub fn label(&self) -> &str {
        if self.device_id.is_empty() || self.device_name.is_empty() {
            "micro par défaut"
        } else {
            &self.device_name
        }
    }
}

fn default_true() -> bool {
    true
}

/// Identifiant court et unique (horodatage + compteur), suffisant pour
/// distinguer scènes, sources et dossiers de profil.
pub fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:x}{:02x}", n & 0xff)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Config {
        let mut config = Config::default();
        config.add_source(Source {
            id: new_id(),
            name: "Firefox 1".into(),
            capture_cursor: true,
            relaunch: true,
            kind: SourceKind::Browser {
                profile_dir: PathBuf::from(r"C:\Users\x\AppData\Local\Polyptyque\profils\abc"),
                start_url: Some("https://example.org".into()),
                browser: BrowserRef {
                    name: "Firefox".into(),
                    exe_path: PathBuf::from(r"C:\Program Files\Mozilla Firefox\firefox.exe"),
                    family: BrowserFamily::Gecko,
                },
            },
        });
        config.add_source(Source {
            id: new_id(),
            name: "Discord".into(),
            capture_cursor: false,
            relaunch: false,
            kind: SourceKind::Application {
                exe_path: PathBuf::from(r"C:\Users\x\AppData\Local\Discord\app-1.0\Discord.exe"),
                window_class: "Chrome_WidgetWin_1".into(),
                window_title: "Discord".into(),
                launch_args: vec![],
            },
        });
        config
    }

    #[test]
    fn round_trip_toml() {
        let config = sample();
        let text = toml::to_string_pretty(&config).unwrap();
        let back = parse(&text).unwrap();
        assert_eq!(back.scenes.len(), 1);
        let sources = back.active_sources();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].name, "Firefox 1");
        assert!(matches!(sources[1].kind, SourceKind::Application { .. }));
    }

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(format!("polyptyque-test-{}", new_id()));
        let path = dir.join("config.toml");
        let config = sample();
        config.save(&path).unwrap();
        let (loaded, warning) = Config::load_or_default(&path);
        assert!(warning.is_none());
        assert_eq!(loaded.active_sources().len(), 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_file_is_kept_aside() {
        let dir = std::env::temp_dir().join(format!("polyptyque-test-{}", new_id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(&path, "ceci n'est pas du toml = = =").unwrap();
        let (loaded, warning) = Config::load_or_default(&path);
        assert!(warning.is_some());
        assert_eq!(loaded.scenes.len(), 1);
        assert!(dir.join("config.toml.invalide").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn damaged_file_is_restored_from_backup() {
        let dir = std::env::temp_dir().join(format!("polyptyque-test-{}", new_id()));
        let path = dir.join("config.toml");
        let mut config = sample();
        config.save(&path).unwrap();
        config.active_scene_mut().name = "Modifiée".into();
        config.save(&path).unwrap();
        assert!(dir.join("config.toml.precedent").exists());
        assert_eq!(fs::read_dir(dir.join("sauvegardes")).unwrap().count(), 1);
        // Coupure de courant : fichier vide.
        fs::write(&path, "").unwrap();
        let (loaded, warning) = Config::load_or_default(&path);
        assert!(warning.unwrap().contains("restaurée"));
        assert_eq!(loaded.active_sources().len(), 2);
        // Le fichier restauré est réécrit.
        assert!(!fs::read_to_string(&path).unwrap().trim().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn daily_backups_are_pruned() {
        let dir = std::env::temp_dir().join(format!("polyptyque-test-{}", new_id()));
        let backups = dir.join("sauvegardes");
        fs::create_dir_all(&backups).unwrap();
        for day in 1..=8 {
            fs::write(backups.join(format!("config_2020-01-0{day}.toml")), "x").unwrap();
        }
        let path = dir.join("config.toml");
        let mut config = sample();
        config.save(&path).unwrap();
        config.active_scene_mut().name = "Modifiée".into();
        config.save(&path).unwrap();
        let mut names: Vec<String> =
            fs::read_dir(&backups).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names.len(), BACKUP_DAYS);
        assert_eq!(names[0], "config_2020-01-05.toml");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn profiles_and_browsers_follow_the_portable_folder() {
        let old_root = Path::new("/ancien-pc/Polyptyque/données/profils");
        let dir = std::env::temp_dir().join(format!("polyptyque-test-{}", new_id()));
        let firefox =
            BrowserRef { name: "Firefox".into(), exe_path: dir.join("absent.exe"), family: BrowserFamily::Gecko };
        let mut config = Config::default();
        let source = Source::browser("Concert", firefox, None, old_root);
        let id = source.id.clone();
        config.add_source(source);
        assert!(config.has_missing_browser());
        let installed = vec![BrowserInstall {
            name: "Firefox".into(),
            exe_path: PathBuf::from("C:/Program Files/Mozilla Firefox/firefox.exe"),
            family: BrowserFamily::Gecko,
        }];
        let profiles = dir.join("profils");
        assert!(config.relocate(&profiles, &installed));
        let SourceKind::Browser { profile_dir, browser, .. } = &config.sources[0].kind else {
            panic!("source navigateur attendue");
        };
        assert_eq!(profile_dir, &profiles.join(&id));
        assert_eq!(browser.exe_path, installed[0].exe_path);
        // Une seconde fois : rien à changer.
        assert!(!config.relocate(&profiles, &installed));
    }

    #[test]
    fn normalize_fixes_invariants() {
        let mut config = Config { scenes: vec![], active_scene: "inexistante".into(), ..Default::default() };
        config.settings.video.width = 1921;
        config.settings.video.fps = 0;
        config.normalize();
        assert_eq!(config.scenes.len(), 1);
        assert_eq!(config.active_scene, config.scenes[0].id);
        assert_eq!(config.settings.video.width, 1920);
        assert_eq!(config.settings.video.fps, 1);
    }

    #[test]
    fn schedules_follow_their_scene() {
        use crate::schedule::{Repeat, TimeOfDay};
        let mut config = sample();
        let scene = config.active_scene.clone();
        let start = TimeOfDay::new(21, 0).unwrap();
        let end = TimeOfDay::new(22, 0).unwrap();
        config.schedules.push(Schedule::new(&scene, start, end, Repeat::Weekly { days: vec![1, 2, 3] }));
        let text = toml::to_string_pretty(&config).unwrap();
        let mut back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.schedules, config.schedules);
        // Scène supprimée : ses programmations aussi.
        back.scenes.push(Scene::new("Autre"));
        back.scenes.retain(|s| s.id != scene);
        back.normalize();
        assert!(back.schedules.is_empty());
    }

    #[test]
    fn source_constructors() {
        let browser = BrowserRef {
            name: "Firefox".into(),
            exe_path: PathBuf::from(r"C:\ff\firefox.exe"),
            family: BrowserFamily::Gecko,
        };
        let source = Source::browser(" Firefox A ", browser, Some("  "), Path::new("/profils"));
        assert_eq!(source.name, "Firefox A");
        assert_eq!(source.kind_label(), "Navigateur Firefox");
        match &source.kind {
            SourceKind::Browser { profile_dir, start_url, .. } => {
                assert_eq!(profile_dir, &Path::new("/profils").join(&source.id));
                assert!(start_url.is_none());
            }
            _ => panic!("source navigateur attendue"),
        }
        let window = WindowInfo {
            handle: 1,
            pid: 2,
            title: "Discord".into(),
            class: "Chrome_WidgetWin_1".into(),
            exe: PathBuf::from(r"C:\d\Discord.exe"),
        };
        let app = Source::application("Discord", &window, false, true);
        assert_eq!(app.kind_label(), "Application Discord");
        assert!(!app.should_launch());
    }

    #[test]
    fn older_config_gets_new_defaults() {
        let text = "version = 1\nactive_scene = \"x\"\n[settings]\noutput_dir = \"/tmp\"\n";
        let config: Config = toml::from_str(text).unwrap();
        assert!(config.settings.previews);
        assert_eq!(config.settings.hotkeys.toggle_recording, "Ctrl+Maj+F9");
        assert_eq!(config.settings.update_channel, None);
        assert!(!config.settings.launch_at_startup);
        assert_eq!(config.settings.close_with_schedules, CloseChoice::Ask);
        assert!(config.schedules.is_empty());
    }

    #[test]
    fn close_choice_is_written_in_words() {
        let mut settings = Settings { close_with_schedules: CloseChoice::Background, ..Settings::default() };
        let text = toml::to_string(&settings).unwrap();
        assert!(text.contains("close_with_schedules = \"background\""));
        settings = toml::from_str(&text.replace("\"background\"", "\"quit\"")).unwrap();
        assert_eq!(settings.close_with_schedules, CloseChoice::Quit);
    }

    #[test]
    fn update_channel_follows_the_version_until_chosen() {
        assert_eq!(UpdateChannel::of_version("1.0.0-beta.1"), UpdateChannel::Beta);
        assert_eq!(UpdateChannel::of_version("1.0.0"), UpdateChannel::Stable);
        assert_eq!(UpdateChannel::of_version("1.0.0+abc-def"), UpdateChannel::Stable);
        let mut settings = Settings::default();
        assert_eq!(settings.update_channel("1.0.0-beta.2"), UpdateChannel::Beta);
        assert_eq!(settings.update_channel("1.0.0"), UpdateChannel::Stable);
        // Rien n'est écrit tant que l'utilisateur n'a pas choisi.
        assert!(!toml::to_string(&settings).unwrap().contains("update_channel"));
        settings.update_channel = Some(UpdateChannel::Stable);
        assert_eq!(settings.update_channel("1.0.0-beta.2"), UpdateChannel::Stable);
        let back: Settings = toml::from_str(&toml::to_string(&settings).unwrap()).unwrap();
        assert_eq!(back.update_channel, Some(UpdateChannel::Stable));
    }

    /// Fichier écrit par la version 1 (sources rangées dans les scènes),
    /// avec la même source navigateur dans deux scènes.
    const VERSION_1: &str = r#"
version = 1
active_scene = "s2"

[[scenes]]
id = "s1"
name = "Podcast"

[[scenes.sources]]
id = "a"
name = "Concert"
capture_cursor = true
relaunch = true

[scenes.sources.kind]
type = "browser"
profile_dir = 'C:\Polyptyque\profils\a'

[scenes.sources.kind.browser]
name = "Firefox"
exe_path = 'C:\Program Files\Mozilla Firefox\firefox.exe'
family = "gecko"

[[scenes.sources]]
id = "b"
name = "Discord"
capture_cursor = false
relaunch = false

[scenes.sources.kind]
type = "application"
exe_path = 'C:\Discord\Discord.exe'
window_class = "Chrome_WidgetWin_1"
window_title = "Discord"

[[scenes]]
id = "s2"
name = "Archive"
"#;

    #[test]
    fn version_1_sources_move_out_of_their_scenes() {
        let config = parse(VERSION_1).unwrap();
        assert_eq!(config.version, CONFIG_VERSION);
        assert_eq!(config.active_scene, "s2");
        let ids: Vec<&str> = config.sources.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
        assert_eq!(config.scenes[0].source_ids, ["a", "b"]);
        assert!(config.scenes[1].source_ids.is_empty());
        let SourceKind::Browser { profile_dir, .. } = &config.sources[0].kind else {
            panic!("source navigateur attendue");
        };
        assert_eq!(profile_dir, Path::new(r"C:\Polyptyque\profils\a"));
        assert!(!config.sources[1].capture_cursor);
        // Réécrit au nouveau format, relu à l'identique.
        let text = toml::to_string_pretty(&config).unwrap();
        assert!(text.contains("version = 2"));
        let back = parse(&text).unwrap();
        assert_eq!(back.scenes[0].source_ids, ["a", "b"]);
        assert_eq!(back.sources.len(), 2);
    }

    #[test]
    fn a_pasted_source_is_the_same_source() {
        let mut config = sample();
        let podcast = config.active_scene.clone();
        let firefox = config.sources[0].id.clone();
        config.scenes.push(Scene::new("Archive"));
        config.active_scene = config.scenes[1].id.clone();
        config.paste_source(&firefox).unwrap();
        config.normalize();
        // Une seule source, dans les deux scènes.
        assert_eq!(config.sources.len(), 2);
        assert_eq!(config.active_sources()[0].id, firefox);
        assert_eq!(config.scenes_with(&firefox).len(), 2);
        // La renommer dans une scène la renomme partout.
        config.sources[0].name = "Concert".into();
        assert_eq!(config.scene_sources(config.scene(&podcast).unwrap())[0].name, "Concert");
        // Jamais deux fois dans la même scène.
        assert_eq!(config.paste_source(&firefox), Err(PasteError::AlreadyInScene));
        assert_eq!(config.paste_source("inconnue"), Err(PasteError::Missing));
        assert_eq!(config.active_scene().source_ids.len(), 1);
    }

    #[test]
    fn removing_a_source_only_affects_the_current_scene() {
        let mut config = sample();
        let podcast = config.active_scene.clone();
        let firefox = config.sources[0].id.clone();
        let archive = config.duplicate_scene(&podcast, "Archive").unwrap();
        assert_eq!(config.active_scene, archive);
        config.remove_source(&firefox);
        assert_eq!(config.active_scene().source_ids.len(), 1);
        assert!(config.scene(&podcast).unwrap().contains(&firefox));
        assert!(config.source(&firefox).is_some());
        // Retirée de la dernière scène qui l'avait : elle disparaît.
        config.active_scene = podcast;
        config.remove_source(&firefox);
        assert!(config.source(&firefox).is_none());
        assert_eq!(config.sources.len(), 1);
    }

    #[test]
    fn a_duplicated_scene_shares_its_sources() {
        use crate::schedule::{Repeat, TimeOfDay};
        let mut config = sample();
        let podcast = config.active_scene.clone();
        let start = TimeOfDay::new(21, 0).unwrap();
        let end = TimeOfDay::new(22, 0).unwrap();
        config.schedules.push(Schedule::new(&podcast, start, end, Repeat::Weekly { days: vec![1] }));
        let copy = config.duplicate_scene(&podcast, "Podcast (copie)").unwrap();
        config.normalize();
        assert_eq!(config.scenes.len(), 2);
        assert_eq!(config.sources.len(), 2);
        assert_eq!(config.scene(&copy).unwrap().source_ids, config.scene(&podcast).unwrap().source_ids);
        assert_eq!(config.schedules.len(), 1);
        assert!(config.duplicate_scene("inconnue", "X").is_none());
        // Supprimer l'original garde les sources, toujours dans la copie.
        assert!(config.sources_only_in(&podcast).is_empty());
        config.scenes.retain(|s| s.id != podcast);
        config.normalize();
        assert_eq!(config.active_sources().len(), 2);
        // Supprimer la dernière scène qui les a : elles disparaissent.
        assert_eq!(config.sources_only_in(&copy).len(), 2);
        config.scenes = vec![Scene::new("Vide")];
        config.normalize();
        assert!(config.sources.is_empty());
    }

    #[test]
    fn sources_move_within_their_scene() {
        let mut config = sample();
        let third = config.sources[1].clone();
        let third = Source { id: new_id(), name: "Troisième".into(), ..third };
        config.add_source(third);
        let ids = config.active_scene().source_ids.clone();
        let copy = config.duplicate_scene(&config.active_scene.clone(), "Copie").unwrap();
        // La première passe en dernier, les autres remontent.
        assert!(config.move_source(0, 2));
        assert_eq!(config.active_scene().source_ids, [ids[1].clone(), ids[2].clone(), ids[0].clone()]);
        // Et revient en tête.
        assert!(config.move_source(2, 0));
        assert_eq!(config.active_scene().source_ids, ids);
        assert!(config.move_source(1, 2));
        assert_eq!(config.active_scene().source_ids, [ids[0].clone(), ids[2].clone(), ids[1].clone()]);
        // Rien à faire, ou hors limites.
        assert!(!config.move_source(1, 1));
        assert!(!config.move_source(0, 3));
        assert!(!config.move_source(3, 0));
        // L'ordre est propre à chaque scène : l'autre ne bouge pas.
        assert_eq!(config.active_scene, copy);
        config.active_scene = config.scenes[0].id.clone();
        assert_eq!(config.active_scene().source_ids, ids);
    }

    #[test]
    fn normalize_drops_unknown_and_repeated_sources() {
        let mut config = sample();
        let firefox = config.sources[0].id.clone();
        let twin = config.sources[0].clone();
        config.sources.push(twin);
        let scene = config.active_scene_mut();
        scene.source_ids.push(firefox.clone());
        scene.source_ids.push("inconnue".into());
        config.normalize();
        assert_eq!(config.sources.len(), 2);
        assert_eq!(config.active_scene().source_ids.len(), 2);
        assert_eq!(config.active_scene().source_ids[0], firefox);
    }

    #[test]
    fn ids_are_unique() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
    }
}
