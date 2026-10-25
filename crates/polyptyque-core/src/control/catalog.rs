//! Catalogue des commandes du canal : nom, description, paramètres.
//!
//! Source unique pour Polyptyque (vérification des paramètres reçus), le
//! mode commande (lecture des arguments, aide), le serveur MCP (outils et
//! leurs schémas JSON) et la référence `docs/control.md` (un test vérifie
//! qu'elle n'oublie aucune commande).
//!
//! Les descriptions passent par `tr!` : en français ou en anglais selon la
//! langue en cours. Le serveur MCP et `polyptyque-cli commands` les
//! donnent en anglais, la langue la plus sûre pour un modèle.

use std::time::Duration;

use serde_json::{json, Map, Value};
use tr::tr;

use super::protocol::CommandError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    String,
    Integer,
    Boolean,
    /// Liste de textes.
    Strings,
    /// Objet, avec ses champs.
    Object,
}

impl ParamType {
    pub fn name(self) -> &'static str {
        match self {
            ParamType::String => "string",
            ParamType::Integer => "integer",
            ParamType::Boolean => "boolean",
            ParamType::Strings => "string[]",
            ParamType::Object => "object",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: &'static str,
    pub ty: ParamType,
    pub required: bool,
    pub description: String,
    /// Valeurs admises (textes, ou éléments d'une liste).
    pub choices: &'static [&'static str],
    /// Champs d'un objet.
    pub fields: Vec<Param>,
}

impl Param {
    fn new(name: &'static str, ty: ParamType, required: bool, description: String) -> Self {
        Self { name, ty, required, description, choices: &[], fields: Vec::new() }
    }

    fn choices(mut self, choices: &'static [&'static str]) -> Self {
        self.choices = choices;
        self
    }

    fn fields(mut self, fields: Vec<Param>) -> Self {
        self.fields = fields;
        self
    }

    /// Schéma JSON du paramètre.
    pub fn schema(&self) -> Value {
        let mut schema = match self.ty {
            ParamType::String => json!({ "type": "string" }),
            ParamType::Integer => json!({ "type": "integer" }),
            ParamType::Boolean => json!({ "type": "boolean" }),
            ParamType::Strings => {
                let mut items = json!({ "type": "string" });
                if !self.choices.is_empty() {
                    items["enum"] = json!(self.choices);
                }
                json!({ "type": "array", "items": items })
            }
            ParamType::Object => object_schema(&self.fields),
        };
        if self.ty == ParamType::String && !self.choices.is_empty() {
            schema["enum"] = json!(self.choices);
        }
        schema["description"] = self.description.clone().into();
        schema
    }

    fn check(&self, value: &Value, path: &str) -> Result<(), CommandError> {
        let ok = match self.ty {
            ParamType::String => value.is_string(),
            ParamType::Integer => value.is_i64() || value.is_u64(),
            ParamType::Boolean => value.is_boolean(),
            ParamType::Strings => value.as_array().is_some_and(|items| items.iter().all(Value::is_string)),
            ParamType::Object => value.is_object(),
        };
        if !ok {
            return Err(CommandError::invalid(tr!("Paramètre « {} » : {} attendu.", path, self.ty.name())));
        }
        if !self.choices.is_empty() {
            let texts: Vec<&str> = match value {
                Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
                _ => value.as_str().into_iter().collect(),
            };
            if let Some(bad) = texts.iter().find(|t| !self.choices.contains(t)) {
                return Err(CommandError::invalid(tr!(
                    "Paramètre « {} » : « {} » n'est pas une valeur admise ({}).",
                    path,
                    bad,
                    self.choices.join(", ")
                ))
                .with("choices", json!(self.choices)));
            }
        }
        if self.ty == ParamType::Object && !self.fields.is_empty() {
            check_object(&self.fields, value, &format!("{path}."))?;
        }
        Ok(())
    }
}

fn object_schema(fields: &[Param]) -> Value {
    let properties: Map<String, Value> = fields.iter().map(|p| (p.name.to_string(), p.schema())).collect();
    let required: Vec<&str> = fields.iter().filter(|p| p.required).map(|p| p.name).collect();
    let mut schema = json!({ "type": "object", "properties": properties, "additionalProperties": false });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    schema
}

fn check_object(fields: &[Param], value: &Value, prefix: &str) -> Result<(), CommandError> {
    let Some(object) = value.as_object() else {
        return Err(CommandError::invalid(tr!("Les paramètres sont attendus sous forme d'objet.")));
    };
    for (key, value) in object {
        let Some(param) = fields.iter().find(|p| p.name == key) else {
            let known: Vec<&str> = fields.iter().map(|p| p.name).collect();
            return Err(CommandError::invalid(tr!(
                "Paramètre inconnu : « {}{} » (admis : {}).",
                prefix,
                key,
                known.join(", ")
            )));
        };
        // `null` vaut absence, pour les clients qui envoient tous les champs.
        if !value.is_null() {
            param.check(value, &format!("{prefix}{key}"))?;
        }
    }
    if let Some(missing) = fields.iter().find(|p| p.required && object.get(p.name).is_none_or(Value::is_null)) {
        return Err(CommandError::invalid(tr!("Paramètre obligatoire manquant : « {}{} ».", prefix, missing.name)));
    }
    Ok(())
}

/// Effet d'une commande, annoncé aux assistants (annotations MCP).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Ne change rien.
    ReadOnly,
    /// Change quelque chose, sans rien effacer.
    Change,
    /// Efface ou arrête quelque chose.
    Destructive,
}

#[derive(Debug, Clone)]
pub struct Command {
    /// « recording.start ».
    pub method: &'static str,
    pub description: String,
    pub params: Vec<Param>,
    pub effect: Effect,
    /// Attente maximale d'une réponse par un client : les commandes longues
    /// (démarrer, arrêter, essai des encodeurs) répondent une fois
    /// l'opération finie.
    pub timeout: Duration,
}

impl Command {
    /// Nom de l'outil MCP : « recording_start ».
    pub fn tool_name(&self) -> String {
        self.method.replace('.', "_")
    }

    /// Schéma JSON de ses paramètres.
    pub fn input_schema(&self) -> Value {
        object_schema(&self.params)
    }

    /// Vérifie les paramètres reçus : types, valeurs admises, paramètres
    /// obligatoires, et aucun paramètre inconnu (une faute de frappe ne
    /// passe pas inaperçue).
    pub fn check_params(&self, params: &Value) -> Result<(), CommandError> {
        check_object(&self.params, params, "")
    }

    /// Description complète, pour `polyptyque-cli commands`.
    pub fn to_json(&self) -> Value {
        fn param(p: &Param) -> Value {
            let mut value = json!({
                "name": p.name,
                "type": p.ty.name(),
                "required": p.required,
                "description": p.description,
            });
            if !p.choices.is_empty() {
                value["choices"] = json!(p.choices);
            }
            if !p.fields.is_empty() {
                value["fields"] = Value::Array(p.fields.iter().map(param).collect());
            }
            value
        }
        json!({
            "method": self.method,
            "description": self.description,
            "effect": match self.effect {
                Effect::ReadOnly => "read_only",
                Effect::Change => "change",
                Effect::Destructive => "destructive",
            },
            "params": self.params.iter().map(param).collect::<Vec<_>>(),
        })
    }
}

/// Commande du catalogue, d'après son nom (« recording.start ») ou celui
/// de son outil MCP (« recording_start »).
pub fn find(name: &str) -> Option<Command> {
    commands().into_iter().find(|c| c.method == name || c.tool_name() == name)
}

const SHORT: Duration = Duration::from_secs(30);
const LONG: Duration = Duration::from_secs(300);

fn command(method: &'static str, effect: Effect, description: String, params: Vec<Param>) -> Command {
    Command { method, description, params, effect, timeout: SHORT }
}

fn long(mut command: Command) -> Command {
    command.timeout = LONG;
    command
}

fn opt(name: &'static str, ty: ParamType, description: String) -> Param {
    Param::new(name, ty, false, description)
}

fn req(name: &'static str, ty: ParamType, description: String) -> Param {
    Param::new(name, ty, true, description)
}

fn scene_param(required: bool, description: String) -> Param {
    Param::new("scene", ParamType::String, required, description)
}

fn source_param() -> Param {
    req("source", ParamType::String, tr!("Source : son identifiant ou son nom."))
}

fn confirm_param() -> Param {
    opt(
        "confirm",
        ParamType::Boolean,
        tr!("true une fois la personne d'accord : sans lui, la commande renvoie « confirmation_required » avec le texte de la question à lui poser."),
    )
}

pub const CODECS: &[&str] = &["h264", "hevc"];
pub const HARDWARE: &[&str] = &["auto", "gpu", "cpu"];
pub const LANGUAGES: &[&str] = &["auto", "french", "english"];
pub const CLOCKS: &[&str] = &["auto", "24h", "12h"];
pub const UPDATES: &[&str] = &["stable", "beta", "off"];
pub const CLOSE_CHOICES: &[&str] = &["ask", "background", "quit"];
pub const DAYS: &[&str] = &["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// Champs des réglages, pour `settings.set` (mêmes noms que `settings.get`).
fn settings_fields() -> Vec<Param> {
    use ParamType as T;
    vec![
        opt("output_dir", T::String, tr!("Dossier des enregistrements (chemin complet).")),
        opt(
            "filename_template",
            T::String,
            tr!("Modèle de nom des fichiers, avec les variables {date}, {time}, {scene}, {source} (ou {heure}, {scène})."),
        ),
        opt("previews", T::Boolean, tr!("Aperçus en direct des sources dans la fenêtre.")),
        opt("convert_to_mp4", T::Boolean, tr!("Conversion MP4 automatique à la fin de chaque enregistrement (le MKV est gardé).")),
        opt("updates", T::String, tr!("Mises à jour proposées : stables, stables et bêta, ou aucune recherche.")).choices(UPDATES),
        opt("open_windows_at_launch", T::Boolean, tr!("Ouvrir les fenêtres de la scène au lancement de Polyptyque.")),
        opt("close_browsers_at_exit", T::Boolean, tr!("Fermer les navigateurs des sources en quittant Polyptyque.")),
        opt(
            "launch_at_startup",
            T::Boolean,
            tr!("Lancer Polyptyque au démarrage de Windows (en arrière-plan), pour que les programmations survivent à un redémarrage."),
        ),
        opt(
            "close_with_schedules",
            T::String,
            tr!("Fermeture de la fenêtre alors qu'une programmation est prévue : demander, continuer en arrière-plan, ou quitter."),
        )
        .choices(CLOSE_CHOICES),
        opt("language", T::String, tr!("Langue de Polyptyque.")).choices(LANGUAGES),
        opt("clock", T::String, tr!("Horloge des heures affichées et saisies.")).choices(CLOCKS),
        opt("video", T::Object, tr!("Encodage vidéo, commun à toutes les sources.")).fields(vec![
            opt("width", T::Integer, tr!("Largeur en pixels (paire). 1920 pour du 1080p.")),
            opt("height", T::Integer, tr!("Hauteur en pixels (paire). 1080 pour du 1080p.")),
            opt("fps", T::Integer, tr!("Images par seconde (1 à 240 ; 30 ou 60 en général).")),
            opt("bitrate_kbps", T::Integer, tr!("Débit vidéo en kb/s (250 à 500 000). En 1080p à 60 i/s : 6 000 léger, 12 000 bon, 20 000 très bon.")),
            opt("codec", T::String, tr!("Format vidéo : H.264 (compatibilité maximale au montage) ou HEVC (fichiers plus légers).")).choices(CODECS),
            opt("hardware", T::String, tr!("Matériel d'encodage : automatique (carte graphique si disponible), carte graphique, ou processeur.")).choices(HARDWARE),
        ]),
        opt("audio", T::Object, tr!("Son des sources.")).fields(vec![opt(
            "bitrate_kbps",
            T::Integer,
            tr!("Débit audio en kb/s (64 à 512 ; 192 par défaut)."),
        )]),
        opt("microphone", T::Object, tr!("Micro, enregistré dans un fichier séparé.")).fields(vec![
            opt(
                "enabled",
                T::Boolean,
                tr!("Activer le micro : la personne doit l'accepter dans la fenêtre de Polyptyque (la commande attend sa réponse, deux minutes au plus)."),
            ),
            opt("device_id", T::String, tr!("Micro choisi (identifiant donné par microphones.list) ; vide pour le micro par défaut de Windows.")),
        ]),
        opt("hotkeys", T::Object, tr!("Raccourcis clavier globaux, au format « Ctrl+Maj+F9 » ; vide pour aucun.")).fields(vec![
            opt("toggle_recording", T::String, tr!("Démarrer / arrêter tout.")),
            opt("show_window", T::String, tr!("Afficher / réduire Polyptyque.")),
            opt("toggle_previews", T::String, tr!("Activer / couper les aperçus.")),
        ]),
    ]
}

/// Toutes les commandes, dans l'ordre de la référence.
pub fn commands() -> Vec<Command> {
    use Effect::*;
    use ParamType as T;
    vec![
        // ----- Généralités
        command(
            "hello",
            ReadOnly,
            tr!("Présentation : version de Polyptyque et du protocole, langue. Le nom du programme client s'affiche dans Polyptyque tant qu'il est connecté."),
            vec![opt("client", T::String, tr!("Nom du programme client, affiché dans Polyptyque (« Claude », « Stream Deck »)."))],
        ),
        command(
            "status",
            ReadOnly,
            tr!("État complet : enregistrement en cours, scène, sources et leurs fenêtres, disque, conversion, prochaine programmation, question en attente."),
            vec![],
        ),
        command(
            "subscribe",
            ReadOnly,
            tr!("État en direct : Polyptyque envoie ensuite la notification « state » à chaque changement, et « message » pour chaque message qu'il affiche."),
            vec![],
        ),
        command("unsubscribe", ReadOnly, tr!("Arrête l'état en direct."), vec![]),
        command(
            "system.info",
            ReadOnly,
            tr!("Système : Windows, cartes graphiques, FFmpeg, dossiers, espace libre. Utile avant de choisir les réglages d'encodage."),
            vec![],
        ),
        long(command(
            "encoders.test",
            ReadOnly,
            tr!("Essaie chaque encodeur (NVIDIA, AMD, Intel, processeur) en H.264 et en HEVC sur cette machine. Peut prendre une minute ; impossible pendant un enregistrement."),
            vec![],
        )),
        command("window.show", Change, tr!("Affiche la fenêtre de Polyptyque au premier plan (depuis l'arrière-plan ou réduite)."), vec![]),
        command(
            "previews.set",
            Change,
            tr!("Active ou coupe les aperçus en direct des sources (sans effet sur les fichiers enregistrés)."),
            vec![req("enabled", T::Boolean, tr!("true pour activer, false pour couper."))],
        ),
        // ----- Scènes
        command(
            "scenes.list",
            ReadOnly,
            tr!("Scènes, dans leur ordre, avec leurs sources et la scène en cours. Une seule scène est en cours : c'est elle qui s'enregistre."),
            vec![],
        ),
        command(
            "scenes.create",
            Change,
            tr!("Crée une scène vide."),
            vec![
                req("name", T::String, tr!("Nom de la scène.")),
                opt("activate", T::Boolean, tr!("En faire la scène en cours (oui par défaut, comme dans la fenêtre).")),
            ],
        ),
        command(
            "scenes.rename",
            Change,
            tr!("Renomme une scène."),
            vec![scene_param(true, tr!("Scène : son identifiant ou son nom.")), req("name", T::String, tr!("Nouveau nom."))],
        ),
        command(
            "scenes.duplicate",
            Change,
            tr!("Duplique une scène : la copie reprend les mêmes sources (partagées, pas copiées) et devient la scène en cours. Ses programmations ne sont pas reprises."),
            vec![
                scene_param(false, tr!("Scène à dupliquer (par défaut, la scène en cours).")),
                opt("name", T::String, tr!("Nom de la copie (par défaut : « <nom> (copie) »).")),
            ],
        ),
        command(
            "scenes.delete",
            Destructive,
            tr!("Supprime une scène, ses programmations, et les sources qui ne sont dans aucune autre scène (leurs profils de navigateur restent sur le disque). La dernière scène ne peut pas être supprimée."),
            vec![scene_param(true, tr!("Scène : son identifiant ou son nom.")), confirm_param()],
        ),
        command(
            "scenes.activate",
            Change,
            tr!("Change la scène en cours (impossible pendant un enregistrement)."),
            vec![scene_param(true, tr!("Scène : son identifiant ou son nom."))],
        ),
        long(command(
            "scenes.open_windows",
            Change,
            tr!("Ouvre les fenêtres manquantes de la scène en cours : navigateurs, et applications dont la relance est activée. Répond une fois les fenêtres apparues (30 secondes au plus)."),
            vec![],
        )),
        // ----- Sources
        command(
            "sources.list",
            ReadOnly,
            tr!("Sources d'une scène, dans leur ordre, ou toutes les sources."),
            vec![
                scene_param(false, tr!("Scène (par défaut, la scène en cours).")),
                opt("all", T::Boolean, tr!("true : toutes les sources, avec les scènes de chacune.")),
            ],
        ),
        command(
            "browsers.list",
            ReadOnly,
            tr!("Navigateurs installés, utilisables pour une source navigateur."),
            vec![],
        ),
        command(
            "windows.list",
            ReadOnly,
            tr!("Fenêtres ouvertes, pour ajouter une source application. Le titre d'une fenêtre n'est donné que si plusieurs fenêtres du même programme sont ouvertes."),
            vec![],
        ),
        command(
            "sources.add_browser",
            Change,
            tr!("Ajoute une source navigateur : un navigateur lancé avec un profil qui lui est propre (connexions et onglets conservés ; son isolé par profil, donc une source = ce qui doit être entendu). Pour Tor Browser, utiliser une source application."),
            vec![
                req("browser", T::String, tr!("Navigateur : son nom (donné par browsers.list) ou le chemin de son exécutable.")),
                opt("name", T::String, tr!("Nom de la source, repris dans le nom des fichiers (par défaut : « Firefox 2 »…).")),
                opt("start_url", T::String, tr!("Adresse ouverte au premier lancement du profil.")),
                opt("open", T::Boolean, tr!("Ouvrir le navigateur tout de suite, pour s'y connecter (oui par défaut).")),
                scene_param(false, tr!("Scène où l'ajouter (par défaut, la scène en cours).")),
            ],
        ),
        command(
            "sources.add_application",
            Change,
            tr!("Ajoute une source application d'après une fenêtre ouverte (Discord, jeu, Tor Browser…)."),
            vec![
                req("window", T::String, tr!("Fenêtre : son identifiant donné par windows.list.")),
                opt("name", T::String, tr!("Nom de la source (par défaut : le nom du programme).")),
                opt("relaunch", T::Boolean, tr!("Relancer le programme s'il n'est pas ouvert (non par défaut ; utile pour Discord, pas pour un jeu lancé par Steam).")),
                opt("capture_cursor", T::Boolean, tr!("Afficher le curseur de la souris dans l'image (oui par défaut).")),
                scene_param(false, tr!("Scène où l'ajouter (par défaut, la scène en cours).")),
            ],
        ),
        command(
            "sources.add_to_scene",
            Change,
            tr!("Ajoute une source existante à une autre scène : c'est la même source, partagée (même fenêtre, même profil), pas une copie. Jamais deux fois dans une scène."),
            vec![source_param(), scene_param(true, tr!("Scène où l'ajouter."))],
        ),
        command(
            "sources.remove",
            Destructive,
            tr!("Retire une source d'une scène. Retirée de sa dernière scène, elle disparaît (son profil de navigateur reste sur le disque)."),
            vec![source_param(), scene_param(false, tr!("Scène (par défaut, la scène en cours).")), confirm_param()],
        ),
        command(
            "sources.move",
            Change,
            tr!("Change la place d'une source dans sa scène (même ordre dans la liste et dans la grille des aperçus)."),
            vec![
                source_param(),
                req("position", T::Integer, tr!("Nouvelle place, 1 pour la première.")),
                scene_param(false, tr!("Scène (par défaut, la scène en cours).")),
            ],
        ),
        command(
            "sources.open_window",
            Change,
            tr!("Ouvre la fenêtre d'une source de la scène en cours (navigateur avec son profil, ou programme)."),
            vec![source_param()],
        ),
        command(
            "sources.restore_window",
            Change,
            tr!("Fait revenir la fenêtre réduite d'une source de la scène en cours (une fenêtre réduite donne une image figée)."),
            vec![source_param()],
        ),
        // ----- Enregistrement
        long(command(
            "recording.start",
            Change,
            tr!("Démarre l'enregistrement de toutes les sources de la scène en cours, ou d'une seule (elle rejoint l'enregistrement en cours s'il y en a un). Un fichier par source. Répond une fois l'enregistrement démarré."),
            vec![
                scene_param(false, tr!("Scène à enregistrer : elle devient la scène en cours (par défaut, la scène en cours).")),
                opt("source", T::String, tr!("Une seule source de la scène : son identifiant ou son nom.")),
                opt("auto_stop", T::Boolean, tr!("Pendant un créneau programmé : arrêter automatiquement à la fin du créneau (non par défaut).")),
            ],
        )),
        long(command(
            "recording.stop",
            Destructive,
            tr!("Arrête l'enregistrement (toutes les sources), ou une seule source. Répond une fois les fichiers finalisés, avec leur liste."),
            vec![opt("source", T::String, tr!("Une seule source : son identifiant ou son nom.")), confirm_param()],
        )),
        command(
            "prompt.answer",
            Change,
            tr!("Répond à la question affichée par une programmation (voir « prompt » dans status) : démarrage manqué, ou arrêt automatique d'un enregistrement lancé pendant un créneau."),
            vec![req("accept", T::Boolean, tr!("true : le bouton principal (enregistrer maintenant, arrêter à la fin du créneau) ; false : l'autre."))],
        ),
        // ----- Réglages
        command(
            "settings.get",
            ReadOnly,
            tr!("Réglages, avec les valeurs proposées par la fenêtre des réglages (résolutions, cadences, raccourcis…)."),
            vec![],
        ),
        long(command(
            "settings.set",
            Change,
            tr!("Modifie des réglages : seuls les champs donnés changent. Impossible pendant un enregistrement, ou pendant que la fenêtre des réglages est ouverte. Activer le micro demande l'accord de la personne dans Polyptyque."),
            settings_fields(),
        )),
        command(
            "microphones.list",
            ReadOnly,
            tr!("Micros disponibles, pour le réglage microphone.device_id."),
            vec![],
        ),
        // ----- Programmations
        command(
            "schedules.list",
            ReadOnly,
            tr!("Programmations : chacune lance une scène d'une heure à une autre, une fois ou certains jours, avec son prochain créneau. Polyptyque doit être ouvert (au besoin en arrière-plan)."),
            vec![],
        ),
        command(
            "schedules.create",
            Change,
            tr!("Crée une programmation. Une fin antérieure au début désigne le lendemain (22:00 → 02:00). Deux programmations activées ne peuvent pas se chevaucher."),
            vec![
                scene_param(true, tr!("Scène à enregistrer : son identifiant ou son nom.")),
                req("start", T::String, tr!("Heure de début, « HH:MM » sur 24 h.")),
                req("end", T::String, tr!("Heure de fin, « HH:MM » sur 24 h.")),
                opt("date", T::String, tr!("Une seule fois : date du début, « AAAA-MM-JJ ». Donner date ou days.")),
                opt("days", T::Strings, tr!("Chaque semaine : jours où le créneau commence (les sept : tous les jours). Donner date ou days.")).choices(DAYS),
                opt("enabled", T::Boolean, tr!("Activée (oui par défaut).")),
            ],
        ),
        command(
            "schedules.update",
            Change,
            tr!("Modifie une programmation : seuls les champs donnés changent (date et days se remplacent l'un l'autre). Impossible pendant que son enregistrement est en cours."),
            vec![
                req("schedule", T::String, tr!("Identifiant de la programmation (donné par schedules.list).")),
                scene_param(false, tr!("Scène à enregistrer : son identifiant ou son nom.")),
                opt("start", T::String, tr!("Heure de début, « HH:MM » sur 24 h.")),
                opt("end", T::String, tr!("Heure de fin, « HH:MM » sur 24 h.")),
                opt("date", T::String, tr!("Une seule fois : date du début, « AAAA-MM-JJ ».")),
                opt("days", T::Strings, tr!("Chaque semaine : jours où le créneau commence.")).choices(DAYS),
            ],
        ),
        command(
            "schedules.enable",
            Change,
            tr!("Active ou désactive une programmation."),
            vec![
                req("schedule", T::String, tr!("Identifiant de la programmation.")),
                req("enabled", T::Boolean, tr!("true pour l'activer, false pour la désactiver.")),
            ],
        ),
        command(
            "schedules.delete",
            Destructive,
            tr!("Supprime une programmation (impossible pendant que son enregistrement est en cours)."),
            vec![req("schedule", T::String, tr!("Identifiant de la programmation."))],
        ),
        // ----- Fichiers
        command(
            "recordings.list",
            ReadOnly,
            tr!("Derniers fichiers du dossier des enregistrements (MKV, MKA, MP4, M4A), du plus récent au plus ancien."),
            vec![opt("limit", T::Integer, tr!("Nombre de fichiers au plus (50 par défaut)."))],
        ),
        command(
            "recordings.open_folder",
            Change,
            tr!("Ouvre le dossier des enregistrements dans l'Explorateur."),
            vec![],
        ),
        command(
            "conversion.start",
            Change,
            tr!("Convertit des MKV (ou MKA) en MP4 (ou M4A) pour le montage, sans perte ni réencodage, en arrière-plan, un fichier à la fois. Le MKV est gardé. Avancement dans status."),
            vec![req("files", T::Strings, tr!("Chemins complets des fichiers à convertir."))],
        ),
        command(
            "conversion.cancel",
            Destructive,
            tr!("Annule les conversions en cours et prévues (les fichiers partiels sont effacés, les MKV restent)."),
            vec![confirm_param()],
        ),
        // ----- Polyptyque
        command(
            "update.install",
            Change,
            tr!("Télécharge la nouvelle version proposée (voir « update_available » dans status), puis Polyptyque redémarre pour l'installer. Jamais pendant un enregistrement."),
            vec![confirm_param()],
        ),
        command(
            "app.quit",
            Destructive,
            tr!("Quitte Polyptyque (un enregistrement en cours est arrêté proprement). Les programmations ne se lancent pas tant qu'il est fermé."),
            vec![confirm_param()],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn methods_are_unique_and_named_for_mcp() {
        let commands = commands();
        let mut names: Vec<&str> = commands.iter().map(|c| c.method).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), commands.len());
        for command in &commands {
            let tool = command.tool_name();
            assert!(tool.len() <= 64 && tool.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{tool}");
            assert_eq!(find(&tool).map(|c| c.method), Some(command.method));
        }
    }

    #[test]
    fn checks_params() {
        let start = find("recording.start").unwrap();
        assert!(start.check_params(&json!({})).is_ok());
        assert!(start.check_params(&json!({ "scene": "Podcast", "auto_stop": true })).is_ok());
        assert!(start.check_params(&json!({ "scene": 3 })).is_err());
        assert!(start.check_params(&json!({ "scenes": "Podcast" })).is_err());
        let create = find("schedules.create").unwrap();
        assert!(create.check_params(&json!({ "scene": "a", "start": "21:00" })).is_err());
        assert!(create
            .check_params(&json!({ "scene": "a", "start": "21:00", "end": "22:00", "days": ["mon", "fri"] }))
            .is_ok());
        assert!(create
            .check_params(&json!({ "scene": "a", "start": "21:00", "end": "22:00", "days": ["lundi"] }))
            .is_err());
        let settings = find("settings.set").unwrap();
        assert!(settings.check_params(&json!({ "video": { "bitrate_kbps": 8000, "codec": "hevc" } })).is_ok());
        assert!(settings.check_params(&json!({ "video": { "bitrate": 8000 } })).is_err());
        assert!(settings.check_params(&json!({ "video": { "codec": "av1" } })).is_err());
        // `null` vaut absence.
        assert!(settings.check_params(&json!({ "language": null })).is_ok());
    }

    #[test]
    fn schemas_are_objects() {
        for command in commands() {
            let schema = command.input_schema();
            assert_eq!(schema["type"], "object");
            assert_eq!(schema["additionalProperties"], false);
        }
        let schema = find("settings.set").unwrap().input_schema();
        assert_eq!(schema["properties"]["video"]["properties"]["codec"]["enum"], json!(["h264", "hevc"]));
    }
}
