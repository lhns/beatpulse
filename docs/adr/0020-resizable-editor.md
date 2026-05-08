# ADR-0020: Editor window is host-resizable

## Status

Accepted.

## Context

The editor window was a fixed 480×540 `egui::CentralPanel`. Once the
"MIDI configuration" foldout is expanded the content reaches ~720 px
tall, so users have to scroll to reach the lower controls. With more
screen real estate available, users in modern hosts (Live, Bitwig,
Reaper, Cubase, FL Studio) expect to drag the plugin window larger
to see everything at once. The user explicitly asked for this with the
constraint:

> "not like scalable just if the host grows the window, give the
> window more space so we don't have to scroll so much"

`nih_plug_egui` provides `resizable_window::ResizableWindow`. It wraps
a `CentralPanel`, paints a drag-handle in the bottom-right corner,
calls `EguiState::set_requested_size((u32, u32))` on drag, and nih-plug
propagates that to the host. The host either honours the resize or
refuses (some older / strict hosts), in which case the user sees the
cursor change but no actual resize and the existing `ScrollArea`
inside the panel still handles overflow.

`EguiState`'s `size` is persisted via the existing
`#[persist = "editor-state"]` field on `BeatpulseParams::editor_state`,
so the resized dimensions survive plugin reload (per-instance state).

## Decision

Wrap the panel body in `nih_plug_egui::resizable_window::ResizableWindow`:

```rust
ResizableWindow::new("beatpulse-window")
    .min_size(egui::Vec2::new(MIN_W as f32, MIN_H as f32))
    .show(egui_ctx, &editor_state, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            build_panel(ui, &params, &shared, setter);
        });
    });
```

Constants: `MIN_W = 420`, `MIN_H = 360`. Initial default stays at the
existing `WINDOW_W × WINDOW_H = 480 × 540`.

## Consequences

- A drag-handle appears in the bottom-right corner of the editor.
- Resized dimensions persist across plugin reload (per-instance state,
  via `#[persist]` on `BeatpulseParams::editor_state`).
- Hosts that refuse plugin-requested resizes show the cursor change but
  no actual resize. The `ScrollArea` inside still gives access to all
  controls. Documented limitation.
- No code changes needed to the inner layout — `ui.available_width()`
  already drives slider/meter widths, so a wider window naturally
  widens those.
- Below the minimum (420×360) the cursor refuses to shrink the window
  further; at the minimum the level meter, section headers and BPM
  display all stay legible.
