use parking_lot::Mutex;
use std::sync::Arc;

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS controllers (
    controller_id TEXT PRIMARY KEY,
    cluster TEXT NOT NULL DEFAULT '',
    env TEXT NOT NULL DEFAULT '[]',
    tag TEXT NOT NULL DEFAULT '[]',
    online INTEGER NOT NULL DEFAULT 0,
    last_seen_at INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT 0
);

-- Drop legacy tables (idempotent, safe on both old and new databases)
DROP TABLE IF EXISTS region_route_cache;
DROP TABLE IF EXISTS cluster_plugin_metadata_cache;
DROP TABLE IF EXISTS service_plugin_metadata_cache;
"#;

pub struct CenterDb {
    conn: Arc<Mutex<rusqlite::Connection>>,
}

impl Clone for CenterDb {
    fn clone(&self) -> Self {
        Self {
            conn: Arc::clone(&self.conn),
        }
    }
}

pub struct DbController {
    pub controller_id: String,
    pub cluster: String,
    pub env: Vec<String>,
    pub tag: Vec<String>,
    pub online: bool,
    pub last_seen_at: i64,
}

impl CenterDb {
    pub fn open(path: &str) -> Result<Self, rusqlite::Error> {
        // Create parent directory if needed
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }
        let conn = rusqlite::Connection::open(path)?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.run_migrations()?;
        Ok(db)
    }

    fn run_migrations(&self) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock();
        conn.execute_batch(SCHEMA_SQL)
    }

    /// Upsert a controller record (online or offline state update).
    pub fn upsert_controller(
        &self,
        id: &str,
        cluster: &str,
        env: &[String],
        tag: &[String],
        online: bool,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock();
        let now = unix_now();
        let env_json = serde_json::to_string(env).unwrap_or_else(|_| "[]".to_string());
        let tag_json = serde_json::to_string(tag).unwrap_or_else(|_| "[]".to_string());
        conn.execute(
            "INSERT INTO controllers(controller_id, cluster, env, tag, online, last_seen_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
             ON CONFLICT(controller_id) DO UPDATE SET
               cluster = excluded.cluster,
               env = excluded.env,
               tag = excluded.tag,
               online = excluded.online,
               last_seen_at = excluded.last_seen_at",
            rusqlite::params![id, cluster, env_json, tag_json, online as i64, now],
        )?;
        Ok(())
    }

    /// Mark a controller offline (`online=0`) and refresh `last_seen_at`.
    /// No-op if the controller row does not exist. This is the narrow-update
    /// path used from the fed-sync offline branches, which don't have the
    /// `cluster`/`env`/`tag` metadata handy — use `upsert_controller` when
    /// those fields matter (e.g. registration).
    pub fn mark_offline(&self, id: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE controllers SET online = 0, last_seen_at = ?1 WHERE controller_id = ?2",
            rusqlite::params![unix_now(), id],
        )?;
        Ok(())
    }

    /// Delete a controller record from DB.
    pub fn delete_controller(&self, id: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM controllers WHERE controller_id = ?1",
            rusqlite::params![id],
        )?;
        Ok(())
    }

    #[cfg(test)]
    fn open_in_memory() -> Result<Self, rusqlite::Error> {
        let conn = rusqlite::Connection::open_in_memory()?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.run_migrations()?;
        Ok(db)
    }

    /// List all controller records from DB.
    pub fn list_controllers(&self) -> Result<Vec<DbController>, rusqlite::Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT controller_id, cluster, env, tag, online, last_seen_at FROM controllers ORDER BY controller_id",
        )?;
        let rows = stmt.query_map([], |row| {
            let env_json: String = row.get(2)?;
            let tag_json: String = row.get(3)?;
            let env: Vec<String> = serde_json::from_str(&env_json).unwrap_or_default();
            let tag: Vec<String> = serde_json::from_str(&tag_json).unwrap_or_default();
            Ok(DbController {
                controller_id: row.get(0)?,
                cluster: row.get(1)?,
                env,
                tag,
                online: row.get::<_, i64>(4)? != 0,
                last_seen_at: row.get(5)?,
            })
        })?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_then_list_roundtrips_fields() {
        let db = CenterDb::open_in_memory().unwrap();
        db.upsert_controller(
            "cluster-a/ctrl-1",
            "cluster-a",
            &["prod".to_string()],
            &["edge".to_string()],
            true,
        )
        .unwrap();
        let rows = db.list_controllers().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].controller_id, "cluster-a/ctrl-1");
        assert_eq!(rows[0].cluster, "cluster-a");
        assert_eq!(rows[0].env, vec!["prod".to_string()]);
        assert_eq!(rows[0].tag, vec!["edge".to_string()]);
        assert!(rows[0].online);
        assert!(rows[0].last_seen_at > 0);
    }

    #[test]
    fn mark_offline_flips_online_flag_without_wiping_metadata() {
        let db = CenterDb::open_in_memory().unwrap();
        db.upsert_controller("c1", "cluster-k", &["prod".into()], &["edge".into()], true)
            .unwrap();
        db.mark_offline("c1").unwrap();
        let rows = db.list_controllers().unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].online, "online flag must flip to false");
        assert_eq!(rows[0].cluster, "cluster-k", "mark_offline must not touch cluster");
        assert_eq!(rows[0].env, vec!["prod".to_string()]);
        assert_eq!(rows[0].tag, vec!["edge".to_string()]);
    }

    #[test]
    fn mark_offline_on_missing_row_is_noop() {
        let db = CenterDb::open_in_memory().unwrap();
        db.mark_offline("nope").unwrap();
        assert!(db.list_controllers().unwrap().is_empty());
    }

    #[test]
    fn delete_controller_removes_row() {
        let db = CenterDb::open_in_memory().unwrap();
        db.upsert_controller("c1", "k", &[], &[], true).unwrap();
        db.delete_controller("c1").unwrap();
        assert!(db.list_controllers().unwrap().is_empty());
    }
}
