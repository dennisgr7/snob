//! What an automation's action found last, kept by the account that read it
//! (`022_automation_marks.sql`): what its "changes" filter compares the next
//! run against. Written only once a run's report is made, so a run stopped
//! halfway leaves the next one comparing against the same accounts.

use rusqlite::{Connection, OptionalExtension, params};
use snob_core::{Epoch, Pk};

use super::StoreError;

/// What one action found last, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub taken_at: Epoch,
    pub members: Vec<Pk>,
}

/// The mark of `node` of `automation`, if a run ever left one.
pub fn get(conn: &Connection, automation: &str, node: u32) -> Result<Option<Mark>, StoreError> {
    let row: Option<(i64, String)> = conn
        .query_row(
            "SELECT taken_at, members FROM automation_marks WHERE automation = ?1 AND node = ?2",
            params![automation, node],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(row.map(|(taken_at, members)| Mark {
        taken_at: Epoch::new(taken_at),
        // A row this version cannot read is a baseline laid again, never a
        // run that fails.
        members: serde_json::from_str(&members).unwrap_or_default(),
    }))
}

/// Replaces the mark of `node` of `automation`.
pub fn set(
    conn: &Connection,
    automation: &str,
    node: u32,
    taken_at: Epoch,
    members: &[Pk],
) -> Result<(), StoreError> {
    let members = serde_json::to_string(members).unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        "INSERT INTO automation_marks (automation, node, taken_at, members) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(automation, node) DO UPDATE SET taken_at = excluded.taken_at,
           members = excluded.members",
        params![automation, node, taken_at.get(), members],
    )?;
    Ok(())
}

/// Forgets every mark of an automation that is no more.
pub fn forget(conn: &Connection, automation: &str) -> Result<(), StoreError> {
    conn.execute(
        "DELETE FROM automation_marks WHERE automation = ?1",
        params![automation],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mark_is_replaced_whole_and_forgotten_with_its_automation() {
        let store = crate::store::Store::in_memory().unwrap();
        let conn = store.conn();
        assert_eq!(get(conn, "a1", 2).unwrap(), None);
        set(conn, "a1", 2, Epoch::new(5), &[Pk::new(3), Pk::new(4)]).unwrap();
        set(conn, "a1", 2, Epoch::new(6), &[Pk::new(4)]).unwrap();
        assert_eq!(
            get(conn, "a1", 2).unwrap(),
            Some(Mark {
                taken_at: Epoch::new(6),
                members: vec![Pk::new(4)]
            })
        );
        forget(conn, "a1").unwrap();
        assert_eq!(get(conn, "a1", 2).unwrap(), None);
    }
}
