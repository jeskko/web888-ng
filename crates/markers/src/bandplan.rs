// SPDX-License-Identifier: GPL-3.0-or-later
//! Band plans for the band ribbon: the amateur bands of an IARU region with
//! their segments (CW, narrow-band digital, all modes...) and centres of
//! activity, and the other services' bands (broadcast, maritime, aero,
//! time signals...) from the ITU table of allocations. Compiled in from
//! `data/` (sources in `data/README.md`).

use crate::Tune;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub title: String,
    #[serde(default)]
    pub version: String,
    pub url: String,
}

/// A part of an amateur band and what it is for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub lo: f64,
    pub hi: f64,
    /// "cw", "digi", "all", "beacon", "sat", "fm" or "other".
    #[serde(rename = "use")]
    pub usage: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Modes the plan names for it ("rtty", "psk31"...).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modes: Vec<String>,
}

/// A centre of activity the plan names (QRP, emergency...).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Centre {
    pub hz: f64,
    pub name: String,
    pub mode: Tune,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub emission: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Band {
    pub name: String,
    pub lo: f64,
    pub hi: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default)]
    pub segments: Vec<Segment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub centres: Vec<Centre>,
}

/// Another service's band.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Service {
    pub name: String,
    /// "broadcast", "maritime", "aero", "time", "ism", "cb" or "other".
    pub service: String,
    pub lo: f64,
    pub hi: f64,
    pub regions: Vec<u8>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// A part of another band of the service (an RR Appendix 17 maritime
    /// sub-band), drawn over it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sub: bool,
}

#[derive(Deserialize)]
struct AmateurFile {
    region: u8,
    title: String,
    sources: Vec<Source>,
    bands: Vec<Band>,
}

#[derive(Deserialize)]
struct ServiceFile {
    sources: Vec<Source>,
    bands: Vec<Service>,
}

/// One region's ribbon.
#[derive(Clone, Debug, Serialize)]
pub struct Plan {
    pub region: u8,
    pub title: String,
    pub amateur: Vec<Band>,
    pub services: Vec<Service>,
    pub sources: Vec<Source>,
}

const AMATEUR: [&str; 3] = [
    include_str!("../data/iaru-r1.json"),
    include_str!("../data/iaru-r2.json"),
    include_str!("../data/iaru-r3.json"),
];
const SERVICES: &str = include_str!("../data/itu-services.json");

fn services() -> &'static ServiceFile {
    static S: OnceLock<ServiceFile> = OnceLock::new();
    S.get_or_init(|| {
        let mut f: ServiceFile = serde_json::from_str(SERVICES)
            .unwrap_or_else(|e| panic!("data/itu-services.json: {e}"));
        f.bands.sort_by(|a, b| a.lo.total_cmp(&b.lo));
        f
    })
}

/// The ribbon for IARU region `region` (1-3; others give region 1).
pub fn plan(region: u8) -> &'static Plan {
    static P: [OnceLock<Plan>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let i = (region.clamp(1, 3) - 1) as usize;
    P[i].get_or_init(|| {
        let a: AmateurFile = serde_json::from_str(AMATEUR[i])
            .unwrap_or_else(|e| panic!("data/iaru-r{}.json: {e}", i + 1));
        assert_eq!(
            a.region as usize,
            i + 1,
            "data/iaru-r{}.json: region",
            i + 1
        );
        let s = services();
        let mut sources = a.sources;
        sources.extend(s.sources.iter().cloned());
        Plan {
            region: a.region,
            title: a.title,
            amateur: a.bands,
            services: s
                .bands
                .iter()
                .filter(|b| b.regions.contains(&a.region))
                .cloned()
                .collect(),
            sources,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundled data is consistent: bands sorted and apart, segments
    /// inside their band, in order and not overlapping, centres inside.
    #[test]
    fn bundled_plans_are_consistent() {
        for r in 1..=3 {
            let p = plan(r);
            assert_eq!(p.region, r);
            for w in p.amateur.windows(2) {
                assert!(
                    w[0].hi <= w[1].lo,
                    "R{r}: {} overlaps {}",
                    w[0].name,
                    w[1].name
                );
            }
            for b in &p.amateur {
                assert!(b.lo < b.hi, "R{r} {}", b.name);
                let mut at = b.lo;
                for s in &b.segments {
                    assert!(
                        s.lo >= at && s.lo < s.hi && s.hi <= b.hi,
                        "R{r} {}: segment {}-{}",
                        b.name,
                        s.lo,
                        s.hi
                    );
                    assert!(
                        ["cw", "digi", "all", "beacon", "sat", "fm", "other"]
                            .contains(&s.usage.as_str()),
                        "R{r} {}: use {}",
                        b.name,
                        s.usage
                    );
                    at = s.hi;
                }
                for c in &b.centres {
                    assert!(
                        c.hz >= b.lo && c.hz <= b.hi,
                        "R{r} {}: centre {}",
                        b.name,
                        c.hz
                    );
                }
            }
            for s in &p.services {
                assert!(
                    s.lo < s.hi && s.regions.iter().all(|r| (1..=3).contains(r)),
                    "{}",
                    s.name
                );
                assert!(
                    [
                        "broadcast",
                        "maritime",
                        "aero",
                        "time",
                        "ism",
                        "cb",
                        "other"
                    ]
                    .contains(&s.service.as_str()),
                    "{}: service {}",
                    s.name,
                    s.service
                );
            }
        }
    }
}
