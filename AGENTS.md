# Notes for AI agents working on this repo

## Never use the real database

The person running this app keeps their real life in it: personal projects, work, health, family. None of that belongs in this public repo.

- **Don't read, copy or test against the real data folder** (`~/Library/Application Support/catapp` on macOS, `~/.local/share/catapp` on Linux, or wherever `CATAPP_HOME` points for the installed service). That includes copying `catapp.db` "just to test", and pointing a dev build or the MCP server at it.
- **Don't test against the running app** on the usual port (8787). It's the live copy.
- **Never put real data into the repo**: no project names, tasks, notes or people from anyone's database in code, test fixtures, screenshots, commit messages, PR titles or descriptions, or issues. Use made-up examples ("Learn Blender", "Book the dentist").

**Private projects** are encrypted in the browser and hidden from the CLI, MCP and the assistant on purpose (`Db::show_private` is for the web app only). Keep it that way: don't add paths that hand their rows to anything but the web app, and never try to read or decrypt anyone's.

The MCP tools you may have for this app (`catapp`, `maisyapp`, ...) are the owner's real organiser too. Use them for what the owner asks (like posting a status note), never as test data.

## Testing with made-up data

Every command takes its data folder from `CATAPP_HOME`, so give it a fresh, empty one:

```sh
export CATAPP_HOME=$(mktemp -d)
cargo run -- todo "Book the dentist"
cargo run -- new "Learn Blender" -n "Model the mug"
cargo run -- idea "Learn to juggle"
cargo run -- serve --port 8799 --bind 127.0.0.1   # then open http://localhost:8799
```

Unit tests use an in-memory database (`Db::memory()`) and need nothing else: `cargo test`.

## After changing the code

`cargo test` and `cargo clippy`. Keep the README general: other people run their own copies under their own names.
