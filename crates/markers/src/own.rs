// SPDX-License-Identifier: GPL-3.0-or-later
//! The station's own markers: a CSV file (on the configuration partition)
//! that the admin pages upload and download, edited in a spreadsheet or a
//! text editor. The first line names the columns:
//!
//! ```text
//! khz,name,mode,note,category,emission,schedule,dial_khz
//! 4625,The Buzzer,usb,UVB-76,other,,H24,
//! ```
//!
//! `khz` and `name` are needed; `mode` is usb, lsb, cw or am (default am);
//! the others may be left out or empty. The separator is a comma or, as
//! many spreadsheets write it, a semicolon (whichever the header line
//! uses). Values may be quoted ("a, b"; "" for a quote).

use crate::{Marker, Tune};

/// At most this many markers, and this many bytes of file.
pub const MAX_MARKERS: usize = 5000;
pub const MAX_BYTES: usize = 1 << 20;

const COLUMNS: [&str; 8] = [
    "khz", "name", "mode", "note", "category", "emission", "schedule", "dial_khz",
];

/// One CSV line's fields.
fn fields(line: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == sep && !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|f| f.trim().to_string()).collect()
}

/// The markers in `text`; an error names the first bad line.
pub fn parse(text: &str) -> Result<Vec<Marker>, String> {
    if text.len() > MAX_BYTES {
        return Err(format!("more than {MAX_BYTES} bytes"));
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty());
    let Some((_, head)) = lines.next() else {
        return Ok(Vec::new());
    };
    let sep = if head.contains(';') && !head.contains(',') {
        ';'
    } else {
        ','
    };
    let head: Vec<String> = fields(head, sep)
        .into_iter()
        .map(|h| h.to_ascii_lowercase())
        .collect();
    let col = |name: &str| head.iter().position(|h| h == name);
    let (Some(khz), Some(name)) = (col("khz"), col("name")) else {
        return Err(format!(
            "the first line must name the columns, at least khz and name ({})",
            COLUMNS.join(",")
        ));
    };
    if let Some(u) = head
        .iter()
        .find(|h| !h.is_empty() && !COLUMNS.contains(&h.as_str()))
    {
        return Err(format!(
            "unknown column \"{u}\" (known: {})",
            COLUMNS.join(",")
        ));
    }
    let mut out = Vec::new();
    for (i, line) in lines {
        let n = i + 1;
        let f = fields(line, sep);
        let get = |c: Option<usize>| c.and_then(|c| f.get(c)).map(String::as_str).unwrap_or("");
        let k: f64 = get(Some(khz))
            .replace(',', ".")
            .parse()
            .map_err(|_| format!("line {n}: khz \"{}\" is not a number", get(Some(khz))))?;
        if !(k > 0.0 && k < 1e6) {
            return Err(format!("line {n}: khz {k} is out of range"));
        }
        let label = get(Some(name));
        if label.is_empty() {
            return Err(format!("line {n}: no name"));
        }
        let mode = match get(col("mode")).to_ascii_lowercase().as_str() {
            "" | "am" => Tune::Am,
            "usb" => Tune::Usb,
            "lsb" => Tune::Lsb,
            "cw" => Tune::Cw,
            m => return Err(format!("line {n}: mode \"{m}\" (usb, lsb, cw or am)")),
        };
        let dial = match get(col("dial_khz")) {
            "" => None,
            d => {
                let k: f64 = d
                    .replace(',', ".")
                    .parse()
                    .map_err(|_| format!("line {n}: dial_khz \"{d}\" is not a number"))?;
                Some((k * 1000.0).round())
            }
        };
        out.push(Marker {
            hz: (k * 1000.0).round(),
            dial,
            name: label.into(),
            mode,
            emission: get(col("emission")).to_ascii_lowercase(),
            category: get(col("category")).to_ascii_lowercase(),
            params: None,
            note: get(col("note")).into(),
            schedule: get(col("schedule")).into(),
            src: "own".into(),
        });
        if out.len() > MAX_MARKERS {
            return Err(format!("more than {MAX_MARKERS} markers"));
        }
    }
    out.sort_by(|a, b| a.hz.total_cmp(&b.hz));
    Ok(out)
}

fn quote(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.into()
    }
}

/// The markers as a CSV file `parse` reads back.
pub fn to_csv(markers: &[Marker]) -> String {
    let mut out = COLUMNS.join(",");
    out.push('\n');
    for m in markers {
        let mode = serde_json::to_value(m.mode)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        let dial = m
            .dial
            .map(|d| format!("{}", d / 1000.0))
            .unwrap_or_default();
        let row = [
            format!("{}", m.hz / 1000.0),
            quote(&m.name),
            mode,
            quote(&m.note),
            quote(&m.category),
            quote(&m.emission),
            quote(&m.schedule),
            dial,
        ];
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

/// The list in `path`: none if the file is missing.
pub fn load(path: &std::path::Path) -> Result<Vec<Marker>, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => parse(&t).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comma_and_semicolon_files() {
        let a = parse("khz,name,mode,note\n4625,The Buzzer,usb,\"UVB-76, Russia\"\n198,BBC R4,,\n")
            .unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(
            (a[0].hz, a[0].name.as_str(), a[0].mode),
            (198_000.0, "BBC R4", Tune::Am)
        );
        assert_eq!(
            (a[1].mode, a[1].note.as_str(), a[1].src.as_str()),
            (Tune::Usb, "UVB-76, Russia", "own")
        );
        // a spreadsheet's semicolons, a decimal comma, a byte-order mark
        let b = parse("\u{feff}khz;name;mode\n7039,5;Nets;cw\n").unwrap();
        assert_eq!((b[0].hz, b[0].mode), (7_039_500.0, Tune::Cw));
    }

    #[test]
    fn errors_name_the_line() {
        assert!(
            parse("freq,name\n1,x\n")
                .unwrap_err()
                .contains("khz and name")
        );
        assert!(
            parse("khz,name,colour\n1,x,red\n")
                .unwrap_err()
                .contains("unknown column")
        );
        assert!(
            parse("khz,name\n1000,a\nabc,b\n")
                .unwrap_err()
                .starts_with("line 3")
        );
        assert!(
            parse("khz,name,mode\n1000,a,fm\n")
                .unwrap_err()
                .contains("mode")
        );
        assert!(parse("khz,name\n1000,\n").unwrap_err().contains("no name"));
        assert_eq!(parse("").unwrap(), vec![]);
    }

    #[test]
    fn round_trip() {
        let a = parse("khz,name,mode,note,category,emission,schedule,dial_khz\n10100.8,DWD,usb,\"Pinneberg \"\"RTTY\"\"\",rtty,rtty,H24,10099.4\n").unwrap();
        assert_eq!(parse(&to_csv(&a)).unwrap(), a);
        assert_eq!(a[0].dial, Some(10_099_400.0));
    }
}
