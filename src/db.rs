use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, ffi, params};

use crate::model::{Project, Status, Ticket};

/// Stored in the SQLite header so orgx databases can be told apart from others ("ORGX").
const APPLICATION_ID: i32 = 0x4F52_4758;
/// 1 and 2 were single-board `project.db` files; 3 is `data.db` with multiple projects;
/// 4 makes the names of projects that aren't deleted unique.
const SCHEMA_VERSION: i32 = 4;

/// Names are compared case-insensitively, so "Work" and "work" can't both exist. Deleted
/// projects don't count, so a name can be reused after deleting its project.
const PROJECT_NAME_INDEX: &str = "projects_name_unique";
const PROJECT_NAME_INDEX_SQL: &str = "
CREATE UNIQUE INDEX projects_name_unique ON projects (name COLLATE NOCASE)
WHERE deleted_at IS NULL;
";

const SCHEMA: &str = "
CREATE TABLE projects (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT    NOT NULL,
    deleted_at TEXT
);
CREATE TABLE tickets (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects (id),
    title      TEXT    NOT NULL,
    details    TEXT    NOT NULL DEFAULT '',
    status     TEXT    NOT NULL CHECK (status IN ('backlog', 'ready', 'in-progress', 'review', 'completed')),
    position   INTEGER NOT NULL,
    created_at TEXT    NOT NULL,
    updated_at TEXT    NOT NULL,
    deleted_at TEXT
);
CREATE INDEX tickets_project_status_position ON tickets (project_id, status, position);
";

/// (name, declared type, not null, primary key)
type Column = (&'static str, &'static str, bool, bool);

const PROJECT_COLUMNS: &[Column] = &[
    ("id", "INTEGER", false, true),
    ("name", "TEXT", true, false),
    ("deleted_at", "TEXT", false, false),
];

const TICKET_COLUMNS: &[Column] = &[
    ("id", "INTEGER", false, true),
    ("project_id", "INTEGER", true, false),
    ("title", "TEXT", true, false),
    ("details", "TEXT", true, false),
    ("status", "TEXT", true, false),
    ("position", "INTEGER", true, false),
    ("created_at", "TEXT", true, false),
    ("updated_at", "TEXT", true, false),
    ("deleted_at", "TEXT", false, false),
];

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Sqlite(rusqlite::Error),
    /// The file exists but is not a usable orgx database.
    Invalid(String),
    NotFound(i64),
    ProjectNotFound(i64),
    /// Another project that isn't deleted already has this name.
    DuplicateProjectName(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Sqlite(e) => write!(f, "database error: {e}"),
            Error::Invalid(reason) => write!(f, "not a valid orgx database: {reason}"),
            Error::NotFound(id) => write!(f, "ticket #{id} no longer exists"),
            Error::ProjectNotFound(id) => write!(f, "project #{id} no longer exists"),
            Error::DuplicateProjectName(name) => {
                write!(f, "a project named “{name}” already exists")
            }
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

/// `$XDG_CONFIG_HOME/orgx/data.db`, normally `~/.config/orgx/data.db`.
pub fn default_path() -> PathBuf {
    gtk::glib::user_config_dir().join("orgx").join("data.db")
}

const TICKET_SELECT: &str =
    "SELECT id, title, details, status, created_at, updated_at FROM tickets";

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens the database at `path`, creating it (and its directory) when the file does not
    /// exist or is empty. Existing files are validated before use.
    pub fn open(path: &Path) -> Result<Self> {
        let store = match std::fs::metadata(path) {
            Ok(meta) if !meta.is_file() => {
                return Err(Error::Invalid("path is not a regular file".into()));
            }
            Ok(meta) if meta.len() > 0 => {
                let conn = Connection::open_with_flags(
                    path,
                    OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?;
                validate(&conn)?;
                let store = Self { conn };
                store.check_readable()?;
                store
            }
            Ok(_) => Self::create(path)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                Self::create(path)?
            }
            Err(e) => return Err(e.into()),
        };
        store.conn.pragma_update(None, "foreign_keys", true)?;
        Ok(store)
    }

    fn create(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(&format!(
            "BEGIN;
             {SCHEMA}
             {PROJECT_NAME_INDEX_SQL}
             PRAGMA application_id = {APPLICATION_ID};
             PRAGMA user_version = {SCHEMA_VERSION};
             COMMIT;"
        ))?;
        Ok(Self { conn })
    }

    /// Reads every row once, so bad data is reported at startup rather than mid-session.
    fn check_readable(&self) -> Result<()> {
        let unreadable = |e: Error| Error::Invalid(format!("contains unreadable data ({e})"));
        for project in self.list_projects().map_err(unreadable)? {
            self.list(project.id).map_err(unreadable)?;
        }
        Ok(())
    }

    /// Projects that aren't deleted, oldest first.
    pub fn list_projects(&self) -> Result<Vec<Project>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, name FROM projects WHERE deleted_at IS NULL ORDER BY id")?;
        let projects = stmt
            .query_map([], |r| {
                Ok(Project {
                    id: r.get(0)?,
                    name: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(projects)
    }

    pub fn get_project(&self, id: i64) -> Result<Project> {
        self.conn
            .query_row(
                "SELECT id, name FROM projects WHERE id = ?1 AND deleted_at IS NULL",
                [id],
                |r| {
                    Ok(Project {
                        id: r.get(0)?,
                        name: r.get(1)?,
                    })
                },
            )
            .optional()?
            .ok_or(Error::ProjectNotFound(id))
    }

    pub fn create_project(&mut self, name: &str) -> Result<Project> {
        self.conn
            .execute("INSERT INTO projects (name) VALUES (?1)", [name])
            .map_err(|e| duplicate_name(e, name))?;
        Ok(Project {
            id: self.conn.last_insert_rowid(),
            name: name.to_owned(),
        })
    }

    pub fn update_project(&mut self, id: i64, name: &str) -> Result<Project> {
        let changed = self
            .conn
            .execute(
                "UPDATE projects SET name = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                params![id, name],
            )
            .map_err(|e| duplicate_name(e, name))?;
        if changed == 0 {
            return Err(Error::ProjectNotFound(id));
        }
        self.get_project(id)
    }

    /// Soft-deletes a project. Its tickets are left as they are, but can no longer be reached.
    pub fn delete_project(&mut self, id: i64) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE projects SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
            params![id, Utc::now()],
        )?;
        if changed == 0 {
            return Err(Error::ProjectNotFound(id));
        }
        Ok(())
    }

    /// Deleted projects with their deletion times, most recently deleted first.
    pub fn list_deleted_projects(&self) -> Result<Vec<(Project, DateTime<Utc>)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, deleted_at FROM projects
             WHERE deleted_at IS NOT NULL ORDER BY deleted_at DESC, id DESC",
        )?;
        let projects = stmt
            .query_map([], |r| {
                Ok((
                    Project {
                        id: r.get(0)?,
                        name: r.get(1)?,
                    },
                    r.get(2)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(projects)
    }

    /// Undeletes a project. Fails if another project has taken its name meanwhile.
    pub fn restore_project(&mut self, id: i64) -> Result<Project> {
        let name: String = self
            .conn
            .query_row(
                "SELECT name FROM projects WHERE id = ?1 AND deleted_at IS NOT NULL",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or(Error::ProjectNotFound(id))?;
        self.conn
            .execute("UPDATE projects SET deleted_at = NULL WHERE id = ?1", [id])
            .map_err(|e| duplicate_name(e, &name))?;
        self.get_project(id)
    }

    /// Removes a deleted project and all of its tickets from the database for good. Projects
    /// that aren't deleted can't be purged, so this never removes a live board.
    pub fn purge_project(&mut self, id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        let deleted = tx
            .query_row(
                "SELECT 1 FROM projects WHERE id = ?1 AND deleted_at IS NOT NULL",
                [id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !deleted {
            return Err(Error::ProjectNotFound(id));
        }
        tx.execute("DELETE FROM tickets WHERE project_id = ?1", [id])?;
        tx.execute("DELETE FROM projects WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    /// Tickets of a project that aren't deleted, ordered by their position within each column.
    pub fn list(&self, project_id: i64) -> Result<Vec<Ticket>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "{TICKET_SELECT} WHERE project_id = ?1 AND deleted_at IS NULL ORDER BY position, id"
        ))?;
        let tickets = stmt
            .query_map([project_id], ticket_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(tickets)
    }

    pub fn get(&self, id: i64) -> Result<Ticket> {
        self.conn
            .query_row(
                &format!("{TICKET_SELECT} WHERE id = ?1 AND deleted_at IS NULL"),
                [id],
                ticket_from_row,
            )
            .optional()?
            .ok_or(Error::NotFound(id))
    }

    /// Inserts a ticket at the top of its column in `project_id`.
    pub fn create_ticket(
        &mut self,
        project_id: i64,
        status: Status,
        title: &str,
        details: &str,
    ) -> Result<Ticket> {
        let now = Utc::now();
        let tx = self.conn.transaction()?;
        let project_exists = tx
            .query_row(
                "SELECT 1 FROM projects WHERE id = ?1 AND deleted_at IS NULL",
                [project_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !project_exists {
            return Err(Error::ProjectNotFound(project_id));
        }
        tx.execute(
            "UPDATE tickets SET position = position + 1
             WHERE project_id = ?1 AND status = ?2 AND deleted_at IS NULL",
            params![project_id, status],
        )?;
        tx.execute(
            "INSERT INTO tickets (project_id, title, details, status, position, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?5)",
            params![project_id, title, details, status, now],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        self.get(id)
    }

    pub fn update_ticket(&mut self, id: i64, title: &str, details: &str) -> Result<Ticket> {
        let changed = self.conn.execute(
            "UPDATE tickets SET title = ?2, details = ?3, updated_at = ?4
             WHERE id = ?1 AND deleted_at IS NULL",
            params![id, title, details, Utc::now()],
        )?;
        if changed == 0 {
            return Err(Error::NotFound(id));
        }
        self.get(id)
    }

    /// Soft-deletes a ticket: it stays in the table with `deleted_at` set, and is hidden
    /// from every other query.
    pub fn delete_ticket(&mut self, id: i64) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE tickets SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
            params![id, Utc::now()],
        )?;
        if changed == 0 {
            return Err(Error::NotFound(id));
        }
        Ok(())
    }

    /// Deleted tickets of a project, by id.
    pub fn list_deleted_tickets(&self, project_id: i64) -> Result<Vec<Ticket>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "{TICKET_SELECT} WHERE project_id = ?1 AND deleted_at IS NOT NULL ORDER BY id"
        ))?;
        let tickets = stmt
            .query_map([project_id], ticket_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(tickets)
    }

    /// Undeletes a ticket, putting it back at the top of the column it was deleted from.
    pub fn restore_ticket(&mut self, id: i64) -> Result<Ticket> {
        let tx = self.conn.transaction()?;
        let (project_id, status): (i64, Status) = tx
            .query_row(
                "SELECT project_id, status FROM tickets WHERE id = ?1 AND deleted_at IS NOT NULL",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or(Error::NotFound(id))?;
        tx.execute(
            "UPDATE tickets SET position = position + 1
             WHERE project_id = ?1 AND status = ?2 AND deleted_at IS NULL",
            params![project_id, status],
        )?;
        tx.execute(
            "UPDATE tickets SET deleted_at = NULL, position = 0 WHERE id = ?1",
            [id],
        )?;
        tx.commit()?;
        self.get(id)
    }

    /// Removes a deleted ticket from the database for good. Tickets that aren't deleted can't
    /// be purged.
    pub fn purge_ticket(&mut self, id: i64) -> Result<()> {
        let changed = self.conn.execute(
            "DELETE FROM tickets WHERE id = ?1 AND deleted_at IS NOT NULL",
            [id],
        )?;
        if changed == 0 {
            return Err(Error::NotFound(id));
        }
        Ok(())
    }

    /// Moves a ticket to `index` within the `status` column of its project (which may be its
    /// current column).
    pub fn move_ticket(&mut self, id: i64, status: Status, index: usize) -> Result<()> {
        let tx = self.conn.transaction()?;
        let (project_id, current): (i64, Status) = tx
            .query_row(
                "SELECT project_id, status FROM tickets WHERE id = ?1 AND deleted_at IS NULL",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or(Error::NotFound(id))?;

        let mut order: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM tickets
                 WHERE project_id = ?1 AND status = ?2 AND id <> ?3 AND deleted_at IS NULL
                 ORDER BY position, id",
            )?;
            stmt.query_map(params![project_id, status, id], |r| r.get(0))?
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

/// Turns a unique-index violation on the project name into `DuplicateProjectName`.
fn duplicate_name(e: rusqlite::Error, name: &str) -> Error {
    match &e {
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.extended_code == ffi::SQLITE_CONSTRAINT_UNIQUE =>
        {
            Error::DuplicateProjectName(name.to_owned())
        }
        _ => e.into(),
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
    if version != SCHEMA_VERSION && version != 3 {
        return Err(Error::Invalid(format!(
            "unsupported schema version {version} (expected {SCHEMA_VERSION}); \
             files from before multi-project support can't be opened"
        )));
    }

    let integrity: String = conn.pragma_query_value(None, "quick_check", |r| r.get(0))?;
    if integrity != "ok" {
        return Err(Error::Invalid(format!(
            "integrity check failed: {integrity}"
        )));
    }

    validate_table(conn, "projects", PROJECT_COLUMNS)?;
    validate_table(conn, "tickets", TICKET_COLUMNS)?;

    let references_projects: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM pragma_foreign_key_list('tickets')
         WHERE \"table\" = 'projects' AND \"from\" = 'project_id' AND \"to\" = 'id'",
        [],
        |r| r.get(0),
    )?;
    if !references_projects {
        return Err(Error::Invalid(
            "`tickets.project_id` is not a foreign key to `projects.id`".into(),
        ));
    }

    if version == 3 {
        conn.execute_batch(&format!(
            "BEGIN;
             {PROJECT_NAME_INDEX_SQL}
             PRAGMA user_version = {SCHEMA_VERSION};
             COMMIT;"
        ))
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK");
            Error::Invalid(format!(
                "could not make project names unique; rename duplicate projects first ({e})"
            ))
        })?;
    }
    let unique_names: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM pragma_index_list('projects')
         WHERE name = ?1 AND \"unique\" AND partial",
        [PROJECT_NAME_INDEX],
        |r| r.get(0),
    )?;
    if !unique_names {
        return Err(Error::Invalid(format!(
            "missing unique index `{PROJECT_NAME_INDEX}` on project names"
        )));
    }
    Ok(())
}

fn validate_table(conn: &Connection, table: &str, expected: &[Column]) -> Result<()> {
    let mut stmt =
        conn.prepare("SELECT name, type, \"notnull\", pk FROM pragma_table_info(?1) ORDER BY cid")?;
    let columns: Vec<(String, String, bool, bool)> = stmt
        .query_map([table], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get::<_, i64>(2)? != 0,
                r.get::<_, i64>(3)? != 0,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;

    if columns.is_empty() {
        return Err(Error::Invalid(format!("missing `{table}` table")));
    }

    let matches = columns.len() == expected.len()
        && columns.iter().zip(expected).all(
            |((name, ty, not_null, pk), (exp_name, exp_ty, exp_not_null, exp_pk))| {
                name == exp_name
                    && ty.eq_ignore_ascii_case(exp_ty)
                    && not_null == exp_not_null
                    && pk == exp_pk
            },
        );
    if !matches {
        return Err(Error::Invalid(format!(
            "`{table}` table does not match the expected schema"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("orgx-test-{}-{name}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn creates_nested_directory_and_reopens() {
        let dir = std::env::temp_dir().join(format!("orgx-test-{}-dir", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("orgx").join("data.db");

        let mut store = Store::open(&path).unwrap();
        assert!(store.list_projects().unwrap().is_empty(), "starts empty");
        let project = store.create_project("Work").unwrap();
        store
            .create_ticket(project.id, Status::Backlog, "first", "")
            .unwrap();
        let second = store
            .create_ticket(project.id, Status::Backlog, "second", "**hi**")
            .unwrap();
        drop(store);

        let store = Store::open(&path).unwrap();
        let projects = store.list_projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "Work");
        let tickets = store.list(project.id).unwrap();
        assert_eq!(tickets.len(), 2);
        assert_eq!(tickets[0].id, second.id, "new tickets go to the top");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn keeps_projects_separate() {
        let path = temp_path("projects");
        let mut store = Store::open(&path).unwrap();
        let work = store.create_project("Work").unwrap();
        let home = store.create_project("Home").unwrap();
        let w1 = store
            .create_ticket(work.id, Status::Ready, "w1", "")
            .unwrap();
        let h1 = store
            .create_ticket(home.id, Status::Ready, "h1", "")
            .unwrap();
        let w2 = store
            .create_ticket(work.id, Status::Ready, "w2", "")
            .unwrap();

        let ids = |store: &Store, project: i64| -> Vec<i64> {
            store.list(project).unwrap().iter().map(|t| t.id).collect()
        };
        assert_eq!(ids(&store, work.id), vec![w2.id, w1.id]);
        assert_eq!(ids(&store, home.id), vec![h1.id]);

        // Reordering in one project leaves the other's positions alone.
        store.move_ticket(w2.id, Status::Ready, 1).unwrap();
        assert_eq!(ids(&store, work.id), vec![w1.id, w2.id]);
        assert_eq!(ids(&store, home.id), vec![h1.id]);

        assert!(matches!(
            store.create_ticket(999, Status::Ready, "orphan", ""),
            Err(Error::ProjectNotFound(999))
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn enforces_project_foreign_key() {
        let path = temp_path("fk");
        let store = Store::open(&path).unwrap();
        let result = store.conn.execute(
            "INSERT INTO tickets (project_id, title, status, position, created_at, updated_at)
             VALUES (999, 'x', 'ready', 0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        );
        assert!(result.is_err(), "foreign keys are enforced");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn moves_between_and_within_columns() {
        let path = temp_path("move");
        let mut store = Store::open(&path).unwrap();
        let p = store.create_project("Work").unwrap().id;
        let a = store.create_ticket(p, Status::Ready, "a", "").unwrap();
        let b = store.create_ticket(p, Status::Ready, "b", "").unwrap();
        let c = store.create_ticket(p, Status::Review, "c", "").unwrap();

        store.move_ticket(b.id, Status::Ready, 1).unwrap();
        store.move_ticket(c.id, Status::Ready, 1).unwrap();
        let ready: Vec<i64> = store
            .list(p)
            .unwrap()
            .into_iter()
            .filter(|t| t.status == Status::Ready)
            .map(|t| t.id)
            .collect();
        assert_eq!(ready, vec![a.id, c.id, b.id]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn soft_deletes_tickets() {
        let path = temp_path("delete");
        let mut store = Store::open(&path).unwrap();
        let p = store.create_project("Work").unwrap().id;
        let a = store.create_ticket(p, Status::Backlog, "a", "").unwrap();
        let b = store.create_ticket(p, Status::Backlog, "b", "").unwrap();

        store.delete_ticket(a.id).unwrap();
        let ids: Vec<i64> = store.list(p).unwrap().into_iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![b.id]);
        assert!(matches!(store.get(a.id), Err(Error::NotFound(_))));
        assert!(matches!(store.delete_ticket(a.id), Err(Error::NotFound(_))));
        assert!(matches!(
            store.update_ticket(a.id, "x", ""),
            Err(Error::NotFound(_))
        ));
        assert!(matches!(
            store.move_ticket(a.id, Status::Ready, 0),
            Err(Error::NotFound(_))
        ));

        // The row is kept, with its deletion time.
        let (title, deleted_at): (String, Option<String>) = store
            .conn
            .query_row(
                "SELECT title, deleted_at FROM tickets WHERE id = ?1",
                [a.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(title, "a");
        assert!(deleted_at.is_some());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn renames_and_soft_deletes_projects() {
        let path = temp_path("project-crud");
        let mut store = Store::open(&path).unwrap();
        let work = store.create_project("Work").unwrap();
        let home = store.create_project("Home").unwrap();

        let renamed = store.update_project(work.id, "Office").unwrap();
        assert_eq!(renamed.name, "Office");
        assert_eq!(store.get_project(work.id).unwrap().name, "Office");

        store.delete_project(work.id).unwrap();
        let ids: Vec<i64> = store
            .list_projects()
            .unwrap()
            .iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(ids, vec![home.id]);
        assert!(matches!(
            store.get_project(work.id),
            Err(Error::ProjectNotFound(_))
        ));
        assert!(matches!(
            store.update_project(work.id, "x"),
            Err(Error::ProjectNotFound(_))
        ));
        assert!(matches!(
            store.delete_project(work.id),
            Err(Error::ProjectNotFound(_))
        ));
        assert!(matches!(
            store.create_ticket(work.id, Status::Ready, "t", ""),
            Err(Error::ProjectNotFound(_))
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn project_names_are_unique_among_live_projects() {
        let path = temp_path("unique-names");
        let mut store = Store::open(&path).unwrap();
        let work = store.create_project("Work").unwrap();
        let home = store.create_project("Home").unwrap();

        assert!(matches!(
            store.create_project("work"),
            Err(Error::DuplicateProjectName(_))
        ));
        assert!(matches!(
            store.update_project(home.id, "WORK"),
            Err(Error::DuplicateProjectName(_))
        ));
        // Renaming a project to itself, in another case, is fine.
        store.update_project(work.id, "work").unwrap();

        // A deleted project's name is free, but it can't be restored while it's taken.
        store.delete_project(work.id).unwrap();
        let new_work = store.create_project("Work").unwrap();
        assert!(matches!(
            store.restore_project(work.id),
            Err(Error::DuplicateProjectName(_))
        ));
        store.delete_project(new_work.id).unwrap();
        assert_eq!(store.restore_project(work.id).unwrap().name, "work");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn lists_and_restores_deleted_projects() {
        let path = temp_path("restore-projects");
        let mut store = Store::open(&path).unwrap();
        let a = store.create_project("A").unwrap();
        let b = store.create_project("B").unwrap();
        store.delete_project(a.id).unwrap();
        store.delete_project(b.id).unwrap();

        let deleted: Vec<i64> = store
            .list_deleted_projects()
            .unwrap()
            .iter()
            .map(|(p, _)| p.id)
            .collect();
        assert_eq!(deleted, vec![b.id, a.id], "most recently deleted first");

        store.restore_project(a.id).unwrap();
        assert_eq!(store.list_projects().unwrap()[0].id, a.id);
        assert_eq!(store.list_deleted_projects().unwrap().len(), 1);
        assert!(matches!(
            store.restore_project(a.id),
            Err(Error::ProjectNotFound(_))
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn purges_only_deleted_projects() {
        let path = temp_path("purge");
        let mut store = Store::open(&path).unwrap();
        let live = store.create_project("Live").unwrap();
        let gone = store.create_project("Gone").unwrap();
        store
            .create_ticket(live.id, Status::Ready, "keep", "")
            .unwrap();
        let t = store
            .create_ticket(gone.id, Status::Ready, "t", "")
            .unwrap();
        store.delete_ticket(t.id).unwrap();
        store
            .create_ticket(gone.id, Status::Ready, "u", "")
            .unwrap();

        assert!(matches!(
            store.purge_project(live.id),
            Err(Error::ProjectNotFound(_))
        ));
        store.delete_project(gone.id).unwrap();
        store.purge_project(gone.id).unwrap();

        let count = |sql: &str| -> i64 { store.conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(count("SELECT COUNT(*) FROM projects"), 1);
        assert_eq!(
            count("SELECT COUNT(*) FROM tickets"),
            1,
            "its tickets go too"
        );
        assert!(store.list_deleted_projects().unwrap().is_empty());
        assert_eq!(store.list(live.id).unwrap().len(), 1);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn lists_and_restores_deleted_tickets() {
        let path = temp_path("restore-tickets");
        let mut store = Store::open(&path).unwrap();
        let p = store.create_project("Work").unwrap().id;
        let other = store.create_project("Home").unwrap().id;
        let a = store.create_ticket(p, Status::Review, "a", "").unwrap();
        let b = store.create_ticket(p, Status::Review, "b", "").unwrap();
        let c = store.create_ticket(other, Status::Review, "c", "").unwrap();
        store.delete_ticket(a.id).unwrap();
        store.delete_ticket(c.id).unwrap();

        let deleted: Vec<i64> = store
            .list_deleted_tickets(p)
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(deleted, vec![a.id], "only this project's tickets");

        // `a` was below `b`; it comes back at the top of its column.
        let restored = store.restore_ticket(a.id).unwrap();
        assert_eq!(restored.status, Status::Review);
        let ids: Vec<i64> = store.list(p).unwrap().iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![a.id, b.id]);
        assert!(store.list_deleted_tickets(p).unwrap().is_empty());
        assert!(matches!(
            store.restore_ticket(a.id),
            Err(Error::NotFound(_))
        ));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn purges_only_deleted_tickets() {
        let path = temp_path("purge-tickets");
        let mut store = Store::open(&path).unwrap();
        let p = store.create_project("Work").unwrap().id;
        let live = store.create_ticket(p, Status::Ready, "live", "").unwrap();
        let gone = store.create_ticket(p, Status::Ready, "gone", "").unwrap();

        assert!(matches!(
            store.purge_ticket(live.id),
            Err(Error::NotFound(_))
        ));
        store.delete_ticket(gone.id).unwrap();
        store.purge_ticket(gone.id).unwrap();

        assert!(store.list_deleted_tickets(p).unwrap().is_empty());
        assert!(matches!(
            store.restore_ticket(gone.id),
            Err(Error::NotFound(_))
        ));
        let rows: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "the row is gone, not just hidden");
        assert_eq!(store.list(p).unwrap()[0].id, live.id);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn migrates_version_3_files() {
        let path = temp_path("migrate-v3");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&format!(
            "{SCHEMA}
             INSERT INTO projects (name) VALUES ('Work');
             PRAGMA application_id = {APPLICATION_ID};
             PRAGMA user_version = 3;"
        ))
        .unwrap();
        drop(conn);

        let mut store = Store::open(&path).unwrap();
        assert!(matches!(
            store.create_project("Work"),
            Err(Error::DuplicateProjectName(_))
        ));
        drop(store);
        let version: i32 = Connection::open(&path)
            .unwrap()
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        std::fs::remove_file(&path).unwrap();

        // Duplicate names from before the constraint are reported, and the file is left alone.
        let path = temp_path("migrate-v3-duplicates");
        Connection::open(&path)
            .unwrap()
            .execute_batch(&format!(
                "{SCHEMA}
                 INSERT INTO projects (name) VALUES ('Work'), ('work');
                 PRAGMA application_id = {APPLICATION_ID};
                 PRAGMA user_version = 3;"
            ))
            .unwrap();
        let Err(Error::Invalid(reason)) = Store::open(&path) else {
            panic!("duplicate names accepted");
        };
        assert!(reason.contains("rename duplicate projects"), "{reason}");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn rejects_foreign_and_old_files() {
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

        // A single-board `project.db` from before projects existed.
        let path = temp_path("old-version");
        Connection::open(&path)
            .unwrap()
            .execute_batch(&format!(
                "CREATE TABLE tickets (id INTEGER PRIMARY KEY);
                 PRAGMA application_id = {APPLICATION_ID};
                 PRAGMA user_version = 2;"
            ))
            .unwrap();
        let Err(Error::Invalid(reason)) = Store::open(&path) else {
            panic!("old schema accepted");
        };
        assert!(reason.contains("schema version 2"), "{reason}");
        std::fs::remove_file(&path).unwrap();
    }
}
