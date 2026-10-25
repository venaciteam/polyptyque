# Polyptyque control reference

Polyptyque can be driven by other programs on the same computer: an AI assistant (through the MCP server or the command line), a script, or a Stream Deck plugin. They can do everything the window does: set Polyptyque up, prepare scenes and sources, start and follow a recording.

This page is the reference for the command channel, the `polyptyque-cli` command line and its MCP server. The design and its safeguards are described in French in the [specification](cahier-des-charges.md#pilotage-par-dautres-programmes).

## Contents

1. [Turning it on](#turning-it-on)
2. [Safeguards](#safeguards)
3. [Command line](#command-line)
4. [MCP server](#mcp-server)
5. [Command channel](#command-channel)
6. [Errors and exit codes](#errors-and-exit-codes)
7. [Status object](#status-object)
8. [Commands](#commands)

## Turning it on

Control is **off by default**. In Polyptyque: **Settings › Allow control by other programs**. Only the window can turn it on or off; no command can. While it's off, the command channel doesn't exist.

Polyptyque must be running (window shown or in the background, next to the clock). If it's closed, `polyptyque-cli app launch` (MCP tool `app_launch`) starts it in the background and waits until it answers.

`polyptyque-cli.exe` is installed next to `polyptyque.exe`:

| Version | Path |
|---|---|
| Installed | `%LOCALAPPDATA%\PolyptyqueApp\current\polyptyque-cli.exe` |
| Portable | `<Polyptyque folder>\current\polyptyque-cli.exe` |

## Safeguards

- **Local only.** Windows named pipe `\\.\pipe\polyptyque-<account SID>`. Its permissions grant access to the Windows account that started Polyptyque and to nobody else (the default permissions of a named pipe would let every account read it). Connections from another computer are rejected by Windows (`PIPE_REJECT_REMOTE_CLIENTS`). Polyptyque insists on creating the pipe first, and clients check that the pipe belongs to their own account before sending anything.
- **No local web server.** A web page can't open a named pipe, unlike a local port.
- **Microphone.** Turning the mic on through the channel opens a confirmation in Polyptyque's window; only the person in front of the screen can accept it (two minutes, then it's declined).
- **A recording is always visible.** In the background, the icon next to the clock gets a red dot during any recording and Windows shows a notification when one starts; the start message names the program that asked for it; no command can hide the window.
- **Who's connected.** A “Remote control · <program>” pill sits in the top bar while a program is connected.
- **Private window titles.** `windows.list` gives the program of each open window, and its title only when several windows of the same program are open.
- **Same locks as the window.** Nothing about scenes, sources or settings changes during a recording (`locked`). Anything the window asks to confirm needs `confirm: true` (`confirmation_required` otherwise, with the question to ask the person).
- **Log.** Every command received is written to Polyptyque’s log with the program that sent it, never window titles.

## Command line

```text
polyptyque-cli <command> [--parameter value…] [--pretty]
polyptyque-cli help [<command>]
polyptyque-cli call <method> '<json>'
polyptyque-cli commands
polyptyque-cli events
polyptyque-cli app launch
polyptyque-cli mcp
```

- `<command>` is the method name, with a space, a dot or an underscore: `recording start`, `recording.start`, `recording_start`. Hyphens and underscores are interchangeable (`scenes open-windows`).
- Parameters: `--name value` or `--name=value`. A boolean alone means `true` (`--confirm`), or give `true`/`false`. Lists with fixed values are comma-separated (`--days mon,fri`); other lists repeat the option (`--files a.mkv --files b.mkv`). Object fields use their path (`--video.bitrate-kbps 8000`). `--json '{…}'` gives several parameters at once.
- Output: one JSON document on standard output, errors included (`{"error": {"reason": …, "message": …}}`). Non-ASCII characters are written as `\uXXXX` escapes, so the output survives any console code page (PowerShell reads a program’s output in the console code page, not UTF-8). `--pretty` indents it.
- Messages are in Polyptyque’s language (French or English, per its Language setting); command names, parameter names and JSON keys are always English and stable.
- `commands` prints the full catalog as JSON (English descriptions, parameter types and allowed values).
- `events` subscribes to the live status and prints one JSON line per notification (`{"event": "state" | "message", "data": …}`) until the connection is lost.

Examples:

```powershell
polyptyque-cli status --pretty
polyptyque-cli sources add_browser --browser Firefox --name "Guest 1" --start-url https://example.org
polyptyque-cli recording start --scene Podcast
polyptyque-cli schedules create --scene Podcast --start 21:00 --end 22:30 --days mon,wed,fri
polyptyque-cli settings set --video.bitrate-kbps 8000 --video.codec hevc
polyptyque-cli scenes delete --scene Archive --confirm
```

## MCP server

`polyptyque-cli mcp` is an MCP server on standard input and output. Its tools are the commands of the catalog with `_` instead of `.` (`recording_start`), plus `app_launch`; `hello`, `subscribe` and `unsubscribe` are channel-only. It serves both protocol eras: modern stateless requests (`2026-07-28`, with `_meta` on every request, `server/discover`) and the earlier `initialize` handshake (`2025-11-25`, `2025-06-18`, `2025-03-26`, `2024-11-05`). Tool results are the command’s JSON as text; failures come back with `isError: true` and the error object.

Claude Desktop (`claude_desktop_config.json`, *Settings › Developer › Edit Config*):

```json
{
  "mcpServers": {
    "polyptyque": {
      "command": "C:\\Users\\<you>\\AppData\\Local\\PolyptyqueApp\\current\\polyptyque-cli.exe",
      "args": ["mcp"]
    }
  }
}
```

Claude Code:

```powershell
claude mcp add polyptyque -- "$env:LOCALAPPDATA\PolyptyqueApp\current\polyptyque-cli.exe" mcp
```

A skill that teaches an assistant how to use Polyptyque well is in [`skills/polyptyque`](../skills/polyptyque/SKILL.md).

## Command channel

For programs that talk to Polyptyque directly (the Stream Deck plugin, scripts).

- **Windows:** named pipe `\\.\pipe\polyptyque-<SID>`, where `<SID>` is the account’s security identifier (`whoami /user`). Open it for reading and writing, preferably in overlapped mode.
- **Elsewhere** (demo mode, future ports): Unix socket `polyptyque-<user>/control.sock` in `$XDG_RUNTIME_DIR`, or the temporary folder.
- **Framing:** JSON-RPC 2.0, one UTF-8 JSON message per line (`\n`), 1 MiB at most. Parameters are named (an object). Several programs can be connected at the same time.
- **Introduction:** send `hello` with `{"client": "<name>"}` first; the name appears in Polyptyque’s top bar. The answer gives the version, the protocol version (`1`) and the language.
- **Live status:** after `subscribe`, Polyptyque sends `{"jsonrpc":"2.0","method":"state","params":<status>}` on every change (at most twice a second), and `{"method":"message","params":{"level":"info"|"warning"|"error","text":…}}` for every message it shows.
- **Long commands** (`recording.start`, `recording.stop`, `scenes.open_windows`, `encoders.test`, and `settings.set` when it turns the mic on) answer once the operation is over.
- **Messages:** a successful object result gets a `messages` array with what Polyptyque showed during the command.

```json
{"jsonrpc":"2.0","id":1,"method":"hello","params":{"client":"My script"}}
{"jsonrpc":"2.0","id":2,"method":"recording.start","params":{"scene":"Podcast"}}
```

## Errors and exit codes

Errors are JSON-RPC errors whose `data.reason` is stable. The command line writes them as `{"error": {"reason": …, "message": …, …details}}` and exits with the matching code.

| Exit code | Reason | Meaning |
|---|---|---|
| 0 | | Success |
| 1 | `internal` | Internal or unexpected error |
| 2 | `invalid_request` | Unknown command, missing, unknown or mistyped parameter |
| 3 | `not_running` | Polyptyque isn’t running (or the connection was lost) |
| 4 | `control_disabled` | Control is turned off in Polyptyque’s Settings |
| 5 | `locked`, `busy` | Not now: recording or operation in progress, window open in Polyptyque |
| 6 | `not_found`, `ambiguous` | Scene, source, scheduled recording, window or file not found, or a name that matches several |
| 7 | `confirmation_required` | Ask the person `details.question`, then run again with `confirm` |
| 8 | `refused`, `declined`, `timeout` | Invalid value or conflict, declined by the person, too late |
| 9 | `unavailable` | Not possible on this system or in this state (no FFmpeg, no update…) |

Details that may come with an error: `question` (`confirmation_required`), `ids` (`ambiguous`), `scenes`, `sources` or `browsers` (`not_found`), `choices` (invalid value).

## Status object

`status` returns it, and `state` notifications carry it.

```json
{
  "recording": { "active": true, "elapsed_seconds": 754, "scheduled_until": "2026-10-26T22:00:00+01:00", "microphone": false },
  "busy": null,
  "scene": { "id": "18e1…", "name": "Podcast" },
  "sources": [
    { "id": "18e1…", "name": "Discord", "kind": "application", "position": 1, "recording": "recording", "window": "found",
      "file_size_bytes": 5494374, "dropped_frames": 0, "file": "D:\\Videos\\2026-10-26_21h00_Podcast_Discord.mkv" }
  ],
  "disk": { "free_bytes": 412300000000, "hours_left": 49.3, "folder": "D:\\Videos" },
  "conversion": { "active": false, "total": 0, "finished": 0 },
  "next_schedule": { "schedule_id": "18e1…", "scene": { "id": "18e1…", "name": "Podcast" }, "starts_at": "…", "ends_at": "…" },
  "prompt": null,
  "previews": true,
  "microphone": false,
  "window": "visible",
  "update_available": null,
  "control_clients": 1
}
```

- `sources[].recording`: `off`, `starting`, `recording`, `failed`, `finishing`.
- `sources[].window`: `found`, `minimized` (frozen picture), `missing`, `searching`, `unknown`.
- `prompt`: a question shown by a scheduled recording, answered with `prompt.answer`: `kind` is `missed_start` (Polyptyque wasn’t running when a time slot began; without an answer it starts after a countdown), `auto_stop` (a recording started by hand during a time slot) or `info`.
- `window`: `visible`, `minimized` or `background` (icon next to the clock).
- Times are local, RFC 3339.

## Commands

Scenes, sources and scheduled recordings are referred to by their id or by their name (case doesn’t matter); a name shared by several gives `ambiguous`. Scheduled recordings are referred to by id. Positions start at 1.

<!-- Generated from the catalog (crates/polyptyque-core/src/control/catalog.rs): POLYPTYQUE_DOC=maj cargo test -p polyptyque-cli doc -->
<!-- commands:start -->
### `hello`

Introduction: Polyptyque and protocol versions, language. The client program’s name is shown in Polyptyque while it’s connected.

Command channel only (the command line and the MCP server send it themselves; `polyptyque-cli events` gives the live status).

| Parameter | Type | Required | Description |
|---|---|---|---|
| `client` | string |  | Name of the client program, shown in Polyptyque (“Claude”, “Stream Deck”). |

### `status`

Full status: recording in progress, scene, sources and their windows, disk, conversion, next scheduled recording, pending question.

Command line: `polyptyque-cli status` · MCP tool: `status`

No parameters.

### `subscribe`

Live status: Polyptyque then sends a “state” notification on every change, and “message” for every message it shows.

Command channel only (the command line and the MCP server send it themselves; `polyptyque-cli events` gives the live status).

No parameters.

### `unsubscribe`

Stops the live status.

Command channel only (the command line and the MCP server send it themselves; `polyptyque-cli events` gives the live status).

No parameters.

### `system.info`

System: Windows, graphics cards, FFmpeg, folders, free space. Useful before choosing encoding settings.

Command line: `polyptyque-cli system info` · MCP tool: `system_info`

No parameters.

### `encoders.test`

Tries each encoder (NVIDIA, AMD, Intel, CPU) in H.264 and HEVC on this computer. Can take a minute; not possible during a recording.

Command line: `polyptyque-cli encoders test` · MCP tool: `encoders_test`

No parameters.

### `window.show`

Brings Polyptyque’s window to the front (from the background or minimized).

Command line: `polyptyque-cli window show` · MCP tool: `window_show`

No parameters.

### `previews.set`

Turns the sources’ live previews on or off (recorded files aren’t affected).

Command line: `polyptyque-cli previews set` · MCP tool: `previews_set`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `enabled` | boolean | yes | true to turn on, false to turn off. |

### `scenes.list`

Scenes, in order, with their sources and the current scene. Only one scene is current: that’s the one that records.

Command line: `polyptyque-cli scenes list` · MCP tool: `scenes_list`

No parameters.

### `scenes.create`

Creates an empty scene.

Command line: `polyptyque-cli scenes create` · MCP tool: `scenes_create`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Scene name. |
| `activate` | boolean |  | Make it the current scene (yes by default, as in the window). |

### `scenes.rename`

Renames a scene.

Command line: `polyptyque-cli scenes rename` · MCP tool: `scenes_rename`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `scene` | string | yes | Scene: its id or its name. |
| `name` | string | yes | New name. |

### `scenes.duplicate`

Duplicates a scene: the copy uses the same sources (shared, not copied) and becomes the current scene. Its scheduled recordings aren’t copied.

Command line: `polyptyque-cli scenes duplicate` · MCP tool: `scenes_duplicate`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `scene` | string |  | Scene to duplicate (the current scene by default). |
| `name` | string |  | Name of the copy (by default: “<name> (copy)”). |

### `scenes.delete`

Deletes a scene, its scheduled recordings, and the sources that aren’t in any other scene (their browser profiles stay on disk). The last scene can’t be deleted.

Command line: `polyptyque-cli scenes delete` · MCP tool: `scenes_delete`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `scene` | string | yes | Scene: its id or its name. |
| `confirm` | boolean |  | true once the person agrees: without it, the command returns “confirmation_required” with the question to ask them. |

### `scenes.activate`

Changes the current scene (not possible during a recording).

Command line: `polyptyque-cli scenes activate` · MCP tool: `scenes_activate`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `scene` | string | yes | Scene: its id or its name. |

### `scenes.open_windows`

Opens the current scene’s missing windows: browsers, and apps with “Relaunch” turned on. Answers once the windows have appeared (30 seconds at most).

Command line: `polyptyque-cli scenes open_windows` · MCP tool: `scenes_open_windows`

No parameters.

### `sources.list`

A scene’s sources, in order, or all sources.

Command line: `polyptyque-cli sources list` · MCP tool: `sources_list`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `scene` | string |  | Scene (the current scene by default). |
| `all` | boolean |  | true: all sources, with the scenes each one is in. |

### `browsers.list`

Installed browsers, usable for a browser source.

Command line: `polyptyque-cli browsers list` · MCP tool: `browsers_list`

No parameters.

### `windows.list`

Open windows, for adding an app source. A window’s title is only given when several windows of the same program are open.

Command line: `polyptyque-cli windows list` · MCP tool: `windows_list`

No parameters.

### `sources.add_browser`

Adds a browser source: a browser started with its own profile (sign-ins and tabs are kept; sound is separated per profile, so one source = what should be heard). For Tor Browser, use an app source.

Command line: `polyptyque-cli sources add_browser` · MCP tool: `sources_add_browser`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `browser` | string | yes | Browser: its name (from browsers.list) or the path to its executable. |
| `name` | string |  | Source name, used in file names (by default: “Firefox 2”…). |
| `start_url` | string |  | Address opened the first time the profile starts. |
| `open` | boolean |  | Open the browser right away, to sign in (yes by default). |
| `scene` | string |  | Scene to add it to (the current scene by default). |

### `sources.add_application`

Adds an app source from an open window (Discord, a game, Tor Browser…).

Command line: `polyptyque-cli sources add_application` · MCP tool: `sources_add_application`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `window` | string | yes | Window: its id from windows.list. |
| `name` | string |  | Source name (by default: the program’s name). |
| `relaunch` | boolean |  | Relaunch the program if it isn’t running (off by default; useful for Discord, not for a game started from Steam). |
| `capture_cursor` | boolean |  | Show the mouse cursor in the picture (yes by default). |
| `scene` | string |  | Scene to add it to (the current scene by default). |

### `sources.add_to_scene`

Adds an existing source to another scene: it’s the same source, shared (same window, same profile), not a copy. Never twice in one scene.

Command line: `polyptyque-cli sources add_to_scene` · MCP tool: `sources_add_to_scene`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `source` | string | yes | Source: its id or its name. |
| `scene` | string | yes | Scene to add it to. |

### `sources.remove`

Removes a source from a scene. Removed from its last scene, it’s gone (its browser profile stays on disk).

Command line: `polyptyque-cli sources remove` · MCP tool: `sources_remove`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `source` | string | yes | Source: its id or its name. |
| `scene` | string |  | Scene (the current scene by default). |
| `confirm` | boolean |  | true once the person agrees: without it, the command returns “confirmation_required” with the question to ask them. |

### `sources.move`

Changes a source’s position in its scene (same order in the list and in the preview grid).

Command line: `polyptyque-cli sources move` · MCP tool: `sources_move`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `source` | string | yes | Source: its id or its name. |
| `position` | integer | yes | New position, 1 for the first. |
| `scene` | string |  | Scene (the current scene by default). |

### `sources.open_window`

Opens the window of a source in the current scene (browser with its profile, or program).

Command line: `polyptyque-cli sources open_window` · MCP tool: `sources_open_window`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `source` | string | yes | Source: its id or its name. |

### `sources.restore_window`

Brings back the minimized window of a source in the current scene (a minimized window gives a frozen picture).

Command line: `polyptyque-cli sources restore_window` · MCP tool: `sources_restore_window`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `source` | string | yes | Source: its id or its name. |

### `recording.start`

Starts recording all sources of the current scene, or just one (it joins the recording in progress, if any). One file per source. Answers once the recording has started.

Command line: `polyptyque-cli recording start` · MCP tool: `recording_start`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `scene` | string |  | Scene to record: it becomes the current scene (the current scene by default). |
| `source` | string |  | A single source of the scene: its id or its name. |
| `auto_stop` | boolean |  | During a scheduled time slot: stop automatically when the time slot ends (off by default). |

### `recording.stop`

Stops the recording (all sources), or a single source. Answers once the files are finalized, with their list.

Command line: `polyptyque-cli recording stop` · MCP tool: `recording_stop`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `source` | string |  | A single source: its id or its name. |
| `confirm` | boolean |  | true once the person agrees: without it, the command returns “confirmation_required” with the question to ask them. |

### `prompt.answer`

Answers the question shown by a scheduled recording (see “prompt” in status): missed start, or automatic stop of a recording started during a time slot.

Command line: `polyptyque-cli prompt answer` · MCP tool: `prompt_answer`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `accept` | boolean | yes | true: the main button (record now, stop at the end of the time slot); false: the other one. |

### `settings.get`

Settings, with the values the Settings window offers (resolutions, frame rates, shortcuts…).

Command line: `polyptyque-cli settings get` · MCP tool: `settings_get`

No parameters.

### `settings.set`

Changes settings: only the given fields change. Not possible during a recording, or while the Settings window is open. Turning on the mic needs the person’s approval in Polyptyque.

Command line: `polyptyque-cli settings set` · MCP tool: `settings_set`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `output_dir` | string |  | Recordings folder (full path). |
| `filename_template` | string |  | File name pattern, with the variables {date}, {time}, {scene}, {source} (or {heure}, {scène}). |
| `previews` | boolean |  | Live previews of the sources in the window. |
| `convert_to_mp4` | boolean |  | Automatic MP4 conversion at the end of each recording (the MKV is kept). |
| `updates` | `stable` \| `beta` \| `off` |  | Updates offered: stable, stable and beta, or don’t check. |
| `open_windows_at_launch` | boolean |  | Open the scene’s windows when Polyptyque starts. |
| `close_browsers_at_exit` | boolean |  | Close the sources’ browsers when quitting Polyptyque. |
| `launch_at_startup` | boolean |  | Start Polyptyque when Windows starts (in the background), so scheduled recordings survive a restart. |
| `close_with_schedules` | `ask` \| `background` \| `quit` |  | Closing the window while a scheduled recording is coming up: ask, keep running in the background, or quit. |
| `language` | `auto` \| `french` \| `english` |  | Polyptyque’s language. |
| `clock` | `auto` \| `24h` \| `12h` |  | Clock used for displayed and entered times. |
| `video` | object |  | Video encoding, shared by all sources. |
| `video.width` | integer |  | Width in pixels (even). 1920 for 1080p. |
| `video.height` | integer |  | Height in pixels (even). 1080 for 1080p. |
| `video.fps` | integer |  | Frame rate (1 to 240; usually 30 or 60). |
| `video.bitrate_kbps` | integer |  | Video bitrate in kbps (250 to 500,000). At 1080p and 60 fps: 6,000 light, 12,000 good, 20,000 very good. |
| `video.codec` | `h264` \| `hevc` |  | Video format: H.264 (best compatibility for editing) or HEVC (smaller files). |
| `video.hardware` | `auto` \| `gpu` \| `cpu` |  | Encoding hardware: automatic (graphics card if available), graphics card, or CPU. |
| `audio` | object |  | Sources’ sound. |
| `audio.bitrate_kbps` | integer |  | Audio bitrate in kbps (64 to 512; 192 by default). |
| `microphone` | object |  | Mic, recorded to a separate file. |
| `microphone.enabled` | boolean |  | Turn on the mic: the person must accept it in Polyptyque’s window (the command waits for their answer, two minutes at most). |
| `microphone.device_id` | string |  | Chosen mic (id from microphones.list); empty for the Windows default mic. |
| `hotkeys` | object |  | Global keyboard shortcuts, written like “Ctrl+Shift+F9”; empty for none. |
| `hotkeys.toggle_recording` | string |  | Start / stop all. |
| `hotkeys.show_window` | string |  | Show / minimize Polyptyque. |
| `hotkeys.toggle_previews` | string |  | Turn previews on / off. |

### `microphones.list`

Available mics, for the microphone.device_id setting.

Command line: `polyptyque-cli microphones list` · MCP tool: `microphones_list`

No parameters.

### `schedules.list`

Scheduled recordings: each one records a scene from one time to another, once or on some days, with its next time slot. Polyptyque must be running (in the background if need be).

Command line: `polyptyque-cli schedules list` · MCP tool: `schedules_list`

No parameters.

### `schedules.create`

Creates a scheduled recording. An end before the start means the next day (22:00 → 02:00). Two scheduled recordings that are turned on can’t overlap.

Command line: `polyptyque-cli schedules create` · MCP tool: `schedules_create`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `scene` | string | yes | Scene to record: its id or its name. |
| `start` | string | yes | Start time, “HH:MM”, 24-hour. |
| `end` | string | yes | End time, “HH:MM”, 24-hour. |
| `date` | string |  | Once: start date, “YYYY-MM-DD”. Give date or days. |
| `days` | list of `mon`, `tue`, `wed`, `thu`, `fri`, `sat`, `sun` |  | Every week: days the time slot starts (all seven: every day). Give date or days. |
| `enabled` | boolean |  | Turned on (yes by default). |

### `schedules.update`

Edits a scheduled recording: only the given fields change (date and days replace each other). Not possible while it’s recording.

Command line: `polyptyque-cli schedules update` · MCP tool: `schedules_update`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `schedule` | string | yes | Scheduled recording id (from schedules.list). |
| `scene` | string |  | Scene to record: its id or its name. |
| `start` | string |  | Start time, “HH:MM”, 24-hour. |
| `end` | string |  | End time, “HH:MM”, 24-hour. |
| `date` | string |  | Once: start date, “YYYY-MM-DD”. |
| `days` | list of `mon`, `tue`, `wed`, `thu`, `fri`, `sat`, `sun` |  | Every week: days the time slot starts. |

### `schedules.enable`

Turns a scheduled recording on or off.

Command line: `polyptyque-cli schedules enable` · MCP tool: `schedules_enable`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `schedule` | string | yes | Scheduled recording id. |
| `enabled` | boolean | yes | true to turn it on, false to turn it off. |

### `schedules.delete`

Deletes a scheduled recording (not possible while it’s recording).

Command line: `polyptyque-cli schedules delete` · MCP tool: `schedules_delete`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `schedule` | string | yes | Scheduled recording id. |

### `recordings.list`

Latest files in the recordings folder (MKV, MKA, MP4, M4A), newest first.

Command line: `polyptyque-cli recordings list` · MCP tool: `recordings_list`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `limit` | integer |  | Maximum number of files (50 by default). |

### `recordings.open_folder`

Opens the recordings folder in File Explorer.

Command line: `polyptyque-cli recordings open_folder` · MCP tool: `recordings_open_folder`

No parameters.

### `conversion.start`

Converts MKV (or MKA) files to MP4 (or M4A) for editing, losslessly and without re-encoding, in the background, one file at a time. The MKV is kept. Progress in status.

Command line: `polyptyque-cli conversion start` · MCP tool: `conversion_start`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `files` | list of strings | yes | Full paths of the files to convert. |

### `conversion.cancel`

Cancels current and queued conversions (partial files are deleted, the MKVs stay).

Command line: `polyptyque-cli conversion cancel` · MCP tool: `conversion_cancel`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `confirm` | boolean |  | true once the person agrees: without it, the command returns “confirmation_required” with the question to ask them. |

### `update.install`

Downloads the new version on offer (see “update_available” in status), then Polyptyque restarts to install it. Never during a recording.

Command line: `polyptyque-cli update install` · MCP tool: `update_install`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `confirm` | boolean |  | true once the person agrees: without it, the command returns “confirmation_required” with the question to ask them. |

### `app.quit`

Quits Polyptyque (a recording in progress is stopped cleanly). Scheduled recordings don’t start while it’s closed.

Command line: `polyptyque-cli app quit` · MCP tool: `app_quit`

| Parameter | Type | Required | Description |
|---|---|---|---|
| `confirm` | boolean |  | true once the person agrees: without it, the command returns “confirmation_required” with the question to ask them. |

<!-- commands:end -->
