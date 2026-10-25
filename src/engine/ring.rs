//! Lock-free single-producer/single-consumer f32 ring buffer and resamplers.
//!
//! Every device and network stream has its own clock. Like the original (which
//! resamples every audio point against the A1 master clock), each consumer uses an
//! adaptive linear resampler that tracks the ring fill level to absorb drift.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Ring {
    buf: Box<[UnsafeCell<f32>]>,
    cap: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

// SAFETY: producer only writes slots in [head, tail+cap), consumer only reads
// [tail, head); indices are published with Release/Acquire.
unsafe impl Sync for Ring {}
unsafe impl Send for Ring {}

pub struct Producer {
    r: Arc<Ring>,
}

pub struct Consumer {
    r: Arc<Ring>,
}

pub fn ring(cap: usize) -> (Producer, Consumer) {
    let cap = cap.max(16);
    let buf = (0..cap).map(|_| UnsafeCell::new(0.0)).collect::<Vec<_>>().into_boxed_slice();
    let r = Arc::new(Ring { buf, cap, head: AtomicUsize::new(0), tail: AtomicUsize::new(0) });
    (Producer { r: r.clone() }, Consumer { r })
}

impl Producer {
    pub fn free(&self) -> usize {
        let h = self.r.head.load(Ordering::Relaxed);
        let t = self.r.tail.load(Ordering::Acquire);
        self.r.cap - (h - t)
    }

    /// Push as much as fits; returns samples written.
    pub fn push(&mut self, data: &[f32]) -> usize {
        let n = data.len().min(self.free());
        let h = self.r.head.load(Ordering::Relaxed);
        for (i, x) in data[..n].iter().enumerate() {
            unsafe { *self.r.buf[(h + i) % self.r.cap].get() = *x };
        }
        self.r.head.store(h + n, Ordering::Release);
        n
    }
}

impl Consumer {
    pub fn available(&self) -> usize {
        let h = self.r.head.load(Ordering::Acquire);
        let t = self.r.tail.load(Ordering::Relaxed);
        h - t
    }

    pub fn capacity(&self) -> usize {
        self.r.cap
    }

    pub fn pop(&mut self, out: &mut [f32]) -> usize {
        let n = out.len().min(self.available());
        let t = self.r.tail.load(Ordering::Relaxed);
        for (i, o) in out[..n].iter_mut().enumerate() {
            *o = unsafe { *self.r.buf[(t + i) % self.r.cap].get() };
        }
        self.r.tail.store(t + n, Ordering::Release);
        n
    }

    pub fn skip(&mut self, n: usize) {
        let n = n.min(self.available());
        let t = self.r.tail.load(Ordering::Relaxed);
        self.r.tail.store(t + n, Ordering::Release);
    }
}

/// Consumer that resamples `ch`-channel interleaved audio from the producer's rate to
/// the consumer's rate, adapting the ratio to keep the ring near `target` frames.
pub struct AdaptiveReader {
    cons: Consumer,
    ch: usize,
    nominal: f64,
    pos: f64,
    hist: Vec<f32>,
    target: usize,
    primed: bool,
    tmp: Vec<f32>,
    pub underruns: u32,
}

impl AdaptiveReader {
    /// `ratio` = producer_sr / consumer_sr; `target_frames` = desired latency.
    pub fn new(cons: Consumer, ch: usize, ratio: f64, target_frames: usize) -> Self {
        AdaptiveReader {
            cons,
            ch: ch.max(1),
            nominal: ratio,
            pos: 0.0,
            hist: Vec::with_capacity(16384 * ch.max(1)),
            target: target_frames.max(32),
            primed: false,
            tmp: vec![0.0; 512 * ch.max(1)],
            underruns: 0,
        }
    }

    pub fn set_ratio(&mut self, ratio: f64) {
        if ratio.is_finite() && ratio > 0.0 {
            self.nominal = ratio;
        }
    }

    pub fn channels(&self) -> usize {
        self.ch
    }

    pub fn fill_frames(&self) -> usize {
        self.cons.available() / self.ch + self.hist.len() / self.ch
    }

    /// Fill `out` (interleaved, `frames` × ch). Missing data is zero-filled.
    pub fn read(&mut self, out: &mut [f32], frames: usize) {
        let ch = self.ch;
        let out = &mut out[..frames * ch];
        if !self.primed {
            if self.fill_frames() >= self.target {
                self.primed = true;
            } else {
                out.fill(0.0);
                return;
            }
        }
        // Too much latency accumulated (e.g. after a stall): drop to target.
        let fill = self.fill_frames();
        if fill > self.target * 4 + frames {
            let drop = (fill - self.target) * ch;
            let from_hist = drop.min(self.hist.len());
            self.hist.drain(..from_hist);
            self.cons.skip(drop - from_hist);
            self.pos = 0.0;
        }
        let err = (self.fill_frames() as f64 - self.target as f64) / self.target as f64;
        let ratio = self.nominal * (1.0 + (err * 0.002).clamp(-0.005, 0.005));

        for f in 0..frames {
            let idx = self.pos as usize;
            while self.hist.len() / ch < idx + 2 {
                let got = self.cons.pop(&mut self.tmp);
                let got = got - got % ch;
                if got == 0 {
                    break;
                }
                self.hist.extend_from_slice(&self.tmp[..got]);
            }
            if self.hist.len() / ch < idx + 2 {
                out[f * ch..].fill(0.0);
                self.primed = false;
                self.underruns += 1;
                break;
            }
            let frac = (self.pos - idx as f64) as f32;
            let a = idx * ch;
            let b = a + ch;
            for c in 0..ch {
                let x0 = self.hist[a + c];
                let x1 = self.hist[b + c];
                out[f * ch + c] = x0 + (x1 - x0) * frac;
            }
            self.pos += ratio;
        }
        let consumed = (self.pos as usize).min(self.hist.len() / ch);
        self.hist.drain(..consumed * ch);
        self.pos -= consumed as f64;
    }
}

/// Fixed-ratio streaming linear resampler (VBAN out, file playback loading).
pub struct StreamResampler {
    ch: usize,
    ratio: f64,
    pos: f64,
    buf: Vec<f32>,
}

impl StreamResampler {
    /// `ratio` = in_sr / out_sr.
    pub fn new(ch: usize, ratio: f64) -> Self {
        StreamResampler { ch: ch.max(1), ratio, pos: 0.0, buf: Vec::new() }
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let ch = self.ch;
        if (self.ratio - 1.0).abs() < 1e-12 {
            out.extend_from_slice(input);
            return;
        }
        self.buf.extend_from_slice(input);
        let frames = self.buf.len() / ch;
        while (self.pos as usize) + 1 < frames {
            let i = self.pos as usize;
            let frac = (self.pos - i as f64) as f32;
            for c in 0..ch {
                let x0 = self.buf[i * ch + c];
                let x1 = self.buf[(i + 1) * ch + c];
                out.push(x0 + (x1 - x0) * frac);
            }
            self.pos += self.ratio;
        }
        let keep_from = (self.pos as usize).min(frames.saturating_sub(1));
        self.buf.drain(..keep_from * ch);
        self.pos -= keep_from as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_push_pop_wraps() {
        let (mut p, mut c) = ring(16);
        let mut out = [0.0; 10];
        for round in 0..5 {
            let data: Vec<f32> = (0..10).map(|i| (round * 10 + i) as f32).collect();
            assert_eq!(p.push(&data), 10);
            assert_eq!(c.pop(&mut out), 10);
            assert_eq!(out.to_vec(), data);
        }
        assert_eq!(p.push(&[0.0; 40]), 16);
    }

    #[test]
    fn adaptive_reader_passthrough_at_unity() {
        let (mut p, c) = ring(4096);
        let data: Vec<f32> = (0..2000).map(|i| i as f32).collect();
        p.push(&data);
        // 1000 frames buffered == target, so the ratio stays exactly nominal.
        let mut r = AdaptiveReader::new(c, 2, 1.0, 1000);
        let mut out = vec![0.0; 200];
        r.read(&mut out, 100);
        assert_eq!(out, data[..200].to_vec());
    }

    #[test]
    fn stream_resampler_halves() {
        let mut r = StreamResampler::new(1, 2.0);
        let mut out = Vec::new();
        r.process(&(0..100).map(|i| i as f32).collect::<Vec<_>>(), &mut out);
        assert_eq!(&out[..4], &[0.0, 2.0, 4.0, 6.0]);
        assert!((out.len() as i32 - 50).abs() <= 1);
    }

    #[test]
    fn stream_resampler_continuous_across_chunks() {
        let mut r = StreamResampler::new(1, 0.5);
        let mut out = Vec::new();
        r.process(&[0.0, 1.0, 2.0], &mut out);
        r.process(&[3.0, 4.0], &mut out);
        assert_eq!(out, vec![0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5]);
    }
}
