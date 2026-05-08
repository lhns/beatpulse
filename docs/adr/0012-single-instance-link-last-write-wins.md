# ADR-0012: Single-instance Link, last-write-wins

## Status

Accepted, with documented caveat.

## Context

Each BeatPulse instance with `link_enabled = true` joins the same Link
session as a peer. If two instances detect different tempos (e.g. one on
a kick bus, one on a snare bus), they will continuously overwrite each
other's tempo updates with last-write-wins semantics. The Link session
will visibly thrash.

Solutions considered:

1. Detect duplicate-instance situations at runtime (e.g. by checking
   `link.num_peers()` against known-other-instances). Hard to implement
   reliably; instances can't authenticate each other.
2. Make Link enable mutually exclusive across instances of the same
   plugin. Requires inter-instance communication; ugly.
3. Document the constraint and rely on the user.

## Decision

Document the constraint in the user-facing README and in a tooltip on
the Link enable toggle: *"Only enable Link on one BeatPulse instance per
session."* Do not implement runtime detection.

## Consequences

- A user who enables Link on multiple instances by mistake will see
  visibly bad behaviour. The documentation should be clear enough that
  this is recoverable.
- Implementation stays simple.
- Future v2 could add detection if this proves to be a recurring support
  issue.
