//! Programmation des enregistrements : des créneaux à heure fixe, une seule
//! fois ou chaque semaine certains jours, qui lancent chacun une scène.
//!
//! Tout se décide ici, sans interface, pour être vérifié par des tests et
//! piloté plus tard par d'autres moyens que l'interface :
//! - [`Schedule`] décrit une programmation et calcule ses créneaux ;
//! - [`Planner`] décide, à chaque instant, de ce qu'il faut faire
//!   (démarrer, demander, prévenir, arrêter) ;
//! - l'interface exécute ces décisions et lui rend compte du résultat.
//!
//! Les heures sont celles de l'horloge affichée par le système : un créneau
//! « 21:00 → 22:00 » commence à 21:00 en heure d'été comme en heure d'hiver.
//! Une heure de fin antérieure ou égale à l'heure de début désigne le
//! lendemain (« 22:00 → 02:00 »). Aux changements d'heure, comme dans les
//! agendas, une heure qui n'existe pas (02:30 au passage à l'heure d'été)
//! est décalée de la durée du saut (03:30), et une heure qui existe deux
//! fois (02:30 au passage à l'heure d'hiver) est prise à sa première
//! occurrence.

use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeZone, Timelike, Utc,
};
use serde::{Deserialize, Serialize};
use tr::tr;

use crate::config::new_id;
use crate::storage;

/// Retard au-delà duquel un créneau est considéré comme manqué s'il n'a pas
/// commencé pendant que Polyptyque était ouvert : au lieu de démarrer, on
/// demande (avec un compte à rebours).
pub const START_GRACE: Duration = Duration::seconds(30);
/// Durée de la question posée pour un démarrage manqué ou un lancement à
/// la main pendant un créneau, avant que la réponse par défaut s'applique.
pub const PROMPT_COUNTDOWN: Duration = Duration::seconds(120);
/// Écart entre deux observations au-delà duquel l'ordinateur est supposé
/// avoir dormi (ou l'horloge avoir sauté) entre-temps.
const MAX_GAP: Duration = Duration::seconds(30);
/// Jours examinés pour trouver le prochain créneau d'une programmation
/// hebdomadaire (une semaine, plus la veille et une marge).
const LOOKAHEAD_DAYS: i64 = 9;

// ----- Heure de la journée -------------------------------------------------

/// Heure de la journée, à la minute (« 21:00 » dans les réglages).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TimeOfDay(u16);

impl TimeOfDay {
    pub fn new(hour: u32, minute: u32) -> Option<Self> {
        (hour < 24 && minute < 60).then(|| Self((hour * 60 + minute) as u16))
    }

    pub fn hour(self) -> u32 {
        u32::from(self.0) / 60
    }

    pub fn minute(self) -> u32 {
        u32::from(self.0) % 60
    }

    /// Lit une heure saisie à la main : « 21:00 », « 21h00 », « 21h »,
    /// « 21 », « 9:05 », « 21.30 ».
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().to_lowercase().replace(' ', "");
        // « 9:05 PM », « 9 pm » : horloge sur 12 h.
        if text.replace('.', "").ends_with("am") || text.replace('.', "").ends_with("pm") {
            return crate::clock::parse_time(&text).and_then(|(h, m)| Self::new(h, m));
        }
        let (hour, minute) = match text.split_once([':', 'h', '.']) {
            Some((hour, minute)) => (hour, if minute.is_empty() { "0" } else { minute }),
            None => (text.as_str(), "0"),
        };
        if hour.is_empty() || hour.len() > 2 || minute.len() > 2 {
            return None;
        }
        Self::new(hour.parse().ok()?, minute.parse().ok()?)
    }

    /// Heure affichée, selon l'horloge du réglage : « 21:00 » ou « 9:00 PM ».
    /// Le texte enregistré dans les réglages reste « 21:00 » (`Display`).
    pub fn label(self) -> String {
        crate::clock::time(self.hour(), self.minute())
    }

    fn naive(self) -> NaiveTime {
        NaiveTime::from_hms_opt(self.hour(), self.minute(), 0).unwrap_or(NaiveTime::MIN)
    }
}

impl fmt::Display for TimeOfDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}", self.hour(), self.minute())
    }
}

impl TryFrom<String> for TimeOfDay {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text).ok_or_else(|| tr!("heure invalide : « {} »", text))
    }
}

impl From<TimeOfDay> for String {
    fn from(time: TimeOfDay) -> Self {
        time.to_string()
    }
}

// ----- Programmation -------------------------------------------------------

/// Nom d'un jour, du lundi (1) au dimanche (7), tel qu'il s'écrit au
/// milieu d'une phrase (« lundi », « Monday »).
pub fn day_name(day: u8) -> String {
    match day {
        1 => tr!("lundi"),
        2 => tr!("mardi"),
        3 => tr!("mercredi"),
        4 => tr!("jeudi"),
        5 => tr!("vendredi"),
        6 => tr!("samedi"),
        _ => tr!("dimanche"),
    }
}

/// Abréviation d'un jour, du lundi (1) au dimanche (7).
fn day_abbreviation(day: u8) -> String {
    match day {
        1 => tr!("Lun"),
        2 => tr!("Mar"),
        3 => tr!("Mer"),
        4 => tr!("Jeu"),
        5 => tr!("Ven"),
        6 => tr!("Sam"),
        _ => tr!("Dim"),
    }
}

/// Répétition d'une programmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Repeat {
    /// Une seule fois : créneau commençant ce jour-là.
    Once { date: NaiveDate },
    /// Chaque semaine, créneaux commençant ces jours-là (1 lundi … 7
    /// dimanche ; les sept : tous les jours).
    Weekly { days: Vec<u8> },
}

/// Programmation : lance la scène `scene_id` de `start` à `end`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub id: String,
    pub scene_id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub start: TimeOfDay,
    /// Fin, le lendemain si elle est antérieure ou égale au début.
    pub end: TimeOfDay,
    pub repeat: Repeat,
}

/// Créneau d'une programmation : une période précise, en temps universel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub schedule_id: String,
    pub scene_id: String,
    /// Jour (local) où le créneau commence : avec l'identifiant de la
    /// programmation, il désigne le créneau.
    pub day: NaiveDate,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// Désignation d'un créneau, indépendante de l'heure (qui peut changer si la
/// programmation est modifiée).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotKey {
    pub schedule_id: String,
    pub day: NaiveDate,
}

impl Slot {
    pub fn key(&self) -> SlotKey {
        SlotKey { schedule_id: self.schedule_id.clone(), day: self.day }
    }

    pub fn contains(&self, now: DateTime<Utc>) -> bool {
        self.start <= now && now < self.end
    }
}

/// Pourquoi une programmation ne peut pas être enregistrée.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleError {
    SameTimes,
    NoDay,
    InPast,
    /// Chevauche cette autre programmation.
    Overlap(String),
}

impl Schedule {
    pub fn new(scene_id: &str, start: TimeOfDay, end: TimeOfDay, repeat: Repeat) -> Self {
        Self { id: new_id(), scene_id: scene_id.to_string(), enabled: true, start, end, repeat }
    }

    /// Le créneau se termine le lendemain de son début.
    pub fn crosses_midnight(&self) -> bool {
        self.end <= self.start
    }

    /// Un créneau commence ce jour-là.
    pub fn runs_on(&self, day: NaiveDate) -> bool {
        match &self.repeat {
            Repeat::Once { date } => *date == day,
            Repeat::Weekly { days } => days.contains(&(day.weekday().number_from_monday() as u8)),
        }
    }

    /// Début et fin du créneau qui commence le jour `day`, en heure locale
    /// (sans tenir compte des changements d'heure).
    fn naive_interval(&self, day: NaiveDate) -> (NaiveDateTime, NaiveDateTime) {
        let end_day = if self.crosses_midnight() { day.succ_opt().unwrap_or(day) } else { day };
        (day.and_time(self.start.naive()), end_day.and_time(self.end.naive()))
    }

    /// Créneau qui commence le jour `day`, s'il y en a un. Aucun si les
    /// changements d'heure le rendent vide (fin avant le début).
    pub fn slot_on<Tz: TimeZone>(&self, day: NaiveDate, tz: &Tz) -> Option<Slot> {
        if !self.runs_on(day) {
            return None;
        }
        let (start, end) = self.naive_interval(day);
        let start = resolve(tz, start);
        let end = resolve(tz, end);
        (end > start).then(|| Slot { schedule_id: self.id.clone(), scene_id: self.scene_id.clone(), day, start, end })
    }

    /// Créneau en cours à l'instant `now`.
    pub fn slot_at<Tz: TimeZone>(&self, now: DateTime<Utc>, tz: &Tz) -> Option<Slot> {
        let today = now.with_timezone(tz).date_naive();
        // Un créneau commencé la veille peut se terminer aujourd'hui.
        [today.pred_opt(), Some(today)]
            .into_iter()
            .flatten()
            .filter_map(|day| self.slot_on(day, tz))
            .find(|slot| slot.contains(now))
    }

    /// Prochain créneau commençant après `now`. Une programmation unique a
    /// son créneau à sa date, aussi lointaine soit-elle.
    pub fn next_slot<Tz: TimeZone>(&self, now: DateTime<Utc>, tz: &Tz) -> Option<Slot> {
        if let Repeat::Once { date } = &self.repeat {
            return self.slot_on(*date, tz).filter(|slot| slot.start > now);
        }
        let today = now.with_timezone(tz).date_naive();
        (-1..=LOOKAHEAD_DAYS)
            .filter_map(|offset| today.checked_add_signed(Duration::days(offset)))
            .filter_map(|day| self.slot_on(day, tz))
            .find(|slot| slot.start > now)
    }

    /// La programmation a encore un créneau, en cours ou à venir.
    pub fn is_active<Tz: TimeZone>(&self, now: DateTime<Utc>, tz: &Tz) -> bool {
        self.enabled && (self.slot_at(now, tz).is_some() || self.next_slot(now, tz).is_some())
    }

    /// Vérifie la programmation avant de l'enregistrer : heures, jours,
    /// créneau unique déjà passé, chevauchement avec une autre
    /// programmation activée (un seul enregistrement à la fois). Des
    /// créneaux bout à bout ne se chevauchent pas.
    pub fn validate<Tz: TimeZone>(
        &self,
        others: &[Schedule],
        now: DateTime<Utc>,
        tz: &Tz,
    ) -> Result<(), ScheduleError> {
        if self.start == self.end {
            return Err(ScheduleError::SameTimes);
        }
        match &self.repeat {
            Repeat::Weekly { days } if days.is_empty() => return Err(ScheduleError::NoDay),
            Repeat::Once { date } => {
                if self.slot_on(*date, tz).is_none_or(|slot| slot.end <= now) {
                    return Err(ScheduleError::InPast);
                }
            }
            Repeat::Weekly { .. } => {}
        }
        if !self.enabled {
            return Ok(());
        }
        let today = now.with_timezone(tz).date_naive();
        let days: Vec<NaiveDate> = match &self.repeat {
            Repeat::Once { date } => vec![*date],
            // Deux semaines : tous les cas d'une répétition hebdomadaire,
            // et toute programmation unique proche.
            Repeat::Weekly { .. } => (-1..=14).filter_map(|o| today.checked_add_signed(Duration::days(o))).collect(),
        };
        let mine: Vec<_> = days.iter().filter(|d| self.runs_on(**d)).map(|d| self.naive_interval(*d)).collect();
        for other in others.iter().filter(|o| o.enabled && o.id != self.id) {
            let overlaps = days
                .iter()
                .flat_map(|d| [d.pred_opt(), Some(*d), d.succ_opt()])
                .flatten()
                .filter(|d| other.runs_on(*d))
                .map(|d| other.naive_interval(d))
                .any(|(start, end)| mine.iter().any(|(s, e)| *s < end && start < *e));
            if overlaps {
                return Err(ScheduleError::Overlap(other.id.clone()));
            }
        }
        Ok(())
    }

    /// « 21:00 → 22:00 », « 22:00 → 02:00 (lendemain) ».
    pub fn times_label(&self) -> String {
        if self.crosses_midnight() {
            tr!("{} → {} (lendemain)", self.start.label(), self.end.label())
        } else {
            format!("{} → {}", self.start.label(), self.end.label())
        }
    }

    /// « Tous les jours », « Du lundi au vendredi », « Lun, mer, ven »,
    /// « Une fois, le 25/10/2026 ».
    pub fn repeat_label(&self) -> String {
        self.repeat_label_at(chrono::Local::now().date_naive())
    }

    /// Idem, l'année n'étant écrite que si ce n'est pas celle de `today`.
    pub fn repeat_label_at(&self, today: NaiveDate) -> String {
        match &self.repeat {
            Repeat::Once { date } => tr!("Une fois, le {}", crate::clock::date(*date, today)),
            Repeat::Weekly { days } => {
                let mut days: Vec<u8> = days.iter().copied().filter(|d| (1..=7).contains(d)).collect();
                days.sort_unstable();
                days.dedup();
                match days.as_slice() {
                    [1, 2, 3, 4, 5, 6, 7] => tr!("Tous les jours"),
                    [1, 2, 3, 4, 5] => tr!("Du lundi au vendredi"),
                    [6, 7] => tr!("Le week-end"),
                    [day] => tr!("Chaque {}", day_name(*day)),
                    days => days.iter().map(|&d| day_abbreviation(d)).collect::<Vec<_>>().join(", "),
                }
            }
        }
    }
}

/// Instant correspondant à une heure locale. Heure inexistante (passage à
/// l'heure d'été) : décalée de la durée du saut, comme dans les agendas.
/// Heure double (passage à l'heure d'hiver) : la première.
fn resolve<Tz: TimeZone>(tz: &Tz, naive: NaiveDateTime) -> DateTime<Utc> {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => t.with_timezone(&Utc),
        LocalResult::None => {
            // Décalage en vigueur juste avant le saut.
            let before = tz.offset_from_utc_datetime(&(naive - Duration::hours(3))).fix();
            Utc.from_utc_datetime(&(naive - Duration::seconds(i64::from(before.local_minus_utc()))))
        }
    }
}

/// Prochain créneau parmi les programmations activées.
pub fn next_slot<Tz: TimeZone>(schedules: &[Schedule], now: DateTime<Utc>, tz: &Tz) -> Option<Slot> {
    schedules.iter().filter(|s| s.enabled).filter_map(|s| s.next_slot(now, tz)).min_by_key(|slot| slot.start)
}

/// Une programmation au moins a encore un créneau, en cours ou à venir.
pub fn any_active<Tz: TimeZone>(schedules: &[Schedule], now: DateTime<Utc>, tz: &Tz) -> bool {
    schedules.iter().any(|s| s.is_active(now, tz))
}

/// Moment d'un créneau, vu de `now` : « aujourd'hui à 21:00 », « demain à
/// 21:00 », « lundi à 21:00 » (dans la semaine), « le 02/11 à 21:00 ».
pub fn when_label<Tz: TimeZone>(at: DateTime<Utc>, now: DateTime<Utc>, tz: &Tz) -> String {
    let at = at.with_timezone(tz).naive_local();
    let today = now.with_timezone(tz).date_naive();
    let time = crate::clock::time(at.hour(), at.minute());
    match (at.date() - today).num_days() {
        0 => tr!("aujourd'hui à {}", time),
        1 => tr!("demain à {}", time),
        2..=6 => tr!("{} à {}", day_name(at.weekday().number_from_monday() as u8), time),
        _ => tr!("le {} à {}", crate::clock::date(at.date(), today), time),
    }
}

/// Heure locale d'un instant : « 22:00 ».
pub fn clock_label<Tz: TimeZone>(at: DateTime<Utc>, tz: &Tz) -> String {
    let at = at.with_timezone(tz).naive_local();
    crate::clock::time(at.hour(), at.minute())
}

fn default_true() -> bool {
    true
}

// ----- Planificateur -------------------------------------------------------

/// Décision du planificateur, à exécuter par l'interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Un créneau commence : lancer sa scène.
    Start(Slot),
    /// Le créneau a commencé alors que Polyptyque était fermé ou
    /// l'ordinateur en veille : demander s'il faut lancer l'enregistrement,
    /// avec un compte à rebours ; sans réponse, il démarre.
    AskMissed(Slot),
    /// Un enregistrement lancé à la main est déjà en cours au début du
    /// créneau : il continue tel quel, la programmation ne lance rien et ne
    /// l'arrêtera pas.
    ManualRunning(Slot),
    /// Fin du créneau de l'enregistrement dont la programmation a la charge :
    /// l'arrêter.
    Stop(Slot),
}

/// Créneau réglé : plus rien à y lancer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DoneSlot {
    schedule_id: String,
    day: NaiveDate,
    /// Fin du créneau : passé cette heure, le souvenir est inutile.
    end: DateTime<Utc>,
}

#[derive(Default, Serialize, Deserialize)]
struct PlannerFile {
    #[serde(default)]
    done: Vec<DoneSlot>,
}

/// Décide de ce que fait la programmation, à chaque instant.
///
/// Principe : la programmation n'arrête que l'enregistrement dont elle a la
/// charge (lancé par elle, ou lancé à la main avec l'arrêt automatique
/// accepté), jamais un autre. Un créneau « réglé » (enregistrement refusé,
/// arrêté à la main, impossible, ou enregistrement manuel déjà en cours à
/// son début) n'est plus relancé ; ce souvenir est gardé sur le disque pour
/// survivre à une fermeture de Polyptyque. Un enregistrement interrompu
/// malgré lui (plantage, arrêt de Windows, mise en veille) laisse au
/// contraire le créneau à reprendre : au retour, la question du démarrage
/// manqué est posée.
#[derive(Default)]
pub struct Planner {
    done: Vec<DoneSlot>,
    /// Créneau dont l'enregistrement en cours dépend.
    owned: Option<Slot>,
    /// Créneau en cours de traitement : démarrage ou question en attente.
    handling: Option<SlotKey>,
    /// Début de l'observation continue : Polyptyque ouvert, ordinateur
    /// éveillé, horloge sans saut.
    awake_since: Option<DateTime<Utc>>,
    last_seen: Option<DateTime<Utc>>,
    /// Programmations créées, modifiées ou activées pendant cette séance, et
    /// quand : un créneau déjà commencé à ce moment-là n'a pas été « vu »
    /// commencer, il fait l'objet d'une question.
    changed: Vec<(String, DateTime<Utc>)>,
    /// Créneaux réglés modifiés depuis le dernier enregistrement sur le disque.
    dirty: bool,
}

impl Planner {
    /// Reprend les créneaux réglés gardés sur le disque. Fichier absent :
    /// aucun. Fichier illisible : aucun non plus (au pire, une question de
    /// démarrage manqué), avec un message pour le journal.
    pub fn load(path: &Path) -> (Planner, Option<String>) {
        let (done, warning) = match fs::read_to_string(path) {
            Ok(text) => match toml::from_str::<PlannerFile>(&text) {
                Ok(file) => (file.done, None),
                Err(e) => (Vec::new(), Some(format!("état des programmations illisible : {e}"))),
            },
            Err(e) if e.kind() == ErrorKind::NotFound => (Vec::new(), None),
            Err(e) => (Vec::new(), Some(format!("état des programmations illisible : {e}"))),
        };
        (Planner { done, ..Planner::default() }, warning)
    }

    /// Enregistre les créneaux réglés, s'ils ont changé.
    pub fn save_if_changed(&mut self, path: &Path) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.dirty = false;
        let text =
            toml::to_string_pretty(&PlannerFile { done: self.done.clone() }).context("état des programmations")?;
        storage::write_atomic(path, text.as_bytes())
    }

    /// Créneau dont l'enregistrement en cours dépend.
    pub fn owned(&self) -> Option<&Slot> {
        self.owned.as_ref()
    }

    pub fn is_done(&self, key: &SlotKey) -> bool {
        self.done.iter().any(|d| d.schedule_id == key.schedule_id && d.day == key.day)
    }

    /// À appeler régulièrement (deux fois par seconde), y compris pendant
    /// une opération longue (`busy`) : rien n'est alors décidé, mais
    /// l'observation continue. `recording` : un enregistrement est en cours.
    pub fn tick<Tz: TimeZone>(
        &mut self,
        schedules: &[Schedule],
        now: DateTime<Utc>,
        tz: &Tz,
        recording: bool,
        busy: bool,
    ) -> Option<Action> {
        self.observe(now);
        if let Some(owned) = self.owned.as_ref().filter(|owned| now >= owned.end).cloned() {
            self.owned = None;
            self.mark_done(&owned);
            return Some(Action::Stop(owned));
        }
        if busy || self.handling.is_some() {
            return None;
        }
        self.prune(now);
        for schedule in schedules.iter().filter(|s| s.enabled) {
            let Some(slot) = schedule.slot_at(now, tz) else {
                continue;
            };
            let key = slot.key();
            if self.is_done(&key) || self.owned.as_ref().is_some_and(|owned| owned.key() == key) {
                continue;
            }
            if recording {
                self.mark_done(&slot);
                return Some(Action::ManualRunning(slot));
            }
            self.handling = Some(key);
            let changed = self.changed.iter().find(|(id, _)| *id == slot.schedule_id).map(|(_, at)| *at);
            let seen = self.awake_since.is_some_and(|since| slot.start >= since)
                && changed.is_none_or(|changed| slot.start >= changed);
            let on_time = seen || now - slot.start <= START_GRACE;
            return Some(if on_time { Action::Start(slot) } else { Action::AskMissed(slot) });
        }
        None
    }

    /// Le créneau traité a lancé son enregistrement : la programmation en a
    /// la charge.
    pub fn started(&mut self, slot: Slot) {
        self.handling = None;
        self.owned = Some(slot);
    }

    /// Le créneau traité ne lancera rien : enregistrement refusé, ou
    /// impossible à démarrer.
    pub fn give_up(&mut self, slot: &Slot) {
        self.handling = None;
        self.mark_done(slot);
    }

    /// Enregistrement lancé à la main pendant ce créneau : plus rien à y
    /// lancer. Avec `auto_stop`, la programmation l'arrêtera à la fin du
    /// créneau.
    pub fn claim(&mut self, slot: &Slot, auto_stop: bool) {
        if self.handling.as_ref().is_some_and(|key| *key == slot.key()) {
            self.handling = None;
        }
        self.mark_done(slot);
        if auto_stop {
            self.owned = Some(slot.clone());
        }
    }

    /// L'utilisateur garde finalement l'enregistrement en cours au-delà du
    /// créneau : la programmation ne l'arrêtera pas.
    pub fn release(&mut self) {
        self.owned = None;
    }

    /// Créneau en cours dont la programmation n'a pas la charge de
    /// l'enregistrement : à proposer quand on lance un enregistrement à la
    /// main.
    pub fn manual_slot<Tz: TimeZone>(&self, schedules: &[Schedule], now: DateTime<Utc>, tz: &Tz) -> Option<Slot> {
        schedules
            .iter()
            .filter(|s| s.enabled)
            .filter_map(|s| s.slot_at(now, tz))
            .find(|slot| self.owned.as_ref().is_none_or(|owned| owned.key() != slot.key()))
    }

    /// L'enregistrement en cours s'arrête pour de bon (arrêt voulu, disque
    /// plein…) : s'il dépendait d'un créneau, celui-ci n'est pas relancé.
    pub fn recording_stopped(&mut self) {
        if let Some(owned) = self.owned.take() {
            self.mark_done(&owned);
        }
    }

    /// L'enregistrement en cours est interrompu malgré lui (mise en veille) :
    /// son créneau reste à reprendre.
    pub fn recording_interrupted(&mut self) {
        self.owned = None;
    }

    /// Programmation créée, modifiée, activée ou supprimée à l'instant
    /// `now` : ses créneaux réglés sont oubliés, et un créneau déjà commencé
    /// fera l'objet d'une question plutôt que d'un démarrage d'office.
    pub fn schedule_changed(&mut self, schedule_id: &str, now: DateTime<Utc>) {
        let before = self.done.len();
        self.done.retain(|d| d.schedule_id != schedule_id);
        self.dirty |= self.done.len() != before;
        self.changed.retain(|(id, _)| id != schedule_id);
        self.changed.push((schedule_id.to_string(), now));
    }

    fn observe(&mut self, now: DateTime<Utc>) {
        let continuous = self.last_seen.is_some_and(|last| now >= last && now - last <= MAX_GAP);
        if !continuous {
            self.awake_since = Some(now);
        }
        self.last_seen = Some(now);
    }

    fn mark_done(&mut self, slot: &Slot) {
        if !self.is_done(&slot.key()) {
            self.done.push(DoneSlot { schedule_id: slot.schedule_id.clone(), day: slot.day, end: slot.end });
            self.dirty = true;
        }
    }

    fn prune(&mut self, now: DateTime<Utc>) {
        let before = self.done.len();
        self.done.retain(|d| d.end > now);
        self.dirty |= self.done.len() != before;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::Paris;

    fn time(text: &str) -> TimeOfDay {
        TimeOfDay::parse(text).unwrap()
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// Instant correspondant à une heure de Paris sans ambiguïté.
    fn paris(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Paris.with_ymd_and_hms(y, m, d, h, min, 0).single().unwrap().with_timezone(&Utc)
    }

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    fn weekly(start: &str, end: &str, days: &[u8]) -> Schedule {
        Schedule::new("scene", time(start), time(end), Repeat::Weekly { days: days.to_vec() })
    }

    fn once(start: &str, end: &str, day: NaiveDate) -> Schedule {
        Schedule::new("scene", time(start), time(end), Repeat::Once { date: day })
    }

    const ALL: [u8; 7] = [1, 2, 3, 4, 5, 6, 7];

    #[test]
    fn times_are_read_leniently() {
        assert_eq!(time("21:00").to_string(), "21:00");
        assert_eq!(time(" 21h30 ").to_string(), "21:30");
        assert_eq!(time("21h").to_string(), "21:00");
        assert_eq!(time("9").to_string(), "09:00");
        assert_eq!(time("9:5").to_string(), "09:05");
        assert_eq!(time("21.45").to_string(), "21:45");
        assert_eq!(time("9:05 PM").to_string(), "21:05");
        assert_eq!(time("12 am").to_string(), "00:00");
        for bad in ["", "24:00", "12:60", "abc", "123:00", "12:000", "h30"] {
            assert_eq!(TimeOfDay::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn schedules_survive_the_settings_file() {
        let schedules = vec![weekly("22:00", "02:00", &[1, 3]), once("21:00", "22:00", date(2026, 10, 25))];
        #[derive(Serialize, Deserialize)]
        struct File {
            schedules: Vec<Schedule>,
        }
        let text = toml::to_string_pretty(&File { schedules: schedules.clone() }).unwrap();
        assert!(text.contains("start = \"22:00\""));
        assert!(text.contains("date = \"2026-10-25\""));
        let back: File = toml::from_str(&text).unwrap();
        assert_eq!(back.schedules, schedules);
        assert!(toml::from_str::<File>(&text.replace("22:00", "25:00")).is_err());
    }

    #[test]
    fn daily_slot() {
        let s = weekly("21:00", "22:00", &ALL);
        let slot = s.slot_on(date(2026, 10, 23), &Paris).unwrap();
        assert_eq!(slot.start, paris(2026, 10, 23, 21, 0));
        assert_eq!(slot.end, paris(2026, 10, 23, 22, 0));
        assert_eq!(s.slot_at(paris(2026, 10, 23, 21, 30), &Paris), Some(slot.clone()));
        assert_eq!(s.slot_at(paris(2026, 10, 23, 22, 0), &Paris), None);
        assert_eq!(s.next_slot(paris(2026, 10, 23, 21, 30), &Paris).unwrap().day, date(2026, 10, 24));
        assert_eq!(s.next_slot(paris(2026, 10, 23, 20, 0), &Paris), Some(slot));
    }

    #[test]
    fn slot_crossing_midnight_belongs_to_its_start_day() {
        // Vendredi 23/10/2026, 22:00 → samedi 02:00, programmé les jours de semaine.
        let s = weekly("22:00", "02:00", &[1, 2, 3, 4, 5]);
        assert!(s.crosses_midnight());
        let slot = s.slot_at(paris(2026, 10, 24, 1, 0), &Paris).unwrap();
        assert_eq!(slot.day, date(2026, 10, 23));
        assert_eq!(slot.end, paris(2026, 10, 24, 2, 0));
        // Samedi soir : pas de créneau.
        assert_eq!(s.slot_at(paris(2026, 10, 24, 23, 0), &Paris), None);
        // Prochain : lundi soir.
        assert_eq!(s.next_slot(paris(2026, 10, 24, 1, 0), &Paris).unwrap().day, date(2026, 10, 26));
    }

    #[test]
    fn autumn_clock_change_lengthens_the_night() {
        // Dimanche 25/10/2026 : à 03:00, on revient à 02:00.
        let s = weekly("01:00", "04:00", &ALL);
        let slot = s.slot_on(date(2026, 10, 25), &Paris).unwrap();
        assert_eq!(slot.start, utc(2026, 10, 24, 23, 0));
        assert_eq!(slot.end, utc(2026, 10, 25, 3, 0));
        assert_eq!(slot.end - slot.start, Duration::hours(4));
        // 02:30 existe deux fois : la première (encore en heure d'été).
        let s = weekly("02:30", "05:00", &ALL);
        assert_eq!(s.slot_on(date(2026, 10, 25), &Paris).unwrap().start, utc(2026, 10, 25, 0, 30));
        // Début et fin dans l'heure double : tous deux à la première.
        let s = weekly("02:10", "02:50", &ALL);
        let slot = s.slot_on(date(2026, 10, 25), &Paris).unwrap();
        assert_eq!(slot.end - slot.start, Duration::minutes(40));
    }

    #[test]
    fn spring_clock_change_shortens_the_night() {
        // Dimanche 29/03/2026 : à 02:00, on passe à 03:00.
        let s = weekly("01:00", "04:00", &ALL);
        let slot = s.slot_on(date(2026, 3, 29), &Paris).unwrap();
        assert_eq!(slot.end - slot.start, Duration::hours(2));
        // 02:30 n'existe pas : 03:30, comme dans les agendas.
        let s = weekly("02:30", "05:00", &ALL);
        assert_eq!(s.slot_on(date(2026, 3, 29), &Paris).unwrap().start, paris(2026, 3, 29, 3, 30));
        // Créneau 02:15 → 03:00 : vide cette nuit-là.
        let s = weekly("02:15", "03:00", &ALL);
        assert_eq!(s.slot_on(date(2026, 3, 29), &Paris), None);
        assert!(s.slot_on(date(2026, 3, 30), &Paris).is_some());
    }

    #[test]
    fn distant_once_schedule_is_upcoming() {
        // Une date lointaine (au-delà de la semaine examinée pour une
        // répétition) reste à venir : liste, pastille, anti-veille.
        let s = once("21:00", "22:00", date(2027, 10, 31));
        let now = paris(2026, 10, 25, 10, 0);
        assert_eq!(s.next_slot(now, &Paris).unwrap().start, paris(2027, 10, 31, 21, 0));
        assert!(s.is_active(now, &Paris));
        assert!(any_active(std::slice::from_ref(&s), now, &Paris));
        assert_eq!(next_slot(&[s], now, &Paris).unwrap().day, date(2027, 10, 31));
    }

    #[test]
    fn once_schedule_expires() {
        let s = once("21:00", "22:00", date(2026, 10, 25));
        assert!(s.is_active(paris(2026, 10, 23, 12, 0), &Paris));
        assert!(s.is_active(paris(2026, 10, 25, 21, 30), &Paris));
        assert!(!s.is_active(paris(2026, 10, 25, 22, 0), &Paris));
        let mut off = s.clone();
        off.enabled = false;
        assert!(!off.is_active(paris(2026, 10, 23, 12, 0), &Paris));
    }

    #[test]
    fn validation() {
        let now = paris(2026, 10, 23, 12, 0);
        assert_eq!(weekly("21:00", "21:00", &ALL).validate(&[], now, &Paris), Err(ScheduleError::SameTimes));
        assert_eq!(weekly("21:00", "22:00", &[]).validate(&[], now, &Paris), Err(ScheduleError::NoDay));
        assert_eq!(once("10:00", "11:00", date(2026, 10, 23)).validate(&[], now, &Paris), Err(ScheduleError::InPast));
        // En cours : encore valable.
        assert_eq!(once("11:00", "13:00", date(2026, 10, 23)).validate(&[], now, &Paris), Ok(()));

        let daily = weekly("21:00", "22:00", &ALL);
        let others = [daily.clone()];
        let overlap = once("21:30", "23:00", date(2026, 10, 30));
        assert_eq!(overlap.validate(&others, now, &Paris), Err(ScheduleError::Overlap(daily.id.clone())));
        // Bout à bout : accepté.
        assert_eq!(once("22:00", "23:00", date(2026, 10, 30)).validate(&others, now, &Paris), Ok(()));
        // La nuit qui passe minuit chevauche le créneau matinal du lendemain.
        let night = weekly("23:00", "02:00", &[5]);
        let morning = weekly("01:00", "03:00", &[6]);
        assert!(matches!(morning.validate(std::slice::from_ref(&night), now, &Paris), Err(ScheduleError::Overlap(_))));
        // Une programmation désactivée ne gêne pas, et n'est pas gênée.
        let mut off = night.clone();
        off.enabled = false;
        assert_eq!(morning.validate(&[off.clone()], now, &Paris), Ok(()));
        assert_eq!(off.validate(&[morning], now, &Paris), Ok(()));
        // Elle-même ne compte pas.
        assert_eq!(daily.validate(&others, now, &Paris), Ok(()));
    }

    #[test]
    fn labels() {
        assert_eq!(weekly("21:00", "22:00", &ALL).repeat_label(), "Tous les jours");
        assert_eq!(weekly("21:00", "22:00", &[5, 4, 3, 2, 1]).repeat_label(), "Du lundi au vendredi");
        assert_eq!(weekly("21:00", "22:00", &[6, 7]).repeat_label(), "Le week-end");
        assert_eq!(weekly("21:00", "22:00", &[3]).repeat_label(), "Chaque mercredi");
        assert_eq!(weekly("21:00", "22:00", &[1, 3, 5]).repeat_label(), "Lun, Mer, Ven");
        assert_eq!(
            once("21:00", "22:00", date(2026, 10, 25)).repeat_label_at(date(2026, 10, 1)),
            "Une fois, le dim. 25 oct."
        );
        assert_eq!(weekly("22:00", "02:00", &ALL).times_label(), "22:00 → 02:00 (lendemain)");
        let now = paris(2026, 10, 23, 12, 0);
        assert_eq!(when_label(paris(2026, 10, 23, 21, 0), now, &Paris), "aujourd'hui à 21:00");
        assert_eq!(when_label(paris(2026, 10, 24, 21, 0), now, &Paris), "demain à 21:00");
        assert_eq!(when_label(paris(2026, 10, 26, 21, 0), now, &Paris), "lundi à 21:00");
        assert_eq!(when_label(paris(2026, 11, 2, 21, 0), now, &Paris), "le lun. 2 nov. à 21:00");
    }

    // ----- Planificateur ---------------------------------------------------

    /// Fait avancer le planificateur de `from` à `to`, deux fois par
    /// seconde, et renvoie les décisions prises.
    fn run(
        planner: &mut Planner,
        schedules: &[Schedule],
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        recording: bool,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        let mut now = from;
        while now <= to {
            actions.extend(planner.tick(schedules, now, &Paris, recording, false));
            now += Duration::milliseconds(500);
        }
        actions
    }

    #[test]
    fn slot_starts_on_time_then_stops() {
        let schedules = [weekly("21:00", "22:00", &ALL)];
        let mut planner = Planner::default();
        let actions = run(&mut planner, &schedules, paris(2026, 10, 23, 20, 59), paris(2026, 10, 23, 21, 1), false);
        let [Action::Start(slot)] = actions.as_slice() else { panic!("{actions:?}") };
        planner.started(slot.clone());
        let actions = run(&mut planner, &schedules, paris(2026, 10, 23, 21, 1), paris(2026, 10, 23, 22, 1), true);
        assert_eq!(actions, vec![Action::Stop(slot.clone())]);
        // Rien d'autre ce soir-là.
        assert!(run(&mut planner, &schedules, paris(2026, 10, 23, 22, 1), paris(2026, 10, 23, 22, 2), false).is_empty());
    }

    #[test]
    fn opening_polyptyque_during_a_slot_asks() {
        let schedules = [weekly("21:00", "22:00", &ALL)];
        // Ouvert dans les 30 s : démarrage direct.
        let mut planner = Planner::default();
        let actions =
            planner.tick(&schedules, paris(2026, 10, 23, 21, 0) + Duration::seconds(20), &Paris, false, false);
        assert!(matches!(actions, Some(Action::Start(_))));
        // Ouvert plus tard : question.
        let mut planner = Planner::default();
        let action = planner.tick(&schedules, paris(2026, 10, 23, 21, 10), &Paris, false, false);
        let Some(Action::AskMissed(slot)) = action else { panic!("{action:?}") };
        // Pas de seconde question tant que la première attend.
        assert!(
            run(&mut planner, &schedules, paris(2026, 10, 23, 21, 10), paris(2026, 10, 23, 21, 12), false).is_empty()
        );
        // Refus : plus rien dans ce créneau.
        planner.give_up(&slot);
        assert!(
            run(&mut planner, &schedules, paris(2026, 10, 23, 21, 12), paris(2026, 10, 23, 21, 14), false).is_empty()
        );
    }

    #[test]
    fn waking_up_during_a_slot_asks() {
        let schedules = [weekly("21:00", "22:00", &ALL)];
        let mut planner = Planner::default();
        assert!(run(&mut planner, &schedules, paris(2026, 10, 23, 20, 0), paris(2026, 10, 23, 20, 1), false).is_empty());
        // L'ordinateur dort de 20:01 à 21:05.
        let action = planner.tick(&schedules, paris(2026, 10, 23, 21, 5), &Paris, false, false);
        assert!(matches!(action, Some(Action::AskMissed(_))), "{action:?}");
    }

    #[test]
    fn manual_recording_at_slot_start_is_left_alone() {
        let schedules = [weekly("21:00", "22:00", &ALL)];
        let mut planner = Planner::default();
        let actions = run(&mut planner, &schedules, paris(2026, 10, 23, 20, 59), paris(2026, 10, 23, 21, 1), true);
        assert!(matches!(actions.as_slice(), [Action::ManualRunning(_)]), "{actions:?}");
        // Arrêté à la main à 21:30 : la programmation ne lance rien, et
        // n'arrête rien à 22:00.
        let actions = run(&mut planner, &schedules, paris(2026, 10, 23, 21, 30), paris(2026, 10, 23, 22, 1), false);
        assert!(actions.is_empty(), "{actions:?}");
    }

    #[test]
    fn manual_stop_is_not_restarted_even_after_a_restart() {
        let dir = std::env::temp_dir().join(format!("polyptyque-schedule-{}", new_id()));
        let path = dir.join("programmation.toml");
        let schedules = [weekly("21:00", "22:00", &ALL)];
        let (mut planner, warning) = Planner::load(&path);
        assert_eq!(warning, None);
        let actions = run(&mut planner, &schedules, paris(2026, 10, 23, 20, 59), paris(2026, 10, 23, 21, 0), false);
        let [Action::Start(slot)] = actions.as_slice() else { panic!("{actions:?}") };
        planner.started(slot.clone());
        planner.recording_stopped();
        planner.save_if_changed(&path).unwrap();
        // Polyptyque fermé puis rouvert dans le créneau.
        let (mut planner, _) = Planner::load(&path);
        assert!(
            run(&mut planner, &schedules, paris(2026, 10, 23, 21, 20), paris(2026, 10, 23, 21, 21), false).is_empty()
        );
        // Le lendemain, le créneau suivant démarre.
        let actions = run(&mut planner, &schedules, paris(2026, 10, 24, 20, 59), paris(2026, 10, 24, 21, 0), false);
        assert!(matches!(actions.as_slice(), [Action::Start(_)]), "{actions:?}");
        // Le souvenir périmé est effacé.
        planner.save_if_changed(&path).unwrap();
        assert!(!fs::read_to_string(&path).unwrap().contains("2026-10-23"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn interrupted_recording_is_offered_again() {
        let schedules = [weekly("21:00", "22:00", &ALL)];
        let mut planner = Planner::default();
        let actions = run(&mut planner, &schedules, paris(2026, 10, 23, 20, 59), paris(2026, 10, 23, 21, 0), false);
        let [Action::Start(slot)] = actions.as_slice() else { panic!("{actions:?}") };
        planner.started(slot.clone());
        // Mise en veille à 21:10, réveil à 21:40.
        planner.recording_interrupted();
        let action = planner.tick(&schedules, paris(2026, 10, 23, 21, 40), &Paris, false, false);
        assert!(matches!(action, Some(Action::AskMissed(_))), "{action:?}");
        // Plantage : un nouveau planificateur (sans souvenir) redemande aussi.
        let mut fresh = Planner::default();
        let action = fresh.tick(&schedules, paris(2026, 10, 23, 21, 40), &Paris, false, false);
        assert!(matches!(action, Some(Action::AskMissed(_))), "{action:?}");
    }

    #[test]
    fn manual_start_during_a_slot() {
        let schedules = [weekly("21:00", "22:00", &ALL)];
        let mut planner = Planner::default();
        let action = planner.tick(&schedules, paris(2026, 10, 23, 21, 10), &Paris, false, false);
        let Some(Action::AskMissed(slot)) = action else { panic!("{action:?}") };
        planner.give_up(&slot);
        // Lancé à la main à 21:20, arrêt automatique accepté.
        let manual = planner.manual_slot(&schedules, paris(2026, 10, 23, 21, 20), &Paris).unwrap();
        assert_eq!(manual.key(), slot.key());
        planner.claim(&manual, true);
        let actions = run(&mut planner, &schedules, paris(2026, 10, 23, 21, 20), paris(2026, 10, 23, 22, 0), true);
        assert_eq!(actions, vec![Action::Stop(manual.clone())]);
        // Sans l'arrêt automatique : rien à la fin du créneau.
        let mut planner = Planner::default();
        planner.claim(&manual, false);
        assert!(run(&mut planner, &schedules, paris(2026, 10, 23, 21, 20), paris(2026, 10, 23, 22, 1), true).is_empty());
        // Pendant l'enregistrement dont elle a la charge, la programmation
        // ne propose pas son propre créneau.
        let mut planner = Planner::default();
        planner.started(manual.clone());
        assert_eq!(planner.manual_slot(&schedules, paris(2026, 10, 23, 21, 20), &Paris), None);
    }

    #[test]
    fn busy_delays_but_does_not_miss() {
        // Créneaux bout à bout : l'arrêt du premier (finalisation) retarde
        // un peu le second, qui démarre quand même sans question.
        let schedules = [weekly("20:00", "21:00", &ALL), weekly("21:00", "22:00", &ALL)];
        let mut planner = Planner::default();
        let actions = run(&mut planner, &schedules, paris(2026, 10, 23, 19, 59), paris(2026, 10, 23, 20, 0), false);
        let [Action::Start(first)] = actions.as_slice() else { panic!("{actions:?}") };
        planner.started(first.clone());
        let actions = run(&mut planner, &schedules, paris(2026, 10, 23, 20, 0), paris(2026, 10, 23, 21, 0), true);
        assert_eq!(actions, vec![Action::Stop(first.clone())]);
        let mut now = paris(2026, 10, 23, 21, 0);
        for _ in 0..90 {
            now += Duration::milliseconds(500);
            assert_eq!(planner.tick(&schedules, now, &Paris, false, true), None);
        }
        let action = planner.tick(&schedules, now + Duration::milliseconds(500), &Paris, false, false);
        assert!(matches!(action, Some(Action::Start(ref slot)) if slot.schedule_id == schedules[1].id), "{action:?}");
    }

    #[test]
    fn schedule_created_during_its_slot_asks() {
        let mut schedule = weekly("21:00", "22:00", &ALL);
        let mut planner = Planner::default();
        assert!(run(&mut planner, &[], paris(2026, 10, 23, 20, 59), paris(2026, 10, 23, 21, 10), false).is_empty());
        planner.schedule_changed(&schedule.id, paris(2026, 10, 23, 21, 10));
        let action = planner.tick(&[schedule.clone()], paris(2026, 10, 23, 21, 10), &Paris, false, false);
        let Some(Action::AskMissed(slot)) = action else { panic!("{action:?}") };
        planner.give_up(&slot);
        // Modifiée ensuite pour reprendre plus tard dans la soirée : le
        // créneau refusé est oublié, le nouveau démarre à l'heure.
        schedule.start = time("21:30");
        planner.schedule_changed(&schedule.id, paris(2026, 10, 23, 21, 12));
        let actions = run(&mut planner, &[schedule], paris(2026, 10, 23, 21, 12), paris(2026, 10, 23, 21, 30), false);
        assert!(matches!(actions.as_slice(), [Action::Start(_)]), "{actions:?}");
    }

    #[test]
    fn disabled_schedules_do_nothing() {
        let mut schedule = weekly("21:00", "22:00", &ALL);
        schedule.enabled = false;
        let mut planner = Planner::default();
        assert!(
            run(&mut planner, &[schedule], paris(2026, 10, 23, 20, 59), paris(2026, 10, 23, 21, 1), false).is_empty()
        );
    }
}
