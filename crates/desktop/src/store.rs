use rusqlite::Connection;

use crate::DesktopError;

/// Migrates the original unversioned library database without discarding installed records.
pub(crate) fn initialize(db: &Connection) -> Result<(), DesktopError> {
    let version: i64 = db.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > 1 {
        return Err(DesktopError::UnsupportedHost(format!(
            "library schema {version} is newer than this application; upgrade RecompHub before changing this library"
        )));
    }
    if version == 1 {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS device_favorites (
               game_id INTEGER PRIMARY KEY CHECK(game_id > 0),
               position INTEGER NOT NULL CHECK(position >= 0)
             );",
        )?;
        return Ok(());
    }
    db.execute_batch("PRAGMA foreign_keys = ON;")?;
    let tx = db.unchecked_transaction()?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS installs (
           port_id INTEGER PRIMARY KEY, game_id INTEGER NOT NULL, game_name TEXT NOT NULL,
           game_slug TEXT NOT NULL, port_name TEXT NOT NULL, port_slug TEXT NOT NULL,
           manifest_json TEXT NOT NULL, active_path TEXT NOT NULL, previous_path TEXT,
           status TEXT NOT NULL, error TEXT, offer_json TEXT NOT NULL, last_checked_at TEXT,
           previous_manifest_json TEXT, previous_offer_json TEXT,
           suppressed_recipe_id TEXT, suppressed_version TEXT, suppressed_asset_id INTEGER,
           suppressed_sha256 TEXT
         );
         CREATE TABLE IF NOT EXISTS updates (
           port_id INTEGER PRIMARY KEY REFERENCES installs(port_id) ON DELETE CASCADE,
           offer_json TEXT NOT NULL, checked_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS operations (
           operation_id TEXT PRIMARY KEY, port_id INTEGER NOT NULL, kind TEXT NOT NULL,
           phase TEXT NOT NULL, prior_offer_json TEXT, candidate_offer_json TEXT,
           part_path TEXT, staging_path TEXT, final_path TEXT, error TEXT,
           created_at TEXT NOT NULL
         );
         CREATE UNIQUE INDEX IF NOT EXISTS operations_one_active_per_port
           ON operations(port_id) WHERE phase NOT IN ('committed','failed','cancelled');
         CREATE TABLE IF NOT EXISTS running_processes (
           port_id INTEGER PRIMARY KEY, pid INTEGER NOT NULL, started_at INTEGER NOT NULL,
           executable_path TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS device_favorites (
           game_id INTEGER PRIMARY KEY CHECK(game_id > 0),
           position INTEGER NOT NULL CHECK(position >= 0)
         );",
    )?;
    // Existing schemas already have these base columns. Additions are checked independently so
    // interrupted/pre-release databases can be migrated without losing installed rows.
    for (name, definition) in [
        ("previous_manifest_json", "TEXT"),
        ("previous_offer_json", "TEXT"),
        ("suppressed_recipe_id", "TEXT"),
        ("suppressed_version", "TEXT"),
        ("suppressed_asset_id", "INTEGER"),
        ("suppressed_sha256", "TEXT"),
    ] {
        if !has_column(&tx, "installs", name)? {
            tx.execute_batch(&format!(
                "ALTER TABLE installs ADD COLUMN {name} {definition}"
            ))?;
        }
    }
    tx.pragma_update(None, "user_version", 1)?;
    tx.commit()?;
    Ok(())
}

fn has_column(db: &Connection, table: &str, wanted: &str) -> Result<bool, DesktopError> {
    let mut statement = db.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
    for column in columns {
        if column? == wanted {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unversioned_library_migrates_in_place_and_retains_installs() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE installs (port_id INTEGER PRIMARY KEY, game_id INTEGER NOT NULL, game_name TEXT NOT NULL, game_slug TEXT NOT NULL, port_name TEXT NOT NULL, port_slug TEXT NOT NULL, manifest_json TEXT NOT NULL, active_path TEXT NOT NULL, previous_path TEXT, status TEXT NOT NULL, error TEXT, offer_json TEXT NOT NULL, last_checked_at TEXT); INSERT INTO installs VALUES (7,8,'Game','game','Port','port','{}','games/7/versions/old',NULL,'ready',NULL,'{}',NULL);").unwrap();
        initialize(&db).unwrap();
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        let port_id: i64 = db
            .query_row("SELECT port_id FROM installs", [], |row| row.get(0))
            .unwrap();
        let has_journal: i64 = db
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='operations'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, 1);
        assert_eq!(port_id, 7);
        assert_eq!(has_journal, 1);
    }

    #[test]
    fn active_install_pointer_and_journal_roll_back_together_on_constraint_failure() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE installs (port_id INTEGER PRIMARY KEY, game_id INTEGER NOT NULL, game_name TEXT NOT NULL, game_slug TEXT NOT NULL, port_name TEXT NOT NULL, port_slug TEXT NOT NULL, manifest_json TEXT NOT NULL, active_path TEXT NOT NULL, previous_path TEXT, status TEXT NOT NULL, error TEXT, offer_json TEXT NOT NULL, last_checked_at TEXT); INSERT INTO installs VALUES (7,8,'Game','game','Port','port','{}','games/7/old',NULL,'ready',NULL,'{}',NULL);").unwrap();
        initialize(&db).unwrap();
        db.execute("INSERT INTO operations(operation_id,port_id,kind,phase,created_at) VALUES ('op-1',7,'update','queued','now')", []).unwrap();
        let tx = db.unchecked_transaction().unwrap();
        tx.execute(
            "UPDATE installs SET active_path='games/7/new' WHERE port_id=7",
            [],
        )
        .unwrap();
        assert!(tx.execute("INSERT INTO operations(operation_id,port_id,kind,phase,created_at) VALUES ('op-2',7,'update','queued','now')", []).is_err());
        drop(tx);
        let active: String = db
            .query_row(
                "SELECT active_path FROM installs WHERE port_id=7",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let journal_rows: i64 = db
            .query_row(
                "SELECT count(*) FROM operations WHERE port_id=7",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(active, "games/7/old");
        assert_eq!(journal_rows, 1);
    }

    #[test]
    fn newer_library_schema_refuses_mutation() {
        let db = Connection::open_in_memory().unwrap();
        db.pragma_update(None, "user_version", 2).unwrap();
        assert!(initialize(&db).is_err());
    }
}
