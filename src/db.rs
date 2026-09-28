use std::fmt;
use std::io;
use std::path::Path;

use chrono::Utc;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params};

use crate::model::{Status, Ticket};

/// Stored in the SQLite header so project files can be told apart from other databases ("ORGX").
const APPLICATION_ID: i32 = 0x4F52_4758;
const SCHEMA_VERSION: i32 = 1;

const SCHEMA: &str = "
CREATE TABLE tickets (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    title      TEXT    NOT NULL,
    details    TEXT    NOT NULL DEFAULT '',
    status     TEXT    NOT NULL CHECK (status IN ('backlog', 'ready', 'in-progress', 'review', 'completed')),
    position   INTEGER NOT NULL,
    created_at TEXT    NOT NULL,
    updated_at TEXT    NOT NULL
);
CREATE INDEX tickets_status_position ON tickets (status, position);
";

/// Expected `tickets` columns: (name, declared type, not null, primary key).
const TICKET_COLUMNS: &[(&str, &str, bool, bool)] = &[
    ("id", "INTEGER", false, true),
    ("title", "TEXT", true, false),
    ("details", "TEXT", true, false),
    ("status", "TEXT", true, false),
    ("position", "INTEGER", true, false),
    ("created_at", "TEXT", true, false),
    ("updated_at", "TEXT", true, false),
];

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Sqlite(rusqlite::Error),
    /// The file exists but is not a usable project database.
    Invalid(String),
    NotFound(i64),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Sqlite(e) => write!(f, "database error: {e}"),
            Error::Invalid(reason) => write!(f, "not a valid project database: {reason}"),
            Error::NotFound(id) => write!(f, "ticket #{id} no longer exists"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Sqlite(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

impl ToSql for Status {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.as_str()))
    }
}

impl FromSql for Status {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        Status::parse(text)
            .ok_or_else(|| FromSqlError::Other(format!("unknown status {text:?}").into()))
    }
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens the project at `path`, creating it when the file does not exist (or is empty).
    /// Existing files are validated before use.
    pub fn open(path: &Path) -> Result<Self> {
        match std::fs::metadata(path) {
            Ok(meta) if !meta.is_file() => Err(Error::Invalid("path is not a regular file".into())),
            Ok(meta) if meta.len() > 0 => {
                let conn = Connection::open_with_flags(
                    path,
                    OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?;
                validate(&conn)?;
                let store = Self { conn };
                store
                    .list()
                    .map_err(|e| Error::Invalid(format!("contains unreadable tickets ({e})")))?;
                Ok(store)
            }
            Ok(_) => Self::create(path),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::create(path),
            Err(e) => Err(e.into()),
        }
    }

    fn create(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(&format!(
            "BEGIN;
             {SCHEMA}
             PRAGMA application_id = {APPLICATION_ID};
             PRAGMA user_version = {SCHEMA_VERSION};
             COMMIT;"
        ))?;
        Ok(Self { conn })
    }

    /// All tickets, ordered by their position within each column.
    pub fn list(&self) -> Result<Vec<Ticket>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, title, details, status, created_at, updated_at
             FROM tickets ORDER BY position, id",
        )?;
        let tickets = stmt
            .query_map([], ticket_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(tickets)
    }

    pub fn get(&self, id: i64) -> Result<Ticket> {
        self.conn
            .query_row(
                "SELECT id, title, details, status, created_at, updated_at FROM tickets WHERE id = ?1",
                [id],
                ticket_from_row,
            )
            .optional()?
            .ok_or(Error::NotFound(id))
    }

    /// Inserts a ticket at the top of its column.
    pub fn create_ticket(&mut self, status: Status, title: &str, details: &str) -> Result<Ticket> {
        let now = Utc::now();
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE tickets SET position = position + 1 WHERE status = ?1",
            [status],
        )?;
        tx.execute(
            "INSERT INTO tickets (title, details, status, position, created_at, updated_at)
             VALUES (?1, ?2, ?3, 0, ?4, ?4)",
            params![title, details, status, now],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        self.get(id)
    }

    pub fn update_ticket(&mut self, id: i64, title: &str, details: &str) -> Result<Ticket> {
        let changed = self.conn.execute(
            "UPDATE tickets SET title = ?2, details = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, title, details, Utc::now()],
        )?;
        if changed == 0 {
            return Err(Error::NotFound(id));
        }
        self.get(id)
    }

    pub fn delete_ticket(&mut self, id: i64) -> Result<()> {
        if self
            .conn
            .execute("DELETE FROM tickets WHERE id = ?1", [id])?
            == 0
        {
            return Err(Error::NotFound(id));
        }
        Ok(())
    }

    /// Moves a ticket to `index` within the `status` column (which may be its current one).
    pub fn move_ticket(&mut self, id: i64, status: Status, index: usize) -> Result<()> {
        let tx = self.conn.transaction()?;
        let current: Status = tx
            .query_row("SELECT status FROM tickets WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?
            .ok_or(Error::NotFound(id))?;

        let mut order: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM tickets WHERE status = ?1 AND id <> ?2 ORDER BY position, id",
            )?;
            stmt.query_map(params![status, id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        order.insert(index.min(order.len()), id);

        if current != status {
            tx.execute(
                "UPDATE tickets SET status = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, status, Utc::now()],
            )?;
        }
        {
            let mut stmt = tx.prepare("UPDATE tickets SET position = ?2 WHERE id = ?1")?;
            for (position, ticket_id) in order.iter().enumerate() {
                stmt.execute(params![ticket_id, position as i64])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

fn ticket_from_row(row: &Row<'_>) -> rusqlite::Result<Ticket> {
    Ok(Ticket {
        id: row.get(0)?,
        title: row.get(1)?,
        details: row.get(2)?,
        status: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn validate(conn: &Connection) -> Result<()> {
    // The first read is where SQLite notices a file that is not a database at all.
    let application_id: i32 = conn
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .map_err(|e| Error::Invalid(format!("file is not a SQLite database ({e})")))?;

    if application_id != APPLICATION_ID {
        return Err(Error::Invalid("SQLite file was not created by orgx".into()));
    }

    let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version != SCHEMA_VERSION {
        return Err(Error::Invalid(format!(
            "unsupported schema version {version} (expected {SCHEMA_VERSION})"
        )));
    }

    let integrity: String = conn.pragma_query_value(None, "quick_check", |r| r.get(0))?;
    if integrity != "ok" {
        return Err(Error::Invalid(format!(
            "integrity check failed: {integrity}"
        )));
    }

    let mut stmt = conn.prepare(
        "SELECT name, type, \"notnull\", pk FROM pragma_table_info('tickets') ORDER BY cid",
    )?;
    let columns: Vec<(String, String, bool, bool)> = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get::<_, i64>(2)? != 0,
                r.get::<_, i64>(3)? != 0,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;

    if columns.is_empty() {
        return Err(Error::Invalid("missing `tickets` table".into()));
    }

    let schema_matches = columns.len() == TICKET_COLUMNS.len()
        && columns.iter().zip(TICKET_COLUMNS).all(
            |((name, ty, not_null, pk), (exp_name, exp_ty, exp_not_null, exp_pk))| {
                name == exp_name
                    && ty.eq_ignore_ascii_case(exp_ty)
                    && not_null == exp_not_null
                    && pk == exp_pk
            },
        );

    if !schema_matches {
        return Err(Error::Invalid(
            "`tickets` table does not match the expected schema".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("orgx-test-{}-{name}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn creates_and_reopens_project() {
        let path = temp_path("reopen");
        let mut store = Store::open(&path).unwrap();
        store.create_ticket(Status::Backlog, "first", "").unwrap();
        let second = store
            .create_ticket(Status::Backlog, "second", "**hi**")
            .unwrap();
        drop(store);

        let store = Store::open(&path).unwrap();
        let tickets = store.list().unwrap();
        assert_eq!(tickets.len(), 2);
        assert_eq!(tickets[0].id, second.id, "new tickets go to the top");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn moves_between_and_within_columns() {
        let path = temp_path("move");
        let mut store = Store::open(&path).unwrap();
        let a = store.create_ticket(Status::Ready, "a", "").unwrap();
        let b = store.create_ticket(Status::Ready, "b", "").unwrap();
        let c = store.create_ticket(Status::Review, "c", "").unwrap();

        store.move_ticket(b.id, Status::Ready, 1).unwrap();
        store.move_ticket(c.id, Status::Ready, 1).unwrap();
        let ready: Vec<i64> = store
            .list()
            .unwrap()
            .into_iter()
            .filter(|t| t.status == Status::Ready)
            .map(|t| t.id)
            .collect();
        assert_eq!(ready, vec![a.id, c.id, b.id]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn deletes_tickets() {
        let path = temp_path("delete");
        let mut store = Store::open(&path).unwrap();
        let a = store.create_ticket(Status::Backlog, "a", "").unwrap();
        let b = store.create_ticket(Status::Backlog, "b", "").unwrap();

        store.delete_ticket(a.id).unwrap();
        let ids: Vec<i64> = store.list().unwrap().into_iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![b.id]);
        assert!(matches!(store.delete_ticket(a.id), Err(Error::NotFound(_))));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_foreign_files() {
        let path = temp_path("foreign-text");
        std::fs::write(
            &path,
            "definitely not sqlite, just some text that is long enough",
        )
        .unwrap();
        assert!(matches!(Store::open(&path), Err(Error::Invalid(_))));
        std::fs::remove_file(&path).unwrap();

        let path = temp_path("foreign-db");
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE tickets (id INTEGER)")
            .unwrap();
        assert!(matches!(Store::open(&path), Err(Error::Invalid(_))));
        std::fs::remove_file(&path).unwrap();
    }
}
