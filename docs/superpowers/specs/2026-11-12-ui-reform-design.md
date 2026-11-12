# indir UI reform: filling cards, a quiet footer, a mini strip

Date: 2026-11-12. Branch `ui-reform`. Decisions are Bayram's unless marked *(builder's call)*.

## Why

indir works, but the window reads as plain and busy: every row carries two or three buttons, an eight-part
segmented bar and a monospace line of numbers, and the footer stacks five controls. The window opens small
(820x560) and wastes a wide screen. Bayram wants it bold and modern, calmer, and at home at any size.

## Goals

1. **Filling cards** (mockup C). Each download is a card; there is no separate progress bar: the card's own
   background fills from left to right in its file type's colour. Big percentage on the right; speed and time
   left under the name.
2. **Colour by file type.** Archives (zip, 7z, rar, tar, gz, xz) purple; disc images (iso, img, dmg) teal; video
   (mp4, mkv, mov, webm, avi) coral; audio (mp3, flac, m4a, wav, ogg) amber; apps and installers (exe, msi,
   AppImage, deb, rpm, pkg, apk) pink; documents (pdf, docx, xlsx, pptx, epub, txt) blue; anything else gray.
   The badge keeps today's short type label (ZIP, ISO, MP4…). *(Groups and hues: builder's call within this.)*
3. **States on the card.**
   - downloading: fill in the type colour, percentage, `9.8 MB/s · 7 s left`.
   - queued: fill in neutral gray, muted percentage, `Waiting its turn`.
   - paused: fill in the type colour at lower strength, `Paused`.
   - failed: a red outline and the problem in red text, `Try again` in its actions.
   - done: card fully filled, a check in place of the percentage, `210 MB · done 21:13`; per Bayram's playback
     the done fill is green.
   - unknown size (no Content-Length): no percentage; a slow moving sheen across the card and bytes so far.
4. **Less on screen.**
   - Row buttons only appear when the pointer is over the card or a control inside it has keyboard focus:
     round icon buttons at the right edge (pause/resume or start now, then remove; for done: open, show in
     folder, remove). Everything stays in the right-click menu as today.
   - The segmented strip and "N connections" leave the card. They live in a details area that opens when any
     card (done ones too) is clicked once, and shows the parts strip, connections, the link and the folder.
     A double click on a done card opens the file, as today; a single click never launches anything, so a stray
     click can't run an installer. *(Details-on-click is the builder's call; Bayram did not object at playback.)*
   - Footer: left, total speed and counts (`41.2 MB/s · 2 downloading · 1 waiting`); right, two round buttons:
     pause all / resume all, and `⋯`. The `⋯` menu holds When done…, Clear finished, Mini mode and Settings.
     While a When done action is set, the `⋯` button carries a small coloured dot and the summary line says
     it (`· then shut down`) so it is not forgotten. *(Dot plus summary text: builder's call.)*
   - The update pill stays in the footer when an update exists.
5. **Window.**
   - Opens larger: 1000x680 by default *(builder's call)*; min size stays 480x360.
   - Remembers its position and size between launches (and whether it was maximised), and falls back to the
     default if the saved place is off every screen.
   - Wide screens: as the window grows the list becomes 2, then 3 columns (a new column starts once each can be
     ~420 px; cap 3). Cards stretch to fill their column, so a maximised 34" ultrawide (~2300 CSS px) gets three
     ~750 px cards, never side margins or a narrow strip in the middle. The add bar and the footer span the whole
     window. Cards in a row share a height. *(Stretch rather than a 4th column or margins: builder's call, from
     his rule that a centred narrow box reads as cramped.)*
6. **Mini mode.** A small strip, always on top, about 320x90: the whole strip fills with the overall progress
   of active downloads (bytes done / bytes total across downloading + queued with known sizes), and shows the
   total speed, time left, and how many are running. No title bar; it can be dragged anywhere. Clicking it
   brings back the main window and hides the strip. Entered from the `⋯` menu (and a shortcut, Ctrl/Cmd+M
   *(builder's call)*). Its position is remembered too. With nothing downloading it reads `Nothing downloading`
   in neutral gray.
   - Progress never runs backwards *(builder's call)*: the strip counts a batch, from the moment something starts
     after an idle spell until nothing is downloading or waiting. Bytes of downloads finished during the batch
     stay counted; a download whose size is unknown adds nothing to the total until its size is known.
   - The When done countdown and problems are never hidden *(builder's call, for safety)*: when the countdown
     starts, the main window comes back and the strip hides, so the countdown runs exactly as today with its
     Cancel in reach. A download that stops with a problem turns the strip's outline red; clicking it brings the
     main window back as usual.
   - Linux under Wayland *(builder's call, an accepted limitation)*: Wayland doesn't let an app place its own
     window or reliably keep it on top, so there the strip and the main window open where the compositor puts
     them and the strip may not stay on top. Size is still remembered. Everything works fully on Windows,
     macOS and Linux X11. The README says so in one line.
7. Light and dark both keep working (follow the system as today); the empty state, settings sheet, countdown,
   menus and drop overlay get the new look but keep their content and behaviour.

## Non-goals

- No change to the engine, the settings' options, the When done countdown's behaviour, or the commands the UI
  calls, except what mini mode and the window memory need.
- No build step, framework or bundler: `ui/` stays vanilla HTML/CSS/JS with `withGlobalTauri`.
- No new features beyond the above (browser extension, scheduler and auto-update are separate projects that
  come after this one).

## Anti-goals (would make it a failure even if it works)

- Colour that fights readability: text on a filled card must read in both themes at 0% and 100% fill.
- Actions you can't reach without a mouse: hidden buttons must appear on keyboard focus, and every action stays
  in the right-click menu.
- Layout that jumps: a card must not change height when its buttons appear or its state changes.
- A wide window with a narrow strip in the middle, or a list of 1000 px-wide cards.

## Constraints

- Tauri 2; the CSP stays `script-src 'self'; style-src 'self'` (no inline scripts or style blocks; setting
  `element.style` properties from JS is fine).
- Windows 10/11, macOS 11+, Linux (WebKitGTK). Menus stay HTML in the window (Wayland can't place native popups).
- Fonts: system UI font for text, a system monospace only where numbers must not jiggle (or tabular-nums).
- Builds and GUI tests run on render-01 in the `indir-dev:local` container (recipe in the project memory), never
  on core-01. Never benchmark against speed-test servers.
- The README screenshots (`docs/screenshot-{light,dark}.png`, 1640x1040) are retaken for the new look.
- A merge to main is not a release; tagging stays Bayram's call.

## Left to the builder

The exact palette stops, spacing, radii, animation timing, icon drawing, how mini mode is made (second window
vs. resizing the main one), which crate remembers window state, and how the details area opens (in place vs.
below the card).
