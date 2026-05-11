# Architecture Decision Records

Decisions are recorded one per file, numbered sequentially, in the
[Michael Nygard](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions)
template (Status / Context / Decision / Consequences). New decisions get
a new ADR; existing ADRs are not edited except to mark them Superseded.

| #    | Title                                                                  | Status            |
|------|------------------------------------------------------------------------|-------------------|
| 0001 | [Don't ship MIDI Beat Clock from this plugin](0001-reject-midi-clock-output.md) | Accepted          |
| 0002 | [Build it ourselves rather than use existing tools](0002-build-vs-buy.md) | Accepted          |
| 0003 | [Use aubio for onset detection rather than madmom or BeatNet](0003-aubio-over-madmom-beatnet.md) | Accepted          |
| 0004 | [Add a phase-locked loop on top of onset detection](0004-pll-on-top-of-onset-detection.md) | Accepted          |
| 0005 | [Reject simulated tap-tempo as a control mechanism](0005-reject-simulated-tap-tempo.md) | Accepted          |
| 0006 | [Reject OSC as the v1 cross-application transport](0006-defer-osc-output.md) | Deferred          |
| 0007 | [Use Ableton Link as the primary cross-application transport](0007-ableton-link-as-primary-transport.md) | Accepted          |
| 0008 | [Choose Rust + nih-plug over C++ + JUCE](0008-rust-nih-plug-over-cpp-juce.md) | Accepted          |
| 0009 | [Accept GPL-3 licensing for the project](0009-gpl3-licensing.md)        | Accepted          |
| 0010 | [Defer phase publishing on Link to a later version](0010-defer-link-phase-publishing.md) | Deferred          |
| 0011 | [Defer sidechain input to v2](0011-defer-sidechain-input.md)            | Deferred          |
| 0012 | [Single-instance Link, last-write-wins](0012-single-instance-link-last-write-wins.md) | Accepted          |
| 0013 | [pluginval as the primary plugin validator](0013-pluginval-as-primary-validator.md) | Accepted          |
| 0014 | [Ballroom Dataset for beat-detection evaluation](0014-ballroom-dataset-for-beat-detection-eval.md) | Accepted          |
| 0015 | [In-process Link listener test on top of manual LinkHut procedure](0015-link-integration-test-via-rusty-link-listener.md) | Accepted          |
| 0016 | [BeatPLL cold-starts by snapping to the first observed period](0016-beat-pll-cold-start-snap.md) | Accepted          |
| 0017 | [PulseGenerator wrap detection requires a drop greater than half a period](0017-pulse-generator-wrap-threshold.md) | Accepted          |
| 0018 | [UI indicator LED reflects beats, not PPQN pulses](0018-beat-led-not-pulse-led.md) | Accepted          |
| 0019 | [egui_kittest for UI snapshot testing](0019-egui-kittest-for-ui-snapshots.md) | Accepted          |
| 0020 | [Editor window is host-resizable](0020-resizable-editor.md) | Accepted          |
| 0021 | [Resize gives space, doesn't scale UI](0021-resize-not-scale.md) | Accepted          |
| 0022 | [Host-driven editor resize is unsupported pending nih-plug fix](0022-host-driven-resize-unsupported.md) | Accepted          |
| 0023 | [Latency offset is user-tunable, not auto-detected](0023-latency-offset-user-tunable.md) | Accepted          |
| 0024 | [Tempo stability is its own parameter, decoupled from Sensitivity](0024-tempo-stability-separate-param.md) | Accepted          |
| 0025 | [Pulse firing requires monotonic forward progress](0025-pulse-firing-monotonic.md) | Accepted          |
| 0026 | [Lookahead-consensus tracker as opt-in alternative to per-onset PLL feedback](0026-lookahead-consensus-tracker.md) | Accepted          |
| 0027 | [Aubio `Tempo` as the default tracking mode](0027-aubio-tempo-tracking-mode.md) | Accepted          |
| 0028 | [Future direction: multi-band-accent + comb-filter resonator tracker](0028-future-multi-band-comb-resonator-tracker.md) | Deferred          |
