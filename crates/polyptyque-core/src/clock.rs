//! Heures et dates telles qu'elles s'affichent (voir « Langues et formats »
//! dans le cahier des charges) : horloge sur 24 h ou 12 h selon le réglage,
//! dates écrites avec le nom du mois, dans la langue de l'interface.
//!
//! Les fichiers (réglages, journaux, noms d'enregistrements) gardent leurs
//! formats fixes : seul l'affichage passe par ici.

use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{Datelike, NaiveDate, Weekday};
use tr::tr;

static TWELVE_HOUR: AtomicBool = AtomicBool::new(false);

/// Horloge en vigueur (réglage « Horloge », ou format régional de Windows en
/// automatique), fixée par l'interface.
pub fn set_twelve_hour(on: bool) {
    TWELVE_HOUR.store(on, Ordering::Relaxed);
}

pub fn twelve_hour() -> bool {
    TWELVE_HOUR.load(Ordering::Relaxed)
}

/// « 21:05 », ou « 9:05 PM » en 12 h.
pub fn time(hour: u32, minute: u32) -> String {
    time_with(twelve_hour(), hour, minute)
}

fn time_with(twelve_hour: bool, hour: u32, minute: u32) -> String {
    if twelve_hour {
        let suffix = if hour < 12 { "AM" } else { "PM" };
        let hour = match hour % 12 {
            0 => 12,
            h => h,
        };
        format!("{hour}:{minute:02} {suffix}")
    } else {
        format!("{hour:02}:{minute:02}")
    }
}

/// Lit une heure saisie : « 21:00 », « 21h », « 9:00 PM », « 9 pm »,
/// « 12 AM » (minuit), quelle que soit l'horloge.
pub fn parse_time(text: &str) -> Option<(u32, u32)> {
    let lower = text.trim().to_ascii_lowercase().replace('.', "");
    let (body, half) = if let Some(rest) = lower.strip_suffix("am") {
        (rest.trim(), Some(false))
    } else if let Some(rest) = lower.strip_suffix("pm") {
        (rest.trim(), Some(true))
    } else {
        (lower.as_str(), None)
    };
    let (hour, minute) = match body.split_once([':', 'h']) {
        Some((h, m)) => (h.trim(), m.trim()),
        None => (body, ""),
    };
    let hour: u32 = hour.parse().ok()?;
    let minute: u32 = if minute.is_empty() { 0 } else { minute.parse().ok()? };
    if minute > 59 {
        return None;
    }
    let hour = match half {
        Some(pm) => {
            if !(1..=12).contains(&hour) {
                return None;
            }
            (hour % 12) + if pm { 12 } else { 0 }
        }
        None if hour <= 23 => hour,
        None => return None,
    };
    Some((hour, minute))
}

/// Jour abrégé, dans une date : « dim. », « Sun ».
fn weekday_short(day: Weekday) -> String {
    match day {
        Weekday::Mon => tr!("Jour abrégé dans une date" => "lun."),
        Weekday::Tue => tr!("Jour abrégé dans une date" => "mar."),
        Weekday::Wed => tr!("Jour abrégé dans une date" => "mer."),
        Weekday::Thu => tr!("Jour abrégé dans une date" => "jeu."),
        Weekday::Fri => tr!("Jour abrégé dans une date" => "ven."),
        Weekday::Sat => tr!("Jour abrégé dans une date" => "sam."),
        Weekday::Sun => tr!("Jour abrégé dans une date" => "dim."),
    }
}

/// Mois abrégé : « oct. », « Oct ».
fn month_short(month: u32) -> String {
    match month {
        1 => tr!("Mois abrégé" => "janv."),
        2 => tr!("Mois abrégé" => "févr."),
        3 => tr!("Mois abrégé" => "mars"),
        4 => tr!("Mois abrégé" => "avr."),
        5 => tr!("Mois abrégé" => "mai"),
        6 => tr!("Mois abrégé" => "juin"),
        7 => tr!("Mois abrégé" => "juil."),
        8 => tr!("Mois abrégé" => "août"),
        9 => tr!("Mois abrégé" => "sept."),
        10 => tr!("Mois abrégé" => "oct."),
        11 => tr!("Mois abrégé" => "nov."),
        _ => tr!("Mois abrégé" => "déc."),
    }
}

/// Nom complet d'un mois (1 à 12) : « octobre », « October ».
pub fn month_name(month: u32) -> String {
    match month {
        1 => tr!("janvier"),
        2 => tr!("février"),
        3 => tr!("mars"),
        4 => tr!("avril"),
        5 => tr!("mai"),
        6 => tr!("juin"),
        7 => tr!("juillet"),
        8 => tr!("août"),
        9 => tr!("septembre"),
        10 => tr!("octobre"),
        11 => tr!("novembre"),
        _ => tr!("décembre"),
    }
}

/// Nombre de jours d'un mois (1 à 12) : 28 à 31.
pub fn days_in_month(year: i32, month: u32) -> u32 {
    let next =
        if month == 12 { NaiveDate::from_ymd_opt(year + 1, 1, 1) } else { NaiveDate::from_ymd_opt(year, month + 1, 1) };
    next.and_then(|d| d.pred_opt()).map_or(31, |d| d.day())
}

/// « dim. 25 oct. » (« Sun, Oct 25 »), avec l'année si ce n'est pas celle
/// de `today` : « dim. 25 oct. 2027 » (« Sun, Oct 25, 2027 »).
pub fn date(date: NaiveDate, today: NaiveDate) -> String {
    let (weekday, day, month) = (weekday_short(date.weekday()), date.day(), month_short(date.month()));
    if date.year() == today.year() {
        tr!("{weekday} {day} {month}", weekday, day, month)
    } else {
        let year = date.year();
        tr!("{weekday} {day} {month} {year}", weekday, day, month, year)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_times_in_both_clocks() {
        assert_eq!(parse_time("21:00"), Some((21, 0)));
        assert_eq!(parse_time("21h30"), Some((21, 30)));
        assert_eq!(parse_time("9:05 PM"), Some((21, 5)));
        assert_eq!(parse_time("9 pm"), Some((21, 0)));
        assert_eq!(parse_time("12 AM"), Some((0, 0)));
        assert_eq!(parse_time("12:30 p.m."), Some((12, 30)));
        assert_eq!(parse_time("13 PM"), None);
        assert_eq!(parse_time("24:00"), None);
        assert_eq!(parse_time("9:61"), None);
    }

    #[test]
    fn writes_times_in_both_clocks() {
        // Sans toucher à l'horloge partagée : les autres tests tournent en
        // même temps.
        assert_eq!(time_with(false, 21, 5), "21:05");
        assert_eq!(time_with(true, 21, 5), "9:05 PM");
        assert_eq!(time_with(true, 0, 0), "12:00 AM");
        assert_eq!(time_with(true, 12, 0), "12:00 PM");
    }

    #[test]
    fn counts_days_in_month() {
        assert_eq!(days_in_month(2026, 10), 31);
        assert_eq!(days_in_month(2026, 11), 30);
        assert_eq!(days_in_month(2026, 12), 31);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2028, 2), 29);
    }

    #[test]
    fn writes_dates_with_month_names() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
        assert_eq!(date(NaiveDate::from_ymd_opt(2026, 10, 25).unwrap(), today), "dim. 25 oct.");
        assert_eq!(date(NaiveDate::from_ymd_opt(2027, 1, 3).unwrap(), today), "dim. 3 janv. 2027");
    }
}
