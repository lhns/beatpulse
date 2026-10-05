# ADR-0029: Move from nih-plug to nice-plug

## Status

Accepted. Supersedes ADR-0022; amends ADR-0008 and ADR-0020.

## Context

`robbert-vdh/nih-plug` (ADR-0008) is in maintenance mode. Its
maintained successor, `RustAudio/nice-plug` (formerly
`BillyDM/nih-plug`), is published on crates.io and implements
host-driven editor resize (the gap in ADR-0022), with min-size clamping
via `ResizeHint`. It also fixes the egui DPI resize bug that left a
black border at non-100% scaling.

## Decision

Depend on `nice-plug`, `nice-plug-egui` and `nice-plug-xtask` from
crates.io instead of the pinned nih-plug git revision.

- The editor is a `NiceEguiApp` (`ui::BeatpulseEditor`).
- `EguiNiceSettings::with_resize_hint` advertises host resize with a
  minimum of `MIN_W`×`MIN_H`, matching the corner handle's minimum.
- The editor state lives on the plugin, not in `BeatpulseParams`.
- On Windows the editor state starts at the physical size for the system
  DPI. nice-plug-egui otherwise reports the size at 100% before the window
  opens (nice-plug#75), and FL Studio sizes its frame from that, clipping
  the content at 125%. Drop this once upstream fixes it.

## Consequences

- Dragging the host's window border resizes the editor.
- The window size is no longer saved in plugin state (nice-plug's
  `EguiEditorState` isn't serializable); restoring it on project reload
  is left to the host. Old `editor-state` entries in saved state are
  ignored.
- MSRV rises to 1.88.
- MIDI output uses `try_send_event`; a full host queue drops the event,
  as before.
- nice-plug describes itself as experimental; expect API churn on bumps.

## Verification

- `cargo test` (incl. `tests/host_resize.rs` and the UI snapshots) and
  `cargo xtask bundle beatpulse --release`.
- Manual, FL Studio VST3 on Windows at 125%: correct initial size, host
  border resize.
- Pending: loading a project saved with the nih-plug build.
