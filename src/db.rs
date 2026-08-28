//! SQLite storage: activities, sessions (open/closed), and time totals.

use anyhow::{Context, Result};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};

use crate::timefmt::now_ms;

pub struct Db {
    conn: Connection,
}

#[derive(Debug, Clone)]
pub struct Running {
    pub name: String,
    pub started_at: i64,
}

#[derive(Debug)]
pub struct Stopped {
    pub name: String,
    pub started_at: i64,
    pub ended_at: i64,
}

#[derive(Debug)]
pub struct Total {
    pub name: String,
    pub ms: i64,
}

#[derive(Debug)]
pub struct Activity {
    pub name: String,
    pub total: i64,
    pub sessions: i64,
    pub last: i64,
}

pub enum StartOutcome {
    Started,
    AlreadyRunning,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS activities (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS activities_name ON activities (LOWER(name));
CREATE TABLE IF NOT EXISTS sessions (
    id INTEGER PRIMARY KEY,
    activity_id INTEGER NOT NULL REFERENCES activities(id),
    started_at INTEGER NOT NULL,
    ended_at INTEGER
);
CREATE INDEX IF NOT EXISTS sessions_open ON sessions (activity_id) WHERE ended_at IS NULL;
CREATE INDEX IF NOT EXISTS sessions_started ON sessions (started_at);
"#;

fn db_path() -> std::path::PathBuf {
    if let Ok(d) = std::env::var("XDG_DATA_HOME") {
        let p = std::path::Path::new(&d);
        if p.is_absolute() {
            return p.join("enoch").join("enoch.db");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home).join(".local/share/enoch/enoch.db")
}

impl Db {
    pub fn open() -> Result<Self> {
        let path = db_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let conn = Connection::open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", 1)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    fn activity_id(&self, name: &str) -> Result<Option<i64>> {
        let id = self
            .conn
            .query_row(
                "SELECT id FROM activities WHERE LOWER(name) = LOWER(?)",
                params![name],
                |r| r.get::<_, i64>(0),
            )
            .optional()?;
        Ok(id)
    }

    /// Start an activity (created if new). No-op if already running.
    pub fn start(&self, name: &str) -> Result<StartOutcome> {
        let now = now_ms();
        let aid = match self.activity_id(name)? {
            Some(id) => id,
            None => {
                self.conn.execute(
                    "INSERT INTO activities (name, created_at) VALUES (?, ?)",
                    params![name, now],
                )?;
                self.conn.last_insert_rowid()
            }
        };
        let open: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE activity_id = ? AND ended_at IS NULL",
            params![aid],
            |r| r.get(0),
        )?;
        if open > 0 {
            return Ok(StartOutcome::AlreadyRunning);
        }
        self.conn.execute(
            "INSERT INTO sessions (activity_id, started_at) VALUES (?, ?)",
            params![aid, now],
        )?;
        Ok(StartOutcome::Started)
    }

    /// Stop open sessions. Empty `names` stops everything running.
    pub fn stop(&self, names: &[String]) -> Result<Vec<Stopped>> {
        let now = now_ms();
        let mut q = String::from(
            "SELECT s.id, a.name, s.started_at
             FROM sessions s
             JOIN activities a ON a.id = s.activity_id
             WHERE s.ended_at IS NULL",
        );
        if !names.is_empty() {
            let ph = vec!["LOWER(?)"; names.len()].join(", ");
            q.push_str(&format!(
                " AND a.id IN (SELECT id FROM activities WHERE LOWER(name) IN ({ph}))"
            ));
        }
        q.push_str(" ORDER BY s.started_at");

        let tx = self.conn.unchecked_transaction()?;
        let mut found: Vec<(i64, String, i64)> = Vec::new();
        {
            let mut stmt = tx.prepare(&q)?;
            let rows = stmt.query_map(params_from_iter(names.iter().cloned()), |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?;
            for row in rows {
                found.push(row?);
            }
        }
        for (id, _, _) in &found {
            tx.execute("UPDATE sessions SET ended_at = ? WHERE id = ?", params![now, id])?;
        }
        tx.commit()?;
        Ok(found
            .into_iter()
            .map(|(_, name, started_at)| Stopped {
                name,
                started_at,
                ended_at: now,
            })
            .collect())
    }

    pub fn running(&self) -> Result<Vec<Running>> {
        let mut stmt = self.conn.prepare(
            "SELECT a.name, s.started_at
             FROM sessions s
             JOIN activities a ON a.id = s.activity_id
             WHERE s.ended_at IS NULL
             ORDER BY s.started_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Running {
                name: r.get(0)?,
                started_at: r.get(1)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Per-activity time within [from, to), clipping sessions to the window.
    /// Open sessions count up to now, clipped at `to`.
    pub fn totals(&self, from: i64, to: i64) -> Result<Vec<Total>> {
        let now = now_ms();
        let mut stmt = self.conn.prepare(
            "SELECT name, total FROM (
                SELECT a.name AS name,
                       SUM(MAX(0, MIN(COALESCE(s.ended_at, :now), :to) - MAX(s.started_at, :from))) AS total
                FROM sessions s
                JOIN activities a ON a.id = s.activity_id
                WHERE s.started_at < :to
                  AND (s.ended_at IS NULL OR s.ended_at > :from)
                GROUP BY a.id
             ) WHERE total > 0
             ORDER BY total DESC",
        )?;
        let rows = stmt.query_map(
            rusqlite::named_params! { ":from": from, ":to": to, ":now": now },
            |r| Ok(Total { name: r.get(0)?, ms: r.get(1)? }),
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// All activities with all-time total, session count, last session start.
    pub fn activities(&self) -> Result<Vec<Activity>> {
        let now = now_ms();
        let mut stmt = self.conn.prepare(
            "SELECT a.name,
                    COALESCE(SUM(MAX(0, MIN(COALESCE(s.ended_at, :now), :now) - MAX(s.started_at, 0))), 0),
                    COUNT(s.id),
                    COALESCE(MAX(s.started_at), 0)
             FROM activities a
             LEFT JOIN sessions s ON s.activity_id = a.id
             GROUP BY a.id
             ORDER BY a.name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map(rusqlite::named_params! { ":now": now }, |r| {
            Ok(Activity {
                name: r.get(0)?,
                total: r.get(1)?,
                sessions: r.get(2)?,
                last: r.get(3)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    pub fn names(&self) -> Result<Vec<String>> {
        let mut stmt =
            self.conn.prepare("SELECT DISTINCT name FROM activities ORDER BY name COLLATE NOCASE")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }
}
