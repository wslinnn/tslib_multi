//! Fixed-delay jitter buffer for incoming voice.
//!
//! The buffer schedules each packet at `first_arrival + target_delay +
//! n * frame_duration`. It never grows its delay with observed jitter:
//! packets that arrive after their scheduled time are dropped. Missing
//! sequence numbers are concealed with a bounded number of PLC frames; after
//! that the buffer skips to the next real packet so playback can resume.

use crate::config::JitterBufferConfig;
use std::collections::HashMap;
use std::time::Instant;

/// One output produced by [`JitterBuffer::pump`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JitterOutput {
    /// A received packet, ready to decode.
    Packet(Vec<u8>),
    /// A missing packet; the decoder should run PLC.
    Plc,
}

/// Counters for one user's jitter buffer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JitterStats {
    /// Packets currently buffered.
    pub buffered: usize,
    /// Packets that arrived after their play deadline.
    pub late_dropped: u64,
    /// Duplicate sequence numbers that were ignored.
    pub duplicates: u64,
    /// Packets dropped because the buffer was full.
    pub capacity_dropped: u64,
    /// PLC frames emitted for sequence gaps.
    pub plc_frames: u64,
}

/// Per-user fixed-delay jitter buffer.
pub struct JitterBuffer {
    config: JitterBufferConfig,
    packets: HashMap<u16, Vec<u8>>,
    /// Arrival time of the first packet in the current talk burst.
    first_arrival: Option<Instant>,
    /// Next sequence number to play.
    next_seq: u16,
    /// When `next_seq` should be played.
    next_due: Instant,
    /// Playback has started for this burst.
    started: bool,
    /// Consecutive PLC frames emitted for the current gap.
    plc_run: u32,
    /// Playback ran dry and is waiting for the next packet.
    waiting: bool,
    late_dropped: u64,
    duplicates: u64,
    capacity_dropped: u64,
    plc_frames: u64,
}

impl JitterBuffer {
    /// Create a buffer with the given configuration.
    pub fn new(config: JitterBufferConfig) -> Self {
        Self {
            config,
            packets: HashMap::new(),
            first_arrival: None,
            next_seq: 0,
            next_due: Instant::now(),
            started: false,
            plc_run: 0,
            waiting: false,
            late_dropped: 0,
            duplicates: 0,
            capacity_dropped: 0,
            plc_frames: 0,
        }
    }

    /// Insert a packet. Returns false when it was rejected as late/duplicate.
    pub fn push(&mut self, seq: u16, data: Vec<u8>, now: Instant) -> bool {
        if self.first_arrival.is_none() {
            // First packet of a burst: start the fixed-delay timer.
            self.first_arrival = Some(now);
            self.next_seq = seq;
            self.next_due = now + self.config.target_delay();
            self.started = false;
            self.plc_run = 0;
            self.waiting = false;
            self.packets.insert(seq, data);
            self.enforce_capacity();
            return true;
        }

        if self.waiting {
            let delta = seq.wrapping_sub(self.next_seq);
            if delta >= 0x8000 {
                // A new burst with a sequence behind the old one: start over
                // instead of treating every packet as late forever.
                self.clear();
                return self.push(seq, data, now);
            }
            // Resume immediately after an underrun instead of bursting to
            // catch up with the old schedule.
            self.next_due = now;
            self.waiting = false;
        }

        if self.started {
            let delta = seq.wrapping_sub(self.next_seq);
            if delta >= 0x8000 {
                self.late_dropped += 1;
                return false;
            }
        }

        if self.packets.contains_key(&seq) {
            self.duplicates += 1;
            return false;
        }

        self.packets.insert(seq, data);
        self.enforce_capacity();
        true
    }

    /// Produce all outputs whose play deadline has passed.
    pub fn pump(&mut self, now: Instant) -> Vec<JitterOutput> {
        let mut outputs = Vec::new();

        if !self.started {
            let Some(first_arrival) = self.first_arrival else {
                return outputs;
            };
            // Fixed delay: never start earlier, even if several frames are
            // already buffered.
            let start_at = first_arrival + self.config.target_delay();
            if now < start_at {
                return outputs;
            }
            self.started = true;
            self.next_due = start_at;
        }

        let frame = self.config.frame_duration();

        while now >= self.next_due {
            if let Some(data) = self.packets.remove(&self.next_seq) {
                outputs.push(JitterOutput::Packet(data));
                self.plc_run = 0;
                self.next_seq = self.next_seq.wrapping_add(1);
                self.next_due += frame;
                continue;
            }

            if !self.has_later_packet() {
                // Underrun: wait for the next packet, then resume from it.
                self.waiting = true;
                break;
            }

            if self.plc_run < self.config.max_plc_frames {
                outputs.push(JitterOutput::Plc);
                self.plc_frames += 1;
                self.plc_run += 1;
                self.next_seq = self.next_seq.wrapping_add(1);
                self.next_due += frame;
                continue;
            }

            // Too many consecutive losses: skip the silent gap and resume at
            // the earliest buffered packet.
            let Some(earliest) = self.earliest_seq() else {
                self.waiting = true;
                break;
            };
            self.next_seq = earliest;
            self.plc_run = 0;
            self.next_due = now;
        }

        outputs
    }

    /// Number of buffered packets.
    pub fn depth(&self) -> usize {
        self.packets.len()
    }

    /// Snapshot of this buffer's counters.
    pub fn stats(&self) -> JitterStats {
        JitterStats {
            buffered: self.packets.len(),
            late_dropped: self.late_dropped,
            duplicates: self.duplicates,
            capacity_dropped: self.capacity_dropped,
            plc_frames: self.plc_frames,
        }
    }

    /// Reset the buffer for a new talk burst.
    pub fn clear(&mut self) {
        self.packets.clear();
        self.first_arrival = None;
        self.started = false;
        self.plc_run = 0;
        self.waiting = false;
    }

    /// Whether playback is waiting for the next packet.
    pub fn is_waiting(&self) -> bool {
        self.waiting
    }

    fn has_later_packet(&self) -> bool {
        self.packets
            .keys()
            .any(|&seq| seq != self.next_seq && seq.wrapping_sub(self.next_seq) < 0x8000)
    }

    fn earliest_seq(&self) -> Option<u16> {
        self.packets
            .keys()
            .copied()
            .min_by_key(|&seq| seq.wrapping_sub(self.next_seq))
    }

    fn enforce_capacity(&mut self) {
        while self.packets.len() > self.config.max_buffered_frames.max(1) {
            let Some(farthest) = self
                .packets
                .keys()
                .copied()
                .max_by_key(|&seq| seq.wrapping_sub(self.next_seq))
            else {
                break;
            };
            self.packets.remove(&farthest);
            self.capacity_dropped += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn config() -> JitterBufferConfig {
        JitterBufferConfig {
            target_delay_ms: 60,
            frame_duration_ms: 20,
            max_plc_frames: 3,
            max_buffered_frames: 8,
        }
    }

    fn packet(seq: u16, now: Instant) -> (u16, Vec<u8>, Instant) {
        (seq, vec![seq as u8], now)
    }

    #[test]
    fn waits_for_fixed_delay_then_plays_in_order() {
        let mut buf = JitterBuffer::new(config());
        let t0 = Instant::now();
        for seq in 1..=3 {
            let (s, d, now) = packet(seq, t0);
            assert!(buf.push(s, d, now));
        }

        assert!(buf.pump(t0 + Duration::from_millis(59)).is_empty());

        let out = buf.pump(t0 + Duration::from_millis(60));
        assert_eq!(out, vec![JitterOutput::Packet(vec![1])]);

        let out = buf.pump(t0 + Duration::from_millis(80));
        assert_eq!(out, vec![JitterOutput::Packet(vec![2])]);

        let out = buf.pump(t0 + Duration::from_millis(100));
        assert_eq!(out, vec![JitterOutput::Packet(vec![3])]);
    }

    #[test]
    fn gap_produces_bounded_plc_then_resumes() {
        let mut buf = JitterBuffer::new(config());
        let t0 = Instant::now();
        buf.push(1, vec![1], t0);
        buf.push(5, vec![5], t0);

        // seq 1 plays at 60 ms.
        let out = buf.pump(t0 + Duration::from_millis(60));
        assert_eq!(out, vec![JitterOutput::Packet(vec![1])]);

        // seq 2..4 are missing: three PLC frames, one per 20 ms.
        assert_eq!(
            buf.pump(t0 + Duration::from_millis(80)),
            vec![JitterOutput::Plc]
        );
        assert_eq!(
            buf.pump(t0 + Duration::from_millis(100)),
            vec![JitterOutput::Plc]
        );
        assert_eq!(
            buf.pump(t0 + Duration::from_millis(120)),
            vec![JitterOutput::Plc]
        );

        // Gap exceeds max PLC: skip to seq 5.
        let out = buf.pump(t0 + Duration::from_millis(140));
        assert_eq!(out, vec![JitterOutput::Packet(vec![5])]);
        assert_eq!(buf.stats().plc_frames, 3);
    }

    #[test]
    fn late_packets_are_dropped() {
        let mut buf = JitterBuffer::new(config());
        let t0 = Instant::now();
        buf.push(2, vec![2], t0);
        buf.pump(t0 + Duration::from_millis(60));

        // next_seq is 3 now; seq 2 is late.
        assert!(!buf.push(2, vec![2], t0 + Duration::from_millis(61)));
        assert_eq!(buf.stats().late_dropped, 1);
    }

    #[test]
    fn duplicates_are_ignored() {
        let mut buf = JitterBuffer::new(config());
        let t0 = Instant::now();
        assert!(buf.push(1, vec![1], t0));
        assert!(!buf.push(1, vec![9], t0));
        assert_eq!(buf.stats().duplicates, 1);
        assert_eq!(buf.depth(), 1);
    }

    #[test]
    fn underrun_resumes_at_next_packet() {
        let mut buf = JitterBuffer::new(config());
        let t0 = Instant::now();
        buf.push(1, vec![1], t0);
        assert_eq!(
            buf.pump(t0 + Duration::from_millis(60)),
            vec![JitterOutput::Packet(vec![1])]
        );

        // No packet for seq 2: playback waits.
        assert!(buf.pump(t0 + Duration::from_millis(80)).is_empty());
        assert!(buf.is_waiting());

        let late = t0 + Duration::from_millis(500);
        buf.push(2, vec![2], late);
        let out = buf.pump(late);
        assert_eq!(out, vec![JitterOutput::Packet(vec![2])]);
    }

    #[test]
    fn sequence_wraps() {
        let mut buf = JitterBuffer::new(config());
        let t0 = Instant::now();
        buf.push(u16::MAX, vec![0xFF], t0);
        buf.push(0, vec![0], t0);
        buf.push(1, vec![1], t0);

        assert_eq!(
            buf.pump(t0 + Duration::from_millis(60)),
            vec![JitterOutput::Packet(vec![0xFF])]
        );
        assert_eq!(
            buf.pump(t0 + Duration::from_millis(80)),
            vec![JitterOutput::Packet(vec![0])]
        );
        assert_eq!(
            buf.pump(t0 + Duration::from_millis(100)),
            vec![JitterOutput::Packet(vec![1])]
        );
    }

    #[test]
    fn new_burst_with_older_seq_restarts() {
        let mut buf = JitterBuffer::new(config());
        let t0 = Instant::now();
        buf.push(100, vec![100], t0);
        assert_eq!(
            buf.pump(t0 + Duration::from_millis(60)),
            vec![JitterOutput::Packet(vec![100])]
        );
        assert!(buf.pump(t0 + Duration::from_millis(80)).is_empty());
        assert!(buf.is_waiting());

        let later = t0 + Duration::from_secs(1);
        // Sequence 10 is behind the old stream; it starts a new burst.
        assert!(buf.push(10, vec![10], later));
        assert_eq!(
            buf.pump(later + Duration::from_millis(60)),
            vec![JitterOutput::Packet(vec![10])]
        );
    }

    #[test]
    fn capacity_drops_farthest_packet() {
        let mut buf = JitterBuffer::new(JitterBufferConfig {
            max_buffered_frames: 2,
            ..config()
        });
        let t0 = Instant::now();
        buf.push(1, vec![1], t0);
        buf.push(2, vec![2], t0);
        buf.push(3, vec![3], t0);

        assert_eq!(buf.depth(), 2);
        assert_eq!(buf.stats().capacity_dropped, 1);
        // seq 3 was farthest from next_seq (1) and got dropped.
        buf.push(9, vec![9], t0);
        assert_eq!(buf.depth(), 2);
    }
}
