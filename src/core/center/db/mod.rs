pub mod sync;

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

-- Drop old table (idempotent, safe on both old and new databases)
DROP TABLE IF EXISTS region_route_cache;

CREATE TABLE IF NOT EXISTS cluster_plugin_metadata_cache (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    controller_id TEXT NOT NULL,
    pm_namespace TEXT NOT NULL,
    pm_name TEXT NOT NULL,
    entries_json TEXT NOT NULL DEFAULT '{}',
    synced_at INTEGER NOT NULL DEFAULT 0,
    UNIQUE(controller_id, pm_namespace, pm_name)
);

CREATE TABLE IF NOT EXISTS service_plugin_metadata_cache (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    controller_id TEXT NOT NULL,
    pm_namespace TEXT NOT NULL,
    pm_name TEXT NOT NULL,
    entries_json TEXT NOT NULL DEFAULT '{}',
    synced_at INTEGER NOT NULL DEFAULT 0,
    UNIQUE(controller_id, pm_namespace, pm_name)
);
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

pub struct DbCachedEntries {
    pub entries_json: String,
    pub synced_at: i64,
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

    pub(crate) fn upsert_plugin_metadata_cache(
        &self,
        table: &str,
        ctrl_id: &str,
        pm_ns: &str,
        pm_name: &str,
        entries_json: &str,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock();
        let now = unix_now();
        conn.execute(
            &format!(
                "INSERT INTO {table}(controller_id, pm_namespace, pm_name, entries_json, synced_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(controller_id, pm_namespace, pm_name) DO UPDATE SET
                   entries_json = excluded.entries_json,
                   synced_at = excluded.synced_at"
            ),
            rusqlite::params![ctrl_id, pm_ns, pm_name, entries_json, now],
        )?;
        Ok(())
    }

    /// Upsert a ClusterRegionRoute PluginMetaData cache entry.
    pub fn upsert_cluster_plugin_metadata_cache(
        &self,
        ctrl_id: &str,
        pm_ns: &str,
        pm_name: &str,
        entries_json: &str,
    ) -> Result<(), rusqlite::Error> {
        self.upsert_plugin_metadata_cache("cluster_plugin_metadata_cache", ctrl_id, pm_ns, pm_name, entries_json)
    }

    /// Upsert a ServiceRegionRoute PluginMetaData cache entry.
    pub fn upsert_service_plugin_metadata_cache(
        &self,
        ctrl_id: &str,
        pm_ns: &str,
        pm_name: &str,
        entries_json: &str,
    ) -> Result<(), rusqlite::Error> {
        self.upsert_plugin_metadata_cache("service_plugin_metadata_cache", ctrl_id, pm_ns, pm_name, entries_json)
    }

    /// Delete a controller record and its associated cache rows from DB.
    pub fn delete_controller(&self, id: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM cluster_plugin_metadata_cache WHERE controller_id = ?1",
            rusqlite::params![id],
        )?;
        conn.execute(
            "DELETE FROM service_plugin_metadata_cache WHERE controller_id = ?1",
            rusqlite::params![id],
        )?;
        conn.execute(
            "DELETE FROM controllers WHERE controller_id = ?1",
            rusqlite::params![id],
        )?;
        Ok(())
    }

    /// Delete all PluginMetaData cache rows.
    pub fn clear_plugin_metadata_cache(&self) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM cluster_plugin_metadata_cache", [])?;
        conn.execute("DELETE FROM service_plugin_metadata_cache", [])?;
        Ok(())
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
