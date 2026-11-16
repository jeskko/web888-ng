// SPDX-License-Identifier: GPL-3.0-or-later
//! The Opus decoder (`opus1`) for the web UI's audio
//! (`crates/web/src/codec.rs`): built for wasm32-unknown-unknown
//! (`scripts/build-wasm.sh`) and run on the page's main thread
//! (`ui/src/audio/codec.ts`), where the frames arrive. A plain C ABI, one
//! decoder per instance:
//!
//! * `od_open(rate, channels)`: a new decoder (12000 or 24000 Hz, 1 or 2
//!   channels); 0 when it opened
//! * `od_input(n)`: room for an n-byte packet
//! * `od_decode(n, samples)`: decodes the n bytes there into up to
//!   `samples` samples a channel; returns how many (a channel's), or -1
//! * `od_output()`: the samples, f32, channels interleaved

use std::cell::RefCell;

struct State {
    dec: Option<opus1::OpusDecoder>,
    chans: usize,
    input: Vec<u8>,
    output: Vec<f32>,
}

thread_local! {
    static STATE: RefCell<State> = const {
        RefCell::new(State { dec: None, chans: 1, input: Vec::new(), output: Vec::new() })
    };
}

#[unsafe(no_mangle)]
pub extern "C" fn od_open(rate: u32, channels: u32) -> i32 {
    let (Ok(sr), ch) = (
        opus1::SampleRate::try_from(rate),
        if channels == 2 {
            opus1::Channels::Stereo
        } else {
            opus1::Channels::Mono
        },
    ) else {
        return -1;
    };
    STATE.with_borrow_mut(|s| match opus1::OpusDecoder::new(sr, ch) {
        Ok(d) => {
            s.dec = Some(d);
            s.chans = if channels == 2 { 2 } else { 1 };
            0
        }
        Err(_) => {
            s.dec = None;
            -1
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn od_input(n: usize) -> *mut u8 {
    STATE.with_borrow_mut(|s| {
        s.input.resize(n, 0);
        s.input.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn od_decode(n: usize, samples: usize) -> i32 {
    STATE.with_borrow_mut(|s| {
        let chans = s.chans;
        let State {
            dec, input, output, ..
        } = s;
        let Some(d) = dec else { return -1 };
        output.resize(samples * chans, 0.0);
        match d.decode(&input[..n.min(input.len())], samples, output) {
            Ok(got) => got as i32,
            Err(_) => -1,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn od_output() -> *const f32 {
    STATE.with_borrow(|s| s.output.as_ptr())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_what_the_server_encodes() {
        let mut e = opus1::OpusEncoder::new(
            opus1::SampleRate::Hz12000,
            opus1::Channels::Mono,
            opus1::Application::Audio,
        )
        .unwrap();
        assert_eq!(od_open(12_000, 1), 0);
        let mut pkt = vec![0u8; 1500];
        let mut last = 0.0f32;
        for k in 0..20 {
            let pcm: Vec<i16> = (0..240)
                .map(|i| (8000.0 * ((k * 240 + i) as f32 * 0.5).sin()) as i16)
                .collect();
            let n = e.encode_i16(&pcm, 240, &mut pkt).unwrap();
            let p = od_input(n);
            unsafe { std::ptr::copy_nonoverlapping(pkt.as_ptr(), p, n) };
            assert_eq!(od_decode(n, 240), 240);
            let out = unsafe { std::slice::from_raw_parts(od_output(), 240) };
            last = out.iter().map(|v| v * v).sum::<f32>() / 240.0;
        }
        // about the input's power (8000/32768)^2 / 2
        assert!((last / 0.0298 - 1.0).abs() < 0.3, "{last}");
        assert_eq!(od_open(44_100, 1), -1);
    }
}
