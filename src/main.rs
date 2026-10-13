//! CatApp: Cat's personal organiser. One SQLite file, three ways in: a
//! phone-friendly web app (`catapp serve`), this CLI, and an MCP server for
//! Claude (`catapp mcp`).

mod brand;
mod db;
mod mcp;
mod voice;
mod web;

use clap::{Parser, Subcommand};
use db::{Db, NewProject, Note, Project, ProjectPatch, TaskPatch, STATUSES};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

const DEFAULT_PORT: u16 = 8787;
/// The project that holds the general to-do list.
const TODO_SLUG: &str = "todo";

#[derive(Parser)]
#[command(name = "catapp", version, about = "A personal organiser: projects, tasks and notes (CatApp, or whatever you name it)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the web app (open it on your phone with the address from `catapp url`)
    Serve {
        #[arg(long, default_value_t = DEFAULT_PORT)]
        port: u16,
        /// 0.0.0.0 lets other devices on your network connect; 127.0.0.1 keeps it to this computer
        #[arg(long, default_value = "0.0.0.0")]
        bind: String,
    },
    /// Print the address (with access token and QR code) to open on your phone
    Url {
        #[arg(long, default_value_t = DEFAULT_PORT)]
        port: u16,
    },
    /// List projects
    #[command(alias = "ls")]
    List {
        /// Only this category (e.g. work, personal)
        #[arg(short, long)]
        category: Option<String>,
        /// Include finished projects (done, abandoned, archived)
        #[arg(short, long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show one project: details, tasks and recent notes
    Show {
        project: String,
        /// How many log entries to show (0 for all)
        #[arg(short, long, default_value_t = 10)]
        notes: usize,
        #[arg(long)]
        json: bool,
    },
    /// Create a project
    New {
        name: String,
        #[arg(short, long, default_value = "personal")]
        category: String,
        #[arg(short, long, default_value = "active")]
        status: String,
        #[arg(long)]
        summary: Option<String>,
        #[arg(short, long)]
        next: Option<String>,
        /// Links or paths, one per line
        #[arg(long)]
        links: Option<String>,
    },
    /// Add a note to a project (text from the arguments, or stdin if none).
    /// The project can be `.`: the project whose links contain the current folder
    Note {
        project: String,
        text: Vec<String>,
        /// Who's writing: agents should give their name (claude, opencode, ...)
        #[arg(long, default_value = "cli")]
        source: String,
    },
    /// Change a project's status, next step or details
    Set {
        project: String,
        #[arg(short, long)]
        status: Option<String>,
        #[arg(short, long)]
        next: Option<String>,
        #[arg(long)]
        summary: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(short, long)]
        category: Option<String>,
        #[arg(long)]
        links: Option<String>,
    },
    /// Work with a project's checklist
    Task {
        #[command(subcommand)]
        cmd: TaskCmd,
    },
    /// Your to-do list: `catapp todo` shows it, `catapp todo some thing` adds to it.
    /// Tick items off with `catapp task done <id>`
    Todo {
        text: Vec<String>,
        /// Include done items
        #[arg(short, long)]
        all: bool,
    },
    /// Latest notes across all projects
    Recent {
        #[arg(short, long, default_value_t = 15)]
        limit: usize,
        #[arg(short, long)]
        category: Option<String>,
    },
    /// Find projects, notes and tasks containing some text
    Search { query: Vec<String> },
    /// Print everything as Markdown
    Export {
        /// Notes to include per project
        #[arg(long, default_value_t = 5)]
        notes: usize,
    },
    /// Delete a project and all its notes and tasks (consider `set --status archived` instead)
    Delete {
        project: String,
        /// Required: confirms you mean it
        #[arg(long)]
        yes: bool,
    },
    /// Run the MCP server on stdio (for Claude)
    Mcp,
    /// Show where the data and token live
    Info,
    /// Show or change the app's name, owner and icon, e.g. `catapp config name MaisyApp`
    Config { key: Option<String>, value: Vec<String> },
    /// Keep the web app running in the background (launchd on macOS, systemd on Linux)
    Service {
        #[command(subcommand)]
        cmd: ServiceCmd,
    },
}

#[derive(Subcommand)]
enum TaskCmd {
    /// Add a task
    Add { project: String, text: Vec<String> },
    /// Mark a task done
    Done { id: i64 },
    /// Mark a task not done
    Undo { id: i64 },
    /// Delete a task
    Rm { id: i64 },
}

#[derive(Subcommand)]
enum ServiceCmd {
    /// Start the app at login and keep it running
    Install {
        #[arg(long, default_value_t = DEFAULT_PORT)]
        port: u16,
    },
    /// Restart it, e.g. after `cargo install` has replaced the binary
    Restart,
    /// Stop it and remove it from login
    Uninstall,
}

fn home() -> PathBuf {
    std::env::var_os("CATAPP_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("catapp"))
}

fn open_db() -> Result<Db, String> {
    let dir = home();
    std::fs::create_dir_all(&dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    Db::open(&dir.join("catapp.db")).map_err(|e| e.to_string())
}

/// The web app's access token, made on first use.
fn token() -> Result<String, String> {
    let path = home().join("token");
    if let Ok(t) = std::fs::read_to_string(&path) {
        if !t.trim().is_empty() {
            return Ok(t.trim().to_string());
        }
    }
    use rand::Rng;
    let t: String = rand::thread_rng().sample_iter(rand::distributions::Alphanumeric).take(32).map(char::from).collect();
    std::fs::create_dir_all(home()).map_err(|e| e.to_string())?;
    std::fs::write(&path, &t).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(t)
}

/// This computer's address on the local network (no packets are sent).
fn lan_ip() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.168.0.1:80").or_else(|_| s.connect("8.8.8.8:80")).ok()?;
    Some(s.local_addr().ok()?.ip().to_string())
}

/// The Bonjour/Avahi name (`name.local`), which stays the same when the IP changes.
fn local_hostname() -> Option<String> {
    let out = if cfg!(target_os = "macos") {
        std::process::Command::new("scutil").args(["--get", "LocalHostName"]).output().ok()?
    } else {
        std::process::Command::new("hostname").output().ok()?
    };
    let name = String::from_utf8(out.stdout).ok()?.trim().split('.').next()?.to_string();
    (!name.is_empty() && name != "localhost").then(|| format!("{name}.local"))
}

fn ago(ts: &str) -> String {
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(ts) else { return ts.into() };
    let secs = (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds().max(0);
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        86400..=604799 => format!("{}d ago", secs / 86400),
        _ => t.with_timezone(&chrono::Local).format("%-d %b %Y").to_string(),
    }
}

fn local_time(ts: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|t| t.with_timezone(&chrono::Local).format("%a %-d %b %Y %H:%M").to_string())
        .unwrap_or_else(|_| ts.into())
}

fn print_project_line(p: &Project) {
    let tasks = if p.tasks_total > 0 { format!(" · {}/{} tasks", p.tasks_done, p.tasks_total) } else { String::new() };
    println!("  {:<9} {}  ({}{}, {})", p.status, p.name, p.slug, tasks, ago(&p.last_activity));
    if !p.next_step.is_empty() && !db::is_finished(&p.status) {
        println!("            next: {}", p.next_step);
    }
}

fn print_note(n: &Note, with_project: bool) {
    let src = if n.source == "web" { String::new() } else { format!(" [{}]", n.source) };
    let proj = if with_project { format!(" {}:", n.project_name) } else { String::new() };
    let mut lines = n.body.lines();
    println!("  {} (#{}){}{} {}", local_time(&n.created_at), n.id, src, proj, lines.next().unwrap_or(""));
    for l in lines {
        println!("      {l}");
    }
}

fn text_or_stdin(words: Vec<String>) -> Result<String, String> {
    if !words.is_empty() {
        return Ok(words.join(" "));
    }
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s).map_err(|e| e.to_string())?;
    Ok(s.trim().to_string())
}

fn json_out<T: serde::Serialize>(v: &T) {
    println!("{}", serde_json::to_string_pretty(v).unwrap());
}

fn run(cli: Cli) -> Result<(), String> {
    let e = |e: db::Error| e.to_string();
    match cli.cmd {
        Cmd::Serve { port, bind } => {
            let state = Arc::new(web::AppState {
                db: Mutex::new(open_db()?),
                token: token()?,
                home: home(),
                assistant: tokio::sync::Mutex::new(()),
                voice: Default::default(),
            });
            let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
            println!("{} on http://localhost:{port}  (run `catapp url` for the phone address)", brand::Brand::load(&home()).name);
            rt.block_on(web::serve(state, &bind, port)).map_err(|e| format!("can't serve on {bind}:{port}: {e}"))?;
        }
        Cmd::Url { port } => {
            let t = token()?;
            println!("On this computer: http://localhost:{port}");
            let mut phone = None;
            if let Some(h) = local_hostname() {
                let u = format!("http://{h}:{port}/?token={t}");
                println!("On your phone:    {u}");
                if let Some(short) = h.strip_suffix(".local") {
                    println!("  short name:     http://{short}:{port}/?token={t}   (PCs on your network; often not phones)");
                }
                phone = Some(u);
            }
            if let Some(ip) = lan_ip() {
                let u = format!("http://{ip}:{port}/?token={t}");
                println!("    or by IP:     {u}");
                phone = phone.or(Some(u));
            }
            if let Some(u) = phone {
                if let Ok(code) = qrcode::QrCode::new(u.as_bytes()) {
                    let img = code
                        .render::<qrcode::render::unicode::Dense1x2>()
                        .dark_color(qrcode::render::unicode::Dense1x2::Light)
                        .light_color(qrcode::render::unicode::Dense1x2::Dark)
                        .build();
                    println!("\nScan with your phone (same Wi-Fi as this computer):\n{img}");
                }
            }
            println!("Opening a link once logs that browser in for a year, at that address only: bookmark the same address.");
            println!("This computer must be awake. It runs as a service if you've done `catapp service install`.");
        }
        Cmd::List { category, all, json } => {
            let db = open_db()?;
            let cat = category.as_deref().map(db::norm_category);
            let mut ps: Vec<Project> = db
                .projects()
                .map_err(e)?
                .into_iter()
                .filter(|p| all || !db::is_finished(&p.status))
                .filter(|p| cat.as_ref().is_none_or(|c| &p.category == c))
                .collect();
            if json {
                json_out(&ps);
                return Ok(());
            }
            ps.sort_by_key(|p| (p.category.clone(), STATUSES.iter().position(|s| *s == p.status)));
            let mut last = String::new();
            for p in &ps {
                if p.category != last {
                    println!("\n{}", db::capitalise(&p.category));
                    last = p.category.clone();
                }
                print_project_line(p);
            }
            if ps.is_empty() {
                println!("No projects yet. Add one with: catapp new \"Name\" --category work");
            }
        }
        Cmd::Show { project, notes, json } => {
            let db = open_db()?;
            let p = db.find(&project).map_err(e)?;
            let d = db.detail(&p, notes).map_err(e)?;
            if json {
                json_out(&d);
                return Ok(());
            }
            println!("{}  [{} · {}]  ({})", p.name, p.category, p.status, p.slug);
            if !p.summary.is_empty() {
                println!("\n{}", p.summary);
            }
            if !p.next_step.is_empty() {
                println!("\nNext step: {}", p.next_step);
            }
            if !p.links.is_empty() {
                println!("\nLinks:");
                for l in p.links.lines() {
                    println!("  {l}");
                }
            }
            if !d.tasks.is_empty() {
                println!("\nTasks ({}/{}):", p.tasks_done, p.tasks_total);
                for t in &d.tasks {
                    println!("  [{}] {}  (#{})", if t.done { "x" } else { " " }, t.text, t.id);
                }
            }
            if !d.notes.is_empty() {
                println!("\nLog:");
                for n in &d.notes {
                    print_note(n, false);
                }
            }
        }
        Cmd::New { name, category, status, summary, next, links } => {
            let p = open_db()?
                .create(NewProject { name, category: Some(category), status: Some(status), summary, next_step: next, links }, "cli")
                .map_err(e)?;
            println!("Created {} ({}, {})", p.name, p.slug, p.category);
        }
        Cmd::Note { project, text, source } => {
            let db = open_db()?;
            let p = db.find(&project).map_err(e)?;
            let source = source.trim().to_lowercase();
            let n = db.add_note(p.id, &text_or_stdin(text)?, if source.is_empty() { "cli" } else { &source }).map_err(e)?;
            println!("Noted on {} (#{})", p.name, n.id);
        }
        Cmd::Set { project, status, next, summary, name, category, links } => {
            let db = open_db()?;
            let p = db.find(&project).map_err(e)?;
            if status.is_none() && next.is_none() && summary.is_none() && name.is_none() && category.is_none() && links.is_none() {
                return Err("nothing to change: give --status, --next, --summary, --name, --category or --links".into());
            }
            let p = db
                .update(p.id, ProjectPatch { name, category, status, summary, next_step: next, links }, "cli")
                .map_err(e)?;
            println!("Updated {}: {} · next: {}", p.name, p.status, if p.next_step.is_empty() { "(none)" } else { &p.next_step });
        }
        Cmd::Task { cmd } => {
            let db = open_db()?;
            match cmd {
                TaskCmd::Add { project, text } => {
                    let p = db.find(&project).map_err(e)?;
                    let t = db.add_task(p.id, &text_or_stdin(text)?).map_err(e)?;
                    println!("Added task #{} to {}", t.id, p.name);
                }
                TaskCmd::Done { id } | TaskCmd::Undo { id } => {
                    let done = matches!(cmd, TaskCmd::Done { .. });
                    let t = db.update_task(id, TaskPatch { done: Some(done), text: None }).map_err(e)?;
                    println!("[{}] {}", if t.done { "x" } else { " " }, t.text);
                }
                TaskCmd::Rm { id } => {
                    db.delete_task(id).map_err(e)?;
                    println!("Deleted task #{id}");
                }
            }
        }
        Cmd::Todo { text, all } => {
            let db = open_db()?;
            let p = match db.find(TODO_SLUG) {
                Ok(p) if p.slug == TODO_SLUG => p,
                _ => {
                    // Created as "Todo" so the slug is exactly "todo", then given a friendlier name
                    // (slugs never change on rename).
                    let p = db
                        .create(
                            NewProject {
                                name: "Todo".into(),
                                category: Some("personal".into()),
                                summary: Some(format!(
                                    "{}'s general to-do list. Shown at the top of the home page.",
                                    brand::Brand::load(&home()).owner
                                )),
                                ..Default::default()
                            },
                            "cli",
                        )
                        .map_err(e)?;
                    db.update(p.id, ProjectPatch { name: Some("To do".into()), ..Default::default() }, "cli").map_err(e)?
                }
            };
            if !text.is_empty() {
                let t = db.add_task(p.id, &text.join(" ")).map_err(e)?;
                println!("Added #{}: {}", t.id, t.text);
                return Ok(());
            }
            let tasks = db.tasks(p.id).map_err(e)?;
            let shown: Vec<_> = tasks.iter().filter(|t| all || !t.done).collect();
            if shown.is_empty() {
                println!("Nothing to do. Add something: catapp todo buy milk");
            }
            for t in shown {
                println!("  [{}] {}  (#{})", if t.done { "x" } else { " " }, t.text, t.id);
            }
        }
        Cmd::Recent { limit, category } => {
            let db = open_db()?;
            let cat = category.as_deref().map(db::norm_category);
            for n in db.recent(limit, cat.as_deref()).map_err(e)? {
                print_note(&n, true);
            }
        }
        Cmd::Search { query } => {
            let (ps, notes, tasks) = open_db()?.search(&query.join(" ")).map_err(e)?;
            if !ps.is_empty() {
                println!("Projects:");
                ps.iter().for_each(print_project_line);
            }
            if !tasks.is_empty() {
                println!("Tasks:");
                for t in &tasks {
                    println!("  [{}] {}  (#{})", if t.done { "x" } else { " " }, t.text, t.id);
                }
            }
            if !notes.is_empty() {
                println!("Notes:");
                notes.iter().for_each(|n| print_note(n, true));
            }
            if ps.is_empty() && notes.is_empty() && tasks.is_empty() {
                println!("Nothing found.");
            }
        }
        Cmd::Export { notes } => print!("{}", open_db()?.export_markdown(notes, &brand::Brand::load(&home()).name).map_err(e)?),
        Cmd::Delete { project, yes } => {
            let db = open_db()?;
            let p = db.find(&project).map_err(e)?;
            if !yes {
                return Err(format!(
                    "this deletes '{}' with its {} notes and {} tasks for good. Add --yes to confirm, or archive it: catapp set {} --status archived",
                    p.name, p.note_count, p.tasks_total, p.slug
                ));
            }
            db.delete_project(p.id).map_err(e)?;
            println!("Deleted {}", p.name);
        }
        Cmd::Mcp => mcp::run(open_db()?, brand::Brand::load(&home())).map_err(|e| e.to_string())?,
        Cmd::Config { key, value } => {
            let mut b = brand::Brand::load(&home());
            match key {
                None => {
                    println!("name:  {}", b.name);
                    println!("owner: {}", b.owner);
                    println!("icon:  {}", if b.icon.is_empty() { format!("auto ({})", b.icon_kind()) } else { b.icon.clone() });
                    let on = |x: bool| if x { "on" } else { "off" };
                    let file = |f: &str| if f.is_empty() { "none".to_string() } else { f.to_string() };
                    println!("voice: {}  (speaker {}, avatar {})", on(b.voice), b.speaker, on(b.avatar));
                    println!("persona: {}", file(&b.persona));
                    println!("memory:  {}", file(&b.memory));
                    println!("\nChange with `catapp config <setting> <value>`:");
                    for (k, d) in brand::KEYS {
                        println!("  {k:<8} {d}");
                    }
                    println!("Reload the web page to see changes.");
                }
                Some(k) if value.is_empty() => return Err(format!("give a value: catapp config {k} <value>")),
                Some(k) => {
                    let old_slug = b.slug();
                    b.set(&k, &value.join(" "))?;
                    b.save(&home())?;
                    println!("{k} set. Reload the web page to see it.");
                    if k == "name" && !b.service_label.is_empty() && b.service_label != b.new_service_label() {
                        println!("Run `catapp service install` to move the background service to the new name.");
                    }
                    let slug = b.slug();
                    if slug != old_slug {
                        println!(
                            "If Claude Code knows it as '{old_slug}', re-register it so its tools use the new name:\n  \
claude mcp remove --scope user {old_slug}\n  claude mcp add --scope user {slug} -- catapp mcp"
                        );
                    }
                }
            }
        }
        Cmd::Info => {
            let h = home();
            println!("Data:   {}", h.join("catapp.db").display());
            println!("Token:  {}", h.join("token").display());
            println!("Back up the data file to keep your notes safe. Set CATAPP_HOME to use another folder.");
        }
        Cmd::Service { cmd } => service(cmd)?,
    }
    Ok(())
}

fn launch_agent_path(label: &str) -> PathBuf {
    dirs::home_dir().unwrap_or_default().join("Library/LaunchAgents").join(format!("{label}.plist"))
}

fn uid() -> Result<String, String> {
    let out = std::process::Command::new("id").arg("-u").output().map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn launchctl(args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new("launchctl").args(args).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("launchctl {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

fn launchd_service(cmd: ServiceCmd) -> Result<(), String> {
    let mut b = brand::Brand::load(&home());
    let app = b.name.clone();
    let domain = format!("gui/{}", uid()?);
    let installed = b.installed_service_label();
    match cmd {
        ServiceCmd::Install { port } => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let log = home().join("catapp.log");
            std::fs::create_dir_all(home()).map_err(|e| e.to_string())?;
            // Replace any earlier install, even one made under an older name.
            let _ = launchctl(&["bootout", &format!("{domain}/{installed}")]);
            let old = launch_agent_path(&installed);
            if old.exists() {
                let _ = std::fs::remove_file(&old);
            }
            let label = b.new_service_label();
            let plist = launch_agent_path(&label);
            let xml = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key>
  <array><string>{}</string><string>serve</string><string>--port</string><string>{port}</string></array>
  <key>EnvironmentVariables</key>
  <dict><key>CATAPP_HOME</key><string>{}</string></dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>{}</string>
  <key>StandardErrorPath</key><string>{}</string>
</dict>
</plist>
"#,
                exe.display(),
                home().display(),
                log.display(),
                log.display()
            );
            std::fs::create_dir_all(plist.parent().unwrap()).map_err(|e| e.to_string())?;
            let _ = launchctl(&["bootout", &format!("{domain}/{label}")]);
            std::fs::write(&plist, xml).map_err(|e| e.to_string())?;
            launchctl(&["bootstrap", &domain, &plist.to_string_lossy()])?;
            b.service_label = label;
            b.save(&home())?;
            println!("{app} now runs in the background and starts at login (port {port}).");
            println!("It runs {}: reinstall the service if you move or rebuild it elsewhere.", exe.display());
            println!("Log: {}", log.display());
        }
        ServiceCmd::Restart => {
            if !launch_agent_path(&installed).exists() {
                return Err("the service isn't installed: run `catapp service install` first".into());
            }
            launchctl(&["kickstart", "-k", &format!("{domain}/{installed}")])?;
            println!("{app} restarted with the current binary.");
        }
        ServiceCmd::Uninstall => {
            let _ = launchctl(&["bootout", &format!("{domain}/{installed}")]);
            let plist = launch_agent_path(&installed);
            if plist.exists() {
                std::fs::remove_file(&plist).map_err(|e| e.to_string())?;
            }
            b.service_label.clear();
            b.save(&home())?;
            println!("{app}'s background service is stopped and removed. Your data is untouched.");
        }
    }
    Ok(())
}

/// The systemd user unit's file.
fn systemd_unit_path(label: &str) -> PathBuf {
    dirs::config_dir().unwrap_or_default().join("systemd/user").join(format!("{label}.service"))
}

/// A systemd user unit that runs the web app and restarts it if it stops.
fn systemd_unit(app: &str, exe: &std::path::Path, home: &std::path::Path, port: u16) -> String {
    // Quoted for systemd, with `%` escaped (it starts a specifier).
    let q = |p: &std::path::Path| format!("\"{}\"", p.display().to_string().replace('%', "%%").replace('"', "\\\""));
    let log = home.join("catapp.log");
    format!(
        "[Unit]
Description={app}: personal organiser web app
After=network-online.target

[Service]
ExecStart={} serve --port {port}
Environment={}
Restart=always
RestartSec=3
StandardOutput=append:{}
StandardError=append:{}

[Install]
WantedBy=default.target
",
        q(exe),
        q(std::path::Path::new(&format!("CATAPP_HOME={}", home.display()))),
        log.display().to_string().replace('%', "%%"),
        log.display().to_string().replace('%', "%%"),
    )
}

fn systemctl(args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .map_err(|e| format!("couldn't run systemctl ({e}): this needs a Linux system running systemd"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("systemctl --user {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

fn systemd_service(cmd: ServiceCmd) -> Result<(), String> {
    let mut b = brand::Brand::load(&home());
    let app = b.name.clone();
    let installed = b.installed_service_label();
    let unit = |label: &str| format!("{label}.service");
    match cmd {
        ServiceCmd::Install { port } => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            std::fs::create_dir_all(home()).map_err(|e| e.to_string())?;
            // Replace any earlier install, even one made under an older name.
            if systemd_unit_path(&installed).exists() {
                let _ = systemctl(&["disable", "--now", &unit(&installed)]);
                let _ = std::fs::remove_file(systemd_unit_path(&installed));
            }
            let label = b.new_service_label();
            let path = systemd_unit_path(&label);
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(&path, systemd_unit(&app, &exe, &home(), port)).map_err(|e| e.to_string())?;
            systemctl(&["daemon-reload"])?;
            systemctl(&["enable", &unit(&label)])?;
            systemctl(&["restart", &unit(&label)])?;
            b.service_label = label.clone();
            b.save(&home())?;
            println!("{app} now runs in the background and starts when you log in (port {port}).");
            println!("It runs {}: reinstall the service if you move or rebuild it elsewhere.", exe.display());
            println!("Log: {}   Status: systemctl --user status {}", home().join("catapp.log").display(), unit(&label));
            println!("To keep it running while you're logged out too: loginctl enable-linger $USER");
        }
        ServiceCmd::Restart => {
            if !systemd_unit_path(&installed).exists() {
                return Err("the service isn't installed: run `catapp service install` first".into());
            }
            systemctl(&["restart", &unit(&installed)])?;
            println!("{app} restarted with the current binary.");
        }
        ServiceCmd::Uninstall => {
            let path = systemd_unit_path(&installed);
            if path.exists() {
                let _ = systemctl(&["disable", "--now", &unit(&installed)]);
                std::fs::remove_file(&path).map_err(|e| e.to_string())?;
                let _ = systemctl(&["daemon-reload"]);
            }
            b.service_label.clear();
            b.save(&home())?;
            println!("{app}'s background service is stopped and removed. Your data is untouched.");
        }
    }
    Ok(())
}

fn service(cmd: ServiceCmd) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        launchd_service(cmd)
    } else if cfg!(target_os = "linux") {
        systemd_service(cmd)
    } else {
        Err("`catapp service` works on macOS and Linux. Elsewhere, run `catapp serve` (with your system's own way of starting it at login).".into())
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("catapp: {msg}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_unit_quotes_paths() {
        let u = systemd_unit("MaisyApp", Path::new("/home/maisy/.cargo/bin/catapp"), Path::new("/home/maisy/.local/share/catapp"), 8787);
        assert!(u.contains("Description=MaisyApp: personal organiser web app"));
        assert!(u.contains("ExecStart=\"/home/maisy/.cargo/bin/catapp\" serve --port 8787\n"));
        assert!(u.contains("Environment=\"CATAPP_HOME=/home/maisy/.local/share/catapp\"\n"));
        assert!(u.contains("StandardOutput=append:/home/maisy/.local/share/catapp/catapp.log\n"));
        assert!(u.contains("Restart=always") && u.contains("WantedBy=default.target"));
        let odd = systemd_unit("X", Path::new("/opt/100% \"cat\"/catapp"), Path::new("/d"), 1);
        assert!(odd.contains(r#"ExecStart="/opt/100%% \"cat\"/catapp" serve"#));
    }
}
