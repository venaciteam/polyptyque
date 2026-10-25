POLYPTYQUE 1.0 BETA 4
=====================

Polyptyque records several windows at once, each in its own file, with
only that window's sound.

(En français : LISEZMOI.txt)

Installation
------------
Polyptyque-Setup.exe installs Polyptyque with no questions and no
administrator rights (shortcuts on the desktop and in the Start menu).
New versions are offered in one click inside Polyptyque (Settings >
Updates: stable versions, or stable and beta versions). Uninstalling
(Windows Settings > Apps) keeps scenes, settings and browser sign-ins.

Portable version
----------------
Polyptyque-portable.zip runs without installation: unzip it anywhere
(drive, USB stick), then run Polyptyque.exe at the root of the folder
(not Update.exe). Scenes, settings, browser sign-ins and logs are stored
in the "données" subfolder (French for "data"): moving the folder, or
copying it to another PC, takes everything with it. Recordings go to the
folder chosen in Settings (Videos\Polyptyque if that folder doesn't exist
on this PC). Settings show "portable" next to the version number.

Contents
--------
- polyptyque.exe         : Polyptyque, with its interface
- polyptyque-console.exe : the console version, for diagnostics (same
                           settings, same scenes; in French only for now)
- ffmpeg.exe             : the video encoder Polyptyque uses (must stay
                           next to it)
- README.txt             : this file (LISEZMOI.txt: in French)
- LICENSE.txt            : Polyptyque's license (GPL v3)
- FFMPEG-LICENSE.txt     : FFmpeg's license

Requirements
------------
- Windows 10 version 2004 or later, or Windows 11 (64-bit).
- NVIDIA graphics card: driver 570 or later. AMD and Intel cards, or CPU
  encoding, work too.

Language
--------
Polyptyque uses French if Windows is in French, and English otherwise;
times follow the Windows regional format (12-hour or 24-hour). Change
them in Settings > "Language" and "Clock".

First launch
------------
Windows may show "Windows protected your PC": the program isn't digitally
signed yet. Click "More info", then "Run anyway".

Using Polyptyque
----------------
1. Run polyptyque.exe.
2. Sources > "Browser": pick the browser, a name, an address. Polyptyque
   opens this browser with a profile of its own: sign in and set up your
   tabs. Sign-ins and tabs are kept. Any sound played in this profile is
   recorded in the source's file.
3. Sources > "App": pick an open window (Discord, a game, Tor Browser…).
4. At launch, Polyptyque opens the current scene's windows by itself
   (browsers, and apps set to relaunch); the "Open windows" button does it
   on demand. This can be changed in Settings.
5. "Start recording". Polyptyque finds or reopens the windows, then shows
   the status of each source. "Stop all" finalizes the files.
6. The round button on each preview starts or stops that source alone,
   even during a recording (a source added mid-recording starts a new
   file).

Keyboard shortcuts (customizable in Settings, active even when Polyptyque
is in the background):
- Ctrl+Shift+F9  : start or stop recording
- Ctrl+Shift+F10 : show or minimize Polyptyque
- Ctrl+Shift+F11 : turn previews on or off

Files are in the Videos\Polyptyque folder (can be changed in Settings),
one MKV file per source.

Editing (Premiere Pro…)
-----------------------
"Convert to MP4…" creates a lossless MP4 copy next to each MKV chosen (the
MKV is kept). Settings can also convert automatically at the end of each
recording. The copy doubles the disk space used.

Mic
---
Off by default. In Settings: "Record the mic in a separate file", then
choose the mic. Its sound is recorded in "…_Mic.mka" (converted to .m4a
for editing). Windows has to allow desktop apps to access the microphone:
Settings > Privacy & security > Microphone.

Scheduled recordings
--------------------
The "Schedule" button at the top opens scheduled recordings: a scene, a
start and end time (the next day if the end is before the start), once or
on specific days of the week. Polyptyque has to stay open: while a
scheduled recording is active, the computer and screen don't go to sleep
on their own. The "Start Polyptyque when Windows starts" option brings it
back (minimized) after a restart.
- Polyptyque closed or computer asleep at the set time: when it's back
  during the time slot, it asks whether to start recording, and starts on
  its own after two minutes without an answer.
- A scheduled recording only stops what it started. A recording started
  by hand during a time slot is only stopped at the end time if you
  agree; stopping a scheduled recording by hand asks for confirmation.

Good to know
------------
- Windows stops drawing a window minimized to the taskbar (frozen
  picture): the "Restore window" button on its card brings it back, and
  moves it to the main screen if its screen was unplugged. A window hidden
  behind another one records normally.
- If a window closes during a recording, the picture turns black and the
  source is reattached automatically as soon as the window is back. The
  "Reopen window" button relaunches the program; a browser (or an app set
  to relaunch) reopens on its own after 20 s.
- If the recordings folder becomes unreachable (external drive
  unplugged), the rest is saved to Videos\Polyptyque.
- Automatic sleep is blocked during a recording. Sleep that's requested
  (closing the lid, power button, menu) can't be: the recording is then
  stopped cleanly, and Polyptyque offers to resume it on wake.
- Long sessions: set the "active hours" of Windows Update to the time
  range when you record, so that it doesn't restart the computer.
- Closing Polyptyque during a recording asks for confirmation, then
  finalizes the files before quitting.
- Shutting down or restarting Windows during a recording: Windows shows
  that Polyptyque is recording and lets you cancel. If the shutdown is
  forced, the files are finalized as well as possible before closing.
- If the encoder stops mid-recording, Polyptyque restarts it on its own:
  the rest is in a "…_part2.mkv" file (about one second lost in between).
- After a crash or power outage during a recording, Polyptyque checks the
  files at the next launch. It offers to repair those that were cut off
  (losslessly) and to resume recording.
- Only one Polyptyque can be open at a time: launching it again brings
  back the window that's already open.

If something goes wrong
-----------------------
Settings > "Report a problem…" prepares a report (user folder and account
name masked) and opens the GitHub page to attach it to. Nothing is sent
automatically.
Logs are in:
  %LOCALAPPDATA%\Polyptyque\journaux
(paste this path into the File Explorer address bar), or in
données\journaux for the portable version.
Each recording has its own "session_…" folder, with one log per source.
Logs are written in French.
