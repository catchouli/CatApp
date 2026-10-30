//! Storage: one SQLite file shared by the web app, the CLI and the MCP
//! server. WAL mode plus a busy timeout let them all use it at once.

use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Project statuses, in display order.
pub const STATUSES: &[&str] = &["active", "planned", "idea", "paused", "done", "abandoned", "archived"];

/// Statuses meaning no more work is planned: hidden from default lists.
pub const FINISHED: &[&str] = &["done", "abandoned", "archived"];

/// The built-in lists: projects with these slugs show as panels on the home page, not as cards.
pub const TODO_SLUG: &str = "todo";
pub const IDEAS_SLUG: &str = "ideas";

/// A built-in list's name and summary, or None if `slug` isn't one.
pub fn builtin_list(slug: &str, owner: &str) -> Option<(&'static str, String)> {
    match slug {
        TODO_SLUG => Some(("To do", format!("{owner}'s general to-do list. Shown at the top of the home page."))),
        IDEAS_SLUG => Some(("Ideas", format!("{owner}'s ideas: things to maybe do some day. Shown on the home page."))),
        _ => None,
    }
}

pub fn is_finished(status: &str) -> bool {
    FINISHED.contains(&status)
}

#[derive(Debug)]
pub enum Error {
    NotFound(String),
    Invalid(String),
    Db(rusqlite::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound(s) | Error::Invalid(s) => f.write_str(s),
            Error::Db(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Db(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[derive(Debug, Clone, Serialize)]
pub struct Project {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub category: String,
    pub status: String,
    pub summary: String,
    pub next_step: String,
    pub links: String,
    pub created_at: String,
    pub updated_at: String,
    /// The later of `updated_at` and the newest note.
    pub last_activity: String,
    pub note_count: i64,
    pub tasks_done: i64,
    pub tasks_total: i64,
    /// Ideas not yet ticked off.
    pub ideas_open: i64,
    /// Encrypted in the browser and hidden from everything but the web app (see `Db::show_private`).
    pub private: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Note {
    pub id: i64,
    pub project_id: i64,
    pub project_slug: String,
    pub project_name: String,
    pub category: String,
    /// "note" for things people write; "status" and "next" for changes the
    /// app records itself.
    pub kind: String,
    pub body: String,
    /// Where it was written: web, cli or claude.
    pub source: String,
    pub created_at: String,
    pub edited_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Task {
    pub id: i64,
    pub project_id: i64,
    /// "task" for the checklist, "idea" for things to maybe do some day.
    pub kind: String,
    pub text: String,
    pub done: bool,
    pub position: i64,
    pub created_at: String,
    pub done_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectDetail {
    #[serde(flatten)]
    pub project: Project,
    pub tasks: Vec<Task>,
    pub ideas: Vec<Task>,
    pub notes: Vec<Note>,
}

/// A task or idea with the project it's on, for the pages that gather them from every project.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectItem {
    #[serde(flatten)]
    pub item: Task,
    pub project_slug: String,
    pub project_name: String,
    pub project_status: String,
    pub project_private: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NewProject {
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub next_step: Option<String>,
    #[serde(default)]
    pub links: Option<String>,
    /// Its text comes already encrypted (only the web app makes these).
    #[serde(default)]
    pub private: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProjectPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub next_step: Option<String>,
    #[serde(default)]
    pub links: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TaskPatch {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub done: Option<bool>,
    /// "task" or "idea": turns one into the other.
    #[serde(default)]
    pub kind: Option<String>,
}

const PROJECT_SELECT: &str = "SELECT p.id, p.slug, p.name, p.category, p.status, p.summary, p.next_step, p.links,
    p.created_at, p.updated_at,
    MAX(p.updated_at, COALESCE((SELECT MAX(n.created_at) FROM notes n WHERE n.project_id = p.id), '')),
    (SELECT COUNT(*) FROM notes n WHERE n.project_id = p.id AND n.kind = 'note'),
    (SELECT COUNT(*) FROM tasks t WHERE t.project_id = p.id AND t.kind = 'task' AND t.done = 1),
    (SELECT COUNT(*) FROM tasks t WHERE t.project_id = p.id AND t.kind = 'task'),
    (SELECT COUNT(*) FROM tasks t WHERE t.project_id = p.id AND t.kind = 'idea' AND t.done = 0),
    p.private
    FROM projects p";

const NOTE_SELECT: &str = "SELECT n.id, n.project_id, p.slug, p.name, p.category, n.kind, n.body, n.source,
    n.created_at, n.edited_at FROM notes n JOIN projects p ON p.id = n.project_id";

const TASK_SELECT: &str = "SELECT id, project_id, kind, text, done, position, created_at, done_at FROM tasks";

/// What a checklist item can be.
pub const KINDS: &[&str] = &["task", "idea"];

fn project_row(r: &Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get(0)?,
        slug: r.get(1)?,
        name: r.get(2)?,
        category: r.get(3)?,
        status: r.get(4)?,
        summary: r.get(5)?,
        next_step: r.get(6)?,
        links: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
        last_activity: r.get(10)?,
        note_count: r.get(11)?,
        tasks_done: r.get(12)?,
        tasks_total: r.get(13)?,
        ideas_open: r.get(14)?,
        private: r.get::<_, i64>(15)? != 0,
    })
}

fn note_row(r: &Row) -> rusqlite::Result<Note> {
    Ok(Note {
        id: r.get(0)?,
        project_id: r.get(1)?,
        project_slug: r.get(2)?,
        project_name: r.get(3)?,
        category: r.get(4)?,
        kind: r.get(5)?,
        body: r.get(6)?,
        source: r.get(7)?,
        created_at: r.get(8)?,
        edited_at: r.get(9)?,
    })
}

fn task_row(r: &Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: r.get(0)?,
        project_id: r.get(1)?,
        kind: r.get(2)?,
        text: r.get(3)?,
        done: r.get::<_, i64>(4)? != 0,
        position: r.get(5)?,
        created_at: r.get(6)?,
        done_at: r.get(7)?,
    })
}

pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "project".into()
    } else {
        out
    }
}

/// Categories are lowercase words; empty means "personal".
pub fn norm_category(c: &str) -> String {
    let c = c.trim().to_lowercase();
    if c.is_empty() {
        "personal".into()
    } else {
        c
    }
}

fn check_status(s: &str) -> Result<String> {
    let s = s.trim().to_lowercase();
    if STATUSES.contains(&s.as_str()) {
        Ok(s)
    } else {
        Err(Error::Invalid(format!("unknown status '{s}': use one of {}", STATUSES.join(", "))))
    }
}

fn check_kind(k: &str) -> Result<String> {
    let k = k.trim().to_lowercase();
    if KINDS.contains(&k.as_str()) {
        Ok(k)
    } else {
        Err(Error::Invalid(format!("unknown kind '{k}': use task or idea")))
    }
}

/// What everything on a built-in list is: tasks on the to-do list, ideas on the ideas list.
fn list_kind(slug: &str) -> Option<&'static str> {
    match slug {
        TODO_SLUG => Some("task"),
        IDEAS_SLUG => Some("idea"),
        _ => None,
    }
}

fn non_empty(s: &str, what: &str) -> Result<String> {
    let s = s.trim();
    if s.is_empty() {
        Err(Error::Invalid(format!("{what} can't be empty")))
    } else {
        Ok(s.to_string())
    }
}

pub struct Db {
    conn: Connection,
    /// Whether private projects exist as far as callers can tell. Only the web app turns this on: to
    /// the CLI, MCP and the assistant they're simply not there. Their text is encrypted either way.
    private: bool,
}

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    #[cfg(test)]
    pub fn memory() -> Result<Db> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Db> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS projects (
                    id INTEGER PRIMARY KEY,
                    slug TEXT NOT NULL UNIQUE,
                    name TEXT NOT NULL,
                    category TEXT NOT NULL DEFAULT 'personal',
                    status TEXT NOT NULL DEFAULT 'active',
                    summary TEXT NOT NULL DEFAULT '',
                    next_step TEXT NOT NULL DEFAULT '',
                    links TEXT NOT NULL DEFAULT '',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS notes (
                    id INTEGER PRIMARY KEY,
                    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                    kind TEXT NOT NULL DEFAULT 'note',
                    body TEXT NOT NULL,
                    source TEXT NOT NULL DEFAULT 'web',
                    created_at TEXT NOT NULL,
                    edited_at TEXT
                );
                CREATE INDEX IF NOT EXISTS notes_by_project ON notes(project_id, created_at);
                CREATE TABLE IF NOT EXISTS tasks (
                    id INTEGER PRIMARY KEY,
                    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                    text TEXT NOT NULL,
                    done INTEGER NOT NULL DEFAULT 0,
                    position INTEGER NOT NULL,
                    created_at TEXT NOT NULL,
                    done_at TEXT
                );
                PRAGMA user_version = 1;",
            )?;
        }
        if version < 2 {
            // Checklist items become tasks or ideas; the ideas list's items are ideas.
            conn.execute_batch(&format!(
                "BEGIN;
                ALTER TABLE tasks ADD COLUMN kind TEXT NOT NULL DEFAULT 'task';
                UPDATE tasks SET kind = 'idea' WHERE project_id IN (SELECT id FROM projects WHERE slug = '{IDEAS_SLUG}');
                PRAGMA user_version = 2;
                COMMIT;"
            ))?;
        }
        if version < 3 {
            // Private projects, and somewhere to keep the salt and check value for their passphrase.
            conn.execute_batch(
                "BEGIN;
                ALTER TABLE projects ADD COLUMN private INTEGER NOT NULL DEFAULT 0;
                CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                PRAGMA user_version = 3;
                COMMIT;",
            )?;
        }
        Ok(Db { conn, private: false })
    }

    /// Lets this connection see private projects. For the web app only.
    pub fn show_private(mut self) -> Db {
        self.private = true;
        self
    }

    fn hidden(&self, p: &Project) -> bool {
        p.private && !self.private
    }

    // ---- private projects ------------------------------------------------------------

    /// The passphrase's salt and check value, as the browser stored them, if one has been set.
    pub fn vault(&self) -> Result<Option<Value>> {
        self.require_private()?;
        let v: Option<String> = self.conn.query_row("SELECT value FROM settings WHERE key = 'vault'", [], |r| r.get(0)).optional()?;
        Ok(v.and_then(|v| serde_json::from_str(&v).ok()))
    }

    /// Stores the passphrase's salt and check value. Only once: changing the passphrase would mean
    /// re-encrypting everything.
    pub fn set_vault(&self, v: &Value) -> Result<()> {
        self.require_private()?;
        if self.vault()?.is_some() {
            return Err(Error::Invalid("the passphrase is already set".into()));
        }
        self.conn.execute("INSERT INTO settings (key, value) VALUES ('vault', ?1)", [v.to_string()])?;
        Ok(())
    }

    fn require_private(&self) -> Result<()> {
        if self.private {
            Ok(())
        } else {
            Err(Error::NotFound("private projects are only in the web app".into()))
        }
    }

    // ---- projects ------------------------------------------------------------------

    /// Every project, most recently active first.
    pub fn projects(&self) -> Result<Vec<Project>> {
        let mut st = self.conn.prepare(&format!("{PROJECT_SELECT} ORDER BY 11 DESC"))?;
        let rows = st.query_map([], project_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows.into_iter().filter(|p| !self.hidden(p)).collect())
    }

    pub fn project_by_id(&self, id: i64) -> Result<Project> {
        self.conn
            .query_row(&format!("{PROJECT_SELECT} WHERE p.id = ?1"), [id], project_row)
            .optional()?
            .filter(|p| !self.hidden(p))
            .ok_or_else(|| Error::NotFound(format!("no project with id {id}")))
    }

    /// Finds a project by slug, id, exact name, or a unique part of its name. A folder path
    /// (absolute, `~/...`, or `.` for the current folder) finds the project whose links
    /// contain that folder or one of its parents.
    pub fn find(&self, query: &str) -> Result<Project> {
        let q = query.trim();
        if q.is_empty() {
            return Err(Error::Invalid("which project? (give its name or slug)".into()));
        }
        if q == "." || q.starts_with('/') || q.starts_with("~/") {
            let dir = if q == "." { std::env::current_dir().map_err(|e| Error::Invalid(e.to_string()))? } else { expand(q) };
            return self.find_by_dir(&dir);
        }
        let all = self.projects()?;
        if let Some(p) = all.iter().find(|p| p.slug == q) {
            return Ok(p.clone());
        }
        if let Ok(id) = q.parse::<i64>() {
            if let Some(p) = all.iter().find(|p| p.id == id) {
                return Ok(p.clone());
            }
        }
        let lq = q.to_lowercase();
        if let Some(p) = all.iter().find(|p| p.name.to_lowercase() == lq) {
            return Ok(p.clone());
        }
        let sq = slugify(q);
        let hits: Vec<&Project> =
            all.iter().filter(|p| p.name.to_lowercase().contains(&lq) || p.slug.contains(&sq)).collect();
        match hits.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err(Error::NotFound(format!("no project matches '{q}'"))),
            many => Err(Error::Invalid(format!(
                "'{q}' matches several projects: {}",
                many.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>().join(", ")
            ))),
        }
    }

    /// The project whose links include `dir` or its nearest parent folder.
    pub fn find_by_dir(&self, dir: &Path) -> Result<Project> {
        let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        let mut best: Option<(usize, Vec<Project>)> = None;
        for p in self.projects()? {
            let depth = p
                .links
                .lines()
                .filter_map(|l| l.split_whitespace().next())
                .filter(|t| !t.contains("://"))
                .map(expand)
                .filter(|path| path.is_dir())
                .filter_map(|path| path.canonicalize().ok())
                .filter(|path| dir.starts_with(path))
                .map(|path| path.components().count())
                .max();
            if let Some(d) = depth {
                match &mut best {
                    Some((bd, ps)) if *bd == d => ps.push(p),
                    Some((bd, _)) if *bd > d => {}
                    _ => best = Some((d, vec![p])),
                }
            }
        }
        match best {
            Some((_, ps)) if ps.len() == 1 => Ok(ps.into_iter().next().unwrap()),
            Some((_, ps)) => Err(Error::Invalid(format!(
                "{} belongs to several projects: {}",
                dir.display(),
                ps.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>().join(", ")
            ))),
            None => Err(Error::NotFound(format!(
                "no project has {} in its links (add the folder to a project's links to connect them)",
                dir.display()
            ))),
        }
    }

    pub fn detail(&self, project: &Project, notes_limit: usize) -> Result<ProjectDetail> {
        Ok(ProjectDetail {
            project: project.clone(),
            tasks: self.items(project.id, "task")?,
            ideas: self.items(project.id, "idea")?,
            notes: self.notes(project.id, notes_limit)?,
        })
    }

    pub fn create(&self, p: NewProject, source: &str) -> Result<Project> {
        let name = non_empty(&p.name, "name")?;
        let category = norm_category(p.category.as_deref().unwrap_or(""));
        let status = check_status(p.status.as_deref().unwrap_or("active"))?;
        if p.private {
            self.require_private()?;
        }
        // A private project's name is encrypted, so its slug can't come from it.
        let base = if p.private { "private".into() } else { slugify(&name) };
        let mut slug = base.clone();
        let mut n = 2;
        while self.conn.query_row("SELECT 1 FROM projects WHERE slug = ?1", [&slug], |_| Ok(())).optional()?.is_some() {
            slug = format!("{base}-{n}");
            n += 1;
        }
        let t = now();
        self.conn.execute(
            "INSERT INTO projects (slug, name, category, status, summary, next_step, links, created_at, updated_at, private)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9)",
            params![
                slug,
                name,
                category,
                status,
                p.summary.unwrap_or_default().trim(),
                p.next_step.unwrap_or_default().trim(),
                p.links.unwrap_or_default().trim(),
                t,
                p.private as i64
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.add_entry(id, "status", &format!("Project created ({status})"), source)?;
        self.project_by_id(id)
    }

    /// Changes a project. Status and next-step changes are also recorded in
    /// its log, so the history shows how it moved.
    pub fn update(&self, id: i64, patch: ProjectPatch, source: &str) -> Result<Project> {
        let cur = self.project_by_id(id)?;
        let name = match &patch.name {
            Some(n) => non_empty(n, "name")?,
            None => cur.name.clone(),
        };
        let category = patch.category.as_deref().map(norm_category).unwrap_or(cur.category.clone());
        let status = match &patch.status {
            Some(s) => check_status(s)?,
            None => cur.status.clone(),
        };
        let summary = patch.summary.as_deref().map(|s| s.trim().to_string()).unwrap_or(cur.summary.clone());
        let next_step = patch.next_step.as_deref().map(|s| s.trim().to_string()).unwrap_or(cur.next_step.clone());
        let links = patch.links.as_deref().map(|s| s.trim().to_string()).unwrap_or(cur.links.clone());
        self.conn.execute(
            "UPDATE projects SET name = ?1, category = ?2, status = ?3, summary = ?4, next_step = ?5, links = ?6,
             updated_at = ?7 WHERE id = ?8",
            params![name, category, status, summary, next_step, links, now(), id],
        )?;
        if status != cur.status {
            self.add_entry(id, "status", &format!("Status: {} → {}", cur.status, status), source)?;
        }
        if next_step != cur.next_step && !next_step.is_empty() {
            // A private project's next step is encrypted, so its entry is just that; the app adds the words.
            let body = if cur.private { next_step.clone() } else { format!("Next step: {next_step}") };
            self.add_entry(id, "next", &body, source)?;
        }
        self.project_by_id(id)
    }

    pub fn delete_project(&self, id: i64) -> Result<()> {
        let n = self.conn.execute("DELETE FROM projects WHERE id = ?1", [id])?;
        if n == 0 {
            return Err(Error::NotFound(format!("no project with id {id}")));
        }
        Ok(())
    }

    pub fn categories(&self) -> Result<Vec<String>> {
        let mut st = self.conn.prepare("SELECT DISTINCT category FROM projects ORDER BY category")?;
        let mut cats: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        for c in ["work", "personal"] {
            if !cats.iter().any(|x| x == c) {
                cats.push(c.into());
            }
        }
        cats.sort();
        Ok(cats)
    }

    // ---- notes ---------------------------------------------------------------------

    fn add_entry(&self, project_id: i64, kind: &str, body: &str, source: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO notes (project_id, kind, body, source, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![project_id, kind, body, source, now()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn add_note(&self, project_id: i64, body: &str, source: &str) -> Result<Note> {
        let body = non_empty(body, "a note")?;
        self.project_by_id(project_id)?;
        let id = self.add_entry(project_id, "note", &body, source)?;
        self.note(id)
    }

    pub fn note(&self, id: i64) -> Result<Note> {
        self.conn
            .query_row(&format!("{NOTE_SELECT} WHERE n.id = ?1 AND (?2 OR p.private = 0)"), params![id, self.private], note_row)
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("no note with id {id}")))
    }

    /// A project's log, newest first. `limit` 0 means all.
    pub fn notes(&self, project_id: i64, limit: usize) -> Result<Vec<Note>> {
        let lim = if limit == 0 { -1 } else { limit as i64 };
        let mut st =
            self.conn.prepare(&format!("{NOTE_SELECT} WHERE n.project_id = ?1 ORDER BY n.created_at DESC, n.id DESC LIMIT ?2"))?;
        let rows = st.query_map(params![project_id, lim], note_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn edit_note(&self, id: i64, body: &str) -> Result<Note> {
        let body = non_empty(body, "a note")?;
        self.note(id)?;
        let n = self.conn.execute("UPDATE notes SET body = ?1, edited_at = ?2 WHERE id = ?3", params![body, now(), id])?;
        if n == 0 {
            return Err(Error::NotFound(format!("no note with id {id}")));
        }
        self.note(id)
    }

    pub fn delete_note(&self, id: i64) -> Result<()> {
        self.note(id)?;
        if self.conn.execute("DELETE FROM notes WHERE id = ?1", [id])? == 0 {
            return Err(Error::NotFound(format!("no note with id {id}")));
        }
        Ok(())
    }

    /// Recent notes across projects (written notes only, not status changes). Never private ones.
    pub fn recent(&self, limit: usize, category: Option<&str>) -> Result<Vec<Note>> {
        let mut st = self.conn.prepare(&format!(
            "{NOTE_SELECT} WHERE n.kind = 'note' AND p.private = 0 AND (?1 IS NULL OR p.category = ?1)
             ORDER BY n.created_at DESC, n.id DESC LIMIT ?2"
        ))?;
        let rows = st.query_map(params![category, limit as i64], note_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Notes, projects and tasks containing the text (case-insensitive). Private projects are
    /// encrypted, so they're never searched.
    pub fn search(&self, q: &str) -> Result<(Vec<Project>, Vec<Note>, Vec<Task>)> {
        let q = non_empty(q, "a search")?.to_lowercase();
        let projects = self
            .projects()?
            .into_iter()
            .filter(|p| !p.private)
            .filter(|p| [&p.name, &p.summary, &p.next_step, &p.links].iter().any(|s| s.to_lowercase().contains(&q)))
            .collect();
        let pat = format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let mut st = self.conn.prepare(&format!(
            "{NOTE_SELECT} WHERE lower(n.body) LIKE ?1 ESCAPE '\\' AND p.private = 0 ORDER BY n.created_at DESC LIMIT 50"
        ))?;
        let notes = st.query_map([&pat], note_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut st = self.conn.prepare(&format!(
            "{TASK_SELECT} WHERE lower(text) LIKE ?1 ESCAPE '\\'
             AND project_id IN (SELECT id FROM projects WHERE private = 0) LIMIT 50"
        ))?;
        let tasks = st.query_map([&pat], task_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((projects, notes, tasks))
    }

    // ---- tasks ---------------------------------------------------------------------

    /// A project's tasks and ideas, in checklist order.
    pub fn tasks(&self, project_id: i64) -> Result<Vec<Task>> {
        let mut st = self.conn.prepare(&format!("{TASK_SELECT} WHERE project_id = ?1 ORDER BY position, id"))?;
        let rows = st.query_map([project_id], task_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Just a project's tasks, or just its ideas.
    pub fn items(&self, project_id: i64, kind: &str) -> Result<Vec<Task>> {
        Ok(self.tasks(project_id)?.into_iter().filter(|t| t.kind == kind).collect())
    }

    /// Open ideas on projects' own ideas lists (not the general one), most recently active project first.
    pub fn project_ideas(&self) -> Result<Vec<ProjectItem>> {
        Ok(self.open_items("idea", false)?.into_iter().filter(|i| i.project_slug != IDEAS_SLUG).collect())
    }

    /// Every open task or idea (`kind`): the to-do or ideas list's first, then each project's, most
    /// recently active first. Private projects' too, for the web app.
    pub fn open_items_everywhere(&self, kind: &str) -> Result<Vec<ProjectItem>> {
        self.open_items(&check_kind(kind)?, self.private)
    }

    fn open_items(&self, kind: &str, private: bool) -> Result<Vec<ProjectItem>> {
        let mut st = self.conn.prepare(
            "SELECT t.id, t.project_id, t.kind, t.text, t.done, t.position, t.created_at, t.done_at, p.slug, p.name, p.status, p.private
             FROM tasks t JOIN projects p ON p.id = t.project_id
             WHERE t.kind = ?1 AND t.done = 0 AND (?2 OR p.private = 0)
             ORDER BY p.slug IN (?3, ?4) DESC, p.updated_at DESC, p.id, t.position, t.id",
        )?;
        let rows = st
            .query_map(params![kind, private, TODO_SLUG, IDEAS_SLUG], |r| {
                Ok(ProjectItem {
                    item: task_row(r)?,
                    project_slug: r.get(8)?,
                    project_name: r.get(9)?,
                    project_status: r.get(10)?,
                    project_private: r.get::<_, i64>(11)? != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn task(&self, id: i64) -> Result<Task> {
        self.conn
            .query_row(
                &format!("{TASK_SELECT} WHERE id = ?1 AND (?2 OR project_id IN (SELECT id FROM projects WHERE private = 0))"),
                params![id, self.private],
                task_row,
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("no task with id {id}")))
    }

    /// Adds a task (an idea on the ideas list).
    pub fn add_task(&self, project_id: i64, text: &str) -> Result<Task> {
        self.add_item(project_id, text, "task")
    }

    /// Adds an idea to a project's own ideas list (or the general one).
    pub fn add_idea(&self, project_id: i64, text: &str) -> Result<Task> {
        self.add_item(project_id, text, "idea")
    }

    fn add_item(&self, project_id: i64, text: &str, kind: &str) -> Result<Task> {
        let text = non_empty(text, if kind == "idea" { "an idea" } else { "a task" })?;
        let p = self.project_by_id(project_id)?;
        let kind = list_kind(&p.slug).unwrap_or(kind);
        let pos: i64 =
            self.conn.query_row("SELECT COALESCE(MAX(position), 0) + 1 FROM tasks WHERE project_id = ?1", [project_id], |r| r.get(0))?;
        self.conn.execute(
            "INSERT INTO tasks (project_id, kind, text, position, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![project_id, kind, text, pos, now()],
        )?;
        self.touch(project_id)?;
        self.task(self.conn.last_insert_rowid())
    }

    pub fn update_task(&self, id: i64, patch: TaskPatch) -> Result<Task> {
        let cur = self.task(id)?;
        let text = match &patch.text {
            Some(t) => non_empty(t, "a task")?,
            None => cur.text.clone(),
        };
        let done = patch.done.unwrap_or(cur.done);
        let done_at = if done { cur.done_at.clone().or_else(|| Some(now())) } else { None };
        let kind = match &patch.kind {
            Some(k) => check_kind(k)?,
            None => cur.kind.clone(),
        };
        if kind != cur.kind {
            let p = self.project_by_id(cur.project_id)?;
            if let Some(fixed) = list_kind(&p.slug).filter(|f| *f != kind) {
                return Err(Error::Invalid(format!("everything on {} is a {fixed}: move it to a project instead", p.name)));
            }
        }
        self.conn.execute(
            "UPDATE tasks SET text = ?1, done = ?2, done_at = ?3, kind = ?4 WHERE id = ?5",
            params![text, done as i64, done_at, kind, id],
        )?;
        self.touch(cur.project_id)?;
        self.task(id)
    }

    /// Moves a task or idea to the end of another project's list. It stays what it is, except that
    /// on the to-do list it becomes a task and on the ideas list an idea. Both projects count it as activity.
    pub fn move_task(&self, id: i64, project_id: i64) -> Result<Task> {
        let cur = self.task(id)?;
        let to = self.project_by_id(project_id)?;
        if cur.project_id == project_id {
            return Ok(cur);
        }
        if to.private != self.project_by_id(cur.project_id)?.private {
            return Err(Error::Invalid("private things stay private: move it between private projects only".into()));
        }
        let kind = list_kind(&to.slug).unwrap_or(&cur.kind);
        let pos: i64 =
            self.conn.query_row("SELECT COALESCE(MAX(position), 0) + 1 FROM tasks WHERE project_id = ?1", [project_id], |r| r.get(0))?;
        self.conn.execute(
            "UPDATE tasks SET project_id = ?1, position = ?2, kind = ?3 WHERE id = ?4",
            params![project_id, pos, kind, id],
        )?;
        self.touch(cur.project_id)?;
        self.touch(project_id)?;
        self.task(id)
    }

    /// A built-in list (see `builtin_list`), made the first time it's needed. It's created under
    /// its slug so the slug is exact, then renamed (slugs never change on rename).
    pub fn ensure_list(&self, slug: &str, owner: &str, source: &str) -> Result<Project> {
        let (name, summary) = builtin_list(slug, owner).ok_or_else(|| Error::Invalid(format!("'{slug}' isn't a built-in list")))?;
        if let Some(p) = self.projects()?.into_iter().find(|p| p.slug == slug) {
            return Ok(p);
        }
        let p = self.create(
            NewProject { name: slug.into(), category: Some("personal".into()), summary: Some(summary), ..Default::default() },
            source,
        )?;
        if p.slug != slug {
            self.delete_project(p.id)?;
            return Err(Error::Invalid(format!("couldn't make the '{slug}' list")));
        }
        self.update(p.id, ProjectPatch { name: Some(name.into()), ..Default::default() }, source)
    }

    /// `find`, except that a built-in list's slug always means that list (made if need be), never a
    /// project whose name happens to contain the word.
    pub fn find_or_list(&self, query: &str, owner: &str, source: &str) -> Result<Project> {
        match builtin_list(query.trim(), owner) {
            Some(_) => self.ensure_list(query.trim(), owner, source),
            None => self.find(query),
        }
    }

    pub fn delete_task(&self, id: i64) -> Result<()> {
        let cur = self.task(id)?;
        self.conn.execute("DELETE FROM tasks WHERE id = ?1", [id])?;
        self.touch(cur.project_id)?;
        Ok(())
    }

    fn touch(&self, project_id: i64) -> Result<()> {
        self.conn.execute("UPDATE projects SET updated_at = ?1 WHERE id = ?2", params![now(), project_id])?;
        Ok(())
    }

    // ---- export --------------------------------------------------------------------

    /// Everything as Markdown, grouped by category, for backups and for
    /// reading at a glance.
    pub fn export_markdown(&self, notes_per_project: usize, app: &str) -> Result<String> {
        let mut out = format!("# Projects\n\n_Exported from {app} on {}_\n", Utc::now().format("%Y-%m-%d %H:%M UTC"));
        let projects = self.projects()?;
        for cat in self.categories()? {
            let mut ps: Vec<&Project> = projects.iter().filter(|p| p.category == cat && p.status != "archived").collect();
            if ps.is_empty() {
                continue;
            }
            ps.sort_by_key(|p| STATUSES.iter().position(|s| *s == p.status).unwrap_or(99));
            out.push_str(&format!("\n## {}\n", capitalise(&cat)));
            for p in ps {
                out.push_str(&format!("\n### {} ({})\n\n", p.name, p.status));
                if !p.summary.is_empty() {
                    out.push_str(&format!("{}\n\n", p.summary));
                }
                if !p.next_step.is_empty() {
                    out.push_str(&format!("**Next step:** {}\n\n", p.next_step));
                }
                if !p.links.is_empty() {
                    out.push_str("**Links:**\n");
                    for l in p.links.lines().filter(|l| !l.trim().is_empty()) {
                        out.push_str(&format!("- {}\n", l.trim()));
                    }
                    out.push('\n');
                }
                let items = self.tasks(p.id)?;
                let (ideas, tasks): (Vec<_>, Vec<_>) = items.iter().partition(|t| t.kind == "idea" && p.slug != IDEAS_SLUG);
                if !tasks.is_empty() {
                    for t in &tasks {
                        out.push_str(&format!("- [{}] {}\n", if t.done { "x" } else { " " }, t.text));
                    }
                    out.push('\n');
                }
                let ideas: Vec<_> = ideas.iter().filter(|t| !t.done).collect();
                if !ideas.is_empty() {
                    out.push_str("Ideas:\n");
                    for t in ideas {
                        out.push_str(&format!("- {}\n", t.text));
                    }
                    out.push('\n');
                }
                let notes: Vec<Note> = self.notes(p.id, 0)?.into_iter().filter(|n| n.kind == "note").take(notes_per_project).collect();
                if !notes.is_empty() {
                    out.push_str("Recent notes:\n");
                    for n in notes {
                        out.push_str(&format!("- {}: {}\n", &n.created_at[..10], n.body.replace('\n', " ")));
                    }
                    out.push('\n');
                }
            }
        }
        Ok(out)
    }
}

/// `~/x` to an absolute path.
pub fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(p),
    }
}

pub fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new(name: &str, cat: &str) -> NewProject {
        NewProject { name: name.into(), category: Some(cat.into()), ..Default::default() }
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("Gaussian Splatting: viewer → trainer"), "gaussian-splatting-viewer-trainer");
        assert_eq!(slugify("  !! "), "project");
    }

    #[test]
    fn create_find_update() {
        let db = Db::memory().unwrap();
        let a = db.create(new("Home lab backups", "Work"), "test").unwrap();
        let b = db.create(new("Home lab network", "work"), "test").unwrap();
        db.create(new("Home lab network", "work"), "test").unwrap(); // slug gets -2
        assert_eq!(a.category, "work");
        assert_eq!(db.find("home-lab-backups").unwrap().id, a.id);
        assert_eq!(db.find("backups").unwrap().id, a.id);
        assert_eq!(db.find("home lab network").unwrap().id, b.id);
        assert!(matches!(db.find("home lab"), Err(Error::Invalid(_))));
        assert!(matches!(db.find("nothing"), Err(Error::NotFound(_))));

        let p = db.update(a.id, ProjectPatch { status: Some("paused".into()), next_step: Some("Evals".into()), ..Default::default() }, "test").unwrap();
        assert_eq!(p.status, "paused");
        let log = db.notes(a.id, 0).unwrap();
        assert_eq!(log[0].kind, "next");
        assert_eq!(log[1].body, "Status: active → paused");
        assert!(db.update(a.id, ProjectPatch { status: Some("bogus".into()), ..Default::default() }, "test").is_err());
        let p = db.update(a.id, ProjectPatch { status: Some("Abandoned".into()), ..Default::default() }, "test").unwrap();
        assert!(p.status == "abandoned" && is_finished(&p.status) && !is_finished("paused"));
    }

    #[test]
    fn find_by_folder() {
        let db = Db::memory().unwrap();
        let root = std::env::temp_dir().join(format!("catapp-test-{}", std::process::id()));
        let inner = root.join("repo/src");
        std::fs::create_dir_all(&inner).unwrap();
        let repo = root.join("repo");
        let a = db.create(NewProject { name: "Outer".into(), links: Some(format!("https://x.y\n{}", root.display())), ..Default::default() }, "t").unwrap();
        let b = db.create(NewProject { name: "Repo".into(), links: Some(format!("{} (the code)", repo.display())), ..Default::default() }, "t").unwrap();
        assert_eq!(db.find(&inner.display().to_string()).unwrap().id, b.id, "deepest match wins");
        assert_eq!(db.find(&root.display().to_string()).unwrap().id, a.id);
        assert!(matches!(db.find("/nowhere/at/all"), Err(Error::NotFound(_))));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn notes_tasks_search() {
        let db = Db::memory().unwrap();
        let p = db.create(new("Splats", "personal"), "test").unwrap();
        db.add_note(p.id, "Read the 3DGS paper", "cli").unwrap();
        assert!(db.add_note(p.id, "   ", "cli").is_err());
        let t = db.add_task(p.id, "Viewer").unwrap();
        let t = db.update_task(t.id, TaskPatch { done: Some(true), ..Default::default() }).unwrap();
        assert!(t.done && t.done_at.is_some());
        let p = db.project_by_id(p.id).unwrap();
        assert_eq!((p.note_count, p.tasks_done, p.tasks_total), (1, 1, 1));
        assert_eq!(db.recent(10, Some("personal")).unwrap().len(), 1);
        assert!(db.recent(10, Some("work")).unwrap().is_empty());
        let (_, notes, tasks) = db.search("3dgs").unwrap();
        assert_eq!((notes.len(), tasks.len()), (1, 0));
        assert!(db.export_markdown(3, "CatApp").unwrap().contains("- [x] Viewer"));
    }

    #[test]
    fn move_tasks_and_lists() {
        let db = Db::memory().unwrap();
        let todo = db.ensure_list(TODO_SLUG, "Sam", "test").unwrap();
        assert_eq!((todo.slug.as_str(), todo.name.as_str()), ("todo", "To do"));
        assert_eq!(db.ensure_list(TODO_SLUG, "Sam", "test").unwrap().id, todo.id, "made once");
        db.create(new("Product ideas", "work"), "test").unwrap();
        let ideas = db.find_or_list(IDEAS_SLUG, "Sam", "test").unwrap();
        assert_eq!((ideas.slug.as_str(), ideas.name.as_str()), ("ideas", "Ideas"));
        assert!(db.ensure_list("splats", "Sam", "test").is_err());
        assert_eq!(db.find_or_list("ideas", "Sam", "test").unwrap().id, ideas.id);
        assert_eq!(db.find_or_list("product", "Sam", "test").unwrap().slug, "product-ideas");
        let p = db.create(new("Splats", "personal"), "test").unwrap();
        db.add_task(p.id, "Viewer").unwrap();
        let t = db.add_task(todo.id, "Trainer").unwrap();
        let t = db.move_task(t.id, p.id).unwrap();
        assert_eq!((t.project_id, t.position), (p.id, 2), "goes to the end");
        assert!(db.tasks(todo.id).unwrap().is_empty());
        assert_eq!(db.move_task(t.id, p.id).unwrap().position, 2, "moving to the same place is a no-op");
        assert!(matches!(db.move_task(t.id, 9999), Err(Error::NotFound(_))));
        assert_eq!(db.move_task(t.id, ideas.id).unwrap().project_id, ideas.id);
    }

    #[test]
    fn private_projects_are_hidden_without_show_private() {
        // Two connections to one file, like the web app and an agent's MCP server.
        let path = std::env::temp_dir().join(format!("catapp-private-{}.db", std::process::id()));
        let web = Db::open(&path).unwrap().show_private();
        let agent = Db::open(&path).unwrap();
        let open = web.create(new("Learn Blender", "personal"), "test").unwrap();
        let secret = NewProject { name: "enc1:AAAA".into(), private: true, ..Default::default() };
        assert!(agent.create(secret.clone(), "test").is_err(), "only the web app makes private projects");
        let p = web.create(secret, "test").unwrap();
        assert!(p.private && p.slug == "private");
        let n = web.add_note(p.id, "enc1:BBBB", "web").unwrap();
        let t = web.add_task(p.id, "enc1:CCCC").unwrap();
        web.add_idea(p.id, "enc1:DDDD").unwrap();
        web.update(p.id, ProjectPatch { next_step: Some("enc1:EEEE".into()), ..Default::default() }, "web").unwrap();
        assert_eq!(web.notes(p.id, 1).unwrap()[0].body, "enc1:EEEE", "no plain words around the encrypted next step");

        assert_eq!(web.projects().unwrap().len(), 2);
        assert_eq!(agent.projects().unwrap().len(), 1);
        assert!(agent.find("private").is_err() && agent.find(&p.id.to_string()).is_err());
        assert!(agent.project_by_id(p.id).is_err() && agent.add_note(p.id, "hi", "claude").is_err());
        assert!(agent.note(n.id).is_err() && agent.edit_note(n.id, "x").is_err() && agent.delete_note(n.id).is_err());
        assert!(agent.task(t.id).is_err() && agent.delete_task(t.id).is_err());
        assert!(agent.update_task(t.id, TaskPatch { done: Some(true), ..Default::default() }).is_err());
        assert!(agent.vault().is_err() && agent.set_vault(&serde_json::json!({})).is_err());
        assert!(!agent.export_markdown(5, "CatApp").unwrap().contains("enc1:"));
        for db in [&web, &agent] {
            assert!(db.recent(50, None).unwrap().iter().all(|n| n.project_id != p.id), "never in recent notes");
            let (ps, ns, ts) = db.search("enc1").unwrap();
            assert!(ps.is_empty() && ns.is_empty() && ts.is_empty(), "never searched");
            assert!(db.project_ideas().unwrap().is_empty());
        }
        assert_eq!(web.open_items_everywhere("task").unwrap().len(), 1);
        assert!(agent.open_items_everywhere("task").unwrap().is_empty());
        assert!(web.move_task(t.id, open.id).is_err(), "private things don't move out");
        let o = web.add_task(open.id, "Model the mug").unwrap();
        assert!(web.move_task(o.id, p.id).is_err(), "or in");

        assert!(web.vault().unwrap().is_none());
        web.set_vault(&serde_json::json!({ "salt": "s", "check": "c" })).unwrap();
        assert_eq!(web.vault().unwrap().unwrap()["salt"], "s");
        assert!(web.set_vault(&serde_json::json!({ "salt": "t", "check": "d" })).is_err(), "set once");
        drop((web, agent));
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
    }

    #[test]
    fn upgrade_makes_the_ideas_list_ideas() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE projects (id INTEGER PRIMARY KEY, slug TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
                category TEXT NOT NULL DEFAULT 'personal', status TEXT NOT NULL DEFAULT 'active', summary TEXT NOT NULL DEFAULT '',
                next_step TEXT NOT NULL DEFAULT '', links TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
            CREATE TABLE notes (id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL, kind TEXT NOT NULL DEFAULT 'note',
                body TEXT NOT NULL, source TEXT NOT NULL DEFAULT 'web', created_at TEXT NOT NULL, edited_at TEXT);
            CREATE TABLE tasks (id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL, text TEXT NOT NULL,
                done INTEGER NOT NULL DEFAULT 0, position INTEGER NOT NULL, created_at TEXT NOT NULL, done_at TEXT);
            INSERT INTO projects VALUES (1, 'ideas', 'Ideas', 'personal', 'active', '', '', '', 'x', 'x');
            INSERT INTO projects VALUES (2, 'learn-blender', 'Learn Blender', 'personal', 'active', '', '', '', 'x', 'x');
            INSERT INTO tasks (project_id, text, position, created_at) VALUES (1, 'Learn to juggle', 1, 'x'), (2, 'Model the mug', 1, 'x');
            PRAGMA user_version = 1;",
        )
        .unwrap();
        let db = Db::init(conn).unwrap();
        assert_eq!(db.tasks(1).unwrap()[0].kind, "idea");
        assert_eq!(db.tasks(2).unwrap()[0].kind, "task");
    }

    #[test]
    fn project_ideas() {
        let db = Db::memory().unwrap();
        let ideas = db.ensure_list(IDEAS_SLUG, "Sam", "test").unwrap();
        let todo = db.ensure_list(TODO_SLUG, "Sam", "test").unwrap();
        let p = db.create(new("Learn Blender", "personal"), "test").unwrap();
        db.add_task(p.id, "Model the mug").unwrap();
        let i = db.add_idea(p.id, "Animate the mug").unwrap();
        assert_eq!(i.kind, "idea");
        assert_eq!(db.add_task(ideas.id, "Learn to juggle").unwrap().kind, "idea", "the ideas list holds ideas");
        let p = db.project_by_id(p.id).unwrap();
        assert_eq!((p.tasks_total, p.ideas_open), (1, 1), "ideas aren't counted as tasks");
        let d = db.detail(&p, 0).unwrap();
        assert_eq!((d.tasks.len(), d.ideas.len()), (1, 1));
        let elsewhere = db.project_ideas().unwrap();
        assert_eq!(elsewhere.len(), 1, "the general list isn't included");
        assert_eq!((elsewhere[0].project_slug.as_str(), elsewhere[0].item.text.as_str()), ("learn-blender", "Animate the mug"));
        let all = db.open_items_everywhere("idea").unwrap();
        assert_eq!((all.len(), all[0].project_slug.as_str()), (2, "ideas"), "the general list comes first");

        // Moving: the general ideas list makes it an idea, the to-do list a task; projects keep it as it is.
        assert_eq!(db.move_task(i.id, ideas.id).unwrap().kind, "idea");
        assert_eq!(db.move_task(i.id, p.id).unwrap().kind, "idea");
        assert_eq!(db.move_task(i.id, todo.id).unwrap().kind, "task");
        assert_eq!(db.move_task(i.id, p.id).unwrap().kind, "task");

        // Turning one into the other, except on the built-in lists.
        let t = db.update_task(i.id, TaskPatch { kind: Some("Idea".into()), ..Default::default() }).unwrap();
        assert_eq!(t.kind, "idea");
        assert!(db.update_task(i.id, TaskPatch { kind: Some("thing".into()), ..Default::default() }).is_err());
        let j = db.add_task(ideas.id, "Write a novel").unwrap();
        assert!(db.update_task(j.id, TaskPatch { kind: Some("task".into()), ..Default::default() }).is_err());
        let md = db.export_markdown(3, "CatApp").unwrap();
        assert!(md.contains("Ideas:\n- Animate the mug") && md.contains("- [ ] Write a novel"));
    }
}
