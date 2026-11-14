// SPDX-License-Identifier: GPL-3.0-or-later
//! The Morse alphabet (letters, figures, punctuation, prosigns).

/// (character, code); a prosign is one character written `<SK>`.
pub const MORSE: &[(&str, &str)] = &[
    ("A", ".-"),
    ("B", "-..."),
    ("C", "-.-."),
    ("D", "-.."),
    ("E", "."),
    ("F", "..-."),
    ("G", "--."),
    ("H", "...."),
    ("I", ".."),
    ("J", ".---"),
    ("K", "-.-"),
    ("L", ".-.."),
    ("M", "--"),
    ("N", "-."),
    ("O", "---"),
    ("P", ".--."),
    ("Q", "--.-"),
    ("R", ".-."),
    ("S", "..."),
    ("T", "-"),
    ("U", "..-"),
    ("V", "...-"),
    ("W", ".--"),
    ("X", "-..-"),
    ("Y", "-.--"),
    ("Z", "--.."),
    ("0", "-----"),
    ("1", ".----"),
    ("2", "..---"),
    ("3", "...--"),
    ("4", "....-"),
    ("5", "....."),
    ("6", "-...."),
    ("7", "--..."),
    ("8", "---.."),
    ("9", "----."),
    (".", ".-.-.-"),
    (",", "--..--"),
    ("?", "..--.."),
    ("/", "-..-."),
    ("=", "-...-"),
    ("+", ".-.-."),
    ("-", "-....-"),
    ("'", ".----."),
    ("(", "-.--."),
    (")", "-.--.-"),
    (":", "---..."),
    (";", "-.-.-."),
    ("\"", ".-..-."),
    ("@", ".--.-."),
    ("!", "-.-.--"),
    ("&", ".-..."),
    ("<SK>", "...-.-"),
    ("<KN>", "-.--."),
    ("<AS>", ".-..."),
    ("<BK>", "-...-.-"),
    ("<HH>", "........"),
];

/// The code of a character (prosigns as `<SK>`).
pub fn encode(ch: &str) -> Option<&'static str> {
    MORSE.iter().find(|(c, _)| *c == ch).map(|(_, m)| *m)
}

/// The character of a code, `*` if none. `-.--.` reads as `<KN>` (as
/// operators mean it), `.-...` as `&`, `.-.-.` as `+` (AR).
pub fn decode(code: &str) -> &'static str {
    if code == "-.--." {
        return "<KN>";
    }
    MORSE
        .iter()
        .find(|(_, m)| *m == code)
        .map(|(c, _)| *c)
        .unwrap_or("*")
}

/// A text's characters, prosigns (`<SK>`) as one.
pub fn chars(word: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = word.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if c == '<'
            && let Some(k) = word[i..].find('>')
        {
            out.push(word[i..i + k + 1].to_string());
            while let Some(&(j, _)) = it.peek() {
                if j <= i + k {
                    it.next();
                } else {
                    break;
                }
            }
            continue;
        }
        out.push(c.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for (c, m) in MORSE {
            let d = decode(m);
            if *c == "(" {
                assert_eq!(d, "<KN>");
            } else if *c == "<AS>" {
                assert_eq!(d, "&");
            } else {
                assert_eq!(d, *c);
            }
        }
        assert_eq!(decode("......."), "*");
        assert_eq!(chars("R<SK>A"), vec!["R", "<SK>", "A"]);
    }
}
