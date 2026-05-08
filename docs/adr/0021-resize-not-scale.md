# ADR-0021: Resize gives space, doesn't scale UI

## Status

Accepted.

## Context

When ADR-0020 (host-resizable window) was approved, the user clarified
the intent of the request:

> "not like scalable just if the host grows the window, give the
> window more space so we don't have to scroll so much"

Two distinct concepts could have been bundled together:

1. **Window resize** — the host or user changes the editor's outer
   dimensions; existing widgets get more or less area.
2. **UI zoom / DPI scaling** — fonts and widget proportions grow with
   `pixels_per_point` so a 4K display is as readable as a 1080p one.

The user explicitly wanted (1), not (2). nih-plug already handles (2)
separately via the host's reported scale factor (egui's
`pixels_per_point`); we should not conflate the two by, e.g., making
fonts grow proportionally to window size.

## Decision

Window resize affects the *available area* only:

- Font sizes, widget heights, padding, and corner radii stay constant
  across all window sizes.
- The level meter, sliders, and grids stretch to fill the new width
  via `ui.available_width()`.
- The `ScrollArea` inside the resizable window exposes more vertical
  content as the window grows taller.
- DPI / per-monitor scaling continues to be handled by the host via
  the egui scale factor, independent of window resize.

## Consequences

- Resize behaviour is "more cells in the spreadsheet", not "bigger
  spreadsheet cells".
- A resized window at 1.5× width shows the level meter and sliders at
  1.5× their original width but the BPM monospace text stays the same
  34 pt.
- If a user later wants UI zoom (e.g. for a 4K display where the
  editor at native size is too small), that's a separate feature and
  would set egui's `pixels_per_point` rather than the window size.
- The snapshot test `snapshot_resized_panel` (rendered at 720×800)
  exists to lock this in: if anyone later makes fonts grow with size,
  the snapshot will visually diverge from the default view.
