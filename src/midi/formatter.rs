// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! Converts pulse events from `PulseGenerator` into MIDI commands.
//! See `BeatPulse-SPEC.md` §8.
//!
//! This module is deliberately independent of `nih_plug::midi::NoteEvent`
//! so unit tests don't need the full plugin host stack to build. The
//! plugin's `process` adapts these commands to `NoteEvent`s at the boundary.

use crate::dsp::pulse_generator::PulseEvent;
use crate::params::{CcValueMode, MsgType};

/// MIDI command emitted by the formatter, ready to be wrapped in a
/// `nih_plug::midi::NoteEvent` by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiCommand {
    /// CC change. `channel` is 0-indexed (MIDI channel 1 == 0).
    Cc {
        sample_offset: u32,
        channel: u8,
        cc_number: u8,
        value: u8,
    },
    NoteOn {
        sample_offset: u32,
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        sample_offset: u32,
        channel: u8,
        note: u8,
    },
}

/// Snapshot of formatter-relevant params used during a single block. Taken
/// at the start of `process` so mid-block param changes don't desync.
#[derive(Debug, Clone, Copy)]
pub struct FormatterConfig {
    pub msg_type: MsgType,
    pub cc_number: u8,
    pub cc_value_mode: CcValueMode,
    pub cc_value: u8,
    pub note_number: u8,
    pub note_velocity: u8,
    /// Note-off delay after note-on, in samples.
    pub note_length_samples: u32,
    /// 1-indexed MIDI channel from params (1..=16).
    pub midi_channel: u8,
}

impl FormatterConfig {
    fn channel_zero_indexed(&self) -> u8 {
        self.midi_channel.saturating_sub(1).min(15)
    }
}

/// Pending note-off scheduled at an absolute sample position.
#[derive(Debug, Clone, Copy)]
struct PendingNoteOff {
    /// Absolute sample at which the note-off should fire.
    abs_sample: u64,
    note: u8,
    channel: u8,
}

pub struct MidiFormatter {
    /// Toggle state for `CcValueMode::Toggle127_0`. Flips on each pulse.
    toggle_high: bool,
    /// At most one note-off in flight per pulse cadence. If a new pulse
    /// fires before the previous note-off has been emitted, we flush the
    /// pending note-off at the new pulse's sample offset (spec §8 edge case).
    pending: Option<PendingNoteOff>,
}

impl Default for MidiFormatter {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiFormatter {
    pub fn new() -> Self {
        Self {
            toggle_high: true,
            pending: None,
        }
    }

    pub fn reset(&mut self) {
        self.toggle_high = true;
        self.pending = None;
    }

    /// Emit any due note-offs from previous blocks that should fire within
    /// `[block_start_sample, block_start_sample + block_len)`.
    pub fn flush_due_note_offs(
        &mut self,
        block_start_sample: u64,
        block_len: u32,
        out: &mut Vec<MidiCommand>,
    ) {
        if let Some(p) = self.pending {
            let block_end = block_start_sample + block_len as u64;
            if p.abs_sample < block_end {
                let offset = p.abs_sample.saturating_sub(block_start_sample) as u32;
                out.push(MidiCommand::NoteOff {
                    sample_offset: offset.min(block_len.saturating_sub(1)),
                    channel: p.channel,
                    note: p.note,
                });
                self.pending = None;
            }
        }
    }

    /// Process a pulse event: emit the MIDI commands for this pulse and
    /// schedule any future note-off.
    pub fn on_pulse(
        &mut self,
        ev: PulseEvent,
        block_start_sample: u64,
        cfg: &FormatterConfig,
        out: &mut Vec<MidiCommand>,
    ) {
        let channel = cfg.channel_zero_indexed();

        // Stuck-note prevention: if a note-off was scheduled but the new
        // pulse fires before it, flush it at the new pulse offset and clear.
        if let Some(p) = self.pending {
            if p.abs_sample > block_start_sample + ev.sample_offset as u64 {
                out.push(MidiCommand::NoteOff {
                    sample_offset: ev.sample_offset,
                    channel: p.channel,
                    note: p.note,
                });
                self.pending = None;
            }
        }

        match cfg.msg_type {
            MsgType::Cc => self.emit_cc(ev, channel, cfg, out),
            MsgType::Note => self.emit_note(ev, block_start_sample, channel, cfg, out),
            MsgType::Both => {
                self.emit_cc(ev, channel, cfg, out);
                self.emit_note(ev, block_start_sample, channel, cfg, out);
            }
        }
    }

    fn emit_cc(
        &mut self,
        ev: PulseEvent,
        channel: u8,
        cfg: &FormatterConfig,
        out: &mut Vec<MidiCommand>,
    ) {
        let value = match cfg.cc_value_mode {
            CcValueMode::Fixed127 => 127,
            CcValueMode::FixedCustom => cfg.cc_value.min(127),
            CcValueMode::Toggle127_0 => {
                let v = if self.toggle_high { 127 } else { 0 };
                self.toggle_high = !self.toggle_high;
                v
            }
        };
        out.push(MidiCommand::Cc {
            sample_offset: ev.sample_offset,
            channel,
            cc_number: cfg.cc_number.min(127),
            value,
        });
    }

    fn emit_note(
        &mut self,
        ev: PulseEvent,
        block_start_sample: u64,
        channel: u8,
        cfg: &FormatterConfig,
        out: &mut Vec<MidiCommand>,
    ) {
        out.push(MidiCommand::NoteOn {
            sample_offset: ev.sample_offset,
            channel,
            note: cfg.note_number.min(127),
            velocity: cfg.note_velocity.clamp(1, 127),
        });
        let off_abs = block_start_sample + ev.sample_offset as u64 + cfg.note_length_samples as u64;
        self.pending = Some(PendingNoteOff {
            abs_sample: off_abs,
            note: cfg.note_number.min(127),
            channel,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(msg_type: MsgType) -> FormatterConfig {
        FormatterConfig {
            msg_type,
            cc_number: 16,
            cc_value_mode: CcValueMode::Fixed127,
            cc_value: 127,
            note_number: 60,
            note_velocity: 100,
            note_length_samples: 441, // 10 ms @ 44.1 kHz
            midi_channel: 1,
        }
    }

    fn pulse(offset: u32) -> PulseEvent {
        PulseEvent {
            sample_offset: offset,
            is_beat_boundary: false,
        }
    }

    /// M1: CC Fixed127 emits value 127 with correct sample offset.
    #[test]
    fn m1_cc_fixed_127() {
        let mut f = MidiFormatter::new();
        let mut out = Vec::new();
        f.on_pulse(pulse(42), 0, &cfg(MsgType::Cc), &mut out);
        assert_eq!(out.len(), 1);
        match out[0] {
            MidiCommand::Cc {
                sample_offset,
                value,
                cc_number,
                channel,
            } => {
                assert_eq!(sample_offset, 42);
                assert_eq!(value, 127);
                assert_eq!(cc_number, 16);
                assert_eq!(channel, 0);
            }
            _ => panic!("expected CC"),
        }
    }

    /// M2: CC FixedCustom emits cfg.cc_value.
    #[test]
    fn m2_cc_fixed_custom() {
        let mut f = MidiFormatter::new();
        let mut c = cfg(MsgType::Cc);
        c.cc_value_mode = CcValueMode::FixedCustom;
        c.cc_value = 64;
        let mut out = Vec::new();
        f.on_pulse(pulse(0), 0, &c, &mut out);
        assert!(matches!(out[0], MidiCommand::Cc { value: 64, .. }));
    }

    /// M3: Toggle127_0 alternates 127/0/127/0…
    #[test]
    fn m3_cc_toggle() {
        let mut f = MidiFormatter::new();
        let mut c = cfg(MsgType::Cc);
        c.cc_value_mode = CcValueMode::Toggle127_0;
        let mut out = Vec::new();
        for i in 0..4 {
            f.on_pulse(pulse(i * 100), 0, &c, &mut out);
        }
        let values: Vec<u8> = out
            .iter()
            .filter_map(|cmd| match cmd {
                MidiCommand::Cc { value, .. } => Some(*value),
                _ => None,
            })
            .collect();
        assert_eq!(values, vec![127, 0, 127, 0]);
    }

    /// M4: Note-on at pulse + scheduled note-off.
    #[test]
    fn m4_note_on_and_off() {
        let mut f = MidiFormatter::new();
        let c = cfg(MsgType::Note);
        let mut out = Vec::new();
        f.on_pulse(pulse(100), 0, &c, &mut out);
        // Should have emitted a single NoteOn and queued the note-off.
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0],
            MidiCommand::NoteOn {
                sample_offset: 100,
                note: 60,
                velocity: 100,
                channel: 0
            }
        ));
        // Now flush a block that contains the note-off.
        let mut out2 = Vec::new();
        // note_off scheduled at abs sample 100 + 441 = 541.
        // Flush block starting at 100, length 600 covers it.
        f.flush_due_note_offs(100, 600, &mut out2);
        assert_eq!(out2.len(), 1);
        assert!(matches!(
            out2[0],
            MidiCommand::NoteOff {
                note: 60,
                channel: 0,
                ..
            }
        ));
    }

    /// M5: note-off across block boundary fires in the correct future block.
    #[test]
    fn m5_note_off_across_block_boundary() {
        let mut f = MidiFormatter::new();
        let c = cfg(MsgType::Note);
        let mut out = Vec::new();
        // Pulse at sample 0 of a block starting at abs 0, block length 200.
        f.on_pulse(pulse(0), 0, &c, &mut out);
        // Note off scheduled at abs 441. First block (0..200) should not flush.
        let mut after_first = Vec::new();
        f.flush_due_note_offs(0, 200, &mut after_first);
        assert!(after_first.is_empty());
        // Second block (200..600) covers the note-off.
        let mut after_second = Vec::new();
        f.flush_due_note_offs(200, 400, &mut after_second);
        assert_eq!(after_second.len(), 1);
        if let MidiCommand::NoteOff { sample_offset, .. } = after_second[0] {
            // 441 - 200 = 241
            assert_eq!(sample_offset, 241);
        } else {
            panic!("expected NoteOff");
        }
    }

    /// M6: stuck-note prevention. Pulse fires before pending note-off →
    /// note-off flushed at new pulse offset.
    #[test]
    fn m6_stuck_note_prevention() {
        let mut f = MidiFormatter::new();
        let mut c = cfg(MsgType::Note);
        c.note_length_samples = 10_000; // long
        let mut out = Vec::new();
        f.on_pulse(pulse(0), 0, &c, &mut out); // Note-on at 0, off scheduled at 10_000
        out.clear();
        // New pulse at sample 100 — note-off should fire at offset 100.
        f.on_pulse(pulse(100), 0, &c, &mut out);
        // Expected: NoteOff at offset 100, then NoteOn at offset 100.
        assert!(out.len() >= 2);
        assert!(matches!(
            out[0],
            MidiCommand::NoteOff {
                sample_offset: 100,
                ..
            }
        ));
        assert!(matches!(
            out[1],
            MidiCommand::NoteOn {
                sample_offset: 100,
                ..
            }
        ));
    }

    /// M7: channel offset (1-indexed in params, 0-indexed in output).
    #[test]
    fn m7_channel_offset() {
        for ch_param in 1..=16u8 {
            let mut f = MidiFormatter::new();
            let mut c = cfg(MsgType::Cc);
            c.midi_channel = ch_param;
            let mut out = Vec::new();
            f.on_pulse(pulse(0), 0, &c, &mut out);
            if let MidiCommand::Cc { channel, .. } = out[0] {
                assert_eq!(channel, ch_param - 1);
            } else {
                panic!("expected CC");
            }
        }
    }

    /// M8: Both mode emits CC and Note simultaneously at every pulse.
    #[test]
    fn m8_both_mode() {
        let mut f = MidiFormatter::new();
        let c = cfg(MsgType::Both);
        let mut out = Vec::new();
        f.on_pulse(pulse(50), 0, &c, &mut out);
        assert_eq!(out.len(), 2);
        assert!(matches!(
            out[0],
            MidiCommand::Cc {
                sample_offset: 50,
                ..
            }
        ));
        assert!(matches!(
            out[1],
            MidiCommand::NoteOn {
                sample_offset: 50,
                ..
            }
        ));
    }

    /// M9: parameter values reflected in emitted MIDI.
    #[test]
    fn m9_param_values_reflected() {
        let mut f = MidiFormatter::new();
        let mut c = cfg(MsgType::Both);
        c.cc_number = 7;
        c.note_number = 36;
        c.note_velocity = 64;
        let mut out = Vec::new();
        f.on_pulse(pulse(0), 0, &c, &mut out);
        assert!(matches!(out[0], MidiCommand::Cc { cc_number: 7, .. }));
        assert!(matches!(
            out[1],
            MidiCommand::NoteOn {
                note: 36,
                velocity: 64,
                ..
            }
        ));
    }

    /// M10: sample-offset preserved (no rounding to block start).
    #[test]
    fn m10_sample_offset_preserved() {
        let mut f = MidiFormatter::new();
        let c = cfg(MsgType::Cc);
        for &offset in &[0u32, 1, 7, 63, 64, 511, 1023] {
            let mut out = Vec::new();
            f.on_pulse(pulse(offset), 0, &c, &mut out);
            if let MidiCommand::Cc { sample_offset, .. } = out[0] {
                assert_eq!(sample_offset, offset);
            }
        }
    }
}
