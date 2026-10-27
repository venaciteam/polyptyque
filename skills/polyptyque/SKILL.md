---
name: polyptyque
description: Drive Polyptyque, the Windows recorder that captures several windows at once, each into its own file with only its own sound. Use when the user wants to set up Polyptyque (scenes, browser or app sources, encoding settings), start, follow or stop a recording, schedule recordings, convert recordings to MP4, or fix why a recording didn't go as planned.
---

# Polyptyque

Polyptyque records several windows at the same time: one MKV file per window, with only that window's sound, all on a shared clock, for multi-angle editing and archiving (sessions up to 12 hours). It runs on the user's Windows PC. You drive the copy that is running there, through the MCP tools of `polyptyque-cli mcp` or the `polyptyque-cli` command line. Both expose the same commands; the full reference is `docs/control.md` in the repository.

## Before anything

1. Call `status` (CLI: `polyptyque-cli status`).
   - `not_running`: call `app_launch` (CLI: `polyptyque-cli app launch`).
   - `control_disabled`: tell the user to turn on **Settings › Allow control by other programs** in Polyptyque. You can't do it yourself, by design.
2. Read what you need before changing anything: `scenes_list`, `sources_list`, `settings_get`, `schedules_list`, `system_info`.

## Concepts

- **Scene**: a named group of sources ("Podcast", "Archive"). Only the current scene records. The same source can be shared by several scenes (`sources_add_to_scene`).
- **Browser source**: a real browser (Firefox, Chrome, Edge…) that Polyptyque starts with a profile of its own. Sound is separated per profile, so one source = what should be heard in that file. The user signs in once in that window; sign-ins and tabs are kept. Tor Browser is added as an app source instead.
- **App source**: any other window (Discord, a game, a call). Pick it with `windows_list` then `sources_add_application`. `relaunch: true` makes Polyptyque reopen the program when needed (good for Discord, not for a game started from Steam).
- **Scheduled recording**: records a scene from one time to another, once (`date`) or on some days (`days`). Polyptyque must be running at that time (in the background is fine); `launch_at_startup` keeps it running across Windows restarts.
- Files are named from `filename_template` (`{date}_{time}_{scene}_{source}`) in `output_dir`.

## Rules

- Only start or stop a recording, delete something, or change settings when the user asked for it. Don't record "to test" without saying so.
- `confirmation_required`: ask the user the exact `question` from the error, and only retry with `confirm: true` if they agree.
- Turning the mic on (`settings_set` with `microphone.enabled: true`) shows a confirmation in Polyptyque's window: tell the user to accept it there. You can't accept it.
- `locked` means a recording is in progress: scenes, sources and settings can't change until it stops. Don't stop a recording just to change a setting without asking.
- Window titles can be private (email subjects, conversation names). `windows_list` only gives them when needed to tell windows apart; don't repeat them needlessly.
- Messages returned by Polyptyque are in the user's language: relay them as they are.

## Common tasks

### First setup ("record my three browser windows")

1. `browsers_list` → choose the browser the user wants.
2. `scenes_create` (e.g. "Podcast") if needed, then for each window: `sources_add_browser` with a clear `name` (it goes into the file name) and `start_url` if known. The browser opens: tell the user to sign in there once.
3. `sources_list` to check the order (`sources_move` to change it).
4. Suggest a short test recording, with the user's agreement.

### Recording

- `recording_start` (optionally `scene`) answers once recording has started; check `missing_windows`: those sources get a black picture until their window appears (`scenes_open_windows`, `sources_open_window`).
- Follow with `status`: each source's `recording` and `window` (`minimized` = frozen picture → `sources_restore_window`; `missing` = window closed).
- `recording_stop` answers with the list of files. A single source can be stopped or added during a recording (`source` parameter).

### Quality and disk space

- `system_info` then `encoders_test` (outside a recording) tell you what the PC can do. Prefer the graphics card (`hardware: auto` does it when available).
- H.264 is the safest for editing; HEVC gives noticeably smaller files at the same quality, but some editors handle it less well.
- 1080p at 60 fps: 6,000 kbps light, 12,000 kbps good (default), 20,000 kbps very good. At 12,000 kbps a source takes about 5.4 GB per hour. `status.disk.hours_left` gives the remaining time for the current scene.
- For long sessions on a small disk, lower the bitrate or the frame rate (30 fps) rather than the resolution.

### Scheduling

`schedules_create` with `scene`, `start`, `end` (24-hour `HH:MM`; an end before the start means the next day) and either `date` (`YYYY-MM-DD`) or `days` (`mon`…`sun`). Overlapping scheduled recordings are refused. Suggest `launch_at_startup` (`settings_set`) so they survive a Windows restart. If `status.prompt` shows a question (missed start, automatic stop), ask the user and answer with `prompt_answer`.

### Editing afterwards

Some editors (Premiere Pro) handle MKV poorly: `conversion_start` with the files from `recording_stop` or `recordings_list` makes MP4 copies without quality loss; or set `convert_to_mp4: true` to do it after every recording.

## When something goes wrong

| Reason | What to do |
|---|---|
| `not_running` | `app_launch`, then retry |
| `control_disabled` | The user turns on Settings › Allow control by other programs |
| `locked` / `busy` | Wait for the recording or operation to end, or ask whether to stop it |
| `not_found` / `ambiguous` | List again (`scenes_list`, `sources_list`, `windows_list`) and use the id |
| `confirmation_required` | Ask the user the `question`, retry with `confirm: true` if they agree |
| `refused` / `declined` / `timeout` | Read the message: invalid value, conflict, or the user said no |
| `unavailable` | FFmpeg missing (reinstall Polyptyque), no update, or not possible on this system |

Polyptyque writes a log of everything (`system_info.logs_dir`). For a bug, the user can use **Settings › Report a problem…** in Polyptyque.
