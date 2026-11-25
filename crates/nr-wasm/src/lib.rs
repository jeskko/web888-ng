// SPDX-License-Identifier: GPL-3.0-or-later
//! The neural SSB voice noise reduction (web888-nr) for the web UI: built
//! for wasm32-unknown-unknown (`scripts/build-wasm.sh`) and run on the
//! page's thread just before the audio player (`ui/src/nrnet.ts`; about
//! 15 M multiply-adds a second). A plain C ABI, one stream:
//!
//! * `nr_buffer(n)`: room for n samples (12 kHz mono f32); returns where
//!   the caller writes them
//! * `nr_process(n)`: runs them through in place (the output
//!   `web888_nr::LATENCY` samples late)
//! * `nr_floor(f)`: the share 0..1 of what the network takes away that is
//!   kept (1 - strength)
//! * `nr_power(p)`: 1..4, the network's gain raised to p (strengths over
//!   100 %: more of what it partly takes away goes)
//! * `nr_reset()`: a new stream (after retuning, a gap)
//! * `nr_load(n)`: other weights (an `nrnet.bin`, n bytes written at
//!   `nr_weights(n)`) instead of the built-in ones, for comparing models;
//!   1 if they load (a new stream, the floor and power kept), else 0

use std::cell::RefCell;
use web888_nr::{Denoiser, Net};

struct State {
    nr: Denoiser,
    buf: Vec<f32>,
    weights: Vec<u8>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        nr: Denoiser::new(Net::get()),
        buf: Vec::new(),
        weights: Vec::new(),
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn nr_buffer(n: usize) -> *mut f32 {
    STATE.with_borrow_mut(|s| {
        s.buf.resize(n, 0.0);
        s.buf.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nr_process(n: usize) {
    STATE.with_borrow_mut(|s| {
        let n = n.min(s.buf.len());
        let y = s.nr.process(&s.buf[..n]);
        s.buf[..n].copy_from_slice(&y);
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nr_floor(f: f32) {
    STATE.with_borrow_mut(|s| s.nr.floor = f.clamp(0.0, 1.0))
}

#[unsafe(no_mangle)]
pub extern "C" fn nr_power(p: f32) {
    STATE.with_borrow_mut(|s| {
        s.nr.power = if p.is_finite() {
            p.clamp(1.0, 4.0)
        } else {
            1.0
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nr_reset() {
    STATE.with_borrow_mut(|s| s.nr.reset())
}

#[unsafe(no_mangle)]
pub extern "C" fn nr_weights(n: usize) -> *mut u8 {
    STATE.with_borrow_mut(|s| {
        s.weights.resize(n, 0);
        s.weights.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nr_load(n: usize) -> u32 {
    STATE.with_borrow_mut(|s| {
        let n = n.min(s.weights.len());
        match Net::load(&s.weights[..n]) {
            Some(net) => {
                // a model lives as long as the page (a few are loaded at most)
                let (floor, power) = (s.nr.floor, s.nr.power);
                s.nr = Denoiser::new(Box::leak(Box::new(net)));
                s.nr.floor = floor;
                s.nr.power = power;
                s.weights = Vec::new();
                1
            }
            None => 0,
        }
    })
}
