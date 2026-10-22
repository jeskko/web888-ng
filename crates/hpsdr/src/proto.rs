// SPDX-License-Identifier: GPL-3.0-or-later
//! OpenHPSDR Protocol 1 (Metis/Hermes, UDP port 1024) framing, receive side.
//!
//! * Discovery: host sends `EF FE 02` (63 bytes); radio replies `EF FE 02`
//!   (`03` while streaming), MAC, code version, board id.
//! * Start/stop: host sends `EF FE 04 cmd`, cmd bit0 = I/Q stream on.
//! * Host -> radio data (EP2): `EF FE 01 02 seq[4]` + 2 USB frames of 512 B
//!   (`7F 7F 7F C0..C4` + 504 B TX audio); only the C&C bytes matter here.
//! * Radio -> host data (EP6): `EF FE 01 06 seq[4]` + 2 USB frames
//!   (`7F 7F 7F C0..C4` + 504 B: per sample slot, each receiver's I and Q
//!   as 24-bit big-endian, then 2 B mic), `504 / (6 * nrx + 2)` slots.

pub const PORT: u16 = 1024;
pub const PACKET: usize = 1032;
pub const FRAME: usize = 512;
pub const MAX_RX: usize = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostPacket {
    Discovery,
    /// bit0: I/Q on, bit1: wideband on
    StartStop(u8),
    Data {
        seq: u32,
        cc: [[u8; 5]; 2],
    },
}

pub fn parse(pkt: &[u8]) -> Option<HostPacket> {
    if pkt.len() < 4 || pkt[0] != 0xEF || pkt[1] != 0xFE {
        return None;
    }
    match pkt[2] {
        0x02 => Some(HostPacket::Discovery),
        0x04 => Some(HostPacket::StartStop(pkt[3])),
        0x01 if pkt.len() >= PACKET && pkt[3] == 0x02 => {
            let seq = u32::from_be_bytes(pkt[4..8].try_into().unwrap());
            let mut cc = [[0u8; 5]; 2];
            for (k, c) in cc.iter_mut().enumerate() {
                let f = &pkt[8 + k * FRAME..8 + (k + 1) * FRAME];
                if f[0..3] != [0x7F, 0x7F, 0x7F] {
                    return None;
                }
                c.copy_from_slice(&f[3..8]);
            }
            Some(HostPacket::Data { seq, cc })
        }
        _ => None,
    }
}

/// Receiver settings commanded by the host.
#[derive(Clone, Debug, PartialEq)]
pub struct HostState {
    /// 0 = 48k, 1 = 96k, 2 = 192k, 3 = 384k
    pub speed: u8,
    pub nrx: usize,
    pub rx_hz: [u32; MAX_RX],
    /// Hermes step attenuator in dB, if enabled.
    pub atten_db: Option<u8>,
}

impl Default for HostState {
    fn default() -> Self {
        HostState {
            speed: 0,
            nrx: 1,
            rx_hz: [0; MAX_RX],
            atten_db: None,
        }
    }
}

impl HostState {
    pub fn rate(&self) -> u32 {
        48_000 << self.speed
    }

    /// Apply one C&C block. Returns which receivers were retuned.
    pub fn apply(&mut self, c: [u8; 5]) -> Vec<usize> {
        let addr = c[0] >> 1;
        match addr {
            0x00 => {
                self.speed = c[1] & 3;
                self.nrx = (((c[4] >> 3) & 7) as usize + 1).min(MAX_RX);
                vec![]
            }
            0x02..=0x08 => {
                let rx = (addr - 2) as usize;
                let f = u32::from_be_bytes([c[1], c[2], c[3], c[4]]);
                if self.rx_hz[rx] != f {
                    self.rx_hz[rx] = f;
                    vec![rx]
                } else {
                    vec![]
                }
            }
            0x0a => {
                self.atten_db = (c[4] & 0x20 != 0).then_some(c[4] & 0x1f);
                vec![]
            }
            _ => vec![],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Board {
    pub mac: [u8; 6],
    pub code_version: u8,
    /// 0 Metis, 1 Hermes, 2 Griffin, 4 Angelia, 5 Orion, 6 Hermes-Lite.
    pub board_id: u8,
}

pub fn discovery_reply(b: &Board, streaming: bool, nrx: u8) -> [u8; 60] {
    let mut r = [0u8; 60];
    r[0] = 0xEF;
    r[1] = 0xFE;
    r[2] = if streaming { 0x03 } else { 0x02 };
    r[3..9].copy_from_slice(&b.mac);
    r[9] = b.code_version;
    r[10] = b.board_id;
    r[19] = nrx; // Hermes-Lite style receiver count; ignored by others
    r
}

/// Sample slots per USB frame for `nrx` receivers.
pub const fn slots(nrx: usize) -> usize {
    504 / (6 * nrx + 2)
}

/// 24-bit big-endian, from a hardware s32 sample (DC full scale 2^30 ->
/// 2^23, i.e. a full-scale ADC sine -> 2^22).
pub fn put24(dst: &mut [u8], v: i32) {
    let x = (v >> 7).clamp(-(1 << 23), (1 << 23) - 1);
    let b = x.to_be_bytes();
    dst.copy_from_slice(&b[1..4]);
}

pub fn get24(src: &[u8]) -> i32 {
    let v = ((src[0] as i32) << 16) | ((src[1] as i32) << 8) | src[2] as i32;
    (v << 8) >> 8
}

/// Builds EP6 packets from per-receiver I/Q samples.
pub struct Ep6Builder {
    nrx: usize,
    frames: [[u8; FRAME]; 2],
    frame: usize,
    slot: usize,
    seq: u32,
    cc_flags: u8,
}

impl Ep6Builder {
    pub fn new(nrx: usize) -> Self {
        let mut b = Ep6Builder {
            nrx,
            frames: [[0; FRAME]; 2],
            frame: 0,
            slot: 0,
            seq: 0,
            cc_flags: 0,
        };
        b.start_frame();
        b
    }

    pub fn nrx(&self) -> usize {
        self.nrx
    }

    /// Change receiver count (drops a partial packet).
    pub fn set_nrx(&mut self, nrx: usize) {
        self.nrx = nrx;
        self.frame = 0;
        self.slot = 0;
        self.start_frame();
    }

    /// ADC overload flag reported in C1 bit0.
    pub fn set_overload(&mut self, on: bool) {
        self.cc_flags = on as u8;
    }

    fn start_frame(&mut self) {
        let f = &mut self.frames[self.frame];
        f.fill(0);
        f[0..3].copy_from_slice(&[0x7F, 0x7F, 0x7F]);
        // C0 = 0 (status), C1 bit0 = ADC overload, C4 = code version
        f[4] = self.cc_flags;
        f[7] = 0;
    }

    /// Add one sample slot (`iq[k]` for receiver k); returns a finished
    /// 1032-byte packet when two frames are full.
    pub fn push<S: Copy + Into<(i32, i32)>>(&mut self, iq: &[S]) -> Option<[u8; PACKET]> {
        let base = 8 + self.slot * (6 * self.nrx + 2);
        let f = &mut self.frames[self.frame];
        for (k, &s) in iq.iter().take(self.nrx).enumerate() {
            let (i, q) = s.into();
            put24(&mut f[base + 6 * k..base + 6 * k + 3], i);
            put24(&mut f[base + 6 * k + 3..base + 6 * k + 6], q);
        }
        self.slot += 1;
        if self.slot < slots(self.nrx) {
            return None;
        }
        self.slot = 0;
        if self.frame == 0 {
            self.frame = 1;
            self.start_frame();
            return None;
        }
        let mut p = [0u8; PACKET];
        p[0..4].copy_from_slice(&[0xEF, 0xFE, 0x01, 0x06]);
        p[4..8].copy_from_slice(&self.seq.to_be_bytes());
        p[8..8 + FRAME].copy_from_slice(&self.frames[0]);
        p[8 + FRAME..].copy_from_slice(&self.frames[1]);
        self.seq = self.seq.wrapping_add(1);
        self.frame = 0;
        self.start_frame();
        Some(p)
    }
}

/// Decode one EP6 packet into per-receiver samples (for tests and tools).
/// A decoded EP6 packet: sequence number and per-receiver (i, q) samples.
pub type Ep6 = (u32, Vec<Vec<(i32, i32)>>);

pub fn decode_ep6(p: &[u8], nrx: usize) -> Option<Ep6> {
    if p.len() < PACKET || p[0..4] != [0xEF, 0xFE, 0x01, 0x06] {
        return None;
    }
    let seq = u32::from_be_bytes(p[4..8].try_into().unwrap());
    let mut out = vec![Vec::new(); nrx];
    for k in 0..2 {
        let f = &p[8 + k * FRAME..8 + (k + 1) * FRAME];
        if f[0..3] != [0x7F, 0x7F, 0x7F] {
            return None;
        }
        for s in 0..slots(nrx) {
            let base = 8 + s * (6 * nrx + 2);
            for (r, o) in out.iter_mut().enumerate() {
                let b = base + 6 * r;
                o.push((get24(&f[b..b + 3]), get24(&f[b + 3..b + 6])));
            }
        }
    }
    Some((seq, out))
}

/// Host-side packet builders (for tests and tools).
pub mod host {
    use super::*;

    pub fn discovery() -> [u8; 63] {
        let mut p = [0u8; 63];
        p[0..3].copy_from_slice(&[0xEF, 0xFE, 0x02]);
        p
    }

    pub fn start_stop(cmd: u8) -> [u8; 64] {
        let mut p = [0u8; 64];
        p[0..4].copy_from_slice(&[0xEF, 0xFE, 0x04, cmd]);
        p
    }

    pub fn data(seq: u32, cc: [[u8; 5]; 2]) -> [u8; PACKET] {
        let mut p = [0u8; PACKET];
        p[0..4].copy_from_slice(&[0xEF, 0xFE, 0x01, 0x02]);
        p[4..8].copy_from_slice(&seq.to_be_bytes());
        for (k, c) in cc.iter().enumerate() {
            let f = &mut p[8 + k * FRAME..8 + (k + 1) * FRAME];
            f[0..3].copy_from_slice(&[0x7F, 0x7F, 0x7F]);
            f[3..8].copy_from_slice(c);
        }
        p
    }

    pub fn cc_config(speed: u8, nrx: usize) -> [u8; 5] {
        [0x00, speed & 3, 0, 0, (((nrx - 1) as u8) & 7) << 3]
    }

    pub fn cc_rx_freq(rx: usize, hz: u32) -> [u8; 5] {
        let f = hz.to_be_bytes();
        [((rx as u8 + 2) << 1), f[0], f[1], f[2], f[3]]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_per_frame() {
        assert_eq!(slots(1), 63);
        assert_eq!(slots(2), 36);
        assert_eq!(slots(4), 19);
        assert_eq!(slots(7), 11);
    }

    #[test]
    fn i24_roundtrip_and_scale() {
        let mut b = [0u8; 3];
        for v in [
            0,
            1 << 7,
            -(1 << 7),
            (1 << 29),
            -(1 << 29),
            i32::MAX,
            i32::MIN,
        ] {
            put24(&mut b, v);
            assert_eq!(get24(&b), (v >> 7).clamp(-(1 << 23), (1 << 23) - 1));
        }
        put24(&mut b, 1 << 29); // full-scale sine amplitude
        assert_eq!(get24(&b), 1 << 22);
    }

    #[test]
    fn host_state_cc() {
        let mut s = HostState::default();
        s.apply(host::cc_config(0, 3));
        assert_eq!((s.nrx, s.rate()), (3, 48_000));
        assert_eq!(s.apply(host::cc_rx_freq(1, 7_074_000)), vec![1]);
        assert_eq!(s.apply(host::cc_rx_freq(1, 7_074_000)), Vec::<usize>::new());
        assert_eq!(s.rx_hz[1], 7_074_000);
        s.apply([0x14, 0, 0, 0, 0x20 | 12]);
        assert_eq!(s.atten_db, Some(12));
        // MOX bit in C0 doesn't change the address
        assert_eq!(s.apply([0x05, 0, 0x6b, 0xf0, 0x40]), vec![0]);
    }

    #[test]
    fn ep6_roundtrip() {
        let mut b = Ep6Builder::new(2);
        let mut pkt = None;
        let mut k = 0;
        while pkt.is_none() {
            pkt = b.push(&[(k << 7, -(k << 7)), (3 << 7, 4 << 7)]);
            k += 1;
        }
        assert_eq!(k as usize, 2 * slots(2));
        let (seq, rx) = decode_ep6(&pkt.unwrap(), 2).unwrap();
        assert_eq!(seq, 0);
        assert_eq!(rx[0][5], (5, -5));
        assert_eq!(rx[1][40], (3, 4));
        assert!(matches!(
            parse(&host::discovery()),
            Some(HostPacket::Discovery)
        ));
        assert!(matches!(
            parse(&host::start_stop(1)),
            Some(HostPacket::StartStop(1))
        ));
    }
}
