# Polyptyque for Stream Deck · beta

[Français](README.md)

A Windows plugin in French and English, using Polyptyque’s visual identity. Requires **Polyptyque 1.0.0-beta.9 or later** and **Stream Deck 7.1 or later**. No extra account, API key or Internet connection is needed to use it.

## Install and connect

Package **0.1.0.2** fixes saving scene/source/schedule selections and the panel’s communication. Each change is confirmed by reading back Stream Deck’s settings; delayed state updates no longer revert a recent selection. Open the new package to update the installed plugin; action identifiers stay the same.

1. Open `city.vena.polyptyque.streamDeckPlugin` and accept installation in Stream Deck.
2. Open Polyptyque. In **Settings**, enable **Allow control by other programs**.
3. Find the **Polyptyque** category in Stream Deck and drag an action onto a key. The panel shows **Connected to Polyptyque** once it receives live state.
4. Choose a target for Source, Scene or Scheduled recording. For Recording or Previews, choose **Toggle**, **Enable / start** or **Pause / stop**.

**Setting saved in Stream Deck** confirms your choice. Choosing a scene configures the key; press the key to activate the scene in Polyptyque.

The plugin first detects the running copy of Polyptyque (including portable copies), then the standard installation. The panel confirms **Polyptyque detected automatically**; no path is needed for a detected copy. If a portable copy isn’t detected, expand **Connection › Manual path (optional)**, enter the full path to its `polyptyque-cli.exe` (in `current`) and select **Apply to all keys**. Leave this field blank for automatic detection. There are no configuration files to edit. Language follows Stream Deck, with a French/English override per key.

## Actions and feedback

| Action | On press | Live feedback |
|---|---|---|
| Recording | Start/stop the active scene | Session duration; red while recording |
| Source | Start/stop one source in the active scene | Session duration; yellow for a lost/minimized window |
| Scene | Switch to the chosen scene | Name; green when active |
| Windows | Open the active scene’s windows, or open/restore one source | Windows found/total, or source window status |
| Previews | Turn on/off | Actual state; green when on |
| Scheduled recording | Enable/pause the chosen schedule | Scene, time, actual state |
| Disk space | No command | Free space and estimated time; yellow below one hour |

A Source key shows the **session’s** duration, even if the source joined later. Each schedule key controls one scheduled recording. Pausing a schedule doesn’t stop an existing recording; Polyptyque may refuse changes while it locks the schedule. Start-only keys do nothing if recording is already active; use a source key to add another source.

Stopping a recording owned by a schedule first shows **Confirm / Press again**. Press the key again within **5 seconds**; the full question also appears in the panel. A disconnect, scene change or settings change cancels confirmation. Polyptyque still enforces its locks and mic approval.

Keys explicitly go offline if Polyptyque closes or remote control is disabled. Interrupted commands are never retried automatically: check the state after reconnecting before pressing again. Scene/source/schedule lists refresh every four seconds; recording/window state comes from local notifications. Deleted targets stay marked as missing and aren’t silently replaced.

Global shortcuts also work with Stream Deck’s **Hotkey** action, without state feedback: Ctrl+Shift+F9 (recording), Ctrl+Shift+F10 (Polyptyque window), Ctrl+Shift+F11 (previews), unless customized in Polyptyque.

## Build and verify

From this directory with Node.js 24 or later:

```powershell
npm ci
npm test
npm run test:ui
npm run pack
```

Development iterations and user testing use **local builds**, without waiting for GitHub. The GitHub workflow runs only for release tags (`v*`) or a manual dispatch; branch pushes and pull requests don’t trigger builds.

`test:ui` uses an installed Chrome browser in headless mode. The publication workflow installs Chromium through Playwright (`POLYPTYQUE_TEST_BROWSER=chromium`) and also runs these tests. The test host authenticates outgoing messages with the inspector UUID and replies with the action context, matching the native host. Tests cover persisted scene/source/schedule choices after reopening, stale notifications and panel disconnection/recovery. The package and previews go into `../livraison/`. The `.sdPlugin` directory is generated and ignored by Git. Install the **package**, not a symlink. Increment the four-part manifest version in `tools/build.mjs` for every new installable package; Stream Deck may ignore a package with the same version. Plugin and app versions are independent.

The tests cover action state transitions, confirmation expiry/context, CLI errors, disconnects, SDK translation lookup and the built plugin’s communication with a simulated Stream Deck host. Panel tests cover French/English, deleted targets, disabled remote control and CLI path changes without reloading. Physical key presses and feedback from a real capture session still need user testing.

The plugin uses `polyptyque-cli events` for notifications and `polyptyque-cli call` for commands, without a shell or visible console. The CLI checks ownership of the Windows command pipe. There’s no HTTP server, no editing of Polyptyque’s files and no bypassing confirmations. WebSocket connections are only the ones required by Stream Deck for plugins and property inspectors. Settings are stored by Stream Deck; distributed assets stay immutable.

## Distribution

The initial deliverable is an installable package for testing. GitHub publication and an **Elgato Marketplace** submission will be decided after validation. Building the package doesn’t submit it to the Marketplace. GPL-3.0-or-later, like Polyptyque, with bundled dependency licenses.
