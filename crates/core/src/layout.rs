//! Destination layout: `AAAA/MM - NomDuMois/filename` by default (French),
//! or per day, per year, or flat.
//!
//! Date source is chosen by the caller (EXIF first, mtime fallback). This
//! module only renders the folder path from a timestamp.

use chrono::{DateTime, Datelike, TimeZone, Utc};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Fr,
    En,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateStyle {
    /// `AAAA/MM - NomDuMois/`
    YearMonthNamed(Lang),
    /// `AAAA/AAAA-MM-JJ/`
    YearDay,
    /// `AAAA/`
    Year,
    /// `AAAA-MM/`
    YearDashMonth,
    /// Flat, no date folders.
    Flat,
}

impl DateStyle {
    /// Keys used by the UI and the CLI: `month`, `day`, `year`, `flat`.
    pub fn from_key(key: &str, lang: Lang) -> Option<Self> {
        Some(match key {
            "month" => Self::YearMonthNamed(lang),
            "day" => Self::YearDay,
            "year" => Self::Year,
            "flat" => Self::Flat,
            _ => return None,
        })
    }
}

pub fn folder_from_ts(ts: DateTime<Utc>, style: DateStyle) -> PathBuf {
    match style {
        DateStyle::Flat => PathBuf::new(),
        DateStyle::Year => PathBuf::from(format!("{:04}", ts.year())),
        DateStyle::YearDay => PathBuf::from(format!("{:04}", ts.year()))
            .join(format!("{:04}-{:02}-{:02}", ts.year(), ts.month(), ts.day())),
        DateStyle::YearDashMonth => {
            PathBuf::from(format!("{:04}-{:02}", ts.year(), ts.month()))
        }
        DateStyle::YearMonthNamed(lang) => {
            let name = month_name(ts.month(), lang);
            PathBuf::from(format!("{:04}", ts.year()))
                .join(format!("{:02} - {}", ts.month(), name))
        }
    }
}

/// Dates are handled as "wall clock seconds" (local time encoded as if UTC),
/// because EXIF stores local time without a zone. Real UTC instants (file
/// mtime, video mvhd) go through this first so a photo taken at 00:30 on
/// 1 March in Paris is not filed in February.
pub fn utc_to_wall_clock(secs: i64) -> i64 {
    use chrono::{Local, Offset};
    match Utc.timestamp_opt(secs, 0).single() {
        Some(t) => secs + t.with_timezone(&Local).offset().fix().local_minus_utc() as i64,
        None => secs,
    }
}

pub fn folder_from_epoch(secs: i64, style: DateStyle) -> PathBuf {
    let ts = Utc.timestamp_opt(secs, 0).single().unwrap_or_else(|| Utc.timestamp_opt(0, 0).unwrap());
    folder_from_ts(ts, style)
}

pub fn month_name(month: u32, lang: Lang) -> &'static str {
    match lang {
        Lang::Fr => match month {
            1 => "Janvier",
            2 => "Février",
            3 => "Mars",
            4 => "Avril",
            5 => "Mai",
            6 => "Juin",
            7 => "Juillet",
            8 => "Août",
            9 => "Septembre",
            10 => "Octobre",
            11 => "Novembre",
            12 => "Décembre",
            _ => "Inconnu",
        },
        Lang::En => match month {
            1 => "January",
            2 => "February",
            3 => "March",
            4 => "April",
            5 => "May",
            6 => "June",
            7 => "July",
            8 => "August",
            9 => "September",
            10 => "October",
            11 => "November",
            12 => "December",
            _ => "Unknown",
        },
    }
}

/// Returns the last path component of a POSIX path.
pub fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}
