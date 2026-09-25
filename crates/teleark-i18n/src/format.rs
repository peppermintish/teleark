//! Central human-readable formatting policy.
//!
//! Every byte size and rate uses IEC binary units (base 1024): B, KiB, MiB,
//! GiB, TiB, PiB, EiB and the corresponding /s rate. Use this policy for all
//! application surfaces, including diagnostics, previews and status bars.

use crate::SupportedLocale;
use chrono::{DateTime, Local};

const BINARY_BASE: f64 = 1_024.0;
const BINARY_UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

pub fn format_integer(locale: SupportedLocale, value: u64) -> String {
    group_integer(locale, &value.to_string())
}

pub fn format_signed_integer(locale: SupportedLocale, value: i64) -> String {
    group_integer(locale, &value.to_string())
}

/// Formats a finite decimal with localized grouping and at most `fraction_digits`.
/// Non-finite values are rendered as an em dash instead of leaking `NaN` into UI.
pub fn format_decimal(locale: SupportedLocale, value: f64, fraction_digits: usize) -> String {
    if !value.is_finite() {
        return "—".to_owned();
    }

    let mut canonical = format!("{value:.fraction_digits$}");
    if canonical.contains('.') {
        while canonical.ends_with('0') {
            canonical.pop();
        }
        if canonical.ends_with('.') {
            canonical.pop();
        }
    }
    if canonical == "-0" {
        canonical = "0".to_owned();
    }

    let (integer, fraction) = canonical
        .split_once('.')
        .map_or((canonical.as_str(), None), |(integer, fraction)| {
            (integer, Some(fraction))
        });
    let mut localized = group_integer(locale, integer);
    if let Some(fraction) = fraction {
        localized.push(decimal_separator(locale));
        localized.push_str(fraction);
    }
    localized
}

pub fn format_bytes(locale: SupportedLocale, bytes: u64) -> String {
    if bytes < BINARY_BASE as u64 {
        return format!("{} B", format_integer(locale, bytes));
    }

    let mut scaled = bytes as f64;
    let mut unit_index = 0_usize;
    while scaled >= BINARY_BASE && unit_index < BINARY_UNITS.len() - 1 {
        scaled /= BINARY_BASE;
        unit_index += 1;
    }
    format!(
        "{} {}",
        format_decimal(locale, scaled, 1),
        BINARY_UNITS[unit_index]
    )
}

pub fn format_speed(locale: SupportedLocale, bytes_per_second: u64) -> String {
    format!("{}/s", format_bytes(locale, bytes_per_second))
}

/// Formats diagnostic and transfer durations with a stable compact policy.
pub fn format_duration_millis(locale: SupportedLocale, milliseconds: u64) -> String {
    if milliseconds < 1_000 {
        let value = format_integer(locale, milliseconds);
        return match locale {
            SupportedLocale::RuRu => format!("{value} мс"),
            SupportedLocale::KoKr => format!("{value}밀리초"),
            SupportedLocale::ZhCn => format!("{value} 毫秒"),
            SupportedLocale::JaJp => format!("{value} ミリ秒"),
            _ => format!("{value} ms"),
        };
    }
    if milliseconds < 60_000 {
        let value = format_decimal(locale, milliseconds as f64 / 1_000.0, 1);
        return match locale {
            SupportedLocale::RuRu => format!("{value} с"),
            SupportedLocale::KoKr => format!("{value}초"),
            SupportedLocale::ZhCn => format!("{value} 秒"),
            SupportedLocale::JaJp => format!("{value}秒"),
            _ => format!("{value} s"),
        };
    }
    let total_seconds = milliseconds / 1_000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    let minutes = format_integer(locale, minutes);
    let seconds = format_integer(locale, seconds);
    match locale {
        SupportedLocale::RuRu => format!("{minutes} мин {seconds} с"),
        SupportedLocale::KoKr => format!("{minutes}분 {seconds}초"),
        SupportedLocale::ZhCn => format!("{minutes} 分钟 {seconds} 秒"),
        SupportedLocale::JaJp => format!("{minutes}分{seconds}秒"),
        _ => format!("{minutes} min {seconds} s"),
    }
}

/// Formats a Unix millisecond timestamp in the user's local time zone.
///
/// Registered locales share 24-hour time but use different conventional
/// date orderings. Invalid/out-of-range instants remain visibly unavailable.
pub fn format_unix_millis(locale: SupportedLocale, unix_millis: i64) -> String {
    let Some(utc) = DateTime::from_timestamp_millis(unix_millis) else {
        return "—".to_owned();
    };
    let local = utc.with_timezone(&Local);
    match locale {
        SupportedLocale::EnUs => local.format("%m/%d/%Y %H:%M").to_string(),
        SupportedLocale::ZhCn | SupportedLocale::JaJp => local.format("%Y/%m/%d %H:%M").to_string(),
        SupportedLocale::KoKr => local.format("%Y.%m.%d %H:%M").to_string(),
        SupportedLocale::DeDe | SupportedLocale::RuRu => local.format("%d.%m.%Y %H:%M").to_string(),
        SupportedLocale::EsEs
        | SupportedLocale::FrFr
        | SupportedLocale::PtBr => local.format("%d/%m/%Y %H:%M").to_string(),
    }
}

/// Formats a ratio where `1.0` equals 100 percent.
pub fn format_percent(locale: SupportedLocale, ratio: f64, fraction_digits: usize) -> String {
    format!(
        "{}%",
        format_decimal(locale, ratio * 100.0, fraction_digits)
    )
}

fn decimal_separator(locale: SupportedLocale) -> char {
    match locale {
        SupportedLocale::EsEs
        | SupportedLocale::FrFr
        | SupportedLocale::DeDe
        | SupportedLocale::PtBr
        | SupportedLocale::RuRu => ',',
        _ => '.',
    }
}

fn grouping_separator(locale: SupportedLocale) -> char {
    match locale {
        SupportedLocale::EsEs | SupportedLocale::DeDe | SupportedLocale::PtBr => '.',
        SupportedLocale::FrFr => '\u{202f}',
        SupportedLocale::RuRu => '\u{00a0}',
        _ => ',',
    }
}

fn group_integer(locale: SupportedLocale, canonical: &str) -> String {
    let (sign, digits) = canonical
        .strip_prefix('-')
        .map_or(("", canonical), |digits| ("-", digits));
    let separator = grouping_separator(locale);
    let mut reversed = String::with_capacity(canonical.len() + canonical.len() / 3);
    for (position, digit) in digits.chars().rev().enumerate() {
        let boundary = position != 0 && position % 3 == 0;
        if boundary {
            reversed.push(separator);
        }
        reversed.push(digit);
    }
    let grouped: String = reversed.chars().rev().collect();
    format!("{sign}{grouped}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_units_cover_boundaries_and_large_sizes() {
        for (bytes, expected) in [
            (0, "0 B"),
            (999, "999 B"),
            (1023, "1,023 B"),
            (1024, "1 KiB"),
            (1536, "1.5 KiB"),
            (1024 * 1024, "1 MiB"),
            (1024 * 1024 * 1024, "1 GiB"),
            (1_u64 << 40, "1 TiB"),
            (1_u64 << 50, "1 PiB"),
            (1_u64 << 60, "1 EiB"),
            (u64::MAX, "16 EiB"),
        ] {
            assert_eq!(format_bytes(SupportedLocale::EnUs, bytes), expected);
        }
    }

    #[test]
    fn speed_reuses_exactly_the_same_size_policy() {
        assert_eq!(
            format_speed(SupportedLocale::EnUs, 18 * 1024 * 1024),
            "18 MiB/s"
        );
        assert_eq!(format_speed(SupportedLocale::EnUs, 1024), "1 KiB/s");
    }

    #[test]
    fn durations_cover_milliseconds_seconds_and_minutes() {
        assert_eq!(format_duration_millis(SupportedLocale::EnUs, 245), "245 ms");
        assert_eq!(
            format_duration_millis(SupportedLocale::ZhCn, 12_460),
            "12.5 秒"
        );
        assert_eq!(
            format_duration_millis(SupportedLocale::JaJp, 125_000),
            "2分5秒"
        );
    }

    #[test]
    fn counts_and_percentages_are_centralized() {
        assert_eq!(
            format_integer(SupportedLocale::EnUs, 2_851_233),
            "2,851,233"
        );
        assert_eq!(format_percent(SupportedLocale::ZhCn, 0.456, 1), "45.6%");
    }

    #[test]
    fn decimal_formatting_rounds_trims_and_handles_non_finite_values() {
        assert_eq!(
            format_decimal(SupportedLocale::JaJp, 12_345.600, 2),
            "12,345.6"
        );
        assert_eq!(format_decimal(SupportedLocale::EnUs, f64::NAN, 1), "—");
    }

    #[test]
    fn registered_languages_use_their_number_conventions_and_binary_units() {
        for locale in [
            SupportedLocale::EsEs,
            SupportedLocale::DeDe,
            SupportedLocale::PtBr,
        ] {
            assert_eq!(format_decimal(locale, 12_345.6, 1), "12.345,6");
            assert_eq!(format_bytes(locale, 1536), "1,5 KiB");
        }
        assert_eq!(
            format_decimal(SupportedLocale::FrFr, 12_345.6, 1),
            "12\u{202f}345,6"
        );
        assert_eq!(
            format_decimal(SupportedLocale::RuRu, 12_345.6, 1),
            "12\u{00a0}345,6"
        );
        assert_eq!(
            format_decimal(SupportedLocale::KoKr, 12_345.6, 1),
            "12,345.6"
        );
        for locale in SupportedLocale::ALL {
            assert!(format_speed(locale, 1_048_576).ends_with("MiB/s"));
        }
    }

    #[test]
    fn invalid_timestamps_are_rendered_as_unavailable() {
        assert_eq!(format_unix_millis(SupportedLocale::EnUs, i64::MAX), "—");
        assert_eq!(format_unix_millis(SupportedLocale::JaJp, i64::MIN), "—");
    }
}
