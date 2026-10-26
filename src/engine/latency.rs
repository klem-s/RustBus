//! Live latency figures published by the audio threads for the Latency Monitor.
//!
//! Every value is a duration in microseconds stored in an atomic, so the real-time
//! callbacks only do a relaxed store. 0 means "not measured" (point closed or idle).

use std::sync::atomic::{AtomicU32, Ordering};

use crate::params::{NUM_BUSES, NUM_STRIPS};
use crate::vban::NUM_STREAMS;

#[derive(Default)]
pub struct Latency {
    /// Callback period of each WDM input strip's device.
    pub in_period: [AtomicU32; NUM_STRIPS],
    /// Audio waiting in each WDM input strip's ring for the mixer.
    pub in_queue: [AtomicU32; NUM_STRIPS],
    /// Callback period of each WDM output bus's device (A2..B3; A1 is the master).
    pub out_period: [AtomicU32; NUM_BUSES],
    /// Audio waiting in each WDM output bus's ring for its device.
    pub out_queue: [AtomicU32; NUM_BUSES],
    /// Audio waiting in each incoming VBAN stream's ring.
    pub vban_queue: [AtomicU32; NUM_STREAMS],
    /// A1 master period: frames rendered per master callback (or internal clock tick).
    pub master_period: AtomicU32,
}

impl Latency {
    /// Forget every figure (engine restart: closed points must not show stale values).
    pub fn reset(&self) {
        let all = self.in_period.iter().chain(&self.in_queue).chain(&self.out_period).chain(&self.out_queue).chain(&self.vban_queue);
        for a in all.chain(std::iter::once(&self.master_period)) {
            a.store(0, Ordering::Relaxed);
        }
    }
}

/// Store the duration of `frames` at `sr` Hz (real-time safe).
#[inline]
pub fn store(slot: &AtomicU32, frames: usize, sr: u32) {
    let us = if sr == 0 { 0 } else { (frames as u64 * 1_000_000 / sr as u64).min(u32::MAX as u64) as u32 };
    slot.store(us, Ordering::Relaxed);
}

/// Read a figure in milliseconds.
pub fn ms(slot: &AtomicU32) -> f32 {
    slot.load(Ordering::Relaxed) as f32 / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_durations_in_microseconds() {
        let l = Latency::default();
        store(&l.master_period, 480, 48000);
        assert_eq!(ms(&l.master_period), 10.0);
        store(&l.in_queue[0], 441, 44100);
        assert_eq!(l.in_queue[0].load(Ordering::Relaxed), 10_000);
        store(&l.out_queue[1], 100, 0);
        assert_eq!(ms(&l.out_queue[1]), 0.0);
        l.reset();
        assert_eq!(ms(&l.master_period), 0.0);
        assert_eq!(ms(&l.in_queue[0]), 0.0);
    }
}
