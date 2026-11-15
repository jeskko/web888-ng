// SPDX-License-Identifier: GPL-3.0-or-later
//! HFDL ground stations: the system table (version 52) the stations
//! broadcast, as dumphfdl's etc/systable.conf lists it. A squitter's
//! frequency bits index each station's list.

pub struct Station {
    pub id: u8,
    pub name: &'static str,
    pub lat: f32,
    pub lon: f32,
    /// kHz (the USB dial frequency; the burst is 1440 Hz above).
    pub freqs: &'static [f32],
}

pub const VERSION: u16 = 52;

pub static STATIONS: &[Station] = &[
    Station {
        id: 1,
        name: "San Francisco, California",
        lat: 38.3846,
        lon: -121.7596,
        freqs: &[
            21934.0, 17919.0, 13276.0, 11327.0, 10081.0, 8927.0, 6559.0, 5508.0,
        ],
    },
    Station {
        id: 2,
        name: "Molokai, Hawaii",
        lat: 21.1844,
        lon: -157.1868,
        freqs: &[
            21937.0, 17919.0, 13324.0, 13312.0, 13276.0, 11348.0, 11312.0, 10027.0, 8936.0, 8912.0,
            6565.0, 5514.0,
        ],
    },
    Station {
        id: 3,
        name: "Reykjavik, Iceland",
        lat: 63.8472,
        lon: -22.4558,
        freqs: &[17985.0, 15025.0, 11184.0, 8977.0, 6712.0, 5720.0, 3900.0],
    },
    Station {
        id: 4,
        name: "Riverhead, New York",
        lat: 40.8819,
        lon: -72.6376,
        freqs: &[21931.0, 17919.0, 13276.0, 11387.0, 8912.0, 6661.0, 5652.0],
    },
    Station {
        id: 5,
        name: "Auckland, New Zealand",
        lat: -37.0158,
        lon: 174.8096,
        freqs: &[17916.0, 13351.0, 10084.0, 8921.0, 6535.0, 5583.0],
    },
    Station {
        id: 6,
        name: "Hat Yai, Thailand",
        lat: 6.9375,
        lon: 100.3885,
        freqs: &[21949.0, 17928.0, 13270.0, 10066.0, 8825.0, 6535.0, 5655.0],
    },
    Station {
        id: 7,
        name: "Shannon, Ireland",
        lat: 52.7441,
        lon: -8.9268,
        freqs: &[
            11384.0, 10081.0, 8942.0, 8843.0, 6532.0, 5547.0, 3455.0, 2998.0,
        ],
    },
    Station {
        id: 8,
        name: "Johannesburg, South Africa",
        lat: -26.1297,
        lon: 28.2061,
        freqs: &[
            21949.0, 17922.0, 13321.0, 11321.0, 8834.0, 5529.0, 4681.0, 3016.0,
        ],
    },
    Station {
        id: 9,
        name: "Barrow, Alaska",
        lat: 71.2585,
        lon: -156.5774,
        freqs: &[
            21937.0, 21928.0, 17934.0, 17919.0, 11354.0, 10093.0, 10027.0, 8936.0, 8927.0, 6646.0,
            5544.0, 5538.0, 5529.0, 4687.0, 4654.0, 3497.0, 3007.0, 2992.0, 2944.0,
        ],
    },
    Station {
        id: 10,
        name: "Muan, South Korea",
        lat: 35.0324,
        lon: 126.2386,
        freqs: &[
            21931.0, 17958.0, 13342.0, 10060.0, 8939.0, 6619.0, 5502.0, 2941.0,
        ],
    },
    Station {
        id: 11,
        name: "Albrook, Panama",
        lat: 9.0847,
        lon: -79.3740,
        freqs: &[17901.0, 13264.0, 10063.0, 8894.0, 6589.0, 5589.0],
    },
    Station {
        id: 13,
        name: "Santa Cruz, Bolivia",
        lat: -17.6712,
        lon: -63.1571,
        freqs: &[21997.0, 17916.0, 13315.0, 11318.0, 8957.0, 6628.0, 4660.0],
    },
    Station {
        id: 14,
        name: "Krasnoyarsk, Russia",
        lat: 56.1526,
        lon: 92.5833,
        freqs: &[21990.0, 17912.0, 13321.0, 10087.0, 8886.0, 6596.0, 5622.0],
    },
    Station {
        id: 15,
        name: "Al Muharraq, Bahrain",
        lat: 26.3085,
        lon: 50.4723,
        freqs: &[
            21982.0, 17967.0, 13312.0, 10030.0, 8885.0, 6646.0, 5544.0, 2986.0,
        ],
    },
    Station {
        id: 16,
        name: "Agana, Guam",
        lat: 13.4888,
        lon: 144.8282,
        freqs: &[21928.0, 17919.0, 13312.0, 11306.0, 8927.0, 6652.0, 5451.0],
    },
    Station {
        id: 17,
        name: "Canarias, Spain",
        lat: 27.9609,
        lon: -15.4056,
        freqs: &[21955.0, 17928.0, 13303.0, 11348.0, 8948.0, 6529.0],
    },
];

pub fn station(id: u8) -> Option<&'static Station> {
    STATIONS.iter().find(|s| s.id == id)
}

/// The stations that use a frequency (kHz, the listed one).
pub fn on(khz: f32) -> impl Iterator<Item = &'static Station> {
    STATIONS
        .iter()
        .filter(move |s| s.freqs.iter().any(|&f| (f - khz).abs() < 0.5))
}

/// The frequencies (kHz) a squitter's bits name for a station.
pub fn freqs(id: u8, bits: u32) -> Vec<f32> {
    station(id).map_or_else(Vec::new, |s| {
        s.freqs
            .iter()
            .enumerate()
            .filter(|(i, _)| bits >> i & 1 == 1)
            .map(|(_, &f)| f)
            .collect()
    })
}
