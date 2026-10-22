// SPDX-License-Identifier: GPL-3.0-or-later
//! Wideband complex IQ sources: files (as recorded by `hackrf_transfer`,
//! SDR++, GQRX, ...) and, with the `soapy` feature, live SoapySDR devices.
//! Samples are normalised so that a full-scale complex sine has |z| = 1.

use num_complex::Complex32;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::{Duration, Instant};

pub trait IqSource: Send {
    /// Fill `buf` (may be short); 0 = end of stream.
    fn read(&mut self, buf: &mut [Complex32]) -> io::Result<usize>;
    /// True if `read` blocks at the sample rate (a device); files are
    /// paced by the reader instead.
    fn realtime(&self) -> bool {
        false
    }
}

/// Sample format of a raw IQ file (interleaved I, Q, little endian).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// i8 pairs (`hackrf_transfer -r`).
    Cs8,
    /// i16 pairs.
    Cs16,
    /// f32 pairs (GQRX, GNU Radio).
    Cf32,
}

impl Format {
    pub fn bytes(self) -> usize {
        match self {
            Format::Cs8 => 2,
            Format::Cs16 => 4,
            Format::Cf32 => 8,
        }
    }

    /// From a file name's extension (`.cs8`, `.cs16`, `.cf32`/`.raw`/`.iq`).
    pub fn from_path(p: &Path) -> Option<Format> {
        match p.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "cs8" | "s8" => Some(Format::Cs8),
            "cs16" | "s16" => Some(Format::Cs16),
            "cf32" | "fc32" | "raw" | "iq" => Some(Format::Cf32),
            _ => None,
        }
    }

    fn decode(self, b: &[u8], out: &mut [Complex32]) {
        match self {
            Format::Cs8 => {
                for (o, c) in out.iter_mut().zip(b.as_chunks::<2>().0) {
                    *o = Complex32::new(c[0] as i8 as f32, c[1] as i8 as f32) / 128.0;
                }
            }
            Format::Cs16 => {
                for (o, c) in out.iter_mut().zip(b.as_chunks::<4>().0) {
                    let i = i16::from_le_bytes([c[0], c[1]]) as f32;
                    let q = i16::from_le_bytes([c[2], c[3]]) as f32;
                    *o = Complex32::new(i, q) / 32768.0;
                }
            }
            Format::Cf32 => {
                for (o, c) in out.iter_mut().zip(b.as_chunks::<8>().0) {
                    let i = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                    let q = f32::from_le_bytes([c[4], c[5], c[6], c[7]]);
                    *o = Complex32::new(i, q);
                }
            }
        }
    }
}

pub struct FileSource {
    path: PathBuf,
    r: BufReader<File>,
    fmt: Format,
    looped: bool,
    raw: Vec<u8>,
}

impl FileSource {
    pub fn open(path: &Path, fmt: Format, looped: bool) -> io::Result<Self> {
        Ok(FileSource {
            path: path.to_owned(),
            r: BufReader::with_capacity(1 << 20, File::open(path)?),
            fmt,
            looped,
            raw: Vec::new(),
        })
    }
}

impl IqSource for FileSource {
    fn read(&mut self, buf: &mut [Complex32]) -> io::Result<usize> {
        let w = self.fmt.bytes();
        self.raw.resize(buf.len() * w, 0);
        let mut got = 0;
        let mut rewound = false;
        while got < self.raw.len() {
            match self.r.read(&mut self.raw[got..])? {
                0 if self.looped && !rewound => {
                    self.r.seek(SeekFrom::Start(0))?;
                    rewound = true;
                }
                0 => break,
                n => {
                    got += n;
                    rewound = false;
                }
            }
        }
        if got == 0 && self.looped {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("{}: empty file", self.path.display()),
            ));
        }
        let n = got / w;
        self.fmt.decode(&self.raw[..n * w], &mut buf[..n]);
        Ok(n)
    }
}

/// A source running on its own thread, handing out chunks. Files are
/// paced at `rate`; chunks that find the queue full are dropped (counted),
/// like samples a slow reader loses on hardware.
pub struct Feed {
    rx: Receiver<Vec<Complex32>>,
    dropped: Arc<AtomicU64>,
    ended: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

/// Samples per chunk: ~8.5 ms at 7.68 MS/s.
const CHUNK: usize = 1 << 16;
/// Queued chunks: ~140 ms at 7.68 MS/s.
const QUEUE: usize = 16;

impl Feed {
    pub fn spawn(mut src: Box<dyn IqSource>, rate: f64) -> Feed {
        let (tx, rx): (SyncSender<Vec<Complex32>>, _) = sync_channel(QUEUE);
        let dropped = Arc::new(AtomicU64::new(0));
        let ended = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (d, e, s) = (dropped.clone(), ended.clone(), stop.clone());
        std::thread::Builder::new()
            .name("iq-source".into())
            .spawn(move || {
                let t0 = Instant::now();
                let mut sent = 0u64;
                while !s.load(Ordering::Relaxed) {
                    let mut buf = vec![Complex32::default(); CHUNK];
                    let n = match src.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(err) => {
                            eprintln!("iq: source: {err}");
                            break;
                        }
                    };
                    buf.truncate(n);
                    if !src.realtime() {
                        let due = t0 + Duration::from_secs_f64(sent as f64 / rate);
                        if let Some(w) = due.checked_duration_since(Instant::now()) {
                            std::thread::sleep(w);
                        }
                    }
                    sent += n as u64;
                    match tx.try_send(buf) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {
                            d.fetch_add(n as u64, Ordering::Relaxed);
                        }
                        Err(TrySendError::Disconnected(_)) => break,
                    }
                }
                e.store(true, Ordering::Relaxed);
            })
            .expect("spawn iq-source thread");
        Feed {
            rx,
            dropped,
            ended,
            stop,
        }
    }

    pub fn try_next(&self) -> Option<Vec<Complex32>> {
        self.rx.try_recv().ok()
    }

    /// Input samples dropped because the consumer was too slow.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// The source has ended (end of a file without looping, or an error).
    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::Relaxed)
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(feature = "soapy")]
pub use soapy::{SoapyConfig, SoapySource};

#[cfg(feature = "soapy")]
mod soapy {
    use super::*;
    use soapysdr::{Device, Direction, RxStream};

    /// Settings for a live SoapySDR device.
    #[derive(Clone, Debug)]
    pub struct SoapyConfig {
        /// Device arguments, e.g. `driver=hackrf`.
        pub args: String,
        pub center_hz: f64,
        pub rate_hz: f64,
        /// Analog bandwidth (0 = the driver's choice for the rate).
        pub bandwidth_hz: f64,
        /// Named gain elements, e.g. `LNA=16`, `VGA=20`, `AMP=0`.
        pub gains: Vec<(String, f64)>,
        pub antenna: Option<String>,
    }

    pub struct SoapySource {
        _dev: Device,
        stream: RxStream<Complex32>,
    }

    fn err(e: soapysdr::Error) -> io::Error {
        io::Error::other(format!("soapysdr: {e}"))
    }

    impl SoapySource {
        pub fn open(c: &SoapyConfig) -> io::Result<Self> {
            let dev = Device::new(c.args.as_str()).map_err(err)?;
            let rx = Direction::Rx;
            dev.set_sample_rate(rx, 0, c.rate_hz).map_err(err)?;
            dev.set_frequency(rx, 0, c.center_hz, ()).map_err(err)?;
            if c.bandwidth_hz > 0.0 {
                dev.set_bandwidth(rx, 0, c.bandwidth_hz).map_err(err)?;
            }
            if let Some(a) = &c.antenna {
                dev.set_antenna(rx, 0, a.as_str()).map_err(err)?;
            }
            for (name, g) in &c.gains {
                dev.set_gain_element(rx, 0, name.as_str(), *g)
                    .map_err(err)?;
            }
            let mut stream = dev.rx_stream::<Complex32>(&[0]).map_err(err)?;
            stream.activate(None).map_err(err)?;
            Ok(SoapySource { _dev: dev, stream })
        }
    }

    impl IqSource for SoapySource {
        fn read(&mut self, buf: &mut [Complex32]) -> io::Result<usize> {
            let mut got = 0;
            while got < buf.len() {
                match self.stream.read(&mut [&mut buf[got..]], 200_000) {
                    Ok(n) => got += n,
                    // overflow / timeout: hand out what we have, keep going
                    Err(e) if got > 0 => {
                        eprintln!("iq: soapysdr: {e}");
                        break;
                    }
                    Err(e) => {
                        eprintln!("iq: soapysdr: {e}");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
            }
            Ok(got)
        }

        fn realtime(&self) -> bool {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_decode_to_unit_scale() {
        let mut o = [Complex32::default(); 1];
        Format::Cs8.decode(&[0x80, 0x40], &mut o);
        assert_eq!(o[0], Complex32::new(-1.0, 0.5));
        Format::Cs16.decode(
            &(-16384i16)
                .to_le_bytes()
                .iter()
                .chain(&16384i16.to_le_bytes())
                .copied()
                .collect::<Vec<_>>(),
            &mut o,
        );
        assert_eq!(o[0], Complex32::new(-0.5, 0.5));
        let b: Vec<u8> = [0.25f32, -0.75]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        Format::Cf32.decode(&b, &mut o);
        assert_eq!(o[0], Complex32::new(0.25, -0.75));
        assert_eq!(Format::from_path(Path::new("x/cap.CS8")), Some(Format::Cs8));
        assert_eq!(Format::from_path(Path::new("cap.wav")), None);
    }
}
