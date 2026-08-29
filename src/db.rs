//! SQLite storage: activities, sessions (open/closed), and time totals.

use anyhow::{bail, Context, Result};
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

/// One activity with its all-time total, as shown under a category.
#[derive(Debug, Clone)]
pub struct CategoryActivity {
    pub name: String,
    pub total: i64,
}

/// A category with its all-time rollup and member activities.
#[derive(Debug, Clone)]
pub struct Category {
    pub id: i64,
    pub name: String,
    pub total: i64,
    pub activities: Vec<CategoryActivity>,
}

#[derive(Debug, Clone)]
pub struct Activity {
    pub name: String,
    pub total: i64,
    pub sessions: i64,
    pub last: i64,
}

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: i64,
    pub name: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

pub enum StartOutcome {
    Started,
    StartedNew,
    AlreadyRunning,
}

/// History scope: everything, or the time of one activity.
#[derive(Clone, Debug, PartialEq)]
pub enum Scope {
    All,
    Activity(String),
}

impl Scope {
    /// Extra join condition restricting `activities a` to this scope.
    fn join_clause(&self) -> &'static str {
        match self {
            Scope::All => "",
            Scope::Activity(_) => " AND LOWER(a.name) = :scope",
        }
    }
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
CREATE TABLE IF NOT EXISTS categories (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS categories_name ON categories (LOWER(name));
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
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            let has_column =
                conn.prepare("SELECT category_id FROM activities LIMIT 0").is_ok();
            if !has_column {
                conn.execute(
                    "ALTER TABLE activities ADD COLUMN category_id INTEGER REFERENCES categories(id)",
                    [],
                )?;
            }
            conn.pragma_update(None, "user_version", 1)?;
        }
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
        let (aid, created) = match self.activity_id(name)? {
            Some(id) => (id, false),
            None => {
                self.conn.execute(
                    "INSERT INTO activities (name, created_at) VALUES (?, ?)",
                    params![name, now],
                )?;
                (self.conn.last_insert_rowid(), true)
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
        Ok(if created {
            StartOutcome::StartedNew
        } else {
            StartOutcome::Started
        })
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
    pub fn totals(&self, from: i64, to: i64, scope: &Scope) -> Result<Vec<Total>> {
        let now = now_ms();
        let mut stmt = self.conn.prepare(&format!(
            "SELECT name, total FROM (
                SELECT a.name AS name,
                       SUM(MAX(0, MIN(COALESCE(s.ended_at, :now), :to) - MAX(s.started_at, :from))) AS total
                FROM sessions s
                JOIN activities a ON a.id = s.activity_id{}
                WHERE s.started_at < :to
                  AND (s.ended_at IS NULL OR s.ended_at > :from)
                GROUP BY a.id
             ) WHERE total > 0
             ORDER BY total DESC",
            scope.join_clause(),
        ))?;
        let rows: Vec<Total> = match scope {
            Scope::All => stmt
                .query_map(
                    rusqlite::named_params! { ":from": from, ":to": to, ":now": now },
                    |r| Ok(Total { name: r.get(0)?, ms: r.get(1)? }),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?,
            Scope::Activity(name) => stmt
                .query_map(
                    rusqlite::named_params! { ":from": from, ":to": to, ":now": now, ":scope": name.as_str() },
                    |r| Ok(Total { name: r.get(0)?, ms: r.get(1)? }),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        };
        Ok(rows)
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

    /// Sessions overlapping [from, to) and matching `scope`, oldest first.
    pub fn sessions_in(
        &self,
        from: i64,
        to: i64,
        scope: &Scope,
    ) -> Result<Vec<SessionRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT s.id, a.name, s.started_at, s.ended_at
             FROM sessions s
             JOIN activities a ON a.id = s.activity_id{}
             WHERE s.started_at < :to
               AND (s.ended_at IS NULL OR s.ended_at > :from)
             ORDER BY s.started_at, s.id",
            scope.join_clause(),
        ))?;
        let rows: Vec<SessionRow> = match scope {
            Scope::All => stmt
                .query_map(rusqlite::named_params! { ":from": from, ":to": to }, |r| {
                    Ok(SessionRow {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        started_at: r.get(2)?,
                        ended_at: r.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?,
            Scope::Activity(name) => stmt
                .query_map(
                    rusqlite::named_params! { ":from": from, ":to": to, ":scope": name.as_str() },
                    |r| {
                        Ok(SessionRow {
                            id: r.get(0)?,
                            name: r.get(1)?,
                            started_at: r.get(2)?,
                            ended_at: r.get(3)?,
                        })
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        };
        Ok(rows)
    }

    /// Per-calendar-day totals (local time) within [from, to) and matching
    /// `scope`, as (YYYY-MM-DD, ms).
    ///
    /// A session's clipped time is attributed to the day it started; sessions
    /// spanning midnight are not split. Days with no time are absent.
    pub fn day_totals(
        &self,
        from: i64,
        to: i64,
        scope: &Scope,
    ) -> Result<Vec<(String, i64)>> {
        let now = now_ms();
        let mut stmt = self.conn.prepare(&format!(
            "SELECT day, SUM(clip) FROM (
                SELECT strftime('%Y-%m-%d', s.started_at/1000, 'unixepoch', 'localtime') AS day,
                       MAX(0, MIN(COALESCE(s.ended_at, :now), :to) - MAX(s.started_at, :from)) AS clip
                FROM sessions s
                JOIN activities a ON a.id = s.activity_id{}
                WHERE s.started_at < :to
                  AND (s.ended_at IS NULL OR s.ended_at > :from)
             ) WHERE clip > 0
             GROUP BY day
             ORDER BY day",
            scope.join_clause(),
        ))?;
        let rows: Vec<(String, i64)> = match scope {
            Scope::All => stmt
                .query_map(
                    rusqlite::named_params! { ":now": now, ":from": from, ":to": to },
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?,
            Scope::Activity(name) => stmt
                .query_map(
                    rusqlite::named_params! { ":now": now, ":from": from, ":to": to, ":scope": name.as_str() },
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        };
        Ok(rows)
    }

    fn activity_id_or_create(&self, name: &str) -> Result<i64> {
        match self.activity_id(name)? {
            Some(id) => Ok(id),
            None => {
                self.conn.execute(
                    "INSERT INTO activities (name, created_at) VALUES (?, ?)",
                    params![name, now_ms()],
                )?;
                Ok(self.conn.last_insert_rowid())
            }
        }
    }

    /// Insert a closed session, creating the activity if new.
    /// Returns true if the activity was newly created.
    pub fn add_session(&self, name: &str, start: i64, end: i64) -> Result<bool> {
        if end <= start {
            bail!("end must be after start");
        }
        let created = self.activity_id(name)?.is_none();
        let aid = self.activity_id_or_create(name)?;
        self.conn.execute(
            "INSERT INTO sessions (activity_id, started_at, ended_at) VALUES (?, ?, ?)",
            params![aid, start, end],
        )?;
        Ok(created)
    }

    /// Move a closed session to a (possibly new) activity and new times.
    pub fn update_session(&self, id: i64, name: &str, start: i64, end: i64) -> Result<()> {
        if end <= start {
            bail!("end must be after start");
        }
        let open: Option<Option<i64>> = self
            .conn
            .query_row(
                "SELECT ended_at FROM sessions WHERE id = ?",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        match open {
            Some(Some(_)) => {}
            Some(None) => bail!("session is still running; stop it first"),
            None => bail!("no such session"),
        }
        let aid = self.activity_id_or_create(name)?;
        self.conn.execute(
            "UPDATE sessions SET activity_id = ?, started_at = ?, ended_at = ? WHERE id = ?",
            params![aid, start, end, id],
        )?;
        Ok(())
    }

    /// Delete a closed session.
    pub fn delete_session(&self, id: i64) -> Result<()> {
        let open: Option<Option<i64>> = self
            .conn
            .query_row(
                "SELECT ended_at FROM sessions WHERE id = ?",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        match open {
            Some(Some(_)) => {
                self.conn.execute("DELETE FROM sessions WHERE id = ?", params![id])?;
                Ok(())
            }
            Some(None) => bail!("session is still running; stop it first"),
            None => bail!("no such session"),
        }
    }

    /// Activity names for completion, grouped by category then name.
    pub fn names(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT a.name
             FROM activities a
             LEFT JOIN categories c ON c.id = a.category_id
             ORDER BY COALESCE(c.name, '') COLLATE NOCASE, a.name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Category names for completion.
    pub fn category_names(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT name FROM categories ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// All categories with all-time rollups and members, name-ordered.
    pub fn categories(&self) -> Result<Vec<Category>> {
        let mut cats: Vec<Category> = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT id, name FROM categories ORDER BY name COLLATE NOCASE",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (id, name) = row?;
                cats.push(Category {
                    id,
                    name,
                    total: 0,
                    activities: Vec::new(),
                });
            }
        }
        let now = now_ms();
        let mut stmt = self.conn.prepare(
            "SELECT a.name,
                    SUM(CASE WHEN s.id IS NOT NULL
                             THEN MAX(0, MIN(COALESCE(s.ended_at, :now), :now) - MAX(s.started_at, 0))
                             ELSE 0 END) AS total
             FROM activities a
             LEFT JOIN sessions s ON s.activity_id = a.id
             WHERE a.category_id = :cid
             GROUP BY a.id
             HAVING total > 0
             ORDER BY total DESC",
        )?;
        for cat in cats.iter_mut() {
            let rows = stmt
                .query_map(rusqlite::named_params! { ":now": now, ":cid": cat.id }, |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (name, total) in rows {
                cat.activities.push(CategoryActivity { name, total });
                cat.total += total;
            }
        }
        Ok(cats)
    }

    /// All-time totals of activities without a category.
    pub fn unassigned(&self) -> Result<Vec<Total>> {
        let now = now_ms();
        let mut stmt = self.conn.prepare(
            "SELECT a.name,
                    SUM(CASE WHEN s.id IS NOT NULL
                             THEN MAX(0, MIN(COALESCE(s.ended_at, :now), :now) - MAX(s.started_at, 0))
                             ELSE 0 END) AS total
             FROM activities a
             LEFT JOIN sessions s ON s.activity_id = a.id
             WHERE a.category_id IS NULL
             GROUP BY a.id
             HAVING total > 0
             ORDER BY total DESC",
        )?;
        let rows = stmt.query_map(rusqlite::named_params! { ":now": now }, |r| {
            Ok(Total {
                name: r.get(0)?,
                ms: r.get(1)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Map of activity name (lowercase) to category name, for CLI grouping.
    pub fn category_map(&self) -> Result<Vec<(String, Option<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT a.name, c.name
             FROM activities a
             LEFT JOIN categories c ON c.id = a.category_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Resolve a category name (case-insensitive) to its id.
    pub fn category_id(&self, name: &str) -> Result<Option<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM categories WHERE LOWER(name) = LOWER(?)",
        )?;
        let rows = stmt.query_map(params![name], |r| r.get::<_, i64>(0))?;
        rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map(|v| v.into_iter().next())
            .map_err(Into::into)
    }

    pub fn create_category(&self, name: &str) -> Result<i64> {
        match self.conn.execute(
            "INSERT INTO categories (name) VALUES (?)",
            params![name],
        ) {
            Ok(_) => Ok(self.conn.last_insert_rowid()),
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                bail!("category already exists: {name}")
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn rename_category(&self, id: i64, name: &str) -> Result<()> {
        match self.conn.execute(
            "UPDATE categories SET name = ? WHERE id = ?",
            params![name, id],
        ) {
            Ok(n) if n > 0 => Ok(()),
            Ok(_) => bail!("no such category"),
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                bail!("category already exists: {name}")
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Delete a category; its activities become unassigned.
    pub fn delete_category(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE activities SET category_id = NULL WHERE category_id = ?",
            params![id],
        )?;
        let n = self.conn.execute("DELETE FROM categories WHERE id = ?", params![id])?;
        if n == 0 {
            bail!("no such category");
        }
        Ok(())
    }

    /// Move an activity into a category (or out of one when `category` is None).
    pub fn set_activity_category(&self, activity: &str, category: Option<&str>) -> Result<()> {
        let Some(aid) = self.activity_id(activity)? else {
            bail!("no such activity: {activity}");
        };
        let cid = match category {
            None => None,
            Some(name) => {
                let cid = self.conn.query_row(
                    "SELECT id FROM categories WHERE LOWER(name) = LOWER(?)",
                    params![name],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
                .ok_or_else(|| anyhow::anyhow!("no such category: {name}"))?;
                Some(cid)
            }
        };
        self.conn.execute(
            "UPDATE activities SET category_id = ? WHERE id = ?",
            params![cid, aid],
        )?;
        Ok(())
    }
}
