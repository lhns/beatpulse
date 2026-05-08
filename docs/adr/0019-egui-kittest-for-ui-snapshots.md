# ADR-0019: egui_kittest for UI snapshot testing

## Status

Accepted. To be implemented as part of the UI overhaul.

## Context

`nih_plug_egui` runs the editor inside a host-owned `baseview` window —
there's no built-in headless render path. Without one, every UI iteration
requires `cargo xtask bundle` + manual reload in a host. That's slow and
puts a human in every loop, including loops where the next change is "I
need to see what the layout looks like."

For the upcoming UI overhaul (better widget choices per parameter type +
dark theme polish) we want fast visual feedback without a host. Three
candidate approaches:

1. **`egui_kittest`** (egui's official snapshot test crate) — runs egui
   headlessly via wgpu / lavapipe, captures a frame to PNG. Designed for
   exactly this use case. Versions track egui releases; `0.31.1` matches
   our pinned `egui 0.31`.
2. **Standalone build** (`nih_plug` supports `nih_export_standalone!`) —
   build a `.exe`, launch it, screenshot the window. Lower setup cost but
   per-iteration loop is slower.
3. **Hand-rolled headless egui + custom rasteriser** — egui exposes
   tessellated geometry but no software rasteriser. We'd write or vendor
   one. Too much work.

## Decision

Add `egui_kittest = "0.31"` as a dev-dependency, gated behind feature
`ui-snapshots`. New file `tests/ui_snapshot.rs` provides:

- A `StubGuiContext` implementing `nih_plug::context::gui::GuiContext`
  with no-op writes (parameter changes drop on the floor — we render but
  don't interact).
- A `snapshot_default_panel` test that constructs default params + sample
  shared state, builds an `egui_kittest::Harness` sized to the editor
  window, runs the egui pass, and saves PNG to `tests/data/output/`.
- Optional further snapshots (e.g. `snapshot_midi_expanded`) for state
  variants worth visually verifying.

The UI rendering closure is refactored into a public
`build_panel(ui, params, shared, setter)` function so the snapshot test
can call it without duplicating layout code.

## Consequences

- Snapshot iteration is fast (cargo test cycle, no host).
- The PNGs are gitignored (kept under `tests/data/output/`); committing
  them would create a version-control churn on every UI tweak. If
  snapshot regression testing becomes useful later, store golden PNGs in
  a separate directory.
- GPU/wgpu deps land in the build graph only when the feature is enabled
  — default `cargo test` is unchanged.
- Stub GuiContext means widgets that *write* to the host (button clicks,
  drag commits) won't be exercised by the snapshot. Visual-only.
- Future v1.x: hook the snapshot test into a CI job that compares against
  a checked-in golden image; useful once the UI stabilises.
