use chrono::{DateTime, Utc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    Backlog,
    Ready,
    InProgress,
    Review,
    Completed,
}

impl Status {
    /// Board order, left to right.
    pub const ALL: [Status; 5] = [
        Status::Backlog,
        Status::Ready,
        Status::InProgress,
        Status::Review,
        Status::Completed,
    ];

    /// Value stored in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Backlog => "backlog",
            Status::Ready => "ready",
            Status::InProgress => "in-progress",
            Status::Review => "review",
            Status::Completed => "completed",
        }
    }

    pub fn parse(value: &str) -> Option<Status> {
        Status::ALL.into_iter().find(|s| s.as_str() == value)
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Backlog => "Backlog",
            Status::Ready => "Ready",
            Status::InProgress => "In-Progress",
            Status::Review => "Review",
            Status::Completed => "Completed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Project {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct Ticket {
    pub id: i64,
    pub title: String,
    pub details: String,
    pub status: Status,
    pub created_at: DateTime<Utc>,
    /// Kept up to date in the database, though the UI no longer shows it.
    #[allow(dead_code)]
    pub updated_at: DateTime<Utc>,
    /// When the ticket was moved to Completed; `None` while it is in any other column.
    pub completed_at: Option<DateTime<Utc>>,
    /// Set a while after completion, when the ticket leaves the board for the archive.
    pub archived_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct Note {
    pub id: i64,
    /// `None` for untitled notes.
    pub title: Option<String>,
    /// Markdown.
    pub detail: String,
    pub updated_at: DateTime<Utc>,
}
