# ADR-0009: Accept GPL-3 licensing for the project

## Status

Accepted, with a documented escape hatch.

## Context

Two dependencies impose copyleft obligations:

- **aubio** is LGPL-3.
- **Ableton Link** is GPL-2-or-later (commercial license available from
  Ableton for proprietary use).

Combining LGPL-3 and GPL-2-or-later in one binary requires the combined
work to be under GPL-3. Any other license choice would be inconsistent
with one of the dependencies.

## Decision

License the entire project under **GPL-3**. Apply the GPL-3 header to
every source file. Document the license prominently in README.

## Consequences

- The plugin cannot be shipped commercially without (a) replacing aubio
  with permissively-licensed code AND (b) obtaining a commercial Link
  license from Ableton.
- For personal use, GPL-3 is no constraint.
- Forks and modifications must remain GPL-3 if redistributed. This is the
  intended outcome for a personal/hobby tool.
- If a permissive future is desired:
  - aubio can be replaced with ~150 lines of hand-written spectral-flux
    onset detection on top of `rustfft`.
  - Link cannot be replaced; commercial license is the only path.
