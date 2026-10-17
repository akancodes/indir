# Changes

## v0.1.0 (2026-10-17)

The first release.

- Downloads a file in parts over several connections at once: 8 by default, up to 16.
- A connection that finishes early takes over half of the biggest part left; one that goes quiet or crawls is
  taken over whole.
- Notices servers that punish extra connections or refuse them, and settles on what's fastest.
- Pauses and resumes, even after quitting or a restart, and starts over if the file on the server changed.
- Opens a dropped connection again where it stopped.
- Takes several links at once, or a link dragged onto the window.
- Never overwrites a file: a second `report.pdf` becomes `report (1).pdf`.
- Light and dark, following the system; progress on the taskbar button or the Dock icon.
- For Windows 10 and 11 (installer or portable), macOS 11 and later (Apple silicon and Intel), and Linux
  (AppImage, .deb and .rpm).
