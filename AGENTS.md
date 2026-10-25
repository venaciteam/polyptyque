# AGENTS.md

Guidance for AI coding agents working on Polyptyque. To *use* Polyptyque (drive a running copy), see [docs/control.md](docs/control.md) and [skills/polyptyque](skills/polyptyque/SKILL.md) instead.

Polyptyque records several windows at once on Windows, each into its own MKV file with only its own sound. Rust workspace, interface in [Slint](https://slint.dev), installer and updates by [Velopack](https://velopack.io), encoding by one FFmpeg process per source. License: GPL-3.0-or-later.

## Source of truth

The specification is [`docs/cahier-des-charges.md`](docs/cahier-des-charges.md) (in French). Every change of behavior goes there too: the relevant section, the roadmap (« Feuille de route ») and the decision log (« Journal des décisions »). Code comments and working documents are in French.

## Layout

| Crate | Role |
|---|---|
| `polyptyque-core` | Data model (scenes, sources, settings), naming, sync, FFmpeg commands, scheduled recordings, translation catalog reader, control protocol and command catalog (`src/control/`), the `Platform` trait |
| `polyptyque-windows` | Windows implementation of `Platform` (capture, windows, browsers, pipes, notification area icon, command pipe); compiled on Windows only |
| `polyptyque-engine` | Recording sessions: shared clock, capture threads, FFmpeg encoders, MP4 conversion and repair |
| `polyptyque-app` | Slint interface (`ui/`) and its controller (`src/controller.rs` and `src/controller/`), demo mode outside Windows, translations (`lang/`) |
| `polyptyque-cli` | `polyptyque-cli.exe`: JSON command line and MCP server over the command channel |

## Build and test

```sh
cargo test -p polyptyque-core -p polyptyque-app -p polyptyque-cli   # works on any system
cargo test --workspace                                               # also simulated sessions (FFmpeg in PATH)
cargo clippy --workspace
cargo run -p polyptyque-app                                          # outside Windows: demo mode, fake windows
cargo check --target x86_64-pc-windows-msvc -p polyptyque-cli        # Windows code from macOS/Linux
```

- `polyptyque-app` only builds for Windows on Windows (C dependencies). The GitHub workflow builds the Windows installer on every push but runs **no tests**: run them locally.
- Demo mode keeps its data in the temporary folder (`polyptyque-demo`). `POLYPTYQUE_LANG=fr|en` forces the language. Its command channel is a Unix socket, so `polyptyque-cli` works against it.
- Don’t reformat whole files: `cargo fmt --check` reports a few pre-existing differences.

## Text and languages

- **French is the source language** of every user-facing text: `tr!("…")` in Rust, `@tr("…")` in Slint. English lives in `crates/polyptyque-app/lang/en/LC_MESSAGES/polyptyque-app.po`, shared by the app and `polyptyque-cli`.
- After adding or changing texts: `POLYPTYQUE_CATALOGUE=maj cargo test -p polyptyque-app --test catalogue`, then write the English. The test fails while any text has no English or different `{…}` parameters.
- English is written natively, following [`docs/lexique-anglais.md`](docs/lexique-anglais.md): sentence case, contractions, curly quotes, US spelling, “scheduled recording”, “time slot”, “app source”, “remote control”. French: impersonal tone (no « tu », no gendered wording; « vous » only when needed).
- Logs stay in French and are never translated. Never write window titles to the logs.
- Settings keys and on-disk folder names (`données`, `profils`, `journaux`) are never renamed.

## Remote control

- Every command is described once, in `crates/polyptyque-core/src/control/catalog.rs` (descriptions through `tr!`, English given to models).
- The app executes them in `crates/polyptyque-app/src/controller/commands.rs`, through the same actions as the interface, with the same locks and confirmations (`confirm: true`).
- The command list of `docs/control.md` is generated: `POLYPTYQUE_DOC=maj cargo test -p polyptyque-cli doc`.
- Safeguards are part of the specification, not options: local only, off by default and only switchable in the window, mic only with the person’s approval in the window, a recording always visible, window titles withheld unless needed.

## Branches and releases

- Work on `dev`; `main` only receives merges from `dev`. A release is a `vX.Y.Z` tag on the merge commit of `main` (version in `Cargo.toml`, notes in `packaging/notes/vX.Y.Z.md`); the workflow then publishes it.
- Documentation-only commits carry `[skip ci]`, but never the commit that receives a release tag (the publication would be skipped too).
