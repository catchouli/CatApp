//! A minimal MCP server over stdio (JSON-RPC, one message per line), so
//! Claude and other agents can read and update projects. Register it with:
//! `claude mcp add catapp -- catapp mcp` (use the app's slug, e.g. `maisyapp`, if renamed)

use crate::brand::Brand;
use crate::db::{Db, NewProject, ProjectPatch, TaskPatch};
use serde_json::{json, Value};
use std::io::{BufRead, Write};

fn instructions(b: &Brand) -> String {
    let (app, owner) = (&b.name, &b.owner);
    format!(
        "{app} is {owner}'s personal organiser: projects (personal and work), each with a status, a next step, a \
checklist of tasks and a log of dated notes. Projects can be named by slug, id, a unique part of their name, or the \
absolute path of a folder in the project (your working directory finds the project you're working in). At the end \
of a working session, post one short status note (at most 3 lines: what changed, what's next, anything blocked) \
with your agent name as `source`, and update the next step if it changed. Don't create projects unless asked. \
Tasks for {owner}: add one only when {owner} asks, or for something that genuinely needs {owner} after the session \
and only after asking {owner} first; questions and decisions you can settle by asking {owner} in the conversation \
aren't tasks. Put a task on the project it belongs to (the one you're working in, or the one it's about); the \
general to-do list, the project 'todo', is only for things that don't belong to any project. {owner}'s ideas list is the project \
'ideas' (things to maybe do some day). To move a task to another project or list, use update_task with `project`."
    )
}

fn tools() -> Value {
    let project = json!({ "type": "string", "description": "Project slug, id, a unique part of its name, or the absolute path of a folder in it (e.g. your working directory)" });
    let source = json!({ "type": "string", "description": "Who is writing: your agent name, e.g. claude, opencode (default claude)" });
    json!([
        {
            "name": "list_projects",
            "description": "List projects with status, next step, task progress and last activity. Archived projects are hidden unless include_archived is true.",
            "inputSchema": { "type": "object", "properties": {
                "category": { "type": "string", "description": "Only this category, e.g. work or personal" },
                "include_archived": { "type": "boolean" }
            }}
        },
        {
            "name": "get_project",
            "description": "One project in full: details, tasks, and its log (newest first).",
            "inputSchema": { "type": "object", "required": ["project"], "properties": {
                "project": project,
                "notes_limit": { "type": "integer", "description": "How many log entries (default 20, 0 for all)" }
            }}
        },
        {
            "name": "add_note",
            "description": "Add a dated note to a project's log, e.g. a status update at the end of a session.",
            "inputSchema": { "type": "object", "required": ["project", "text"], "properties": {
                "project": project, "text": { "type": "string" }, "source": source
            }}
        },
        {
            "name": "update_project",
            "description": "Change a project's fields. Only the fields given change. Status and next-step changes are recorded in the log automatically.",
            "inputSchema": { "type": "object", "required": ["project"], "properties": {
                "project": project,
                "name": { "type": "string" },
                "category": { "type": "string" },
                "status": { "type": "string", "enum": crate::db::STATUSES },
                "summary": { "type": "string" },
                "next_step": { "type": "string" },
                "links": { "type": "string", "description": "One link or path per line" },
                "source": source
            }}
        },
        {
            "name": "create_project",
            "description": "Create a project. Only when the user asks for one.",
            "inputSchema": { "type": "object", "required": ["name"], "properties": {
                "name": { "type": "string" },
                "category": { "type": "string", "description": "personal, work, or another word (default personal)" },
                "status": { "type": "string", "enum": crate::db::STATUSES },
                "summary": { "type": "string" },
                "next_step": { "type": "string" },
                "links": { "type": "string" }
            }}
        },
        {
            "name": "add_task",
            "description": "Add a checklist item to a project. Prefer the project the task belongs to over the general to-do list ('todo').",
            "inputSchema": { "type": "object", "required": ["project", "text"], "properties": {
                "project": project, "text": { "type": "string" }
            }}
        },
        {
            "name": "update_task",
            "description": "Tick or untick a task, change its text, or move it to another project (e.g. from 'todo' to a project, or to 'ideas'). Task ids come from get_project.",
            "inputSchema": { "type": "object", "required": ["task_id"], "properties": {
                "task_id": { "type": "integer" }, "done": { "type": "boolean" }, "text": { "type": "string" },
                "project": { "type": "string", "description": "Move the task to the end of this project's checklist" }
            }}
        },
        {
            "name": "recent_notes",
            "description": "The latest notes across all projects.",
            "inputSchema": { "type": "object", "properties": {
                "limit": { "type": "integer", "description": "Default 20" },
                "category": { "type": "string" }
            }}
        },
        {
            "name": "search",
            "description": "Find projects, notes and tasks containing some text.",
            "inputSchema": { "type": "object", "required": ["query"], "properties": { "query": { "type": "string" } } }
        }
    ])
}

fn s<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str)
}

fn owned(a: &Value, k: &str) -> Option<String> {
    s(a, k).map(String::from)
}

/// The writer's name for notes: the `source` argument, or "claude".
fn src(a: &Value) -> String {
    let s = s(a, "source").unwrap_or("").trim().to_lowercase();
    if s.is_empty() { "claude".into() } else { s }
}

fn need<'a>(a: &'a Value, k: &str) -> Result<&'a str, String> {
    s(a, k).ok_or_else(|| format!("missing '{k}'"))
}

const KNOWN: &[(&str, &[&str])] = &[
    ("list_projects", &["category", "include_archived"]),
    ("get_project", &["project", "notes_limit"]),
    ("add_note", &["project", "text", "source"]),
    ("update_project", &["project", "name", "category", "status", "summary", "next_step", "links", "source"]),
    ("create_project", &["name", "category", "status", "summary", "next_step", "links"]),
    ("add_task", &["project", "text"]),
    ("update_task", &["task_id", "done", "text", "project"]),
    ("recent_notes", &["limit", "category"]),
    ("search", &["query"]),
];

fn call(db: &Db, owner: &str, name: &str, a: &Value) -> Result<Value, String> {
    let known = KNOWN.iter().find(|(n, _)| *n == name).ok_or_else(|| format!("unknown tool '{name}'"))?.1;
    if let Some(obj) = a.as_object() {
        if let Some(bad) = obj.keys().find(|k| !known.contains(&k.as_str())) {
            return Err(format!("unknown argument '{bad}' for {name}: it takes {}", known.join(", ")));
        }
    }
    let e = |e: crate::db::Error| e.to_string();
    Ok(match name {
        "list_projects" => {
            let all = a.get("include_archived").and_then(Value::as_bool).unwrap_or(false);
            let cat = s(a, "category").map(crate::db::norm_category);
            let ps: Vec<_> = db
                .projects()
                .map_err(e)?
                .into_iter()
                .filter(|p| all || p.status != "archived")
                .filter(|p| cat.as_ref().is_none_or(|c| &p.category == c))
                .map(|p| {
                    json!({
                        "slug": p.slug, "name": p.name, "category": p.category, "status": p.status,
                        "next_step": p.next_step, "tasks": format!("{}/{}", p.tasks_done, p.tasks_total),
                        "notes": p.note_count, "last_activity": p.last_activity
                    })
                })
                .collect();
            json!(ps)
        }
        "get_project" => {
            let p = db.find(need(a, "project")?).map_err(e)?;
            let lim = a.get("notes_limit").and_then(Value::as_u64).unwrap_or(20) as usize;
            v(&db.detail(&p, lim).map_err(e)?)
        }
        "add_note" => {
            let p = db.find(need(a, "project")?).map_err(e)?;
            v(&db.add_note(p.id, need(a, "text")?, &src(a)).map_err(e)?)
        }
        "update_project" => {
            let p = db.find(need(a, "project")?).map_err(e)?;
            let patch = ProjectPatch {
                name: owned(a, "name"),
                category: owned(a, "category"),
                status: owned(a, "status"),
                summary: owned(a, "summary"),
                next_step: owned(a, "next_step"),
                links: owned(a, "links"),
            };
            v(&db.update(p.id, patch, &src(a)).map_err(e)?)
        }
        "create_project" => {
            let np = NewProject {
                name: need(a, "name")?.into(),
                category: owned(a, "category"),
                status: owned(a, "status"),
                summary: owned(a, "summary"),
                next_step: owned(a, "next_step"),
                links: owned(a, "links"),
            };
            v(&db.create(np, "claude").map_err(e)?)
        }
        "add_task" => {
            let p = db.find_or_list(need(a, "project")?, owner, "claude").map_err(e)?;
            v(&db.add_task(p.id, need(a, "text")?).map_err(e)?)
        }
        "update_task" => {
            let id = a.get("task_id").and_then(Value::as_i64).ok_or("missing 'task_id'")?;
            if let Some(to) = s(a, "project") {
                let p = db.find_or_list(to, owner, "claude").map_err(e)?;
                db.move_task(id, p.id).map_err(e)?;
            }
            let patch = TaskPatch { text: owned(a, "text"), done: a.get("done").and_then(Value::as_bool) };
            v(&db.update_task(id, patch).map_err(e)?)
        }
        "recent_notes" => {
            let lim = a.get("limit").and_then(Value::as_u64).unwrap_or(20).min(200) as usize;
            let cat = s(a, "category").map(crate::db::norm_category);
            v(&db.recent(lim, cat.as_deref()).map_err(e)?)
        }
        "search" => {
            let (projects, notes, tasks) = db.search(need(a, "query")?).map_err(e)?;
            json!({ "projects": projects, "notes": notes, "tasks": tasks })
        }
        _ => unreachable!(),
    })
}

fn v<T: serde::Serialize>(x: &T) -> Value {
    serde_json::to_value(x).expect("serialisable")
}

fn handle(db: &Db, brand: &Brand, msg: &Value) -> Option<Value> {
    let id = msg.get("id")?.clone(); // notifications get no reply
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let result = match method {
        "initialize" => {
            let version = msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18");
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": brand.slug(), "version": env!("CARGO_PKG_VERSION") },
                "instructions": instructions(brand)
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or("");
            let args = msg["params"].get("arguments").cloned().unwrap_or(json!({}));
            match call(db, &brand.owner, name, &args) {
                Ok(v) => json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap() }] }),
                Err(err) => json!({ "content": [{ "type": "text", "text": err }], "isError": true }),
            }
        }
        _ => {
            return Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("unknown method '{method}'") } }))
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

pub fn run(db: Db, brand: Brand) -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(&db, &brand, &msg),
            Err(e) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": e.to_string() } })),
        };
        if let Some(r) = reply {
            writeln!(out, "{r}")?;
            out.flush()?;
        }
    }
    Ok(())
}
