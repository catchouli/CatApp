//! Storage: one SQLite file shared by the web app, the CLI and the MCP
//! server. WAL mode plus a busy timeout let them all use it at once.

use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Project statuses, in display order.
pub const STATUSES: &[&str] = &["active", "planned", "idea", "paused", "done", "abandoned", "archived"];

/// Statuses meaning no more work is planned: hidden from default lists.
pub const FINISHED: &[&str] = &["done", "abandoned", "archived"];

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
    pub notes: Vec<Note>,
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
}

const PROJECT_SELECT: &str = "SELECT p.id, p.slug, p.name, p.category, p.status, p.summary, p.next_step, p.links,
    p.created_at, p.updated_at,
    MAX(p.updated_at, COALESCE((SELECT MAX(n.created_at) FROM notes n WHERE n.project_id = p.id), '')),
    (SELECT COUNT(*) FROM notes n WHERE n.project_id = p.id AND n.kind = 'note'),
    (SELECT COUNT(*) FROM tasks t WHERE t.project_id = p.id AND t.done = 1),
    (SELECT COUNT(*) FROM tasks t WHERE t.project_id = p.id)
    FROM projects p";

const NOTE_SELECT: &str = "SELECT n.id, n.project_id, p.slug, p.name, p.category, n.kind, n.body, n.source,
    n.created_at, n.edited_at FROM notes n JOIN projects p ON p.id = n.project_id";

const TASK_SELECT: &str = "SELECT id, project_id, text, done, position, created_at, done_at FROM tasks";

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
        text: r.get(2)?,
        done: r.get::<_, i64>(3)? != 0,
        position: r.get(4)?,
        created_at: r.get(5)?,
        done_at: r.get(6)?,
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
        Ok(Db { conn })
    }

    // ---- projects ------------------------------------------------------------------

    /// Every project, most recently active first.
    pub fn projects(&self) -> Result<Vec<Project>> {
        let mut st = self.conn.prepare(&format!("{PROJECT_SELECT} ORDER BY 11 DESC"))?;
        let rows = st.query_map([], project_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn project_by_id(&self, id: i64) -> Result<Project> {
        self.conn
            .query_row(&format!("{PROJECT_SELECT} WHERE p.id = ?1"), [id], project_row)
            .optional()?
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
            tasks: self.tasks(project.id)?,
            notes: self.notes(project.id, notes_limit)?,
        })
    }

    pub fn create(&self, p: NewProject, source: &str) -> Result<Project> {
        let name = non_empty(&p.name, "name")?;
        let category = norm_category(p.category.as_deref().unwrap_or(""));
        let status = check_status(p.status.as_deref().unwrap_or("active"))?;
        let base = slugify(&name);
        let mut slug = base.clone();
        let mut n = 2;
        while self.conn.query_row("SELECT 1 FROM projects WHERE slug = ?1", [&slug], |_| Ok(())).optional()?.is_some() {
            slug = format!("{base}-{n}");
            n += 1;
        }
        let t = now();
        self.conn.execute(
            "INSERT INTO projects (slug, name, category, status, summary, next_step, links, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            params![
                slug,
                name,
                category,
                status,
                p.summary.unwrap_or_default().trim(),
                p.next_step.unwrap_or_default().trim(),
                p.links.unwrap_or_default().trim(),
                t
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
            self.add_entry(id, "next", &format!("Next step: {next_step}"), source)?;
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
            .query_row(&format!("{NOTE_SELECT} WHERE n.id = ?1"), [id], note_row)
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
        let n = self.conn.execute("UPDATE notes SET body = ?1, edited_at = ?2 WHERE id = ?3", params![body, now(), id])?;
        if n == 0 {
            return Err(Error::NotFound(format!("no note with id {id}")));
        }
        self.note(id)
    }

    pub fn delete_note(&self, id: i64) -> Result<()> {
        if self.conn.execute("DELETE FROM notes WHERE id = ?1", [id])? == 0 {
            return Err(Error::NotFound(format!("no note with id {id}")));
        }
        Ok(())
    }

    /// Recent notes across projects (written notes only, not status changes).
    pub fn recent(&self, limit: usize, category: Option<&str>) -> Result<Vec<Note>> {
        let mut st = self.conn.prepare(&format!(
            "{NOTE_SELECT} WHERE n.kind = 'note' AND (?1 IS NULL OR p.category = ?1)
             ORDER BY n.created_at DESC, n.id DESC LIMIT ?2"
        ))?;
        let rows = st.query_map(params![category, limit as i64], note_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Notes, projects and tasks containing the text (case-insensitive).
    pub fn search(&self, q: &str) -> Result<(Vec<Project>, Vec<Note>, Vec<Task>)> {
        let q = non_empty(q, "a search")?.to_lowercase();
        let projects = self
            .projects()?
            .into_iter()
            .filter(|p| [&p.name, &p.summary, &p.next_step, &p.links].iter().any(|s| s.to_lowercase().contains(&q)))
            .collect();
        let pat = format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let mut st = self.conn.prepare(&format!(
            "{NOTE_SELECT} WHERE lower(n.body) LIKE ?1 ESCAPE '\\' ORDER BY n.created_at DESC LIMIT 50"
        ))?;
        let notes = st.query_map([&pat], note_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut st = self.conn.prepare(&format!("{TASK_SELECT} WHERE lower(text) LIKE ?1 ESCAPE '\\' LIMIT 50"))?;
        let tasks = st.query_map([&pat], task_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((projects, notes, tasks))
    }

    // ---- tasks ---------------------------------------------------------------------

    pub fn tasks(&self, project_id: i64) -> Result<Vec<Task>> {
        let mut st = self.conn.prepare(&format!("{TASK_SELECT} WHERE project_id = ?1 ORDER BY position, id"))?;
        let rows = st.query_map([project_id], task_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn task(&self, id: i64) -> Result<Task> {
        self.conn
            .query_row(&format!("{TASK_SELECT} WHERE id = ?1"), [id], task_row)
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("no task with id {id}")))
    }

    pub fn add_task(&self, project_id: i64, text: &str) -> Result<Task> {
        let text = non_empty(text, "a task")?;
        self.project_by_id(project_id)?;
        let pos: i64 =
            self.conn.query_row("SELECT COALESCE(MAX(position), 0) + 1 FROM tasks WHERE project_id = ?1", [project_id], |r| r.get(0))?;
        self.conn.execute(
            "INSERT INTO tasks (project_id, text, position, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![project_id, text, pos, now()],
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
        self.conn.execute(
            "UPDATE tasks SET text = ?1, done = ?2, done_at = ?3 WHERE id = ?4",
            params![text, done as i64, done_at, id],
        )?;
        self.touch(cur.project_id)?;
        self.task(id)
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
                let tasks = self.tasks(p.id)?;
                if !tasks.is_empty() {
                    for t in &tasks {
                        out.push_str(&format!("- [{}] {}\n", if t.done { "x" } else { " " }, t.text));
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
        let a = db.create(new("VRAssist local-first", "Work"), "test").unwrap();
        let b = db.create(new("VRAssist fine-tune", "work"), "test").unwrap();
        db.create(new("VRAssist fine-tune", "work"), "test").unwrap(); // slug gets -2
        assert_eq!(a.category, "work");
        assert_eq!(db.find("vrassist-local-first").unwrap().id, a.id);
        assert_eq!(db.find("local-first").unwrap().id, a.id);
        assert_eq!(db.find("vrassist fine-tune").unwrap().id, b.id);
        assert!(matches!(db.find("vrassist"), Err(Error::Invalid(_))));
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
}
