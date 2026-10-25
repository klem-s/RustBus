//! Minimal ASIO host. ASIO drivers are in-process COM objects registered under
//! `HKLM\SOFTWARE\ASIO`; this talks to the driver's `IASIO` interface directly, so no
//! Steinberg SDK, bindgen or C++ compiler is needed.
//!
//! The driver has no user-data pointer in its callbacks and only one driver can be
//! loaded per process, so the running stream lives in a single global slot. Driver
//! calls are made from one owner thread (COM single-threaded apartment); audio
//! callbacks arrive on the driver's own thread.

use std::ffi::c_void;
use std::sync::mpsc::{channel, sync_channel, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::Mutex;

/// Callback that renders one block: `(inputs, outputs, frames)`, one Vec per channel.
pub type Process = Box<dyn FnMut(&[Vec<f32>], &mut [Vec<f32>], usize) + Send>;
/// Called (off the audio thread) when the driver asks the host to reopen it.
pub type OnReset = Box<dyn Fn(&str) + Send>;

// ---------------------------------------------------------------- Win32 / COM FFI

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Guid {
    d1: u32,
    d2: u16,
    d3: u16,
    d4: [u8; 8],
}

#[repr(C, align(8))]
struct Msg([u8; 64]);

type Hkey = isize;
const HKEY_LOCAL_MACHINE: Hkey = 0x8000_0002u32 as i32 as isize;
const KEY_READ: u32 = 0x2_0019;
const RRF_RT_REG_SZ: u32 = 0x2;
const CLSCTX_INPROC_SERVER: u32 = 0x1;
const COINIT_APARTMENTTHREADED: u32 = 0x2;
const PM_REMOVE: u32 = 0x1;

#[link(name = "advapi32")]
extern "system" {
    fn RegOpenKeyExW(key: Hkey, sub: *const u16, opts: u32, sam: u32, out: *mut Hkey) -> i32;
    fn RegEnumKeyExW(key: Hkey, i: u32, name: *mut u16, len: *mut u32, r: *mut u32, class: *mut u16, clen: *mut u32, t: *mut c_void) -> i32;
    fn RegGetValueW(key: Hkey, sub: *const u16, value: *const u16, flags: u32, ty: *mut u32, data: *mut c_void, len: *mut u32) -> i32;
    fn RegCloseKey(key: Hkey) -> i32;
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> i32;
    fn CoUninitialize();
    fn CoCreateInstance(clsid: *const Guid, outer: *mut c_void, ctx: u32, iid: *const Guid, out: *mut *mut c_void) -> i32;
    fn CLSIDFromString(s: *const u16, out: *mut Guid) -> i32;
}

#[link(name = "user32")]
extern "system" {
    fn GetDesktopWindow() -> *mut c_void;
    fn PeekMessageW(msg: *mut Msg, hwnd: *mut c_void, min: u32, max: u32, remove: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

fn from_cstr(buf: &[u8]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..n]).trim().to_string()
}

// ---------------------------------------------------------------- IASIO

const ASE_OK: i32 = 0;
const ASE_SUCCESS: i32 = 0x3f48_47a0;

#[repr(C)]
struct ChannelInfo {
    channel: i32,
    is_input: i32,
    is_active: i32,
    group: i32,
    sample_type: i32,
    name: [u8; 32],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct BufferInfo {
    is_input: i32,
    channel: i32,
    buffers: [*mut c_void; 2],
}

#[repr(C)]
struct Callbacks {
    buffer_switch: unsafe extern "C" fn(i32, i32),
    sample_rate_did_change: unsafe extern "C" fn(f64),
    asio_message: unsafe extern "C" fn(i32, i32, *mut c_void, *mut f64) -> i32,
    buffer_switch_time_info: unsafe extern "C" fn(*mut c_void, i32, i32) -> *mut c_void,
}

type This = *mut Iasio;

/// `IASIO` vtable (IUnknown first). On x64 every method uses the one Win64 ABI.
#[repr(C)]
struct Vtbl {
    _query_interface: usize,
    _add_ref: unsafe extern "system" fn(This) -> u32,
    release: unsafe extern "system" fn(This) -> u32,
    init: unsafe extern "system" fn(This, *mut c_void) -> i32,
    get_driver_name: unsafe extern "system" fn(This, *mut u8),
    _get_driver_version: unsafe extern "system" fn(This) -> i32,
    get_error_message: unsafe extern "system" fn(This, *mut u8),
    start: unsafe extern "system" fn(This) -> i32,
    stop: unsafe extern "system" fn(This) -> i32,
    get_channels: unsafe extern "system" fn(This, *mut i32, *mut i32) -> i32,
    get_latencies: unsafe extern "system" fn(This, *mut i32, *mut i32) -> i32,
    get_buffer_size: unsafe extern "system" fn(This, *mut i32, *mut i32, *mut i32, *mut i32) -> i32,
    can_sample_rate: unsafe extern "system" fn(This, f64) -> i32,
    get_sample_rate: unsafe extern "system" fn(This, *mut f64) -> i32,
    set_sample_rate: unsafe extern "system" fn(This, f64) -> i32,
    _get_clock_sources: usize,
    _set_clock_source: usize,
    _get_sample_position: usize,
    get_channel_info: unsafe extern "system" fn(This, *mut ChannelInfo) -> i32,
    create_buffers: unsafe extern "system" fn(This, *mut BufferInfo, i32, i32, *const Callbacks) -> i32,
    dispose_buffers: unsafe extern "system" fn(This) -> i32,
    control_panel: unsafe extern "system" fn(This) -> i32,
    _future: usize,
    output_ready: unsafe extern "system" fn(This) -> i32,
}

#[repr(C)]
struct Iasio {
    vtbl: *const Vtbl,
}

/// Owned driver instance; released on drop.
struct Driver(This);

impl Driver {
    fn v(&self) -> &Vtbl {
        unsafe { &*(*self.0).vtbl }
    }

    fn error_message(&self) -> String {
        let mut buf = [0u8; 128];
        unsafe { (self.v().get_error_message)(self.0, buf.as_mut_ptr()) };
        from_cstr(&buf)
    }

    fn check(&self, what: &str, e: i32) -> Result<(), String> {
        if e == ASE_OK || e == ASE_SUCCESS {
            return Ok(());
        }
        let msg = self.error_message();
        Err(match ase_name(e) {
            Some(n) if msg.is_empty() => format!("{what}: {n}"),
            Some(n) => format!("{what}: {n} ({msg})"),
            None => format!("{what}: error {e} {msg}"),
        })
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        unsafe { (self.v().release)(self.0) };
    }
}

fn ase_name(e: i32) -> Option<&'static str> {
    Some(match e {
        -1000 => "driver not present",
        -999 => "hardware malfunction",
        -998 => "invalid parameter",
        -997 => "invalid mode",
        -996 => "sample clock not advancing",
        -995 => "no clock",
        -994 => "out of memory",
        _ => return None,
    })
}

// ---------------------------------------------------------------- sample formats

/// ASIOSampleType → (bytes, big-endian, kind). Kind: 0 int, 1 float.
fn sample_layout(t: i32) -> Option<(usize, bool, u8, f64)> {
    // Scale is the full-scale value of the integer formats.
    const S16: f64 = 32768.0;
    const S24: f64 = 8_388_608.0;
    const S32: f64 = 2_147_483_648.0;
    Some(match t {
        0 => (2, true, 0, S16),
        1 => (3, true, 0, S24),
        2 => (4, true, 0, S32),
        3 => (4, true, 1, 1.0),
        4 => (8, true, 1, 1.0),
        8 => (4, true, 0, S16),
        9 => (4, true, 0, 131_072.0),
        10 => (4, true, 0, 524_288.0),
        11 => (4, true, 0, S24),
        16 => (2, false, 0, S16),
        17 => (3, false, 0, S24),
        18 => (4, false, 0, S32),
        19 => (4, false, 1, 1.0),
        20 => (8, false, 1, 1.0),
        24 => (4, false, 0, S16),
        25 => (4, false, 0, 131_072.0),
        26 => (4, false, 0, 524_288.0),
        27 => (4, false, 0, S24),
        _ => return None,
    })
}

#[derive(Clone, Copy)]
struct Format {
    bytes: usize,
    big: bool,
    float: bool,
    scale: f64,
}

impl Format {
    fn new(t: i32) -> Option<Format> {
        sample_layout(t).map(|(bytes, big, k, scale)| Format { bytes, big, float: k == 1, scale })
    }

    fn decode(&self, src: &[u8], out: &mut [f32]) {
        for (i, o) in out.iter_mut().enumerate() {
            let b = &src[i * self.bytes..(i + 1) * self.bytes];
            let mut raw = [0u8; 8];
            raw[..self.bytes].copy_from_slice(b);
            if self.big {
                raw[..self.bytes].reverse();
            }
            *o = match (self.float, self.bytes) {
                (true, 4) => f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]),
                (true, _) => f64::from_le_bytes(raw) as f32,
                (false, 2) => i16::from_le_bytes([raw[0], raw[1]]) as f32 / self.scale as f32,
                // 24-bit packed: place in the top of an i32 to sign-extend.
                (false, 3) => (i32::from_le_bytes([0, raw[0], raw[1], raw[2]]) >> 8) as f32 / self.scale as f32,
                (false, _) => (i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as f64 / self.scale) as f32,
            };
        }
    }

    fn encode(&self, src: &[f32], dst: &mut [u8]) {
        for (i, &x) in src.iter().enumerate() {
            let x = if x.is_finite() { x.clamp(-1.0, 1.0) } else { 0.0 };
            let mut raw = [0u8; 8];
            match (self.float, self.bytes) {
                (true, 4) => raw[..4].copy_from_slice(&x.to_le_bytes()),
                (true, _) => raw.copy_from_slice(&(x as f64).to_le_bytes()),
                (false, 2) => raw[..2].copy_from_slice(&int(x, self.scale, i16::MAX as f64).to_le_bytes()[..2]),
                (false, 3) => raw[..3].copy_from_slice(&int(x, self.scale, 8_388_607.0).to_le_bytes()[..3]),
                (false, _) => raw[..4].copy_from_slice(&int(x, self.scale, self.scale - 1.0).to_le_bytes()),
            }
            if self.big {
                raw[..self.bytes].reverse();
            }
            dst[i * self.bytes..(i + 1) * self.bytes].copy_from_slice(&raw[..self.bytes]);
        }
    }
}

fn int(x: f32, scale: f64, max: f64) -> i32 {
    (x as f64 * scale).round().clamp(-scale, max) as i32
}

// ---------------------------------------------------------------- driver list

/// Installed ASIO drivers, as named in the registry.
pub fn list_drivers() -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let mut key: Hkey = 0;
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, wide("SOFTWARE\\ASIO").as_ptr(), 0, KEY_READ, &mut key) != 0 {
            return out;
        }
        for i in 0.. {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            let r = RegEnumKeyExW(key, i, name.as_mut_ptr(), &mut len, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
            if r != 0 {
                break;
            }
            out.push(from_wide(&name));
        }
        RegCloseKey(key);
    }
    out.sort_by_key(|n| n.to_lowercase());
    out
}

fn driver_clsid(name: &str) -> Option<Guid> {
    unsafe {
        let mut buf = [0u16; 64];
        let mut len = (buf.len() * 2) as u32;
        let sub = wide(&format!("SOFTWARE\\ASIO\\{name}"));
        let r = RegGetValueW(HKEY_LOCAL_MACHINE, sub.as_ptr(), wide("CLSID").as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len);
        if r != 0 {
            return None;
        }
        let mut g = Guid::default();
        (CLSIDFromString(buf.as_ptr(), &mut g) >= 0).then_some(g)
    }
}

// ---------------------------------------------------------------- the live stream

struct Live {
    process: Process,
    bufs: Vec<BufferInfo>,
    n_in: usize,
    fmts: Vec<Format>,
    ins: Vec<Vec<f32>>,
    outs: Vec<Vec<f32>>,
    frames: usize,
    driver: This,
    post_output: bool,
    reset: SyncSender<String>,
}

// SAFETY: the raw pointers are the driver's own buffers and interface, valid until
// disposeBuffers/Release, which only happen after the slot has been emptied.
unsafe impl Send for Live {}

static LIVE: Mutex<Option<Live>> = parking_lot::const_mutex(None);

unsafe extern "C" fn buffer_switch(index: i32, _direct: i32) {
    let mut g = LIVE.lock();
    let Some(l) = g.as_mut() else { return };
    let idx = (index & 1) as usize;
    let frames = l.frames;
    for k in 0..l.bufs.len() {
        let f = l.fmts[k];
        let p = l.bufs[k].buffers[idx] as *mut u8;
        if p.is_null() || k >= l.n_in {
            continue;
        }
        let src = std::slice::from_raw_parts(p, frames * f.bytes);
        f.decode(src, &mut l.ins[k][..frames]);
    }
    for o in l.outs.iter_mut() {
        o[..frames].fill(0.0);
    }
    (l.process)(&l.ins, &mut l.outs, frames);
    for k in l.n_in..l.bufs.len() {
        let f = l.fmts[k];
        let p = l.bufs[k].buffers[idx] as *mut u8;
        if p.is_null() {
            continue;
        }
        let dst = std::slice::from_raw_parts_mut(p, frames * f.bytes);
        f.encode(&l.outs[k - l.n_in][..frames], dst);
    }
    if l.post_output {
        ((*(*l.driver).vtbl).output_ready)(l.driver);
    }
}

unsafe extern "C" fn buffer_switch_time_info(params: *mut c_void, index: i32, direct: i32) -> *mut c_void {
    buffer_switch(index, direct);
    params
}

fn request_reset(why: &str) {
    if let Some(l) = LIVE.lock().as_ref() {
        let _ = l.reset.try_send(why.to_string());
    }
}

unsafe extern "C" fn sample_rate_did_change(_sr: f64) {
    std::thread::spawn(|| request_reset("ASIO driver changed its sample rate"));
}

unsafe extern "C" fn asio_message(selector: i32, value: i32, _msg: *mut c_void, _opt: *mut f64) -> i32 {
    match selector {
        // kAsioSelectorSupported
        1 => matches!(value, 1..=6) as i32,
        // kAsioEngineVersion
        2 => 2,
        // kAsioResetRequest / kAsioBufferSizeChange: reopen the driver.
        3 | 4 => {
            // Never block the driver thread on the slot: it may be inside a callback.
            std::thread::spawn(|| request_reset("ASIO driver asked for a reset (settings changed in its control panel?)"));
            1
        }
        // kAsioResyncRequest / kAsioLatenciesChanged
        5 | 6 => 1,
        _ => 0,
    }
}

static CALLBACKS: Callbacks = Callbacks { buffer_switch, sample_rate_did_change, asio_message, buffer_switch_time_info };

// ---------------------------------------------------------------- owner thread

#[derive(Clone, Debug)]
pub struct AsioInfo {
    pub driver: String,
    pub sample_rate: u32,
    pub buffer: usize,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub latency_in: usize,
    pub latency_out: usize,
}

enum Cmd {
    Start { process: Process, outputs: usize, on_reset: OnReset, reply: Sender<Result<(), String>> },
    ControlPanel,
    Close,
}

/// A driver that is loaded and configured but not yet streaming.
pub struct AsioOpen {
    tx: Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
    pub info: AsioInfo,
}

/// A running driver; stops and unloads on drop.
pub struct AsioStream {
    tx: Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
}

/// Load `name` and configure it. `want_sr` / `want_buffer` of 0 keep the driver's choice.
pub fn open(name: &str, want_sr: u32, want_buffer: usize) -> Result<AsioOpen, String> {
    let clsid = driver_clsid(name).ok_or_else(|| format!("ASIO driver not installed: {name}"))?;
    let (tx, rx) = channel::<Cmd>();
    let (info_tx, info_rx) = channel::<Result<AsioInfo, String>>();
    let name = name.to_string();
    let thread = std::thread::Builder::new()
        .name("rustbus-asio".into())
        .spawn(move || owner(clsid, name, want_sr, want_buffer, info_tx, rx))
        .map_err(|e| e.to_string())?;
    match info_rx.recv() {
        Ok(Ok(info)) => Ok(AsioOpen { tx, thread: Some(thread), info }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            let _ = thread.join();
            Err("ASIO driver thread ended unexpectedly".into())
        }
    }
}

impl AsioOpen {
    /// Create the buffers and start streaming `outputs` channels (inputs: all).
    pub fn start(mut self, process: Process, outputs: usize, on_reset: OnReset) -> Result<AsioStream, String> {
        let (reply, rx) = channel();
        let tx = self.tx.clone();
        let thread = self.thread.take();
        tx.send(Cmd::Start { process, outputs, on_reset, reply }).map_err(|_| "ASIO driver thread ended".to_string())?;
        let r = rx.recv().unwrap_or_else(|_| Err("ASIO driver thread ended".into()));
        let stream = AsioStream { tx, thread };
        r.map(|_| stream)
    }
}

impl Drop for AsioOpen {
    fn drop(&mut self) {
        if let Some(t) = self.thread.take() {
            let _ = self.tx.send(Cmd::Close);
            let _ = t.join();
        }
    }
}

impl AsioStream {
    /// Ask the driver to show its own settings panel.
    pub fn control_panel(&self) {
        let _ = self.tx.send(Cmd::ControlPanel);
    }
}

impl Drop for AsioStream {
    fn drop(&mut self) {
        if let Some(t) = self.thread.take() {
            let _ = self.tx.send(Cmd::Close);
            let _ = t.join();
        }
    }
}

/// Pick the buffer size: the request when the driver accepts it, else its preferred size.
fn choose_buffer(want: usize, min: i32, max: i32, pref: i32, gran: i32) -> usize {
    let w = want as i32;
    let ok = want > 0
        && w >= min
        && w <= max
        && match gran {
            -1 => want.is_power_of_two(),
            0 => w == pref,
            g => (w - min) % g == 0,
        };
    (if ok { w } else { pref }).max(1) as usize
}

fn pump_messages() {
    unsafe {
        let mut m = Msg([0; 64]);
        while PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
}

fn owner(clsid: Guid, name: String, want_sr: u32, want_buffer: usize, info_tx: Sender<Result<AsioInfo, String>>, rx: Receiver<Cmd>) {
    unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED) };
    let driver = match load(&clsid, &name, want_sr, want_buffer) {
        Ok((d, info)) => {
            let _ = info_tx.send(Ok(info.clone()));
            Some((d, info))
        }
        Err(e) => {
            let _ = info_tx.send(Err(e));
            None
        }
    };
    if let Some((driver, info)) = driver {
        run(&driver, &info, rx);
        drop(driver);
    }
    unsafe { CoUninitialize() };
}

fn load(clsid: &Guid, name: &str, want_sr: u32, want_buffer: usize) -> Result<(Driver, AsioInfo), String> {
    let mut p: *mut c_void = std::ptr::null_mut();
    // ASIO drivers use their CLSID as the interface id.
    let hr = unsafe { CoCreateInstance(clsid, std::ptr::null_mut(), CLSCTX_INPROC_SERVER, clsid, &mut p) };
    if hr < 0 || p.is_null() {
        return Err(format!("Cannot load ASIO driver {name} (0x{:08X})", hr as u32));
    }
    let d = Driver(p.cast());
    let v = d.v();
    unsafe {
        let hwnd = GetDesktopWindow();
        if (v.init)(d.0, hwnd) == 0 {
            let msg = d.error_message();
            return Err(format!("ASIO driver {name} failed to start{}", if msg.is_empty() { String::new() } else { format!(": {msg}") }));
        }
        let mut drv_name = [0u8; 64];
        (v.get_driver_name)(d.0, drv_name.as_mut_ptr());

        if want_sr > 0 {
            if (v.can_sample_rate)(d.0, want_sr as f64) == ASE_OK {
                d.check("setSampleRate", (v.set_sample_rate)(d.0, want_sr as f64))?;
            }
        }
        let mut sr = 0.0f64;
        d.check("getSampleRate", (v.get_sample_rate)(d.0, &mut sr))?;
        let (mut n_in, mut n_out) = (0, 0);
        d.check("getChannels", (v.get_channels)(d.0, &mut n_in, &mut n_out))?;
        let (mut min, mut max, mut pref, mut gran) = (0, 0, 0, 0);
        d.check("getBufferSize", (v.get_buffer_size)(d.0, &mut min, &mut max, &mut pref, &mut gran))?;
        let buffer = choose_buffer(want_buffer, min, max, pref, gran);

        let chan_name = |input: bool, i: i32| {
            let mut ci = ChannelInfo { channel: i, is_input: input as i32, is_active: 0, group: 0, sample_type: -1, name: [0; 32] };
            (v.get_channel_info)(d.0, &mut ci);
            let n = from_cstr(&ci.name);
            if n.is_empty() {
                format!("{} {}", if input { "In" } else { "Out" }, i + 1)
            } else {
                n
            }
        };
        let inputs = (0..n_in.max(0)).map(|i| chan_name(true, i)).collect();
        let outputs = (0..n_out.max(0)).map(|i| chan_name(false, i)).collect();
        let info = AsioInfo {
            driver: name.to_string(),
            sample_rate: sr.round() as u32,
            buffer,
            inputs,
            outputs,
            latency_in: 0,
            latency_out: 0,
        };
        Ok((d, info))
    }
}

fn run(d: &Driver, info: &AsioInfo, rx: Receiver<Cmd>) {
    let v = d.v();
    let mut started = false;
    let mut on_reset: Option<OnReset> = None;
    let (reset_tx, reset_rx) = sync_channel::<String>(1);
    loop {
        let cmd = match rx.recv_timeout(Duration::from_millis(30)) {
            Ok(c) => Some(c),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Cmd::Close),
        };
        pump_messages();
        if let Ok(why) = reset_rx.try_recv() {
            if let Some(f) = &on_reset {
                f(&why);
            }
        }
        match cmd {
            None => {}
            Some(Cmd::ControlPanel) => unsafe {
                (v.control_panel)(d.0);
            },
            Some(Cmd::Start { process, outputs, on_reset: cb, reply }) => {
                let r = unsafe { start(d, info, process, outputs, reset_tx.clone()) };
                started = r.is_ok();
                on_reset = Some(cb);
                let _ = reply.send(r);
            }
            Some(Cmd::Close) => break,
        }
    }
    if started {
        unsafe {
            (v.stop)(d.0);
        }
    }
    // Empty the slot before the buffers go away; late callbacks then do nothing.
    LIVE.lock().take();
    if started {
        unsafe {
            (v.dispose_buffers)(d.0);
        }
    }
}

unsafe fn start(d: &Driver, info: &AsioInfo, process: Process, outputs: usize, reset: SyncSender<String>) -> Result<(), String> {
    let v = d.v();
    let n_in = info.inputs.len();
    let n_out = outputs.min(info.outputs.len());
    let mut bufs: Vec<BufferInfo> = (0..n_in)
        .map(|i| BufferInfo { is_input: 1, channel: i as i32, buffers: [std::ptr::null_mut(); 2] })
        .chain((0..n_out).map(|i| BufferInfo { is_input: 0, channel: i as i32, buffers: [std::ptr::null_mut(); 2] }))
        .collect();
    let mut fmts = Vec::with_capacity(bufs.len());
    for b in &bufs {
        let mut ci = ChannelInfo { channel: b.channel, is_input: b.is_input, is_active: 0, group: 0, sample_type: -1, name: [0; 32] };
        d.check("getChannelInfo", (v.get_channel_info)(d.0, &mut ci))?;
        fmts.push(Format::new(ci.sample_type).ok_or_else(|| format!("Unsupported ASIO sample type {}", ci.sample_type))?);
    }
    let frames = info.buffer;
    // The slot must be filled before createBuffers: some drivers call back right away.
    *LIVE.lock() = Some(Live {
        process,
        bufs: Vec::new(),
        n_in,
        fmts: fmts.clone(),
        ins: vec![vec![0.0; frames]; n_in],
        outs: vec![vec![0.0; frames]; n_out],
        frames,
        driver: d.0,
        post_output: false,
        reset,
    });
    let e = (v.create_buffers)(d.0, bufs.as_mut_ptr(), bufs.len() as i32, frames as i32, &CALLBACKS);
    if let Err(err) = d.check("createBuffers", e) {
        LIVE.lock().take();
        return Err(err);
    }
    let post_output = (v.output_ready)(d.0) == ASE_OK;
    if let Some(l) = LIVE.lock().as_mut() {
        l.bufs = bufs;
        l.post_output = post_output;
    }
    let e = (v.start)(d.0);
    if let Err(err) = d.check("start", e) {
        LIVE.lock().take();
        (v.dispose_buffers)(d.0);
        return Err(err);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_round_trip() {
        let x = [0.0f32, 0.5, -0.5, 0.999, -1.0];
        for t in [16, 17, 18, 19, 20, 24, 27, 0, 1, 2, 3] {
            let f = Format::new(t).unwrap();
            let mut raw = vec![0u8; x.len() * f.bytes];
            f.encode(&x, &mut raw);
            let mut back = [0.0f32; 5];
            f.decode(&raw, &mut back);
            for (a, b) in x.iter().zip(back) {
                assert!((a - b).abs() < 1e-3, "type {t}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn int32_lsb_layout() {
        let f = Format::new(18).unwrap();
        let mut raw = [0u8; 4];
        f.encode(&[-1.0], &mut raw);
        assert_eq!(i32::from_le_bytes(raw), i32::MIN);
        f.encode(&[1.0], &mut raw);
        assert_eq!(i32::from_le_bytes(raw), i32::MAX);
    }

    /// Live check against an installed driver (silent output):
    /// `RUSTBUS_ASIO="Focusrite USB ASIO" cargo test asio_live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn asio_live() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let name = std::env::var("RUSTBUS_ASIO").unwrap_or_else(|_| "Focusrite USB ASIO".into());
        println!("drivers: {:?}", list_drivers());
        let a = open(&name, 0, 0).expect("open");
        println!("{:?}", a.info);
        let calls = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (c2, p2) = (calls.clone(), peak.clone());
        let process: Process = Box::new(move |ins, _outs, _n| {
            c2.fetch_add(1, Ordering::Relaxed);
            let m = ins.iter().flat_map(|c| c.iter()).fold(0.0f32, |a, x| a.max(x.abs()));
            p2.fetch_max((m * 1e6) as usize, Ordering::Relaxed);
        });
        let s = a.start(process, 2, Box::new(|w| println!("reset: {w}"))).expect("start");
        std::thread::sleep(Duration::from_secs(2));
        drop(s);
        let n = calls.load(Ordering::Relaxed);
        println!("callbacks in 2 s: {n}, input peak {:.4}", peak.load(Ordering::Relaxed) as f32 / 1e6);
        assert!(n > 10);
    }

    #[test]
    fn buffer_choice() {
        assert_eq!(choose_buffer(0, 16, 2048, 256, -1), 256);
        assert_eq!(choose_buffer(128, 16, 2048, 256, -1), 128);
        assert_eq!(choose_buffer(100, 16, 2048, 256, -1), 256);
        assert_eq!(choose_buffer(100, 32, 2048, 256, 8), 256);
        assert_eq!(choose_buffer(96, 32, 2048, 256, 8), 96);
        assert_eq!(choose_buffer(64, 256, 256, 256, 0), 256);
    }
}
