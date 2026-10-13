# CatApp guide

Everything about setting up, configuring and using CatApp. The [README](../README.md) has the short version.

## Setting it up (macOS or Linux)

1. **Install Rust** if you don't have it: <https://rustup.rs>
2. **Build and install** from this folder:
   ```sh
   cargo install --path .
   ```
   This puts the `catapp` command in `~/.cargo/bin`.
3. **Make it yours:**
   ```sh
   catapp config name MaisyApp     # the app's name
   catapp config owner Maisy       # your first name (the assistant uses it)
   catapp config                   # check
   ```
   The icon is a cat for CatApp and a plain tick otherwise. `catapp config icon cat` if you want the cat anyway.
4. **Run it in the background** (starts at login, restarts if it crashes):
   ```sh
   catapp service install
   ```
5. **Open it:** <http://localhost:8787> on the computer running it. For your phone, run `catapp url` and scan the QR code or open the link (same Wi-Fi). Opening the link once logs that browser in for a year. In Safari, Share > Add to Home Screen makes it feel like an app.
6. **Optional: the assistant.** Install [Claude Code](https://claude.com/claude-code) on the same computer and log in once (run `claude` in a terminal). Then the **Ask** button works. Without it, everything else still works, and Ask explains what's missing.

Your data lives in `~/Library/Application Support/catapp/` on macOS and `~/.local/share/catapp/` on Linux: `catapp.db`, the access `token`, `config.json` and a log. Each person's copy is completely separate.

**The service** uses launchd on macOS and a systemd user service on Linux (`systemctl --user status catapp.service`, named after your app). On Linux it runs while you're logged in; `loginctl enable-linger $USER` keeps it running when you're not.

**The port** is 8787 unless you change it: `catapp config port 9000`, then `catapp service restart`. `catapp serve --port` overrides it for one run.

**Updating** after pulling new code: `cargo install --path . && catapp service restart`.

## Using it from a phone or another computer

- `catapp url` prints the links: the computer's `name.local` address (works from phones; on Linux it needs Avahi, which most desktops have), a short name (works from most PCs), and the IP.
- **Each address keeps its own login.** `mymac.local`, `mymac` and the IP count as different sites, so log in once at the address you bookmark.
- The computer has to be awake. If macOS asks whether `catapp` may accept incoming connections, allow it. On Linux, open the port in your firewall if you use one (e.g. `sudo ufw allow 8787/tcp`).
- It's plain HTTP on your home network, protected by the token. Away from home, use a private network like Tailscale. Don't expose it to the internet.
- To rename a Mac (and so its `.local` address): `sudo scutil --set LocalHostName newname` (also `ComputerName` and `HostName`). On Linux: `sudo hostnamectl hostname newname`.

## To-do list

The project with slug `todo` is the general to-do list. It shows at the top of the home page (first eight items; tap one to expand, tick to finish) instead of as a project card.

```sh
catapp todo                  # show open items (creates the list the first time)
catapp todo pay the gas bill # add one
catapp task done 76          # tick one off
```

## Assistant

The **Ask** button (top bar on desktop, floating button on phones; or open `/#/ask`) is a chat with Claude that can only touch this app: "add a to-do to book the dentist", "add a project for learning Blender", "what's next on my projects?". Follow-up messages continue the same conversation; **New chat** starts afresh.

- **How it works:** each message runs Claude Code once in the background (`claude -p`, roughly 7 seconds) with no built-in tools (no shell, files or web), only this app's MCP server, and everything else auto-denied. It runs one request at a time, behind the app's login.
- **It uses your Claude Code login and plan usage.** It's for you on your own computer, not for serving other people.
- **It can't delete anything**, and it's told not to abandon or archive projects unless asked. Its notes are labelled `assistant`.
- **If Claude Code isn't installed or isn't logged in,** Ask replies with what to do instead.
- **Settings** (environment variables for the service): `CATAPP_CLAUDE` (path to `claude`, if it's not in `~/.local/bin`), `CATAPP_ASSISTANT_MODEL` (e.g. `sonnet`, `haiku`).

## Voice mode (optional, off by default)

Talk to the assistant and hear it answer, with an optional character and animated avatar. Everything speech-related runs locally; only the conversation itself goes to Claude, as with typed messages. **It needs a Mac with Apple Silicon** (the speech-to-text runs on Apple's MLX). Everything else works without it.

```sh
sh voice/setup.sh              # once: Python environment, Kokoro voice, Parakeet speech-to-text (~3 GB)
catapp config voice on
```

- **Talking:** tap the mic in the Ask panel and speak. It sends when you pause for a second, or tap again to send straight away. Typed messages work too.
- **Hearing:** replies are read aloud sentence by sentence as they're written. The speaker button in the panel's header turns that off.
- **Faster replies:** while the panel is open, Claude stays running in the background, so replies start in about 1.5 seconds instead of 6. It stops after 3 idle minutes, or 20 minutes without a message.
- **Voice:** `catapp config speaker af_heart` picks a [Kokoro voice](https://huggingface.co/hexgrad/Kokoro-82M/blob/main/VOICES.md). `af_alloy,af_nicole` blends voices evenly, named blends live in `voice/voice_server.py` (e.g. `patchouli`), and `:1.2` on the end changes the speed (`patchouli:1.2`; much above 1.15 can add artifacts).
- **A character:** `catapp config persona ~/path/to/character.md` makes the assistant play the character described in that file (the file name is shown as its name). `catapp config memory ~/path/to/notes.md` lets it read a notes file too (it can't change it). It's still limited to this app's tools. `none` turns either off.
- **Avatar:** `sh voice/setup.sh --avatar` downloads Live2D's runtime and their free Hiyori sample model into the data folder's `avatar/`, then `catapp config avatar on`. Its mouth follows the voice. To use another Live2D (Cubism 3 or later) model, put its folder in `avatar/` in place of `Hiyori/`.
- **Microphone on a phone:** browsers only allow the microphone on HTTPS pages (or on the computer itself, at `localhost`). Over plain HTTP the phone can still hear replies and use keyboard dictation.

## Agent status updates

AI agents working in your repos can post short status notes to the matching project.

- **Register the MCP server** for all your Claude Code sessions: `claude mcp add --scope user catapp -- ~/.cargo/bin/catapp mcp`. If you've renamed the app, register it under its slug instead (e.g. `maisyapp`) so the tools show up as `mcp__maisyapp__*`; `catapp config name` prints the exact commands.
- **Connect repos to projects:** put each repo's folder in its project's links (one per line). An agent passes its working directory as the project, or from a shell: `catapp note . --source claude "what changed, what's next"`. The project whose links contain that folder (or its nearest parent) wins.
- **Ask agents to post:** e.g. a line in `~/.claude/CLAUDE.md`: "At the end of a session in a repo linked to a CatApp project, post one short status note with `add_note` (project = your working directory, source = your name)."
- Notes from the web app and CLI count as yours. Anything else is an agent. Each project's log has **All / Mine / Agents** filters.

## CLI cheatsheet

| Command | Does |
|---|---|
| `catapp ls` / `catapp ls -a` / `catapp ls -c work` | List projects; `-a` includes finished ones (done, abandoned, archived) |
| `catapp show blender` | One project: details, tasks, log |
| `catapp note blender "text"` | Add a note; no text means read stdin. `.` = this folder's project; `--source name` for agents |
| `catapp set blender -s paused -n "next thing"` | Change status / next step / `--summary` / `--name` / `-c` / `--links` |
| `catapp new "Name" -c work -n "first step"` | New project |
| `catapp task add blender "thing"` / `task done 12` / `task undo 12` / `task rm 12` | Checklist |
| `catapp todo` / `catapp todo "thing"` | The to-do list |
| `catapp recent` / `catapp search word` | Latest notes / search everything |
| `catapp export > snapshot.md` | Everything as Markdown |
| `catapp config` | Settings: name, owner, icon, port, voice, speaker, persona, memory, avatar |
| `catapp url` / `catapp info` | Links for other devices / where the data is |
| `catapp service install` / `restart` / `uninstall` | The background service |

Projects can be named by slug, id, any unique part of the name, or a folder path.

**Statuses:** active, planned, idea, paused, done, abandoned (no further work planned), archived. Done, abandoned and archived count as finished: hidden unless you ask for them (`-a`, or "Show finished" in the app). Status and next-step changes are logged automatically.

## Code

| File | What |
|---|---|
| `src/db.rs` | Schema, queries, folder lookup, Markdown export, unit tests |
| `src/web.rs` | axum server, JSON API, token/cookie auth, the assistant endpoints |
| `src/voice.rs` | Voice mode: the kept-running Claude session (streamed replies) and the voice helper |
| `voice/` | `voice_server.py` (Kokoro and Parakeet, embedded in the binary) and `setup.sh` |
| `src/mcp.rs` | MCP over stdio (hand-written JSON-RPC, 9 tools) |
| `src/brand.rs` | Settings from `config.json`: name, owner, icon, port, voice |
| `src/main.rs` | CLI (clap), `url` with QR code, the launchd and systemd services |
| `static/` | Single-page front end (vanilla JS), embedded in the binary |

`cargo test` runs the unit tests. Set `CATAPP_HOME` to use a different data folder, e.g. for a test copy.
