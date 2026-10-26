//! VBAN (VB-Audio network protocol) — configuration, packet codec, network threads.
//! The protocol itself is publicly specified by VB-Audio; config element names follow
//! the settings format observed in the binary (ANALYSIS §6, §8).

use std::fmt::Write as _;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use roxmltree::Node;

use crate::engine::ring::{Consumer, Producer, StreamResampler};
use crate::xml::{ab, ai};

pub const DEFAULT_PORT: u16 = 6980;
pub const NUM_STREAMS: usize = 8;
pub const HEADER_LEN: usize = 28;
pub const MAX_PAYLOAD: usize = 1436;

pub const SR_TABLE: [u32; 21] = [
    6000, 12000, 24000, 48000, 96000, 192000, 384000, 8000, 16000, 32000, 64000, 128000, 256000, 512000, 11025, 22050, 44100,
    88200, 176400, 352800, 705600,
];

pub const PROTO_AUDIO: u8 = 0x00;
pub const PROTO_SERIAL: u8 = 0x20;
pub const PROTO_TEXT: u8 = 0x40;
pub const PROTO_SERVICE: u8 = 0x60;

/// Sample data types (low 3 bits of format_bit).
pub const FMT_U8: u8 = 0;
pub const FMT_I16: u8 = 1;
pub const FMT_I24: u8 = 2;
pub const FMT_I32: u8 = 3;
pub const FMT_F32: u8 = 4;
pub const FMT_F64: u8 = 5;

pub const FORMAT_NAMES: [&str; 6] = ["PCM 8 bits", "PCM 16 bits", "PCM 24 bits", "PCM 32 bits", "32 bits float", "64 bits float"];
pub const NQ_NAMES: [&str; 5] = ["Optimal", "Fast", "Medium", "Slow", "Very Slow"];

pub fn sample_bytes(fmt: u8) -> usize {
    match fmt {
        FMT_U8 => 1,
        FMT_I16 => 2,
        FMT_I24 => 3,
        FMT_I32 | FMT_F32 => 4,
        FMT_F64 => 8,
        _ => 0,
    }
}

pub fn sr_index(sr: u32) -> Option<u8> {
    SR_TABLE.iter().position(|x| *x == sr).map(|i| i as u8)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub proto: u8,
    pub sr_index: u8,
    /// Samples per frame (1..=256). For TEXT/SERIAL this field is protocol-specific.
    pub samples: u16,
    pub channels: u16,
    pub data_type: u8,
    pub codec: u8,
    pub name: String,
    pub frame: u32,
}

impl Header {
    pub fn sample_rate(&self) -> u32 {
        SR_TABLE.get(self.sr_index as usize).copied().unwrap_or(0)
    }

    pub fn encode(&self, out: &mut [u8; HEADER_LEN]) {
        out[..4].copy_from_slice(b"VBAN");
        out[4] = (self.proto & 0xE0) | (self.sr_index & 0x1F);
        out[5] = (self.samples.clamp(1, 256) - 1) as u8;
        out[6] = (self.channels.clamp(1, 256) - 1) as u8;
        out[7] = (self.codec & 0xF0) | (self.data_type & 0x07);
        out[8..24].fill(0);
        let n = self.name.len().min(16);
        out[8..8 + n].copy_from_slice(&self.name.as_bytes()[..n]);
        out[24..28].copy_from_slice(&self.frame.to_le_bytes());
    }

    pub fn decode(pkt: &[u8]) -> Option<(Header, &[u8])> {
        if pkt.len() < HEADER_LEN || &pkt[..4] != b"VBAN" {
            return None;
        }
        let name_raw = &pkt[8..24];
        let end = name_raw.iter().position(|b| *b == 0).unwrap_or(16);
        Some((
            Header {
                proto: pkt[4] & 0xE0,
                sr_index: pkt[4] & 0x1F,
                samples: pkt[5] as u16 + 1,
                channels: pkt[6] as u16 + 1,
                data_type: pkt[7] & 0x07,
                codec: pkt[7] & 0xF0,
                name: String::from_utf8_lossy(&name_raw[..end]).into_owned(),
                frame: u32::from_le_bytes([pkt[24], pkt[25], pkt[26], pkt[27]]),
            },
            &pkt[HEADER_LEN..],
        ))
    }
}

pub fn encode_samples(fmt: u8, input: &[f32], out: &mut Vec<u8>) {
    for &x in input {
        let x = x.clamp(-1.0, 1.0);
        match fmt {
            FMT_U8 => out.push(((x * 127.0) as i32 + 128) as u8),
            FMT_I16 => out.extend_from_slice(&((x * 32767.0) as i16).to_le_bytes()),
            FMT_I24 => out.extend_from_slice(&((x * 8_388_607.0) as i32).to_le_bytes()[..3]),
            FMT_I32 => out.extend_from_slice(&((x as f64 * 2_147_483_647.0) as i32).to_le_bytes()),
            FMT_F32 => out.extend_from_slice(&x.to_le_bytes()),
            FMT_F64 => out.extend_from_slice(&(x as f64).to_le_bytes()),
            _ => {}
        }
    }
}

pub fn decode_samples(fmt: u8, data: &[u8], out: &mut Vec<f32>) {
    match fmt {
        FMT_U8 => out.extend(data.iter().map(|b| (*b as f32 - 128.0) / 128.0)),
        FMT_I16 => out.extend(data.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)),
        FMT_I24 => out.extend(data.chunks_exact(3).map(|c| (i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8) as f32 / 8_388_608.0)),
        FMT_I32 => out.extend(data.chunks_exact(4).map(|c| (i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64 / 2_147_483_648.0) as f32)),
        FMT_F32 => out.extend(data.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))),
        FMT_F64 => out.extend(data.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap()) as f32)),
        _ => {}
    }
}

// ---------------------------------------------------------------- configuration

#[derive(Clone, Debug, PartialEq)]
pub struct StreamIn {
    pub enabled: bool,
    pub name: String,
    pub ip: String,
    pub port: u16,
    /// Destination strip (0-based).
    pub strip: usize,
    /// Net quality 0..4 → jitter buffer size.
    pub nq: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StreamOut {
    pub enabled: bool,
    pub name: String,
    pub ip: String,
    pub port: u16,
    /// Source bus (0-based).
    pub bus: usize,
    pub sr: u32,
    pub channels: u8,
    pub nq: u8,
    pub format: u8,
}

/// Command (TEXT) / MIDI data / video entries we keep for file fidelity.
#[derive(Clone, Debug, PartialEq)]
pub struct GenericEntry {
    pub enabled: bool,
    pub name: String,
    pub ip: String,
    pub port: u16,
    pub extra: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VbanConfig {
    pub enabled: bool,
    pub username: String,
    pub color: u32,
    pub inputs: Vec<StreamIn>,
    pub outputs: Vec<StreamOut>,
    pub command_in: GenericEntry,
    pub data_in: GenericEntry,
    pub data_out: GenericEntry,
    pub frame_out: GenericEntry,
}

impl Default for VbanConfig {
    fn default() -> Self {
        let g = |name: &str, extra: &[(&str, &str)]| GenericEntry {
            enabled: false,
            name: name.into(),
            ip: String::new(),
            port: DEFAULT_PORT,
            extra: extra.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
        };
        VbanConfig {
            enabled: false,
            username: String::new(),
            color: 0x00AA9678,
            inputs: (0..NUM_STREAMS)
                .map(|i| StreamIn { enabled: false, name: format!("Stream{}", i + 1), ip: String::new(), port: DEFAULT_PORT, strip: if i == 0 { 2 } else { 0 }, nq: 1 })
                .collect(),
            outputs: (0..NUM_STREAMS)
                .map(|i| StreamOut {
                    enabled: false,
                    name: format!("Stream{}", i + 1),
                    ip: String::new(),
                    port: DEFAULT_PORT,
                    bus: 0,
                    sr: 48000,
                    channels: 2,
                    nq: 0,
                    format: FMT_I16,
                })
                .collect(),
            command_in: g("Command1", &[("mouse", "0")]),
            data_in: g("MIDI1", &[]),
            data_out: g("MIDI1", &[("source", "7")]),
            frame_out: g("VIDEO1", &[("source", "0"), ("mbps", "12"), ("fps", "20"), ("cursor", "0"), ("format", "JPG"), ("quality", "50")]),
        }
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('\'', "&apos;").replace('"', "&quot;")
}

impl VbanConfig {
    pub fn write_items(&self, o: &mut String) {
        let _ = writeln!(o, "\t<VBAN status=\"{}\" username=\"{}\" color=\"{:08X}\" />", self.enabled as i32, esc(&self.username), self.color);
        for (i, s) in self.inputs.iter().enumerate() {
            let _ = writeln!(
                o,
                "\t<VBANStreamIn index='{}' in='{}' status='{}' name='{}' ip='{}' port='{}' NQ='{}' />",
                i + 1,
                s.strip,
                s.enabled as i32,
                esc(&s.name),
                esc(&s.ip),
                s.port,
                s.nq
            );
        }
        // Attribute order follows the original format strings (ANALYSIS strings 954–958).
        let gen = |o: &mut String, tag: &str, e: &GenericEntry| {
            let get = |k: &str| e.extra.iter().find(|(n, _)| n == k).map(|(_, v)| esc(v));
            let _ = write!(o, "\t<{tag} index='1'");
            if let Some(v) = get("source") {
                let _ = write!(o, " source='{v}'");
            }
            let _ = write!(o, " status='{}' name='{}'", e.enabled as i32, esc(&e.name));
            if let Some(v) = get("mouse") {
                let _ = write!(o, " mouse='{v}'");
            }
            let _ = write!(o, " ip='{}' port='{}'", esc(&e.ip), e.port);
            for (k, v) in &e.extra {
                if k != "source" && k != "mouse" {
                    let _ = write!(o, " {k}='{}'", esc(v));
                }
            }
            o.push_str(" />\n");
        };
        gen(o, "VBANDataIn", &self.data_in);
        gen(o, "VBANCommandIn", &self.command_in);
        for (i, s) in self.outputs.iter().enumerate() {
            let _ = writeln!(
                o,
                "\t<VBANStreamOut index='{}' bus='{}' status='{}' name='{}' ip='{}' port='{}' SR='{}' ch='{}' NQ='{}' format='{}' />",
                i + 1,
                s.bus,
                s.enabled as i32,
                esc(&s.name),
                esc(&s.ip),
                s.port,
                s.sr,
                s.channels,
                s.nq,
                s.format
            );
        }
        gen(o, "VBANDataOut", &self.data_out);
        gen(o, "VBANFrameOut", &self.frame_out);
    }

    pub fn read(sec: Node) -> VbanConfig {
        let mut c = VbanConfig::default();
        for n in sec.children().filter(Node::is_element) {
            let idx = ai(n, "index").unwrap_or(1).max(1) as usize - 1;
            let name = n.attribute("name").unwrap_or("").to_string();
            let ip = n.attribute("ip").unwrap_or("").to_string();
            let port = ai(n, "port").unwrap_or(DEFAULT_PORT as i64) as u16;
            let enabled = ab(n, "status").unwrap_or(false);
            let generic = |e: &mut GenericEntry| {
                e.enabled = enabled;
                e.name = name.clone();
                e.ip = ip.clone();
                e.port = port;
                for (k, v) in e.extra.iter_mut() {
                    if let Some(a) = n.attribute(k.as_str()) {
                        *v = a.to_string();
                    }
                }
            };
            match n.tag_name().name() {
                "VBAN" => {
                    c.enabled = enabled;
                    c.username = n.attribute("username").unwrap_or("").to_string();
                    c.color = u32::from_str_radix(n.attribute("color").unwrap_or("0"), 16).unwrap_or(c.color);
                }
                "VBANStreamIn" if idx < NUM_STREAMS => {
                    let s = &mut c.inputs[idx];
                    s.enabled = enabled;
                    s.name = name;
                    s.ip = ip;
                    s.port = port;
                    s.strip = ai(n, "in").unwrap_or(0).clamp(0, crate::params::NUM_STRIPS as i64 - 1) as usize;
                    s.nq = ai(n, "NQ").unwrap_or(1).clamp(0, 4) as u8;
                }
                "VBANStreamOut" if idx < NUM_STREAMS => {
                    let s = &mut c.outputs[idx];
                    s.enabled = enabled;
                    s.name = name;
                    s.ip = ip;
                    s.port = port;
                    s.bus = ai(n, "bus").unwrap_or(0).clamp(0, crate::params::NUM_BUSES as i64 - 1) as usize;
                    s.sr = ai(n, "SR").unwrap_or(48000) as u32;
                    s.channels = ai(n, "ch").unwrap_or(2).clamp(1, 8) as u8;
                    s.nq = ai(n, "NQ").unwrap_or(0).clamp(0, 4) as u8;
                    s.format = ai(n, "format").unwrap_or(FMT_I16 as i64).clamp(0, 5) as u8;
                }
                "VBANCommandIn" => generic(&mut c.command_in),
                "VBANDataIn" => generic(&mut c.data_in),
                "VBANDataOut" => generic(&mut c.data_out),
                "VBANFrameOut" => generic(&mut c.frame_out),
                _ => {}
            }
        }
        c
    }

    /// The UDP port the receiver listens on (the original uses one shared port).
    pub fn listen_port(&self) -> u16 {
        self.inputs
            .iter()
            .filter(|s| s.enabled)
            .map(|s| s.port)
            .chain(self.command_in.enabled.then_some(self.command_in.port))
            .next()
            .unwrap_or(DEFAULT_PORT)
    }
}

// ---------------------------------------------------------------- runtime

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
    fn header_roundtrip() {
        let h = Header { proto: PROTO_AUDIO, sr_index: 3, samples: 256, channels: 2, data_type: FMT_I16, codec: 0, name: "Stream1".into(), frame: 42 };
        let mut b = [0u8; HEADER_LEN];
        h.encode(&mut b);
        assert_eq!(&b[..8], &[b'V', b'B', b'A', b'N', 3, 255, 1, 1]);
        let (d, rest) = Header::decode(&b).unwrap();
        assert_eq!(d, h);
        assert!(rest.is_empty());
        assert_eq!(d.sample_rate(), 48000);
    }

    #[test]
    fn sample_codecs_roundtrip() {
        let x = [0.0f32, 0.5, -0.5, 0.999, -1.0];
        for fmt in [FMT_I16, FMT_I24, FMT_I32, FMT_F32, FMT_F64] {
            let mut bytes = Vec::new();
            encode_samples(fmt, &x, &mut bytes);
            assert_eq!(bytes.len(), x.len() * sample_bytes(fmt));
            let mut back = Vec::new();
            decode_samples(fmt, &bytes, &mut back);
            for (a, b) in x.iter().zip(&back) {
                assert!((a - b).abs() < 1e-3, "fmt {fmt}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn rejects_non_vban() {
        assert!(Header::decode(b"XXXX0000000000000000000000000000").is_none());
        assert!(Header::decode(b"VBAN").is_none());
    }

    #[test]
    fn config_roundtrip() {
        let mut c = VbanConfig::default();
        c.enabled = true;
        c.username = "me".into();
        c.inputs[3] = StreamIn { enabled: true, name: "Mic".into(), ip: "10.0.0.2".into(), port: 6981, strip: 1, nq: 3 };
        c.outputs[7].format = FMT_F32;
        c.command_in.enabled = true;
        c.frame_out.extra[1].1 = "8".into();
        let mut xml = String::from("<VBANConfiguration>\n");
        c.write_items(&mut xml);
        xml.push_str("</VBANConfiguration>\n");
        let doc = roxmltree::Document::parse(&xml).unwrap();
        assert_eq!(VbanConfig::read(doc.root_element()), c, "{xml}");
    }

    #[test]
    fn loopback_audio_and_text() {
        use crate::engine::ring::ring;
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
