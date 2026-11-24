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
//! * `nr_reset()`: a new stream (after retuning, a gap)

use std::cell::RefCell;
use web888_nr::{Denoiser, Net};

struct State {
    nr: Denoiser,
    buf: Vec<f32>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        nr: Denoiser::new(Net::get()),
        buf: Vec::new(),
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
pub extern "C" fn nr_reset() {
    STATE.with_borrow_mut(|s| s.nr.reset())
}
