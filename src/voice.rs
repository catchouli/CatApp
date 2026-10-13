//! Voice mode (off unless `catapp config voice on`): the assistant keeps one Claude Code
//! session running while its panel is open, so replies start in about a second and a half
//! instead of six, and streams them so the page can speak each sentence as it arrives.
//! Speech goes through a local helper (`voice/voice_server.py`: Kokoro text-to-speech,
//! Parakeet speech-to-text) set up by `voice/setup.sh`. Both processes stop after a few
//! idle minutes and start again when needed.

use crate::brand::Brand;
use crate::db;
use crate::web::{assistant_prompt, claude_command, explain_failure};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, Mutex};

/// Stop a process when nothing has used it (or pinged it from an open panel) for this long.
const IDLE: Duration = Duration::from_secs(180);
const TURN_TIMEOUT: Duration = Duration::from_secs(180);
const HELPER_SCRIPT: &str = include_str!("../voice/voice_server.py");

/// A child process spoken to in JSON lines over stdin/stdout. Killed when dropped.
struct Proc {
    child: Child,
    stdin: ChildStdin,
    out: Lines<BufReader<ChildStdout>>,
    used: Instant,
}

impl Proc {
    fn spawn(mut cmd: Command) -> std::io::Result<Proc> {
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).kill_on_drop(true);
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let out = BufReader::new(child.stdout.take().expect("piped")).lines();
        Ok(Proc { child, stdin, out, used: Instant::now() })
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    async fn send(&mut self, v: &Value) -> std::io::Result<()> {
        self.used = Instant::now();
        self.stdin.write_all(format!("{v}\n").as_bytes()).await?;
        self.stdin.flush().await
    }

    /// The next JSON line, skipping anything else; None when the process has gone.
    async fn recv(&mut self) -> Option<Value> {
        loop {
            let line = self.out.next_line().await.ok()??;
            if let Ok(v) = serde_json::from_str(&line) {
                self.used = Instant::now();
                return Some(v);
            }
        }
    }
}

/// The running Claude Code session, the conversation it's continuing and the instructions
/// it started with (a settings change restarts it).
struct Chat {
    p: Proc,
    session: Option<String>,
    prompt: String,
}

#[derive(Default)]
pub struct Voice {
    chat: Arc<Mutex<Option<Chat>>>,
    helper: Mutex<Option<Proc>>,
}

fn invalid(msg: impl Into<String>) -> db::Error {
    db::Error::Invalid(msg.into())
}

const SPOKEN: &str = "\n\nVoice mode: your replies are read aloud by a text-to-speech voice, and {owner} may be talking \
to you through speech recognition, so expect the odd misheard word and go with the most likely meaning. Write natural \
speech: no markdown, lists, links, emoji or emoticons, and keep it short. When a request needs the tools, start with a \
few spoken words (like \"sure, one sec\") before the first tool call, so there's no silence while you work.";

/// The assistant's usual instructions, plus speech, plus the persona and its memory if set.
fn voice_prompt(b: &Brand) -> String {
    let mut p = assistant_prompt(b) + &SPOKEN.replace("{owner}", &b.owner);
    let read = |f: &str| (!f.is_empty()).then(|| std::fs::read_to_string(f).ok()).flatten();
    if let Some(persona) = read(&b.persona) {
        p += &format!(
            "\n\n## Persona\n\nIn voice mode you are the character below: their personality, voice and attitude. You're \
still {app}'s assistant, though: you can only use the {app} tools and the rules above still apply. Anything the \
character description says about files, chats, sending messages or updating a memory file doesn't apply here.\n\n{}",
            persona.trim(),
            app = b.name
        );
        if let Some(memory) = read(&b.memory) {
            p += &format!(
                "\n\n## Your memory\n\nYour memory notes, for context. You can't change them from here.\n\n{}",
                memory.trim()
            );
        }
    }
    p
}

impl Voice {
    /// Make sure a Claude session is running for this conversation (`session` None = a new
    /// chat), restarting it if the browser has moved to a different conversation.
    async fn ensure_chat(slot: &mut Option<Chat>, home: &Path, b: &Brand, session: Option<String>) -> Result<(), db::Error> {
        let prompt = voice_prompt(b);
        if let Some(c) = slot.as_mut() {
            if c.session == session && c.prompt == prompt && c.p.alive() {
                return Ok(());
            }
        }
        *slot = None;
        let mut cmd = claude_command(home, b, &prompt)?;
        cmd.args(["--input-format", "stream-json", "--output-format", "stream-json"])
            .args(["--verbose", "--include-partial-messages"]);
        if let Some(s) = &session {
            cmd.args(["--resume", s]);
        }
        let p = Proc::spawn(cmd).map_err(|e| invalid(format!("couldn't start Claude Code: {e}")))?;
        *slot = Some(Chat { p, session, prompt });
        Ok(())
    }

    /// Start Claude and the voice helper ahead of time, or keep them alive while the panel is open.
    pub async fn warm(self: &Arc<Self>, home: &Path, b: &Brand, session: Option<String>) -> Result<(), db::Error> {
        if let Ok(mut slot) = self.chat.try_lock() {
            Self::ensure_chat(&mut slot, home, b, session).await?;
            if let Some(c) = slot.as_mut() {
                c.p.used = Instant::now();
            }
        } // else a reply is streaming, which keeps it alive anyway
        let (me, home) = (self.clone(), home.to_path_buf());
        tokio::spawn(async move {
            if let Ok(mut h) = me.helper(&home).await {
                if let Some(p) = h.as_mut() {
                    p.used = Instant::now();
                }
            }
        });
        Ok(())
    }

    /// Stop the Claude session (the next message starts a new one).
    pub async fn reset(&self) {
        *self.chat.lock().await = None;
    }

    /// Send a message and stream the reply as JSON events:
    /// `{"t":"text","d":"..."}` pieces, `{"t":"tool"}` when it starts using a tool, and finally
    /// `{"t":"done","session":"..."}` or `{"t":"done","error":"..."}`.
    pub async fn chat(&self, home: &Path, b: &Brand, session: Option<String>, message: String) -> Result<mpsc::Receiver<Value>, db::Error> {
        let mut slot = self.chat.clone().lock_owned().await;
        Self::ensure_chat(&mut slot, home, b, session).await?;
        let (tx, rx) = mpsc::channel(256);
        tokio::spawn(async move {
            // Keep reading to the end of the turn even if the browser goes away, so the
            // session is ready for the next message.
            let send = |v: Value| {
                let tx = tx.clone();
                async move {
                    let _ = tx.send(v).await;
                }
            };
            let c = slot.as_mut().expect("just ensured");
            let msg = json!({ "type": "user", "message": { "role": "user", "content": message } });
            if c.p.send(&msg).await.is_err() {
                *slot = None;
                return send(json!({ "t": "done", "error": "the assistant stopped unexpectedly; try again" })).await;
            }
            let mut wrote = false;
            loop {
                let e = match tokio::time::timeout(TURN_TIMEOUT, c.p.recv()).await {
                    Ok(Some(e)) => e,
                    Ok(None) => {
                        *slot = None;
                        return send(json!({ "t": "done", "error": "the assistant stopped unexpectedly; try again" })).await;
                    }
                    Err(_) => {
                        *slot = None;
                        return send(json!({ "t": "done", "error": "the assistant took more than 3 minutes and was stopped" })).await;
                    }
                };
                match e["type"].as_str() {
                    Some("stream_event") => {
                        let ev = &e["event"];
                        match (ev["type"].as_str(), ev["content_block"]["type"].as_str(), ev["delta"]["type"].as_str()) {
                            (Some("content_block_start"), Some("text"), _) if wrote => send(json!({ "t": "text", "d": "\n" })).await,
                            (Some("content_block_start"), Some("tool_use"), _) => send(json!({ "t": "tool" })).await,
                            (Some("content_block_delta"), _, Some("text_delta")) => {
                                wrote = true;
                                send(json!({ "t": "text", "d": ev["delta"]["text"] })).await
                            }
                            _ => {}
                        }
                    }
                    Some("result") => {
                        c.session = e["session_id"].as_str().map(String::from);
                        let done = if e["is_error"].as_bool().unwrap_or(false) {
                            json!({ "t": "done", "session": c.session, "error": explain_failure(e["result"].as_str().unwrap_or("unknown error")) })
                        } else {
                            json!({ "t": "done", "session": c.session })
                        };
                        return send(done).await;
                    }
                    _ => {}
                }
            }
        });
        Ok(rx)
    }

    /// The voice helper, started if it isn't running.
    async fn helper(&self, home: &Path) -> Result<tokio::sync::MutexGuard<'_, Option<Proc>>, String> {
        let mut h = self.helper.lock().await;
        if h.as_mut().is_some_and(|p| p.alive()) {
            return Ok(h);
        }
        *h = None;
        let dir = home.join("voice");
        let python = dir.join("venv/bin/python");
        if !python.is_file() || !dir.join("kokoro-v1.0.onnx").exists() {
            return Err("Voice isn't set up on this computer yet: run voice/setup.sh from the app's source folder.".into());
        }
        let script = dir.join("voice_server.py");
        if std::fs::read_to_string(&script).ok().as_deref() != Some(HELPER_SCRIPT) {
            std::fs::write(&script, HELPER_SCRIPT).map_err(|e| e.to_string())?;
        }
        let mut cmd = Command::new(python);
        // The background service has a minimal PATH; speech-to-text needs ffmpeg.
        let path = format!("/opt/homebrew/bin:/usr/local/bin:{}", std::env::var("PATH").unwrap_or_default());
        cmd.arg(&script).arg(&dir).env("PATH", path);
        let mut p = Proc::spawn(cmd).map_err(|e| format!("couldn't start the voice helper: {e}"))?;
        match tokio::time::timeout(Duration::from_secs(120), p.recv()).await {
            Ok(Some(v)) if v["ready"] == true => {}
            _ => return Err("the voice helper didn't start (see catapp.log in the data folder)".into()),
        }
        *h = Some(p);
        Ok(h)
    }

    /// One request to the voice helper.
    pub async fn speech(&self, home: &Path, req: Value) -> Result<Value, db::Error> {
        let mut h = self.helper(home).await.map_err(invalid)?;
        let p = h.as_mut().expect("helper running");
        let reply = tokio::time::timeout(Duration::from_secs(60), async {
            p.send(&req).await.ok()?;
            p.recv().await
        })
        .await;
        match reply {
            Ok(Some(v)) => match v["error"].as_str() {
                Some(e) => Err(invalid(format!("voice: {e}"))),
                None => Ok(v),
            },
            _ => {
                *h = None;
                Err(invalid("the voice helper stopped; try again"))
            }
        }
    }

    /// Stop whatever has been idle too long. Runs for the life of the server.
    pub fn reap_idle(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(20)).await;
                if let Ok(mut c) = self.chat.try_lock() {
                    if c.as_ref().is_some_and(|c| c.p.used.elapsed() > IDLE) {
                        *c = None;
                    }
                }
                if let Ok(mut h) = self.helper.try_lock() {
                    if h.as_ref().is_some_and(|p| p.used.elapsed() > IDLE) {
                        *h = None;
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_adds_persona_and_memory_only_when_set() {
        let mut b = Brand::default();
        assert!(voice_prompt(&b).contains("Voice mode") && !voice_prompt(&b).contains("## Persona"));
        let dir = std::env::temp_dir();
        let (persona, memory) = (dir.join("catapp-test-persona.md"), dir.join("catapp-test-memory.md"));
        std::fs::write(&persona, "A grumpy librarian.").unwrap();
        std::fs::write(&memory, "Likes tea.").unwrap();
        b.memory = memory.to_string_lossy().into();
        assert!(!voice_prompt(&b).contains("Likes tea"), "memory only comes with a persona");
        b.persona = persona.to_string_lossy().into();
        let p = voice_prompt(&b);
        assert!(p.contains("A grumpy librarian.") && p.contains("Likes tea.") && p.contains("can only use the CatApp tools"));
    }
}
