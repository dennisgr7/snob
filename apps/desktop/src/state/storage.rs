//! What the settings say of what snob keeps: a size in words, and which
//! lines of it are shown in which order. The sizes are measured by the
//! engine (`snob_engine::storage`); nothing here reads a file.

use snob_core::Pk;
use snob_engine::storage::Kind;
use snob_store::disk::{Size, Volume};

use super::settings::Row;

/// `bytes` as a person reads it, in the language tagged `language`: in
/// units of 1,024 as Windows counts them, with one decimal under a hundred
/// and a decimal comma in Spanish.
pub fn said(bytes: u64, language: &str) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let mut number = if value < 100.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.0}")
    };
    if let Some(whole) = number.strip_suffix(".0") {
        number = whole.to_string();
    }
    if language == "es" {
        number = number.replace('.', ",");
    }
    format!("{number} {}", UNITS[unit])
}

/// The lines of what is kept, after the disk: each kind by what it weighs,
/// the heaviest first, those that hold nothing left out but the cache;
/// then, when there is any, the accounts' heading and each account.
pub fn rows(kinds: &[(Kind, Size)], accounts: impl IntoIterator<Item = Pk>) -> Vec<Row> {
    let mut kept: Vec<(Kind, Size)> = kinds
        .iter()
        .copied()
        .filter(|(kind, size)| size.bytes > 0 || kind.clearable())
        .collect();
    kept.sort_by(|a, b| b.1.bytes.cmp(&a.1.bytes).then(a.0.cmp(&b.0)));
    let accounts: Vec<Row> = accounts.into_iter().map(Row::Account).collect();
    let heading = (!accounts.is_empty()).then_some(Row::Accounts);
    kept.into_iter()
        .map(|(kind, _)| Row::StorageKind(kind))
        .chain(heading)
        .chain(accounts)
        .collect()
}

/// The disk's bar, in bytes adding up to its size: what everything else on
/// it takes, what snob takes (`kept`), and what is free. What snob keeps
/// outside the disk (the stories' scratch may be on another) is never said
/// to take more than all that is used.
pub fn shares(disk: Volume, kept: u64) -> (u64, u64, u64) {
    let free = disk.free.min(disk.total);
    let used = disk.total - free;
    let snob = kept.min(used);
    (used - snob, snob, free)
}

/// Every kind together.
pub fn total(kinds: &[(Kind, Size)]) -> u64 {
    kinds.iter().map(|(_, size)| size.bytes).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_is_said_in_the_unit_it_reads_best_in_with_the_language_s_decimal_mark() {
        assert_eq!(said(0, "en"), "0 B");
        assert_eq!(said(1023, "en"), "1023 B");
        assert_eq!(said(1024, "en"), "1 KB");
        assert_eq!(said(3_250_586, "en"), "3.1 MB");
        assert_eq!(said(3_250_586, "es"), "3,1 MB");
        assert_eq!(said(150 * 1024 * 1024, "es"), "150 MB");
        assert_eq!(said(5 * 1024 * 1024 * 1024, "en"), "5 GB");
    }

    #[test]
    fn the_heaviest_kind_comes_first_and_an_empty_one_only_when_it_is_the_cache() {
        let size = |bytes| Size { bytes, files: 1 };
        let kinds = [
            (Kind::Databases, size(10)),
            (Kind::Cache, size(0)),
            (Kind::BrowserProfiles, size(50)),
            (Kind::Update, size(0)),
            (Kind::Logs, size(10)),
        ];
        assert_eq!(
            rows(&kinds, [Pk::new(7)]),
            [
                Row::StorageKind(Kind::BrowserProfiles),
                Row::StorageKind(Kind::Databases),
                Row::StorageKind(Kind::Logs),
                Row::StorageKind(Kind::Cache),
                Row::Accounts,
                Row::Account(Pk::new(7)),
            ]
        );
        assert_eq!(total(&kinds), 70);
        assert!(
            !rows(&kinds, []).contains(&Row::Accounts),
            "nobody to clear"
        );
    }

    #[test]
    fn the_disk_s_shares_add_up_to_its_size_and_snob_never_to_more_than_is_used() {
        let disk = Volume {
            total: 1000,
            free: 300,
        };
        assert_eq!(shares(disk, 50), (650, 50, 300));
        assert_eq!(shares(disk, 900), (0, 700, 300));
        let odd = Volume {
            total: 100,
            free: 400,
        };
        assert_eq!(shares(odd, 10), (0, 0, 100));
    }
}
