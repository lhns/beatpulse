# ADR-0022: Host-driven editor resize is unsupported pending nih-plug fix

## Status

Accepted, with workaround.

## Context

ADR-0020 made the editor window resizable from the user's perspective:
the in-canvas drag-handle at the bottom-right corner writes the
requested size to `EguiState`, and nih-plug propagates that to the host
which then resizes its plugin chrome accordingly. This is the
**plugin → host** direction.

The user reported the **host → plugin** direction doesn't work: when
they grab the host's plugin-window border (which would normally let the
host tell the plugin "I want you to be this size") the plugin contents
don't follow.

Investigation against nih-plug's source confirms the root cause —
`nih-plug/src/editor.rs` ends with:

```rust
// TODO: Host->Plugin resizing
```

The `Editor` trait exposes `size() -> (u32, u32)` (plugin reports its
size to host) and `set_scale_factor(f32) -> bool` (host reports DPI to
plugin), but there's no `on_size(width, height)` callback. So when a
VST3 host calls `IPlugView::onSize(rect)` or a CLAP host calls
`clap_plugin_gui::set_size`, nih-plug's wrapper has nowhere to
forward the call.

This means our plugin advertises a fixed size at any given moment;
hosts that respect plugin-reported size will refuse to grow their
chrome past that size, hosts that try to grow it anyway will leave
the plugin canvas at its old dimensions inside a larger frame.

## Decision

**Accept the limitation as upstream.** The in-canvas corner-drag
handle (ADR-0020) gives users a working resize path. Document the
limitation in the README so users know to use the corner drag, not the
host's chrome border.

When/if nih-plug merges host-driven resize support — a draft would be a
new `Editor` trait method like:

```rust
fn on_size(&self, _new_size: (u32, u32)) -> bool { false }
```

— we wire it to `EguiState::set_requested_size` and the host-chrome
drag starts working with no other code changes on our side. Keep an
eye on nih-plug PRs / issues for this; it's actively wanted by other
plugins too.

We considered forking nih-plug to implement this ourselves (the VST3
wrapper does receive `IPlugView::onSize` — we'd just need to thread
it through to the Editor) but it's significant work for a corner-case
ergonomic improvement when a working alternative exists.

## Consequences

- Users see two resize affordances depending on the host:
  - **Always works**: drag the bottom-right corner of the BeatPulse
    canvas itself (rendered by ResizableWindow).
  - **Doesn't work**: drag the host's plugin-window border. The host
    may show a resize cursor; nothing happens to the plugin contents.
- The README's "Known limitations" section (added in this commit)
  spells this out so users don't waste time wondering.
- If nih-plug adds the `on_size` callback later, we adopt it with a
  one-line edit and the limitation goes away.
