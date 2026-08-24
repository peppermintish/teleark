//! Central human-readable formatting policy.
//!
//! File sizes and rates use decimal SI units (base 1000) to match TeleArk's UI
//! references: `MB`, `GB`, and `MB/s`. Exact protocol/configuration values remain
//! explicit IEC values such as `1900 MiB`; this module must not relabel them.

use crate::SupportedLocale;

const DECIMAL_BASE: f64 = 1_000.0;
const DECIMAL_UNITS: [&str; 7] = ["B", "kB", "MB", "GB", "TB", "PB", "EB"];

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
    if bytes < DECIMAL_BASE as u64 {
        return format!("{} B", format_integer(locale, bytes));
    }

    let mut scaled = bytes as f64;
    let mut unit_index = 0_usize;
    while scaled >= DECIMAL_BASE && unit_index < DECIMAL_UNITS.len() - 1 {
        scaled /= DECIMAL_BASE;
        unit_index += 1;
    }
    format!(
        "{} {}",
        format_decimal(locale, scaled, 1),
        DECIMAL_UNITS[unit_index]
    )
}

pub fn format_speed(locale: SupportedLocale, bytes_per_second: u64) -> String {
    format!("{}/s", format_bytes(locale, bytes_per_second))
}

/// Formats a ratio where `1.0` equals 100 percent.
pub fn format_percent(locale: SupportedLocale, ratio: f64, fraction_digits: usize) -> String {
    format!(
        "{}%",
        format_decimal(locale, ratio * 100.0, fraction_digits)
    )
}

fn decimal_separator(_locale: SupportedLocale) -> char {
    // All first-release locales conventionally use a decimal point. Keeping the
    // decision here makes adding a comma-decimal locale a data-localized change.
    '.'
}

fn grouping_separator(_locale: SupportedLocale) -> char {
    // en-US, zh-CN, and ja-JP all conventionally group decimal thousands with a comma.
    ','
}

fn group_integer(locale: SupportedLocale, canonical: &str) -> String {
    let (sign, digits) = canonical
        .strip_prefix('-')
        .map_or(("", canonical), |digits| ("-", digits));
    let separator = grouping_separator(locale);
    let mut reversed = String::with_capacity(canonical.len() + canonical.len() / 3);
    for (position, digit) in digits.chars().rev().enumerate() {
        if position != 0 && position % 3 == 0 {
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
    fn decimal_si_file_sizes_match_reference_style() {
        assert_eq!(
            format_bytes(SupportedLocale::EnUs, 73_600_000_000),
            "73.6 GB"
        );
        assert_eq!(format_bytes(SupportedLocale::ZhCn, 4_300_000_000), "4.3 GB");
        assert_eq!(format_bytes(SupportedLocale::JaJp, 999), "999 B");
    }

    #[test]
    fn speed_reuses_exactly_the_same_size_policy() {
        assert_eq!(format_speed(SupportedLocale::EnUs, 18_400_000), "18.4 MB/s");
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
}
