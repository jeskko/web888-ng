// SPDX-License-Identifier: GPL-3.0-or-later
//! HF fax and SSTV (web888-img) for the web UI: built for
//! wasm32-unknown-unknown (`scripts/build-wasm.sh`) and run in a Web
//! Worker (`ui/src/img/worker.ts`). A plain C ABI, one decoder per
//! instance:
//!
//! * `img_buffer(n)`: room for n samples (12 kHz mono f32)
//! * `img_push(n)`: runs them through; returns the length of the JSON at
//!   `img_output()` when there are events (or once a second for the
//!   status), else 0. Image rows are in the blob at `img_blob()`
//!   (`img_blob_len()` bytes), events pointing into it:
//!   `{"ev":[{"k":"faxImage","width","ioc","auto"},{"k":"faxRows","first",
//!   "off","len"},{"k":"faxRedraw"},{"k":"faxDone","lines"},
//!   {"k":"sstvImage","mode","width","height"},{"k":"sstvRows","first",
//!   "off","len"},{"k":"sstvRedraw"},{"k":"sstvDone","lines"}],
//!   "fax":{"state","lines","slant","ioc"},"sstv":{"mode","lines","slant"}
//!   or null}` (fax rows: one byte per pixel; SSTV rows: RGB)
//! * `img_fax_start()`, `img_fax_stop()`, `img_fax_adjust(ppm, shift)`,
//!   `img_fax_ioc(ioc)`, `img_sstv_stop()`, `img_reset()`

use std::cell::RefCell;
use std::fmt::Write;
use web888_img::Imager;
use web888_img::fax::{FaxEvent, FaxState};
use web888_img::sstv::SstvEvent;

struct State {
    img: Imager,
    input: Vec<f32>,
    output: String,
    blob: Vec<u8>,
    since: usize,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        img: Imager::new(),
        input: Vec::new(),
        output: String::new(),
        blob: Vec::new(),
        since: 0,
    });
}

/// The JSON and blob of these events and the status.
pub fn to_json(
    img: &Imager,
    fax: Vec<FaxEvent>,
    sstv: Vec<SstvEvent>,
    blob: &mut Vec<u8>,
) -> String {
    blob.clear();
    let mut s = String::from("{\"ev\":[");
    let sep = |s: &mut String| {
        if !s.ends_with('[') {
            s.push(',');
        }
    };
    for e in fax {
        sep(&mut s);
        match e {
            FaxEvent::Image { width, ioc, auto } => {
                let _ = write!(
                    s,
                    "{{\"k\":\"faxImage\",\"width\":{width},\"ioc\":{ioc},\"auto\":{auto}}}"
                );
            }
            FaxEvent::Rows { first, data } => {
                let _ = write!(
                    s,
                    "{{\"k\":\"faxRows\",\"first\":{first},\"off\":{},\"len\":{}}}",
                    blob.len(),
                    data.len()
                );
                blob.extend(data);
            }
            FaxEvent::Redraw => s.push_str("{\"k\":\"faxRedraw\"}"),
            FaxEvent::Done { lines } => {
                let _ = write!(s, "{{\"k\":\"faxDone\",\"lines\":{lines}}}");
            }
        }
    }
    for e in sstv {
        sep(&mut s);
        match e {
            SstvEvent::Image {
                mode,
                width,
                height,
            } => {
                let _ = write!(
                    s,
                    "{{\"k\":\"sstvImage\",\"mode\":\"{mode}\",\"width\":{width},\"height\":{height}}}"
                );
            }
            SstvEvent::Rows { first, data } => {
                let _ = write!(
                    s,
                    "{{\"k\":\"sstvRows\",\"first\":{first},\"off\":{},\"len\":{}}}",
                    blob.len(),
                    data.len()
                );
                blob.extend(data);
            }
            SstvEvent::Redraw => s.push_str("{\"k\":\"sstvRedraw\"}"),
            SstvEvent::Done { lines } => {
                let _ = write!(s, "{{\"k\":\"sstvDone\",\"lines\":{lines}}}");
            }
        }
    }
    let state = match img.fax.state {
        FaxState::Idle => "idle",
        FaxState::Start => "start",
        FaxState::Image => "image",
    };
    let _ = write!(
        s,
        "],\"fax\":{{\"state\":\"{state}\",\"lines\":{},\"slant\":{:.0},\"ioc\":{}}},\"sstv\":",
        img.fax.lines(),
        img.fax.slant(),
        img.fax.ioc
    );
    match img.sstv.status() {
        Some((mode, lines, slant)) => {
            let _ = write!(
                s,
                "{{\"mode\":\"{mode}\",\"lines\":{lines},\"slant\":{slant:.0}}}}}"
            );
        }
        None => s.push_str("null}"),
    }
    s
}

#[unsafe(no_mangle)]
pub extern "C" fn img_buffer(n: usize) -> *mut f32 {
    STATE.with_borrow_mut(|s| {
        s.input.resize(n, 0.0);
        s.input.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn img_push(n: usize) -> usize {
    STATE.with_borrow_mut(|s| {
        let n = n.min(s.input.len());
        let State { img, input, .. } = s;
        img.push(&input[..n]);
        s.since += n;
        let (fax, sstv) = s.img.poll();
        if fax.is_empty() && sstv.is_empty() && s.since < 12_000 {
            return 0;
        }
        s.since = 0;
        s.output = to_json(&s.img, fax, sstv, &mut s.blob);
        s.output.len()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn img_output() -> *const u8 {
    STATE.with_borrow(|s| s.output.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn img_blob() -> *const u8 {
    STATE.with_borrow(|s| s.blob.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn img_blob_len() -> usize {
    STATE.with_borrow(|s| s.blob.len())
}

#[unsafe(no_mangle)]
pub extern "C" fn img_fax_start() {
    STATE.with_borrow_mut(|s| s.img.fax.start_now());
}

#[unsafe(no_mangle)]
pub extern "C" fn img_fax_stop() {
    STATE.with_borrow_mut(|s| s.img.fax.stop_now());
}

#[unsafe(no_mangle)]
pub extern "C" fn img_fax_adjust(ppm: f64, shift: f64) {
    STATE.with_borrow_mut(|s| s.img.fax.adjust(ppm, shift));
}

#[unsafe(no_mangle)]
pub extern "C" fn img_fax_ioc(ioc: u32) {
    STATE.with_borrow_mut(|s| s.img.fax.set_ioc(ioc));
}

#[unsafe(no_mangle)]
pub extern "C" fn img_sstv_stop() {
    STATE.with_borrow_mut(|s| s.img.sstv.stop_now());
}

#[unsafe(no_mangle)]
pub extern "C" fn img_reset() {
    STATE.with_borrow_mut(|s| {
        s.img = Imager::new();
        s.since = 0;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_and_blob() {
        let img = Imager::new();
        let mut blob = Vec::new();
        let j = to_json(
            &img,
            vec![
                FaxEvent::Image {
                    width: 4,
                    ioc: 576,
                    auto: true,
                },
                FaxEvent::Rows {
                    first: 0,
                    data: vec![1, 2, 3, 4],
                },
            ],
            vec![
                SstvEvent::Rows {
                    first: 3,
                    data: vec![9; 6],
                },
                SstvEvent::Done { lines: 4 },
            ],
            &mut blob,
        );
        assert_eq!(
            j,
            r#"{"ev":[{"k":"faxImage","width":4,"ioc":576,"auto":true},{"k":"faxRows","first":0,"off":0,"len":4},{"k":"sstvRows","first":3,"off":4,"len":6},{"k":"sstvDone","lines":4}],"fax":{"state":"idle","lines":0,"slant":0,"ioc":576},"sstv":null}"#
        );
        assert_eq!(blob, vec![1, 2, 3, 4, 9, 9, 9, 9, 9, 9]);
    }
}
