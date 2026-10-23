// SPDX-License-Identifier: GPL-3.0-or-later
//! Dump (or send) one datagram with test reports, for comparing the
//! layout with other software's (never send test reports to the live
//! server: they would show on the map).
//!
//!   prtest CALL LOCATOR UNIX_TIME dump|HOST:PORT

use web888_skim::pskreporter::{Receiver, Report, datagram};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let now: u32 = a[3].parse().unwrap();
    let server = a[4].as_str();
    let rx = Receiver {
        callsign: a[1].clone(),
        locator: a[2].clone(),
        software: "web888-ng test".into(),
        antenna: String::new(),
    };
    let reports = [
        Report {
            call: "K1ABC".into(),
            freq_hz: 7_075_100,
            snr: -10,
            mode: "FT8".into(),
            locator: "FN42".into(),
            time: now - 60,
        },
        Report {
            call: "W9XYZ".into(),
            freq_hz: 7_075_900,
            snr: -3,
            mode: "FT8".into(),
            locator: String::new(),
            time: now - 45,
        },
    ];
    let d = datagram(now, 1, 0x1357_9bdf, true, &rx, &reports);
    if server == "dump" {
        use std::io::Write;
        std::io::stdout().write_all(&d).unwrap();
        return;
    }
    let s = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    println!("{} bytes to {server}: {:?}", d.len(), s.send_to(&d, server));
}
