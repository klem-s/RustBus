//! VBAN network I/O: UDP receiver / sender threads, live stats and the TEXT command sender.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use super::proto::*;
use crate::ring::{Consumer, Producer, StreamResampler};

/// Live statistics per incoming stream, shown in the VBAN dialog.
#[derive(Default)]
pub struct InStats {
    pub packets: AtomicU32,
    pub errors: AtomicU32,
    pub sr: AtomicU32,
    pub channels: AtomicU32,
    pub format: AtomicU32,
}

#[derive(Default)]
pub struct OutStats {
    pub packets: AtomicU32,
}

pub struct VbanStats {
    pub inputs: Vec<InStats>,
    pub outputs: Vec<OutStats>,
    pub port_error: AtomicBool,
}

impl Default for VbanStats {
    fn default() -> Self {
        VbanStats {
            inputs: (0..NUM_STREAMS).map(|_| InStats::default()).collect(),
            outputs: (0..NUM_STREAMS).map(|_| OutStats::default()).collect(),
            port_error: AtomicBool::new(false),
        }
    }
}

/// Producer side of an incoming stream, owned by the receiver thread.
pub struct InSink {
    pub stream: usize,
    pub channels: usize,
    pub prod: Producer,
}

/// Consumer side of an outgoing stream, owned by the sender thread (engine SR, `channels` ch).
pub struct OutSource {
    pub stream: usize,
    pub cons: Consumer,
}

pub struct VbanRuntime {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl VbanRuntime {
    pub fn start(
        cfg: &VbanConfig,
        engine_sr: u32,
        sinks: Vec<InSink>,
        sources: Vec<OutSource>,
        stats: Arc<VbanStats>,
        text_tx: Sender<String>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> VbanRuntime {
        let stop = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();
        stats.port_error.store(false, Ordering::Relaxed);
        if !cfg.enabled {
            return VbanRuntime { stop, threads };
        }
        let port = cfg.listen_port();
        let bound = UdpSocket::bind(("0.0.0.0", port));
        let send_sock = match &bound {
            Ok(s) => s.try_clone().ok(),
            Err(_) => {
                stats.port_error.store(true, Ordering::Relaxed);
                UdpSocket::bind(("0.0.0.0", 0)).ok()
            }
        };
        if let Ok(sock) = bound {
            let _ = sock.set_read_timeout(Some(Duration::from_millis(50)));
            let cfg = cfg.clone();
            let stop = stop.clone();
            let stats = stats.clone();
            threads.push(std::thread::spawn(move || receiver(sock, cfg, sinks, stats, text_tx, wake, stop)));
        }
        if let Some(sock) = send_sock {
            let _ = sock.set_broadcast(true);
            let cfg = cfg.clone();
            let stop = stop.clone();
            threads.push(std::thread::spawn(move || sender(sock, cfg, engine_sr, sources, stats, stop)));
        }
        VbanRuntime { stop, threads }
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl Drop for VbanRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

fn ip_matches(filter: &str, src: IpAddr) -> bool {
    let f = filter.trim();
    f.is_empty() || f == "0.0.0.0" || f.parse::<IpAddr>().map(|ip| ip == src).unwrap_or(false)
}

fn receiver(
    sock: UdpSocket,
    cfg: VbanConfig,
    mut sinks: Vec<InSink>,
    stats: Arc<VbanStats>,
    text_tx: Sender<String>,
    wake: Arc<dyn Fn() + Send + Sync>,
    stop: Arc<AtomicBool>,
) {
    let mut buf = [0u8; 2048];
    let mut samples = Vec::with_capacity(4096);
    let mut mapped = Vec::with_capacity(4096);
    let mut last_frame = [None::<u32>; NUM_STREAMS];
    while !stop.load(Ordering::Relaxed) {
        let (n, from) = match sock.recv_from(&mut buf) {
            Ok(x) => x,
            Err(_) => continue,
        };
        let Some((h, payload)) = Header::decode(&buf[..n]) else { continue };
        match h.proto {
            PROTO_AUDIO => {
                let Some(si) = cfg.inputs.iter().position(|s| s.enabled && s.name.eq_ignore_ascii_case(&h.name) && ip_matches(&s.ip, from.ip()))
                else {
                    continue;
                };
                let st = &stats.inputs[si];
                st.packets.fetch_add(1, Ordering::Relaxed);
                if let Some(prev) = last_frame[si] {
                    if h.frame != prev.wrapping_add(1) {
                        st.errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
                last_frame[si] = Some(h.frame);
                st.sr.store(h.sample_rate(), Ordering::Relaxed);
                st.channels.store(h.channels as u32, Ordering::Relaxed);
                st.format.store(h.data_type as u32, Ordering::Relaxed);
                if h.codec != 0 || sample_bytes(h.data_type) == 0 {
                    st.errors.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                samples.clear();
                decode_samples(h.data_type, payload, &mut samples);
                let Some(sink) = sinks.iter_mut().find(|s| s.stream == si) else { continue };
                // Map stream channels onto the destination strip's channel count.
                let src_ch = h.channels as usize;
                let dst_ch = sink.channels;
                mapped.clear();
                for frame in samples.chunks_exact(src_ch) {
                    for c in 0..dst_ch {
                        mapped.push(if c < src_ch { frame[c] } else if src_ch == 1 && c == 1 { frame[0] } else { 0.0 });
                    }
                }
                sink.prod.push(&mapped);
            }
            PROTO_TEXT => {
                let c = &cfg.command_in;
                if c.enabled && c.name.eq_ignore_ascii_case(&h.name) && ip_matches(&c.ip, from.ip()) {
                    let text = String::from_utf8_lossy(payload).trim_end_matches('\0').to_string();
                    if text_tx.send(text).is_ok() {
                        wake();
                    }
                }
            }
            _ => {}
        }
    }
}

struct OutState {
    src: OutSource,
    dest: Option<SocketAddr>,
    resampler: StreamResampler,
    pending: Vec<f32>,
    frame: u32,
    ch: usize,
    sr_index: u8,
    per_packet: usize,
}

fn sender(sock: UdpSocket, cfg: VbanConfig, engine_sr: u32, sources: Vec<OutSource>, stats: Arc<VbanStats>, stop: Arc<AtomicBool>) {
    let mut outs: Vec<OutState> = sources
        .into_iter()
        .filter_map(|src| {
            let s = &cfg.outputs[src.stream];
            let sr = if sr_index(s.sr).is_some() { s.sr } else { engine_sr };
            let ch = s.channels.clamp(1, 8) as usize;
            let bytes = sample_bytes(s.format).max(1);
            let dest = format!("{}:{}", s.ip.trim(), s.port).parse::<SocketAddr>().ok();
            Some(OutState {
                src,
                dest,
                resampler: StreamResampler::new(ch, engine_sr as f64 / sr as f64),
                pending: Vec::with_capacity(8192),
                frame: 0,
                ch,
                sr_index: sr_index(sr)?,
                per_packet: (MAX_PAYLOAD / (ch * bytes)).min(256),
            })
        })
        .collect();
    let mut tmp = vec![0.0f32; 8192];
    let mut pkt = Vec::with_capacity(HEADER_LEN + MAX_PAYLOAD);
    let mut hdr = [0u8; HEADER_LEN];
    while !stop.load(Ordering::Relaxed) {
        for o in outs.iter_mut() {
            let n = o.src.cons.pop(&mut tmp);
            if n == 0 {
                continue;
            }
            o.resampler.process(&tmp[..n], &mut o.pending);
            let s = &cfg.outputs[o.src.stream];
            while o.pending.len() >= o.per_packet * o.ch {
                let chunk = o.per_packet * o.ch;
                if let Some(dest) = o.dest {
                    let h = Header {
                        proto: PROTO_AUDIO,
                        sr_index: o.sr_index,
                        samples: o.per_packet as u16,
                        channels: o.ch as u16,
                        data_type: s.format,
                        codec: 0,
                        name: s.name.clone(),
                        frame: o.frame,
                    };
                    h.encode(&mut hdr);
                    pkt.clear();
                    pkt.extend_from_slice(&hdr);
                    encode_samples(s.format, &o.pending[..chunk], &mut pkt);
                    if sock.send_to(&pkt, dest).is_ok() {
                        stats.outputs[o.src.stream].packets.fetch_add(1, Ordering::Relaxed);
                    }
                }
                o.frame = o.frame.wrapping_add(1);
                o.pending.drain(..chunk);
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Send one VBAN-TEXT command packet (used by the "send command" tool in the UI).
pub fn send_text(dest: SocketAddr, stream: &str, text: &str) -> std::io::Result<()> {
    let sock = UdpSocket::bind(("0.0.0.0", 0))?;
    let h = Header { proto: PROTO_TEXT, sr_index: 0, samples: 1, channels: 1, data_type: 0, codec: 0x10, name: stream.into(), frame: 0 };
    let mut hdr = [0u8; HEADER_LEN];
    h.encode(&mut hdr);
    let mut pkt = hdr.to_vec();
    pkt.extend_from_slice(text.as_bytes());
    sock.send_to(&pkt, dest).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_audio_and_text() {
        use crate::ring::ring;
        let mut cfg = VbanConfig::default();
        cfg.enabled = true;
        let port = 39000 + (std::process::id() % 1000) as u16;
        cfg.inputs[0].enabled = true;
        cfg.inputs[0].port = port;
        cfg.command_in.enabled = true;
        cfg.command_in.port = port;
        cfg.outputs[0] = StreamOut { enabled: true, name: "Stream1".into(), ip: "127.0.0.1".into(), port, bus: 0, sr: 48000, channels: 2, nq: 0, format: FMT_F32 };
        let (in_prod, mut in_cons) = ring(65536);
        let (mut out_prod, out_cons) = ring(65536);
        let stats = Arc::new(VbanStats::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut rt = VbanRuntime::start(
            &cfg,
            48000,
            vec![InSink { stream: 0, channels: 2, prod: in_prod }],
            vec![OutSource { stream: 0, cons: out_cons }],
            stats.clone(),
            tx,
            Arc::new(|| {}),
        );
        // f32 stereo → 179 frames per packet; send exactly 4 packets.
        let n = 179 * 2 * 4;
        let data: Vec<f32> = (0..n).map(|i| (i % 100) as f32 / 100.0).collect();
        out_prod.push(&data);
        send_text(format!("127.0.0.1:{port}").parse().unwrap(), "Command1", "Strip[0].Mute = 1").unwrap();
        let t0 = std::time::Instant::now();
        while in_cons.available() < n && t0.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut got = vec![0.0; n];
        assert_eq!(in_cons.pop(&mut got), n);
        assert_eq!(got, data);
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), "Strip[0].Mute = 1");
        rt.stop();
    }
}
