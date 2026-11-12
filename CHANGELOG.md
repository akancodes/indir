# Changes

## Unreleased

- **A new look.** Each download is a card that fills up in its file's colour, with a big percentage. The
  buttons show when you point at a card; click it for the parts, its link and its folder.
- **A quieter status bar:** the speed and counts, *Pause all*, and one **⋯** menu with When done, Clear finished,
  Mini mode and Settings. When something will happen once the downloads are done, the menu's button wears a dot
  and the status line says what.
- **Uses the room it has.** A wider window shows the cards in two or three columns.
- **Remembers the window:** it opens where you left it, at the size you left it, and a bit larger the first time.
- **Mini mode** (Ctrl+M, ⌘M on a Mac): a small strip that stays on top and fills with every download together.
  Click it to come back, drag it anywhere. If the computer is about to sleep or shut down, the window comes back
  with the countdown.

## v0.2.0 (2026-10-18)

- **When done:** put the computer to sleep, shut it down or quit indir once nothing is left to download. A
  minute's countdown comes first, and you can cancel it; it says if a download stopped with a problem. On laptops
  with modern standby, sleep asks Windows for standby the way its Start menu does.
- While anything downloads, the computer doesn't fall asleep by itself. The screen may still turn off.
- **Downloads at once:** 3 to begin with; the others wait their turn and start by themselves. *Start now* lets one
  skip the line.
- **Speed limit** for every download together, from 1 to 20 MB a second. It applies at once, to running downloads
  too, and a connection waiting for its share is never taken for a stuck one.
- *Pause all* and *Resume all* in the status bar.
- A notification when a download finishes or stops, if indir isn't in front.
- A link copied in the browser is already in the field when you switch to indir.
- Right-click a download to copy its link, download it again, or show it in its folder.
- After a restart, nothing starts by itself: what was downloading or waiting is paused.

## v0.1.0 (2026-10-17)

The first release.

- Downloads a file in parts over several connections at once: 8 by default, up to 16.
- A connection that finishes early takes over half of the biggest part left; one that goes quiet or crawls is
  taken over whole.
- Notices servers that punish extra connections or refuse them, and settles on what's fastest.
- Pauses and resumes, even after quitting or a restart. Progress counts only bytes the system has confirmed are on
  disk, so a power cut can't leave holes in a file.
- Notices a file that changes on the server, during a download or between sessions, and starts over rather than
  mix two versions.
- Opens a dropped connection again where it stopped, and asks a link again when the signed address it led to
  expires, as GitHub's release downloads do.
- Takes several links at once, or a link dragged onto the window.
- Never overwrites a file: a second `report.pdf` becomes `report (1).pdf`. Names lose the invisible characters that
  could disguise one, like a `.exe` posing as a `.pdf`.
- Light and dark, following the system; progress on the taskbar button or the Dock icon.
- For Windows 10 and 11 (installer or portable), macOS 11 and later (Apple silicon and Intel), and Linux
  (AppImage, .deb and .rpm).
