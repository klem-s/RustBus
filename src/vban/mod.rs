//! VBAN (VB-Audio network protocol) — configuration, packet codec, network threads.
//! The protocol itself is publicly specified by VB-Audio; config element names follow
//! the settings format observed in the binary (ANALYSIS §6, §8).

pub mod net;
pub mod proto;

pub use net::*;
pub use proto::*;
