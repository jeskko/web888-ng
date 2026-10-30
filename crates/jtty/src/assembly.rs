// SPDX-License-Identifier: GPL-3.0-or-later
//! Frames to messages by timing and frequency, and the duplicate tests
//! (jtty-cuf `receiver_assembly.cpp`). No signal processing.

use crate::protocol::{PayloadBits, SAMPLES_PER_FRAME};
use std::collections::VecDeque;

pub(crate) const RATE_HZ: f64 = 12_000.0;
pub(crate) const FRAME_PERIOD_S: f64 = SAMPLES_PER_FRAME as f64 / RATE_HZ; // 1.888 s
pub(crate) const STEP_PERIOD_S: f64 = FRAME_PERIOD_S / 4.0; // 0.472 s
pub(crate) const STAMP_DF_HZ: f64 = 12.0;
pub(crate) const STAMP_DT_S: f64 = 0.05;
const ALIAS_DF_HZ: f64 = 10.0;
const ALIAS_RESIDUAL_S: f64 = 0.003;
const CONTINUATION_RESIDUAL_S: f64 = 0.1;
/// Internal stand-in for a TEXT5 space.
pub(crate) const STAND_IN: u8 = 0x01;
pub(crate) const MAX_OPEN_MESSAGES: usize = 30;
const MAX_FRAME_STAMPS: usize = 480;
pub(crate) const MAX_MESSAGE_CHARACTERS: usize = 80;

#[derive(Clone, Debug)]
pub(crate) struct FrameEntry {
    pub frequency_hz: f64,
    /// Stream sample position of the frame start (12 kHz).
    pub start_12k: f64,
    pub payload: PayloadBits,
    /// Frame text, TEXT5 spaces as stand-ins.
    pub text: Vec<u8>,
    pub space_after: bool,
    pub eom: bool,
}

impl FrameEntry {
    pub fn seconds(&self) -> f64 {
        self.start_12k / RATE_HZ
    }
}

#[derive(Clone, Debug)]
pub(crate) struct OpenMessage {
    pub text: Vec<u8>,
    pub space_after: bool,
    pub f_last: f64,
    pub t_last: f64,
    pub first: FrameEntry,
    pub frames: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MergeResult {
    StampDuplicate,
    AliasDuplicate,
    StartedOpen,
    StartedComplete,
    Refused,
    Appended,
    Completed,
}

#[derive(Default, Clone, Debug)]
pub struct Work {
    pub windows: usize,
    pub silent_windows: usize,
    pub lookback_windows: usize,
    pub candidates: usize,
    pub gated_candidates: usize,
    pub retries: usize,
    pub fec_hypotheses: usize,
    pub frames_decoded: usize,
    pub duplicate_frames: usize,
    pub subtractions: usize,
    pub messages_started: usize,
    pub incomplete_chains: usize,
}

/// The open messages, frame stamps and recent frames of a stream.
#[derive(Default)]
pub(crate) struct Assembler {
    open: Vec<OpenMessage>,
    stamps: VecDeque<(f64, f64)>,
    history: VecDeque<FrameEntry>,
}

/// Frames kept for re-use of their timing estimate (look-backs reach
/// three steps back): far more than enough.
const HISTORY_S: f64 = 10.0;

impl Assembler {
    pub fn open(&self) -> &[OpenMessage] {
        &self.open
    }

    pub fn history(&self) -> impl Iterator<Item = &FrameEntry> {
        self.history.iter()
    }

    /// Positional list: the last one moves into the gap.
    fn remove_open(&mut self, i: usize) -> OpenMessage {
        self.open.swap_remove(i)
    }

    fn stamp(&mut self, e: &FrameEntry) {
        if self.stamps.len() >= MAX_FRAME_STAMPS {
            self.stamps.pop_front();
        }
        self.stamps.push_back((e.frequency_hz, e.seconds()));
    }

    /// Before each forward window starting at `window_s`: forget old
    /// stamps, time out stale open messages.
    pub fn expire(&mut self, window_s: f64, work: &mut Work) {
        let r = window_s - 3.0 * STEP_PERIOD_S;
        self.stamps.retain(|s| s.1 >= r - STAMP_DT_S);
        let mut i = 0;
        while i < self.open.len() {
            if r - self.open[i].t_last > 3.0 * FRAME_PERIOD_S + CONTINUATION_RESIDUAL_S {
                work.incomplete_chains += 1;
                self.remove_open(i);
            } else {
                i += 1;
            }
        }
        while self
            .history
            .front()
            .is_some_and(|h| h.seconds() < window_s - HISTORY_S)
        {
            self.history.pop_front();
        }
    }

    /// End of a reception: every open message is cut off.
    pub fn finish(&mut self, work: &mut Work) {
        work.incomplete_chains += self.open.len();
        self.open.clear();
    }

    fn continuation_gap(e: &FrameEntry, m: &OpenMessage) -> i64 {
        let dt = e.seconds() - m.t_last;
        let j = (dt / FRAME_PERIOD_S).round() as i64;
        if !(1..=3).contains(&j) {
            return 0;
        }
        if !((dt - j as f64 * FRAME_PERIOD_S).abs() < CONTINUATION_RESIDUAL_S) {
            return 0;
        }
        if !((e.frequency_hz - m.f_last).abs() < 10.0 + 3.0 * (j - 1) as f64) {
            return 0;
        }
        j
    }

    /// A validated frame; a completed message comes back with the result.
    pub fn merge(&mut self, e: &FrameEntry, work: &mut Work) -> (MergeResult, Option<OpenMessage>) {
        let t = e.seconds();
        for &(f, s) in &self.stamps {
            if (f - e.frequency_hz).abs() < STAMP_DF_HZ && (s - t).abs() < STAMP_DT_S {
                work.duplicate_frames += 1;
                return (MergeResult::StampDuplicate, None);
            }
        }
        for m in &self.open {
            if Self::continuation_gap(e, m) != 0 {
                continue;
            }
            let dt = t - m.t_last;
            let s = (dt / STEP_PERIOD_S).round();
            if s.abs() <= 3.0
                && (e.frequency_hz - m.f_last).abs() < ALIAS_DF_HZ
                && (dt - s * STEP_PERIOD_S).abs() < ALIAS_RESIDUAL_S
            {
                work.duplicate_frames += 1;
                return (MergeResult::AliasDuplicate, None);
            }
        }
        let mut chosen = None;
        let mut gap = 0;
        let mut best_df = f64::INFINITY;
        for (i, m) in self.open.iter().enumerate() {
            let j = Self::continuation_gap(e, m);
            if j == 0 {
                continue;
            }
            let df = (e.frequency_hz - m.f_last).abs();
            if df < best_df {
                best_df = df;
                chosen = Some(i);
                gap = j;
            }
        }
        self.history.push_back(e.clone());
        let Some(chosen) = chosen else {
            let mut text = e.text.clone();
            if text.starts_with(b"599 ") {
                text.insert(0, STAND_IN);
            }
            text.truncate(MAX_MESSAGE_CHARACTERS);
            let m = OpenMessage {
                text,
                space_after: e.space_after,
                f_last: e.frequency_hz,
                t_last: t,
                first: e.clone(),
                frames: 1,
            };
            if e.eom {
                work.messages_started += 1;
                self.stamp(e);
                return (MergeResult::StartedComplete, Some(m));
            }
            if self.open.len() >= MAX_OPEN_MESSAGES {
                work.incomplete_chains += 1;
                return (MergeResult::Refused, None);
            }
            work.messages_started += 1;
            self.stamp(e);
            self.open.push(m);
            return (MergeResult::StartedOpen, None);
        };
        let m = &mut self.open[chosen];
        if gap == 1 {
            if m.space_after {
                m.text.push(b' ');
            }
            m.text.extend_from_slice(&e.text);
        } else {
            m.text.extend_from_slice(&[STAND_IN; 5]);
            let text = e.text.strip_prefix(&[STAND_IN]).unwrap_or(&e.text);
            m.text.extend_from_slice(text);
        }
        m.text.truncate(MAX_MESSAGE_CHARACTERS);
        m.space_after = e.space_after;
        m.f_last = e.frequency_hz;
        m.t_last = t;
        m.frames += 1;
        self.stamp(e);
        if e.eom {
            let done = self.remove_open(chosen);
            return (MergeResult::Completed, Some(done));
        }
        (MergeResult::Appended, None)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    Fresh,
    SameText,
    RxRepeat,
}

/// Within-window duplicates, and the channel-Rx decodes (at most 16) that
/// also clear their vicinity in the sync map.
#[derive(Default)]
pub(crate) struct WindowDuplicates {
    decoded: Vec<(Vec<u8>, f64)>,
    rx: Vec<(f64, f64)>,
}

impl WindowDuplicates {
    const SAME_TEXT_DT_S: f64 = 0.032;
    const RX_REPEAT_DF_HZ: f64 = 3.0;
    const RX_REPEAT_DT_S: f64 = 0.05;
    const MAX_RX_RECORDS: usize = 16;

    pub fn test(
        &self,
        text: &[u8],
        frequency_hz: f64,
        seconds: f64,
        fixed_channel: bool,
    ) -> Verdict {
        if self
            .decoded
            .iter()
            .any(|(t, w)| t == text && (seconds - w).abs() < Self::SAME_TEXT_DT_S)
        {
            return Verdict::SameText;
        }
        if fixed_channel
            && self.rx.iter().any(|&(f, s)| {
                (f - frequency_hz).abs() < Self::RX_REPEAT_DF_HZ
                    && (s - seconds).abs() < Self::RX_REPEAT_DT_S
            })
        {
            return Verdict::RxRepeat;
        }
        Verdict::Fresh
    }

    pub fn add_decoded(&mut self, text: &[u8], seconds: f64) {
        self.decoded.push((text.to_vec(), seconds));
    }

    pub fn add_rx(&mut self, frequency_hz: f64, seconds: f64) {
        if self.rx.len() < Self::MAX_RX_RECORDS {
            self.rx.push((frequency_hz, seconds));
        }
    }

    pub fn rx(&self) -> &[(f64, f64)] {
        &self.rx
    }
}

/// The published text: stand-in runs (missing frames) as " ... ", idle
/// runs ("<<<<<") dropped, whitespace collapsed.
pub(crate) fn publication_text(internal: &[u8]) -> String {
    let mut text = Vec::new();
    let mut i = 0;
    while i < internal.len() {
        if internal[i..].starts_with(&[STAND_IN; 5]) {
            text.extend_from_slice(b" ... ");
            i += 5;
        } else {
            text.push(if internal[i] == STAND_IN {
                b' '
            } else {
                internal[i]
            });
            i += 1;
        }
    }
    if text.first() == Some(&b' ') {
        text.remove(0);
    }
    while text.last() == Some(&b' ') {
        text.pop();
    }
    while let Some(at) = text.windows(5).position(|w| w == b"<<<<<") {
        text.splice(at..at + 5, *b" ");
    }
    let mut out = String::new();
    for &c in &text {
        if c == b' ' && (out.is_empty() || out.ends_with(' ')) {
            continue;
        }
        out.push(c as char);
    }
    out.truncate(out.trim_end().len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_frames_show_as_dots() {
        let mut t = b"K1ABC".to_vec();
        t.push(b' ');
        t.extend_from_slice(&[STAND_IN; 5]);
        t.extend_from_slice(b"WORLD");
        assert_eq!(publication_text(&t), "K1ABC ... WORLD");
        assert_eq!(publication_text(b"CQ <<<<< DE  X"), "CQ DE X");
    }
}
