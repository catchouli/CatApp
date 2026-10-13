//! The web app: a JSON API plus a small single-page front end, embedded in
//! the binary. Requests from this Mac are trusted; anything else (your
//! phone) needs the access token once, which is then kept in a cookie.

use crate::brand::Brand;
use crate::db::{self, Db, NewProject, ProjectPatch, TaskPatch};
use crate::voice::Voice;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct AppState {
    pub db: Mutex<Db>,
    pub token: String,
    /// The data folder: the assistant runs there, its MCP server uses it, and config.json lives there.
    pub home: PathBuf,
    /// One assistant request at a time.
    pub assistant: tokio::sync::Mutex<()>,
    /// Voice mode's running Claude session and voice helper.
    pub voice: Arc<Voice>,
}

type St = State<Arc<AppState>>;

const COOKIE: &str = "catapp_token";

pub struct ApiError(db::Error);

impl From<db::Error> for ApiError {
    fn from(e: db::Error) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let code = match &self.0 {
            db::Error::NotFound(_) => StatusCode::NOT_FOUND,
            db::Error::Invalid(_) => StatusCode::BAD_REQUEST,
            db::Error::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (code, Json(json!({ "error": self.0.to_string() }))).into_response()
    }
}

type R = Result<Json<Value>, ApiError>;

fn ok<T: serde::Serialize>(v: T) -> R {
    Ok(Json(serde_json::to_value(v).expect("serialisable")))
}

pub fn router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/api/meta", get(meta))
        .route("/api/projects", get(list_projects).post(create_project))
        .route("/api/projects/{slug}", get(get_project).patch(update_project).delete(delete_project))
        .route("/api/projects/{slug}/notes", post(add_note))
        .route("/api/projects/{slug}/tasks", post(add_task))
        .route("/api/notes/{id}", patch(edit_note).delete(delete_note))
        .route("/api/tasks/{id}", patch(update_task).delete(delete_task))
        .route("/api/recent", get(recent))
        .route("/api/search", get(search))
        .route("/api/assistant", post(assistant))
        .route("/api/voice/warm", post(voice_warm))
        .route("/api/voice/reset", post(voice_reset))
        .route("/api/voice/chat", post(voice_chat))
        .route("/api/voice/tts", post(voice_tts))
        .route("/api/voice/stt", post(voice_stt).layer(DefaultBodyLimit::max(20 << 20)))
        .route("/avatar/{*path}", get(avatar_file))
        .route("/", get(index));
    Router::new()
        .merge(api)
        .route_layer(middleware::from_fn_with_state(state.clone(), auth))
        .route("/app.js", get(|| asset("text/javascript; charset=utf-8", include_str!("../static/app.js"))))
        .route("/style.css", get(|| asset("text/css; charset=utf-8", include_str!("../static/style.css"))))
        .route("/icon.svg", get(icon))
        .route("/manifest.webmanifest", get(manifest))
        .with_state(state)
}

async fn asset(ctype: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, ctype), (header::CACHE_CONTROL, "no-cache")], body).into_response()
}

/// The page, with the app's name filled in.
async fn index(State(st): St) -> Html<String> {
    let b = Brand::load(&st.home);
    Html(include_str!("../static/index.html").replace("__APP__", &html_escape(&b.name)))
}

async fn icon(State(st): St) -> Response {
    let svg = match Brand::load(&st.home).icon_kind() {
        "cat" => include_str!("../static/icon.svg"),
        _ => include_str!("../static/icon-plain.svg"),
    };
    ([(header::CONTENT_TYPE, "image/svg+xml"), (header::CACHE_CONTROL, "no-cache")], svg).into_response()
}

async fn manifest(State(st): St) -> Response {
    let b = Brand::load(&st.home);
    let m = json!({
        "name": b.name, "short_name": b.name,
        "description": format!("{}'s projects, tasks and notes", b.owner),
        "start_url": "/", "display": "standalone",
        "background_color": "#f5f3ee", "theme_color": "#2f5d50",
        "icons": [{ "src": "/icon.svg", "sizes": "any", "type": "image/svg+xml", "purpose": "any" }]
    });
    ([(header::CONTENT_TYPE, "application/manifest+json"), (header::CACHE_CONTROL, "no-cache")], m.to_string()).into_response()
}

// ---- auth --------------------------------------------------------------------------

fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn cookie_token(h: &HeaderMap) -> Option<String> {
    h.get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
}

fn query_token(uri: &Uri) -> Option<String> {
    uri.query()?.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == "token").map(|(_, v)| v.to_string())
}

async fn auth(State(st): St, ConnectInfo(addr): ConnectInfo<SocketAddr>, req: Request, next: Next) -> Response {
    if addr.ip().is_loopback() {
        return next.run(req).await;
    }
    // A link with ?token=... logs this browser in and drops the token from the address bar.
    if let Some(t) = query_token(req.uri()) {
        if ct_eq(&t, &st.token) {
            let mut resp = Redirect::to(req.uri().path()).into_response();
            let cookie = format!("{COOKIE}={}; Path=/; Max-Age=31536000; HttpOnly; SameSite=Lax", st.token);
            resp.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
            return resp;
        }
    }
    let bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(String::from);
    if cookie_token(req.headers()).or(bearer).is_some_and(|t| ct_eq(&t, &st.token)) {
        return next.run(req).await;
    }
    if req.uri().path().starts_with("/api/") {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "not logged in" }))).into_response();
    }
    (StatusCode::UNAUTHORIZED, Html(login_page(&Brand::load(&st.home)))).into_response()
}

fn login_page(b: &Brand) -> String {
    let name = html_escape(&b.name);
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>{name}</title>
<link rel="stylesheet" href="/style.css"><link rel="icon" href="/icon.svg"></head>
<body><main class="login"><img src="/icon.svg" alt="" width="56" height="56"><h1>{name}</h1>
<p>Enter the access token. On the computer running {name}, run <code>catapp url</code> to see it, or scan its QR code.</p>
<p class="hint">Each address remembers its own login for a year. This is <b id="here"></b>: if you logged in before at a different address, use that, or log in here once.</p>
<script>document.getElementById("here").textContent = location.host;</script>
<form onsubmit="location.href='/?token='+encodeURIComponent(this.t.value.trim());return false">
<input name="t" autocomplete="off" autocapitalize="off" placeholder="Access token" required>
<button class="btn primary">Open</button></form></main></body></html>"#
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

// ---- handlers ----------------------------------------------------------------------

async fn meta(State(st): St) -> R {
    let db = st.db.lock().unwrap();
    let b = Brand::load(&st.home);
    let voice = b.voice.then(|| {
        json!({
            "persona": b.persona_name(),
            "avatar": if b.avatar { avatar_model(&st.home) } else { None },
        })
    });
    ok(json!({ "statuses": db::STATUSES, "categories": db.categories()?, "app": b.name, "owner": b.owner, "voice": voice }))
}

async fn list_projects(State(st): St) -> R {
    ok(st.db.lock().unwrap().projects()?)
}

#[derive(Deserialize)]
struct NotesQ {
    notes: Option<usize>,
}

async fn get_project(State(st): St, Path(slug): Path<String>, Query(q): Query<NotesQ>) -> R {
    let db = st.db.lock().unwrap();
    let p = db.find(&slug)?;
    ok(db.detail(&p, q.notes.unwrap_or(0))?)
}

async fn create_project(State(st): St, Json(body): Json<NewProject>) -> R {
    ok(st.db.lock().unwrap().create(body, "web")?)
}

async fn update_project(State(st): St, Path(slug): Path<String>, Json(body): Json<ProjectPatch>) -> R {
    let db = st.db.lock().unwrap();
    let p = db.find(&slug)?;
    ok(db.update(p.id, body, "web")?)
}

async fn delete_project(State(st): St, Path(slug): Path<String>) -> R {
    let db = st.db.lock().unwrap();
    let p = db.find(&slug)?;
    db.delete_project(p.id)?;
    ok(json!({ "ok": true }))
}

#[derive(Deserialize)]
struct Body {
    body: String,
}

async fn add_note(State(st): St, Path(slug): Path<String>, Json(b): Json<Body>) -> R {
    let db = st.db.lock().unwrap();
    let p = db.find(&slug)?;
    ok(db.add_note(p.id, &b.body, "web")?)
}

async fn edit_note(State(st): St, Path(id): Path<i64>, Json(b): Json<Body>) -> R {
    ok(st.db.lock().unwrap().edit_note(id, &b.body)?)
}

async fn delete_note(State(st): St, Path(id): Path<i64>) -> R {
    st.db.lock().unwrap().delete_note(id)?;
    ok(json!({ "ok": true }))
}

#[derive(Deserialize)]
struct TaskBody {
    text: String,
}

async fn add_task(State(st): St, Path(slug): Path<String>, Json(b): Json<TaskBody>) -> R {
    let db = st.db.lock().unwrap();
    let p = db.find(&slug)?;
    ok(db.add_task(p.id, &b.text)?)
}

async fn update_task(State(st): St, Path(id): Path<i64>, Json(b): Json<TaskPatch>) -> R {
    ok(st.db.lock().unwrap().update_task(id, b)?)
}

async fn delete_task(State(st): St, Path(id): Path<i64>) -> R {
    st.db.lock().unwrap().delete_task(id)?;
    ok(json!({ "ok": true }))
}

#[derive(Deserialize)]
struct RecentQ {
    limit: Option<usize>,
    category: Option<String>,
}

async fn recent(State(st): St, Query(q): Query<RecentQ>) -> R {
    ok(st.db.lock().unwrap().recent(q.limit.unwrap_or(20).min(200), q.category.as_deref())?)
}

#[derive(Deserialize)]
struct SearchQ {
    q: String,
}

async fn search(State(st): St, Query(q): Query<SearchQ>) -> R {
    let (projects, notes, tasks) = st.db.lock().unwrap().search(&q.q)?;
    ok(json!({ "projects": projects, "notes": notes, "tasks": tasks }))
}

// ---- assistant -----------------------------------------------------------------------

#[derive(Deserialize)]
struct AskBody {
    message: String,
    session: Option<String>,
}

/// Claude Code, wherever it's installed. The service runs with a minimal PATH, so look in
/// the usual places too.
fn find_claude() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CATAPP_CLAUDE") {
        return Some(PathBuf::from(p));
    }
    let home = dirs::home_dir().unwrap_or_default();
    let mut candidates = vec![home.join(".local/bin/claude"), home.join(".claude/local/claude")];
    candidates.extend(["/opt/homebrew/bin/claude", "/usr/local/bin/claude"].map(PathBuf::from));
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|d| d.join("claude")));
    }
    candidates.into_iter().find(|p| p.is_file())
}

pub(crate) fn assistant_prompt(b: &Brand) -> String {
    let (app, owner, slug) = (&b.name, &b.owner, b.slug());
    format!(
        "You are the assistant built into {app}, {owner}'s personal organiser. {owner} types quick requests, often \
on their phone: \"add a project for X\", \"add a to-do to Y\", \"note on <project> that Z\", \"what's next on \
<project>?\". Use the {slug} tools to do it, then reply in one or two short plain sentences saying exactly what you \
did (name the project). The to-do list is the project with slug 'todo': add to-dos there with add_task. \
\"the to-do list\", \"my to-dos\", \"the main list\" and any to-do that doesn't name a project all mean that list: add it \
there without searching or asking which project. New projects \
default to category 'personal' and status 'active' unless {owner} says otherwise. Pass source 'assistant' when adding \
notes or updating projects. Act rather than ask: only ask a question when the request is genuinely ambiguous. Never \
delete anything or mark things abandoned or archived unless {owner} explicitly asks. Today is {}.",
        chrono::Local::now().format("%A %-d %B %Y")
    )
}

/// Claude's reply to a failed request, made friendlier when the cause is a missing login.
pub(crate) fn explain_failure(text: &str) -> String {
    let t = text.to_lowercase();
    if ["log in", "login", "logged in", "not authenticated", "authentication", "api key", "oauth", "/login"].iter().any(|k| t.contains(k)) {
        "The assistant needs Claude Code to be logged in on the computer running this app. Open a terminal there, run \
`claude`, log in, then try again."
            .into()
    } else {
        format!("Claude Code reported a problem: {}", text.trim())
    }
}

fn not_installed() -> db::Error {
    db::Error::Invalid(
        "The assistant needs Claude Code, which isn't installed on the computer running this app (or isn't where the \
app looks: set CATAPP_CLAUDE to its path). Everything else works without it."
            .into(),
    )
}

/// Claude Code locked to this app: no built-in tools, only this app's MCP server, everything
/// else auto-denied. Callers add the output format, session and message.
pub(crate) fn claude_command(home: &std::path::Path, brand: &Brand, prompt: &str) -> Result<tokio::process::Command, db::Error> {
    let claude = find_claude().filter(|p| p.is_file()).ok_or_else(not_installed)?;
    let exe = std::env::current_exe().map_err(|e| db::Error::Invalid(e.to_string()))?;
    let slug = brand.slug();
    let mcp = json!({ "mcpServers": { &slug: {
        "command": exe, "args": ["mcp"], "env": { "CATAPP_HOME": home }
    }}})
    .to_string();
    let allowed = format!("mcp__{slug}");
    let mut cmd = tokio::process::Command::new(claude);
    cmd.args(["-p", "--tools", "", "--strict-mcp-config", "--mcp-config", &mcp])
        .args(["--allowedTools", &allowed, "--permission-mode", "dontAsk"])
        .args(["--append-system-prompt", prompt]);
    if let Ok(model) = std::env::var("CATAPP_ASSISTANT_MODEL") {
        cmd.args(["--model", &model]);
    }
    cmd.current_dir(home).kill_on_drop(true);
    Ok(cmd)
}

/// A session id from the browser, checked before it goes on a command line.
pub(crate) fn session_id(s: Option<&str>) -> Result<Option<String>, db::Error> {
    match s.filter(|s| !s.is_empty()) {
        Some(s) if s.chars().all(|c| c.is_ascii_hexdigit() || c == '-') => Ok(Some(s.to_string())),
        Some(_) => Err(db::Error::Invalid("bad session id".into())),
        None => Ok(None),
    }
}

async fn assistant(State(st): St, Json(b): Json<AskBody>) -> R {
    let message = b.message.trim();
    if message.is_empty() {
        return Err(db::Error::Invalid("say something to the assistant".into()).into());
    }
    let brand = Brand::load(&st.home);
    let mut cmd = claude_command(&st.home, &brand, &assistant_prompt(&brand))?;
    cmd.args(["--output-format", "json"]);
    if let Some(s) = session_id(b.session.as_deref())? {
        cmd.args(["--resume", &s]);
    }
    cmd.arg("--").arg(message);
    cmd.stdin(std::process::Stdio::null());
    let _one_at_a_time = st.assistant.lock().await;
    let out = tokio::time::timeout(Duration::from_secs(180), cmd.output())
        .await
        .map_err(|_| db::Error::Invalid("the assistant took more than 3 minutes and was stopped".into()))?
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => not_installed(),
            _ => db::Error::Invalid(format!("couldn't start Claude Code: {e}")),
        })?;
    let parsed: Option<Value> = serde_json::from_slice(&out.stdout).ok();
    let Some(d) = parsed.filter(|d| d.get("result").is_some()) else {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: String = err.trim().chars().rev().take(400).collect::<Vec<_>>().into_iter().rev().collect();
        return Err(db::Error::Invalid(explain_failure(if tail.is_empty() { "no output" } else { &tail })).into());
    };
    if d["is_error"].as_bool().unwrap_or(false) {
        return Err(db::Error::Invalid(explain_failure(d["result"].as_str().unwrap_or("unknown error"))).into());
    }
    ok(json!({
        "reply": d["result"],
        "session": d["session_id"],
        "is_error": d["is_error"].as_bool().unwrap_or(false),
    }))
}

// ---- voice mode ----------------------------------------------------------------------

fn voice_on(st: &AppState) -> Result<Brand, ApiError> {
    let b = Brand::load(&st.home);
    if b.voice {
        Ok(b)
    } else {
        Err(db::Error::Invalid("voice mode is off: turn it on with `catapp config voice on`".into()).into())
    }
}

#[derive(Deserialize)]
struct WarmBody {
    session: Option<String>,
}

async fn voice_warm(State(st): St, Json(w): Json<WarmBody>) -> R {
    let b = voice_on(&st)?;
    st.voice.warm(&st.home, &b, session_id(w.session.as_deref())?).await?;
    ok(json!({ "ok": true }))
}

async fn voice_reset(State(st): St) -> R {
    st.voice.reset().await;
    ok(json!({ "ok": true }))
}

/// The reply as it's written: one JSON event per line (see `Voice::chat`).
async fn voice_chat(State(st): St, Json(a): Json<AskBody>) -> Result<Response, ApiError> {
    let b = voice_on(&st)?;
    let message = a.message.trim().to_string();
    if message.is_empty() {
        return Err(db::Error::Invalid("say something to the assistant".into()).into());
    }
    let rx = st.voice.chat(&st.home, &b, session_id(a.session.as_deref())?, message).await?;
    use tokio_stream::StreamExt;
    let lines = tokio_stream::wrappers::ReceiverStream::new(rx).map(|v| Ok::<_, std::convert::Infallible>(format!("{v}\n")));
    Ok(([(header::CONTENT_TYPE, "application/x-ndjson"), (header::CACHE_CONTROL, "no-cache")], axum::body::Body::from_stream(lines)).into_response())
}

#[derive(Deserialize)]
struct TtsBody {
    text: String,
}

/// One sentence in the configured voice, as base64 WAV.
async fn voice_tts(State(st): St, Json(t): Json<TtsBody>) -> R {
    let b = voice_on(&st)?;
    ok(st.voice.speech(&st.home, json!({ "op": "tts", "text": t.text, "voice": b.speaker })).await?)
}

/// A recorded clip (any format ffmpeg reads) to text.
async fn voice_stt(State(st): St, body: Bytes) -> R {
    voice_on(&st)?;
    let dir = st.home.join("voice").join("tmp");
    std::fs::create_dir_all(&dir).map_err(|e| db::Error::Invalid(e.to_string()))?;
    let clip = dir.join(format!("clip-{}.audio", rand::random::<u64>()));
    std::fs::write(&clip, &body).map_err(|e| db::Error::Invalid(e.to_string()))?;
    let r = st.voice.speech(&st.home, json!({ "op": "stt", "path": clip })).await;
    let _ = std::fs::remove_file(&clip);
    ok(r?)
}

/// The Live2D model in the data folder's `avatar/` (the first `*.model3.json`), as a URL path.
fn avatar_model(home: &std::path::Path) -> Option<String> {
    fn find(dir: &std::path::Path, depth: usize) -> Option<PathBuf> {
        let mut entries: Vec<_> = std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        entries.sort();
        if let Some(f) = entries.iter().find(|p| p.is_file() && p.to_string_lossy().ends_with(".model3.json")) {
            return Some(f.clone());
        }
        entries.iter().filter(|p| p.is_dir() && depth > 0).find_map(|d| find(d, depth - 1))
    }
    let root = home.join("avatar");
    let model = find(&root, 3)?;
    let rel = model.strip_prefix(&root).ok()?;
    Some(format!("/avatar/{}", rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")))
}

/// Files under the data folder's `avatar/`: the Live2D model and the Cubism runtime.
async fn avatar_file(State(st): St, Path(path): Path<String>) -> Result<Response, ApiError> {
    let not_found = || ApiError(db::Error::NotFound("avatar file".into()));
    if path.split('/').any(|c| c.is_empty() || c == "." || c == ".." || c.contains('\\')) {
        return Err(not_found());
    }
    let bytes = std::fs::read(st.home.join("avatar").join(&path)).map_err(|_| not_found())?;
    let ctype = match path.rsplit('.').next().unwrap_or("") {
        "json" => "application/json",
        "js" => "text/javascript; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    };
    Ok(([(header::CONTENT_TYPE, ctype), (header::CACHE_CONTROL, "max-age=3600")], bytes).into_response())
}

pub async fn serve(state: Arc<AppState>, bind: &str, port: u16) -> std::io::Result<()> {
    state.voice.clone().reap_idle();
    let listener = tokio::net::TcpListener::bind((bind, port)).await?;
    axum::serve(listener, router(state).into_make_service_with_connect_info::<SocketAddr>()).await
}
