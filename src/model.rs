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
pub struct Ticket {
    pub id: i64,
    pub title: String,
    pub details: String,
    pub status: Status,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
