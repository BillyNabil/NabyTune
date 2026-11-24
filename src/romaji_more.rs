//! Thai and Arabic-script lyrics in Latin letters, worked out on this machine like the rest
//! of romaji.rs. There's no dictionary here, so where the spelling leaves the sound open
//! these are readable guesses rather than the last word.
//!
//! - Thai, by the Royal Thai General System (RTGS), without tones. Thai runs its words
//!   together and leaves some vowels unwritten, so each run of Thai is cut into syllables by
//!   trying every reading its spelling allows and keeping the likeliest: the fewest syllables
//!   and unwritten vowels, short vowels closed and long ones open. Each syllable stands apart,
//!   as most Thai words are one syllable ("khit thueng thoe"), except that an unwritten "a"
//!   leaning on the next syllable joins it, as the two are nearly always one word ("sabai",
//!   "khanom", "sawat di").
//! - Arabic, Persian and Urdu, letter by letter: long vowels as written (ا "a", و "u",
//!   ي "i"), short ones from the vowel marks where a line has them, and where it hasn't, an
//!   "a" wherever a syllable needs a vowel ("habibi", "qamar"). ع is "'"; a hamza is "'"
//!   inside a word and unsaid at either end ("ana", "sha"). The article is "al-", or takes
//!   the sound of a sun letter after it ("ash-shams").

/// Whether the line is mostly in Thai script.
#[cfg(test)]
fn is_thai(line: &str) -> bool {
    mostly(line, is_thai_char)
}

/// Whether the line is mostly in Arabic script (Arabic, Persian, Urdu and the like).
#[cfg(test)]
fn is_arabic(line: &str) -> bool {
    mostly(line, is_arabic_char)
}

/// More of the line's letters in the script than out of it.
#[cfg(test)]
fn mostly(line: &str, script: fn(char) -> bool) -> bool {
    let (mut inside, mut outside) = (0, 0);
    for c in line.chars().filter(|c| c.is_alphabetic()) {
        if script(c) {
            inside += 1;
        } else {
            outside += 1;
        }
    }
    inside > outside
}

/// The line with its first letter in upper case (after any opening quote or bracket, but not
/// after a number).
fn capitalise(text: &str) -> String {
    match text.char_indices().find(|(_, c)| c.is_alphanumeric()).filter(|(_, c)| c.is_alphabetic()) {
        Some((at, first)) => {
            let mut out = text[..at].to_string();
            out.extend(first.to_uppercase());
            out.push_str(&text[at + first.len_utf8()..]);
            out
        }
        None => text.to_string(),
    }
}

// ---------------------------------------------------------------------------------------
// Thai
// ---------------------------------------------------------------------------------------

// Vowel signs written above or below their consonant, and the other marks.
const MAI_HAN_AKAT: char = '\u{0E31}';
const SARA_I: char = '\u{0E34}';
const SARA_II: char = '\u{0E35}';
const SARA_UE: char = '\u{0E36}';
const SARA_UEE: char = '\u{0E37}';
const SARA_U: char = '\u{0E38}';
const SARA_UU: char = '\u{0E39}';
const PHINTHU: char = '\u{0E3A}';
const MAI_TAI_KHU: char = '\u{0E47}';
const THANTHAKHAT: char = '\u{0E4C}';
const NIKHAHIT: char = '\u{0E4D}';
const YAMAKKAN: char = '\u{0E4E}';

fn is_thai_char(c: char) -> bool {
    matches!(c, '\u{0E00}'..='\u{0E7F}')
}

/// Letters and marks that make up Thai words (not digits, ฿ or the section marks).
fn is_thai_word_char(c: char) -> bool {
    matches!(c, '\u{0E01}'..='\u{0E3A}' | '\u{0E40}'..='\u{0E4E}')
}

fn is_consonant(c: char) -> bool {
    matches!(c, 'ก'..='ฮ') && c != 'ฤ' && c != 'ฦ'
}

fn is_tone(c: char) -> bool {
    matches!(c, '\u{0E48}'..='\u{0E4B}')
}

/// A vowel sign written above or below its consonant.
fn is_sign(c: char) -> bool {
    matches!(c, MAI_HAN_AKAT | SARA_I..=SARA_UU | MAI_TAI_KHU)
}

/// A vowel that belongs to the consonant before it: a sign, or ะ า ำ after it.
fn is_vowel_after(c: char) -> bool {
    is_sign(c) || matches!(c, 'ะ' | 'า' | 'ำ')
}

/// เ แ โ ใ ไ, written before the consonant they are said after.
fn is_lead(c: char) -> bool {
    matches!(c, 'เ'..='ไ')
}

/// A consonant starting a syllable.
fn initial(c: char) -> &'static str {
    match c {
        'ก' => "k",
        'ข' | 'ฃ' | 'ค' | 'ฅ' | 'ฆ' => "kh",
        'ง' => "ng",
        'จ' | 'ฉ' | 'ช' | 'ฌ' => "ch",
        'ซ' | 'ศ' | 'ษ' | 'ส' => "s",
        'ญ' | 'ย' => "y",
        'ฎ' | 'ด' => "d",
        'ฏ' | 'ต' => "t",
        'ฐ' | 'ฑ' | 'ฒ' | 'ถ' | 'ท' | 'ธ' => "th",
        'ณ' | 'น' => "n",
        'บ' => "b",
        'ป' => "p",
        'ผ' | 'พ' | 'ภ' => "ph",
        'ฝ' | 'ฟ' => "f",
        'ม' => "m",
        'ร' => "r",
        'ล' | 'ฬ' => "l",
        'ว' => "w",
        'ห' | 'ฮ' => "h",
        _ => "",
    }
}

/// A consonant closing a syllable, if it can: ย and ว close one as the vowels "i" and "o".
fn final_sound(c: char) -> Option<&'static str> {
    Some(match c {
        'ก' | 'ข' | 'ฃ' | 'ค' | 'ฅ' | 'ฆ' => "k",
        'ง' => "ng",
        'จ' | 'ฉ' | 'ช' | 'ซ' | 'ฌ' | 'ฎ' | 'ฏ' | 'ฐ' | 'ฑ' | 'ฒ' | 'ด' | 'ต' | 'ถ' | 'ท' | 'ธ' | 'ศ' | 'ษ' | 'ส' => "t",
        'ญ' | 'ณ' | 'น' | 'ร' | 'ล' | 'ฬ' => "n",
        'บ' | 'ป' | 'ผ' | 'ฝ' | 'พ' | 'ฟ' | 'ภ' => "p",
        'ม' => "m",
        'ย' => "i",
        'ว' => "o",
        _ => return None,
    })
}

/// Two consonants said together at the start of a syllable: true clusters (กร "kr",
/// พล "phl", คว "khw"), ทร ศร สร said "s" and จร "ch" (จริง "ching"), and ห or อ silently
/// leading the next (หน "n").
fn cluster(first: char, second: char) -> Option<&'static str> {
    Some(match (first, second) {
        ('ก', 'ร') => "kr",
        ('ข' | 'ค', 'ร') => "khr",
        ('ต', 'ร') => "tr",
        ('ป', 'ร') => "pr",
        ('พ', 'ร') => "phr",
        ('บ', 'ร') => "br",
        ('ด', 'ร') => "dr",
        ('ฟ', 'ร') => "fr",
        ('ท' | 'ศ' | 'ส' | 'ซ', 'ร') => "s",
        ('จ', 'ร') => "ch",
        ('ก', 'ล') => "kl",
        ('ข' | 'ค', 'ล') => "khl",
        ('ป', 'ล') => "pl",
        ('พ' | 'ผ', 'ล') => "phl",
        ('บ', 'ล') => "bl",
        ('ฟ', 'ล') => "fl",
        ('ก', 'ว') => "kw",
        ('ข' | 'ค', 'ว') => "khw",
        ('ห', 'ง' | 'ญ' | 'น' | 'ม' | 'ย' | 'ร' | 'ล' | 'ว') => initial(second),
        ('อ', 'ย') => "y",
        _ => return None,
    })
}

/// A Thai letter, and whether a tone mark sits on it (so it starts a syllable).
#[derive(Clone, Copy)]
struct Letter {
    c: char,
    toned: bool,
}

/// A run of Thai with the marks that change no sounds taken out: tone marks (noted on their
/// consonant), letters silenced by ์ (with a letter before them that would otherwise hang
/// between two syllables: จันทร์ "chan", ศาสตร์ "sat"), and ํ า written for ำ.
fn thai_letters(run: &[char]) -> Vec<Letter> {
    let mut out: Vec<Letter> = Vec::new();
    let tone_last = |out: &mut Vec<Letter>| {
        if let Some(l) = out.iter_mut().rev().find(|l| is_consonant(l.c)) {
            l.toned = true;
        }
    };
    let mut i = 0;
    while i < run.len() {
        let c = run[i];
        i += 1;
        if is_tone(c) {
            tone_last(&mut out);
        } else if c == THANTHAKHAT {
            if let Some(at) = out.iter().rposition(|l| is_consonant(l.c)) {
                out.truncate(at);
                let n = out.len();
                if n >= 3
                    && is_consonant(out[n - 1].c)
                    && !out[n - 1].toned
                    && is_consonant(out[n - 2].c)
                    && !is_lead(out[n - 3].c)
                    && !(out[n - 1].c == 'ร' && out[n - 2].c == 'ร')
                {
                    out.pop();
                }
            }
        } else if c == NIKHAHIT {
            let mut k = i;
            while k < run.len() && is_tone(run[k]) {
                k += 1;
            }
            if run.get(k) == Some(&'า') {
                if k > i {
                    tone_last(&mut out);
                }
                out.push(Letter { c: 'ำ', toned: false });
                i = k + 1;
            }
        } else if !matches!(c, PHINTHU | YAMAKKAN | 'ๅ' | 'ฯ') {
            out.push(Letter { c, toned: false });
        }
    }
    out
}

#[derive(Clone)]
enum Piece {
    /// A syllable, and whether it is an unwritten "a" leaning on the next one.
    Syllable(String, bool),
    /// ๆ: the word before, again.
    Again,
}

/// One way to read the letters from a point: where it ends, what it says, and how unlikely
/// it is.
struct Reading {
    end: usize,
    pieces: Vec<Piece>,
    cost: u32,
}

/// A vowel after the onset: its Latin, where its letters end, and what it costs open and
/// closed by a final consonant (None: it can't be).
struct Nucleus {
    vowel: &'static str,
    end: usize,
    open: Option<u32>,
    closed: Option<u32>,
    unwritten: bool,
}

/// The Latin for a run of Thai, a syllable per word unless one leans on the next; `word` is
/// the last word, for ๆ, kept from run to run.
fn spell_thai(run: &[char], out: &mut String, word: &mut String) {
    let s = thai_letters(run);
    let n = s.len();
    // The cheapest reading of the first i letters: its cost, where its last part began, and
    // what that part says.
    let mut best: Vec<Option<(u32, usize, Vec<Piece>)>> = vec![None; n + 1];
    best[0] = Some((0, 0, Vec::new()));
    for i in 0..n {
        let Some(cost) = best[i].as_ref().map(|b| b.0) else { continue };
        for reading in readings(&s, i) {
            let total = cost + reading.cost;
            if best[reading.end].as_ref().is_none_or(|b| total < b.0) {
                best[reading.end] = Some((total, i, reading.pieces));
            }
        }
    }
    let mut parts = Vec::new();
    let mut at = n;
    while at > 0 {
        let Some((_, from, pieces)) = best[at].take() else { break };
        parts.push(pieces);
        at = from;
    }
    let mut leaning = false;
    let mut first = true;
    for piece in parts.into_iter().rev().flatten() {
        match piece {
            Piece::Syllable(text, minor) => {
                if first || !leaning {
                    if !first {
                        out.push(' ');
                    }
                    word.clear();
                }
                out.push_str(&text);
                word.push_str(&text);
                leaning = minor;
            }
            Piece::Again => {
                if !word.is_empty() {
                    if !out.is_empty() && !out.ends_with(char::is_whitespace) {
                        out.push(' ');
                    }
                    out.push_str(word);
                }
                leaning = false;
            }
        }
        first = false;
    }
}

/// Every syllable the letters at `i` can start.
fn readings(s: &[Letter], i: usize) -> Vec<Reading> {
    let mut out = Vec::new();
    let c = s[i].c;
    if c == 'ๆ' {
        out.push(Reading { end: i + 1, pieces: vec![Piece::Again], cost: 0 });
    } else if is_lead(c) {
        lead_readings(s, c, i + 1, Vec::new(), 0, &mut out);
        // An unwritten "a" on a consonant before the one the vowel belongs to (เสมอ "samoe",
        // แสดง "sadaeng").
        if s.get(i + 1).is_some_and(|l| is_consonant(l.c) && !l.toned && l.c != 'อ') && s.get(i + 2).is_some_and(|l| is_consonant(l.c)) {
            let minor = Piece::Syllable(format!("{}a", initial(s[i + 1].c)), true);
            lead_readings(s, c, i + 2, vec![minor], 2, &mut out);
        }
    } else if c == 'ฤ' || c == 'ฦ' {
        let vowel = if c == 'ฤ' { "rue" } else { "lue" };
        syllable(s, "", &Nucleus { vowel, end: i + 1, open: Some(0), closed: Some(0), unwritten: false }, Vec::new(), 0, &mut out);
    } else if is_consonant(c) {
        for (onset, q) in onsets(s, i) {
            for nucleus in plain_nuclei(s, q) {
                syllable(s, &onset, &nucleus, Vec::new(), 0, &mut out);
            }
        }
    }
    if out.is_empty() {
        // A vowel out of place: said as it would be alone.
        let alone = match c {
            'ะ' | 'า' | MAI_HAN_AKAT => "a",
            'ำ' => "am",
            SARA_I | SARA_II => "i",
            SARA_UE | SARA_UEE => "ue",
            SARA_U | SARA_UU => "u",
            'เ' => "e",
            'แ' => "ae",
            'โ' => "o",
            'ใ' | 'ไ' => "ai",
            _ => "",
        };
        out.push(Reading { end: i + 1, pieces: vec![Piece::Syllable(alone.to_string(), false)], cost: 50 });
    }
    out
}

/// The consonant at `k` alone, and with the next as a cluster where they make one.
fn onsets(s: &[Letter], k: usize) -> Vec<(String, usize)> {
    let first = s[k];
    let mut out = vec![(initial(first.c).to_string(), k + 1)];
    if !first.toned {
        if let Some(both) = s.get(k + 1).and_then(|second| cluster(first.c, second.c)) {
            out.push((both.to_string(), k + 2));
        }
    }
    out
}

/// The syllables a leading vowel can make with the consonants from `k`.
fn lead_readings(s: &[Letter], lead: char, k: usize, before: Vec<Piece>, extra: u32, out: &mut Vec<Reading>) {
    if !s.get(k).is_some_and(|l| is_consonant(l.c)) {
        return;
    }
    for (onset, q) in onsets(s, k) {
        for nucleus in lead_nuclei(s, lead, q) {
            syllable(s, &onset, &nucleus, before.clone(), extra, out);
        }
    }
}

/// A syllable from its onset and vowel, open and closed by the next letter (as the vowel
/// allows), after the `before` pieces.
fn syllable(s: &[Letter], onset: &str, nucleus: &Nucleus, before: Vec<Piece>, extra: u32, out: &mut Vec<Reading>) {
    let minor = nucleus.unwritten && nucleus.closed.is_none();
    let make = |end, text: String, cost| {
        let mut pieces = before.clone();
        pieces.push(Piece::Syllable(text, minor));
        Reading { end, pieces, cost: 10 + extra + cost }
    };
    if let Some(cost) = nucleus.open {
        out.push(make(nucleus.end, format!("{onset}{}", nucleus.vowel), cost));
    }
    if let Some(cost) = nucleus.closed {
        // An unwritten "o" doesn't end in ย or ว.
        let fit = !nucleus.unwritten || !matches!(s.get(nucleus.end).map(|l| l.c), Some('ย' | 'ว'));
        if let Some(last) = final_at(s, nucleus.end).filter(|_| fit) {
            out.push(make(nucleus.end + 1, format!("{onset}{}{last}", nucleus.vowel), cost));
        }
    }
}

/// The letter at `q` as a final consonant, unless a tone mark or a vowel of its own makes it
/// the next syllable's onset.
fn final_at(s: &[Letter], q: usize) -> Option<&'static str> {
    let l = s.get(q)?;
    if l.toned || s.get(q + 1).is_some_and(|next| is_vowel_after(next.c)) {
        return None;
    }
    final_sound(l.c)
}

/// The vowels after an onset with no leading vowel, at `q`, with their costs: a long vowel
/// closed or a short one open is a little less likely; an unwritten vowel more so.
fn plain_nuclei(s: &[Letter], q: usize) -> Vec<Nucleus> {
    let at = |k: usize| s.get(k).map(|l| l.c);
    let bare = |k: usize| s.get(k).is_some_and(|l| !l.toned);
    let written = |vowel, end, open, closed| Nucleus { vowel, end, open, closed, unwritten: false };
    let mut out = Vec::new();
    match at(q) {
        Some('ะ') => out.push(written("a", q + 1, Some(0), None)),
        Some(MAI_HAN_AKAT) if at(q + 1) == Some('ว') => {
            out.push(written("ua", if at(q + 2) == Some('ะ') { q + 3 } else { q + 2 }, Some(0), None))
        }
        Some(MAI_HAN_AKAT) => out.push(written("a", q + 1, None, Some(0))),
        Some('า') => out.push(written("a", q + 1, Some(0), Some(1))),
        Some('ำ') => out.push(written("am", q + 1, Some(0), None)),
        Some(SARA_I) => out.push(written("i", q + 1, Some(3), Some(0))),
        Some(SARA_II) => out.push(written("i", q + 1, Some(0), Some(1))),
        Some(SARA_UE) => out.push(written("ue", q + 1, Some(3), Some(0))),
        Some(SARA_UEE) if at(q + 1) == Some('อ') => out.push(written("ue", q + 2, Some(0), Some(1))),
        Some(SARA_UEE) => out.push(written("ue", q + 1, Some(3), Some(0))),
        Some(SARA_U) => out.push(written("u", q + 1, Some(3), Some(0))),
        Some(SARA_UU) => out.push(written("u", q + 1, Some(0), Some(1))),
        Some(MAI_TAI_KHU) if at(q + 1) == Some('อ') => out.push(written("o", q + 2, Some(0), Some(0))),
        Some(MAI_TAI_KHU) => out.push(written("o", q + 1, Some(0), None)),
        Some('ฤ') => out.push(written("rue", q + 1, Some(0), Some(0))),
        _ => {}
    }
    // Consonants that stand for a vowel after another: อ "o", ว "ua" before a final, รร.
    if at(q) == Some('อ') && bare(q) {
        out.push(written("o", q + 1, Some(0), Some(1)));
    }
    if at(q) == Some('ว') && bare(q) && at(q + 1).is_some_and(is_consonant) {
        out.push(written("ua", q + 1, None, Some(1)));
    }
    if at(q) == Some('ร') && at(q + 1) == Some('ร') {
        out.push(written("a", q + 2, None, Some(0)));
        out.push(written("an", q + 2, Some(0), None));
    }
    // No vowel written: "o" before a final (คน "khon"), or an "a" leaning on the syllable
    // after (สบาย "sabai"), which needs a consonant to lean on.
    if !at(q).is_some_and(is_vowel_after) {
        out.push(Nucleus { vowel: "o", end: q, open: None, closed: Some(4), unwritten: true });
        let leans = at(q).is_some_and(is_consonant);
        out.push(Nucleus { vowel: "a", end: q, open: Some(if leans { 6 } else { 12 }), closed: None, unwritten: true });
    }
    out
}

/// The vowels a leading vowel makes with what follows the onset, at `q`.
fn lead_nuclei(s: &[Letter], lead: char, q: usize) -> Vec<Nucleus> {
    let at = |k: usize| s.get(k).map(|l| l.c);
    let bare = |k: usize| s.get(k).is_some_and(|l| !l.toned);
    let n = |vowel, end, open, closed| Nucleus { vowel, end, open, closed, unwritten: false };
    let plain = match lead {
        'เ' => "e",
        'แ' => "ae",
        'โ' => "o",
        _ => "ai",
    };
    let mut out = Vec::new();
    match (lead, at(q), at(q + 1), at(q + 2)) {
        ('เ', Some('า'), Some('ะ'), _) => out.push(n("o", q + 2, Some(0), None)),
        ('เ', Some('า'), ..) => out.push(n("ao", q + 1, Some(0), None)),
        ('เ', Some(SARA_II), Some('ย'), Some('ะ')) => out.push(n("ia", q + 3, Some(0), None)),
        ('เ', Some(SARA_II), Some('ย'), _) => out.push(n("ia", q + 2, Some(0), Some(1))),
        ('เ', Some(SARA_UEE), Some('อ'), Some('ะ')) => out.push(n("uea", q + 3, Some(0), None)),
        ('เ', Some(SARA_UEE), Some('อ'), _) => out.push(n("uea", q + 2, Some(0), Some(1))),
        ('เ', Some('อ'), Some('ะ'), _) => out.push(n("oe", q + 2, Some(0), None)),
        ('เ', Some('อ'), ..) => out.push(n("oe", q + 1, Some(0), None)),
        ('เ', Some(SARA_I), ..) => out.push(n("oe", q + 1, None, Some(0))),
        ('เ', Some('ย'), ..) if bare(q) => out.push(n("oei", q + 1, Some(0), None)),
        ('เ' | 'แ', Some(MAI_TAI_KHU), ..) => out.push(n(plain, q + 1, None, Some(0))),
        ('เ' | 'แ' | 'โ', Some('ะ'), ..) => out.push(n(plain, q + 1, Some(0), None)),
        ('ใ' | 'ไ', Some('ย'), ..) if bare(q) => out.push(n("ai", q + 1, Some(0), None)),
        _ => {}
    }
    if !at(q).is_some_and(is_vowel_after) {
        out.push(if plain == "ai" { n("ai", q, Some(0), None) } else { n(plain, q, Some(3), Some(1)) });
    }
    out
}

/// The line in Latin letters (RTGS, no tones); None if there's no Thai in it.
pub fn thai(line: &str) -> Option<String> {
    if !line.chars().any(|c| is_thai_char(c) && c.is_alphabetic()) {
        return None;
    }
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut word = String::new();
    let mut i = 0;
    while i < chars.len() {
        if is_thai_word_char(chars[i]) {
            let start = i;
            while i < chars.len() && is_thai_word_char(chars[i]) {
                i += 1;
            }
            spell_thai(&chars[start..i], &mut out, &mut word);
            continue;
        }
        match chars[i] {
            c @ '๐'..='๙' => out.push(char::from(b'0' + (c as u32 - '๐' as u32) as u8)),
            // A zero-width space, which some Thai text puts between words.
            '\u{200B}' => {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            '๏' | '๚' | '๛' => {}
            c => out.push(c),
        }
        i += 1;
    }
    Some(capitalise(out.trim()))
}

// ---------------------------------------------------------------------------------------
// Arabic, Persian and Urdu
// ---------------------------------------------------------------------------------------

const TATWEEL: char = '\u{0640}';
const FATHATAN: char = '\u{064B}';
const DAMMATAN: char = '\u{064C}';
const KASRATAN: char = '\u{064D}';
const FATHA: char = '\u{064E}';
const DAMMA: char = '\u{064F}';
const KASRA: char = '\u{0650}';
const SHADDA: char = '\u{0651}';
const SUKUN: char = '\u{0652}';
const DAGGER_ALIF: char = '\u{0670}';
const ZWNJ: char = '\u{200C}';

fn is_arabic_char(c: char) -> bool {
    matches!(c, '\u{0600}'..='\u{06FF}' | '\u{0750}'..='\u{077F}' | '\u{0870}'..='\u{08FF}' | '\u{FB50}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFC}')
}

/// Vowel marks and the other signs written over or under Arabic letters.
fn is_arabic_mark(c: char) -> bool {
    matches!(
        c,
        '\u{0610}'..='\u{061A}' | '\u{064B}'..='\u{065F}' | DAGGER_ALIF | '\u{06D6}'..='\u{06DC}' | '\u{06DF}'..='\u{06E8}' | '\u{06EA}'..='\u{06ED}' | '\u{08D3}'..='\u{08FF}'
    )
}

/// The letters of the joined and ligature forms (U+FB50 to U+FEFC) that some old lyrics
/// files are written in, pushed as the letters they stand for.
fn unform(c: char, out: &mut Vec<char>) {
    // From U+FE80: each letter, and how many forms it has there.
    const FORMS: [(char, u32); 36] = [
        ('ء', 1), ('آ', 2), ('أ', 2), ('ؤ', 2), ('إ', 2), ('ئ', 4), ('ا', 2), ('ب', 4), ('ة', 2), ('ت', 4), ('ث', 4), ('ج', 4),
        ('ح', 4), ('خ', 4), ('د', 2), ('ذ', 2), ('ر', 2), ('ز', 2), ('س', 4), ('ش', 4), ('ص', 4), ('ض', 4), ('ط', 4), ('ظ', 4),
        ('ع', 4), ('غ', 4), ('ف', 4), ('ق', 4), ('ك', 4), ('ل', 4), ('م', 4), ('ن', 4), ('ه', 4), ('و', 2), ('ى', 2), ('ي', 4),
    ];
    // Persian and Urdu letters' forms, first and last.
    const EXTRA: [(u32, u32, char); 17] = [
        (0xFB50, 0xFB51, 'ٱ'), (0xFB56, 0xFB59, 'پ'), (0xFB66, 0xFB69, 'ٹ'), (0xFB6A, 0xFB6D, 'ڤ'), (0xFB7A, 0xFB7D, 'چ'),
        (0xFB88, 0xFB89, 'ڈ'), (0xFB8A, 0xFB8B, 'ژ'), (0xFB8C, 0xFB8D, 'ڑ'), (0xFB8E, 0xFB91, 'ک'), (0xFB92, 0xFB95, 'گ'),
        (0xFB9E, 0xFB9F, 'ں'), (0xFBA4, 0xFBA5, 'ۀ'), (0xFBA6, 0xFBA9, 'ہ'), (0xFBAA, 0xFBAD, 'ھ'), (0xFBAE, 0xFBAF, 'ے'),
        (0xFBB0, 0xFBB1, 'ۓ'), (0xFBFC, 0xFBFF, 'ی'),
    ];
    const MARKS: [char; 16] = [
        FATHATAN, FATHATAN, DAMMATAN, TATWEEL, KASRATAN, TATWEEL, FATHA, FATHA, DAMMA, DAMMA, KASRA, KASRA, SHADDA, SHADDA, SUKUN, SUKUN,
    ];
    let code = c as u32;
    match c {
        '\u{FE80}'..='\u{FEF4}' => {
            let mut at = 0xFE80;
            for (letter, forms) in FORMS {
                if code < at + forms {
                    out.push(letter);
                    return;
                }
                at += forms;
            }
        }
        '\u{FEF5}'..='\u{FEFC}' => out.extend(['ل', ['آ', 'أ', 'إ', 'ا'][((code - 0xFEF5) / 2) as usize]]),
        '\u{FE70}'..='\u{FE7F}' => out.push(MARKS[(code - 0xFE70) as usize]),
        '\u{FDF2}' => out.extend(['ا', 'ل', 'ل', 'ه']),
        '\u{FB50}'..='\u{FDFF}' => match EXTRA.iter().find(|(first, last, _)| (*first..=*last).contains(&code)) {
            Some((_, _, letter)) => out.push(*letter),
            None => out.push(c),
        },
        _ => out.push(c),
    }
}

/// A consonant's Latin, Persian and Urdu reading some Arabic letters their own way.
fn arabic_consonant(c: char, persian: bool) -> Option<&'static str> {
    Some(match c {
        'ب' => "b",
        'ت' | 'ط' | 'ٹ' => "t",
        'ث' => if persian { "s" } else { "th" },
        'ج' => "j",
        'ح' | 'ه' | 'ہ' | 'ھ' | 'ە' => "h",
        'خ' => "kh",
        'د' | 'ڈ' => "d",
        'ذ' => if persian { "z" } else { "dh" },
        'ض' => if persian { "z" } else { "d" },
        'ر' | 'ڑ' | 'ڕ' => "r",
        'ز' | 'ظ' => "z",
        'س' | 'ص' => "s",
        'ش' => "sh",
        'ع' | 'ء' => "'",
        'غ' => "gh",
        'ف' => "f",
        'ق' => if persian { "gh" } else { "q" },
        'ك' | 'ک' => "k",
        'ل' | 'ڵ' => "l",
        'م' => "m",
        'ن' | 'ں' => "n",
        'پ' => "p",
        'چ' => "ch",
        'ژ' => "zh",
        'گ' | 'ڨ' => "g",
        'ڤ' | 'ۋ' => "v",
        _ => return None,
    })
}

/// A short vowel mark's Latin ("e" and "o" for kasra and damma in Persian and Urdu).
fn short_vowel(mark: char, persian: bool) -> Option<&'static str> {
    Some(match mark {
        FATHA => "a",
        KASRA => if persian { "e" } else { "i" },
        DAMMA => if persian { "o" } else { "u" },
        FATHATAN => "an",
        KASRATAN => "in",
        DAMMATAN => "un",
        _ => return None,
    })
}

/// Letters that can stand for a vowel.
fn is_vowel_letter(c: char) -> bool {
    matches!(c, 'ا' | 'ٱ' | 'آ' | 'و' | 'ي' | 'ی' | 'ى' | 'ة' | 'ۃ' | 'ے')
}

/// The line in Latin letters; None if there's no Arabic script in it.
pub fn arabic(line: &str) -> Option<String> {
    if !line.chars().any(|c| is_arabic_char(c) && c.is_alphabetic()) {
        return None;
    }
    let mut text = Vec::new();
    for c in line.chars() {
        unform(c, &mut text);
    }
    // Letters only Persian and Urdu use mark a line as theirs, to be read their way.
    let persian = text.iter().any(|c| "پچژگکیۀٹڈڑںےھہ".contains(*c));
    let mut out = String::new();
    let mut word = Vec::new();
    for &c in &text {
        if (is_arabic_char(c) && c.is_alphabetic()) || is_arabic_mark(c) {
            if c != TATWEEL {
                word.push(c);
            }
            continue;
        }
        spell_arabic_word(&word, persian, &mut out);
        word.clear();
        match c {
            '،' | '٬' => out.push(','),
            '؟' => out.push('?'),
            '؛' => out.push(';'),
            '۔' | '٫' => out.push('.'),
            '٪' => out.push('%'),
            '«' | '»' => out.push('"'),
            '\u{0660}'..='\u{0669}' => out.push(char::from(b'0' + (c as u32 - 0x0660) as u8)),
            '\u{06F0}'..='\u{06F9}' => out.push(char::from(b'0' + (c as u32 - 0x06F0) as u8)),
            // A non-joiner inside a Persian word joins its parts again; direction marks go.
            ZWNJ | '\u{200E}' | '\u{200F}' | '\u{061C}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' => {}
            c => out.push(c),
        }
    }
    spell_arabic_word(&word, persian, &mut out);
    Some(capitalise(out.trim()))
}

/// Words said otherwise than they're spelt, or too common to guess at.
fn known_arabic(word: &str, persian: bool) -> Option<&'static str> {
    Some(match word {
        "الله" => "allah",
        "لله" => "lillah",
        "والله" => "wallah",
        "بالله" => "billah",
        "يلا" | "يالله" => "yalla",
        "اللي" | "اللى" => "illi",
        "إلى" | "الى" => "ila",
        "إلا" | "الا" => "illa",
        "إنشالله" | "انشالله" => "inshallah",
        "انت" | "أنت" | "إنت" => "anta",
        "كل" => "kull",
        "مش" => "mish",
        "شكرا" => "shukran",
        "ابدا" | "أبدا" => "abadan",
        // Persian words with a final cluster, which the "a" between consonants would break.
        "دوست" => "dust",
        "است" => "ast",
        "هست" => "hast",
        "نیست" => "nist",
        "و" => if persian { "va" } else { "wa" },
        "من" if !persian => "min",
        "هو" if !persian => "huwa",
        "هي" if !persian => "hiya",
        _ => return None,
    })
}

/// A sound in a word: a consonant (and whether its vowel, or lack of one, is written), or a
/// vowel.
enum Sound {
    Consonant(String, bool),
    Vowel(&'static str),
}

/// A vowel after the sounds so far, as one with a short vowel written for the same sound
/// (fatha then ا: "a"; kasra then ي: "i").
fn push_vowel(sounds: &mut Vec<Sound>, vowel: &'static str) {
    if let Some(Sound::Vowel(before)) = sounds.last_mut() {
        if vowel.starts_with(*before) || matches!((*before, vowel), ("e", "i") | ("o", "u")) {
            *before = vowel;
            return;
        }
        if before.starts_with(vowel) {
            return;
        }
    }
    sounds.push(Sound::Vowel(vowel));
}

/// A consonant with its marks: doubled by shadda, then its vowel if one is written.
fn push_consonant(sounds: &mut Vec<Sound>, latin: &str, marks: &[char], vowel: Option<&'static str>) {
    let latin = if marks.contains(&SHADDA) { latin.repeat(2) } else { latin.to_string() };
    sounds.push(Sound::Consonant(latin, vowel.is_some() || marks.contains(&SUKUN)));
    if let Some(vowel) = vowel {
        push_vowel(sounds, vowel);
    }
}

fn spell_arabic_word(word: &[char], persian: bool, out: &mut String) {
    // Each letter with the marks written on it.
    let mut letters: Vec<(char, Vec<char>)> = Vec::new();
    for &c in word {
        if !is_arabic_mark(c) {
            letters.push((c, Vec::new()));
        } else if let Some(letter) = letters.last_mut() {
            letter.1.push(c);
        }
    }
    if letters.is_empty() {
        return;
    }
    let bare: String = letters.iter().map(|l| l.0).collect();
    if let Some(known) = known_arabic(&bare, persian) {
        out.push_str(known);
        return;
    }
    let mut start = 0;
    if let Some((article, at)) = article(&letters, persian) {
        out.push_str(&article);
        // A sun letter's shadda is the article's sound, said already.
        letters[at].1.retain(|m| *m != SHADDA);
        start = at;
    }
    out.push_str(&spell_arabic_stem(&letters[start..], persian));
}

/// The article ال (with و ف ب ك before it, or ل for لل) as "al-" or as the sun letter after
/// it ("ash-"), and where the rest of the word starts.
fn article(letters: &[(char, Vec<char>)], persian: bool) -> Option<(String, usize)> {
    if persian {
        return None;
    }
    let at = |i: usize| letters.get(i).map(|l| l.0);
    let (lead, start) = match (at(0), at(1), at(2)) {
        (Some('ا' | 'ٱ'), Some('ل'), _) => ("a", 2),
        (Some(p @ ('و' | 'ف' | 'ب' | 'ك')), Some('ا' | 'ٱ'), Some('ل')) => {
            (match p {
                'و' => "wa",
                'ف' => "fa",
                'ب' => "bi",
                _ => "ka",
            }, 3)
        }
        (Some('ل'), Some('ل'), _) => ("li", 2),
        _ => return None,
    };
    // A word after it, not ال as part of a short word (الف).
    if letters.len() < start + 2 {
        return None;
    }
    let first = letters[start].0;
    let sound = if "تثدذرزسشصضطظلن".contains(first) { arabic_consonant(first, false).unwrap_or("l") } else { "l" };
    Some((format!("{lead}{sound}-"), start))
}

/// A word (after any article) as sounds, then in Latin with an "a" where a syllable has no
/// vowel written.
fn spell_arabic_stem(letters: &[(char, Vec<char>)], persian: bool) -> String {
    let n = letters.len();
    let mut sounds: Vec<Sound> = Vec::new();
    for (j, (c, marks)) in letters.iter().enumerate() {
        let c = *c;
        let prev = j.checked_sub(1).map(|k| letters[k].0);
        let next = letters.get(j + 1).map(|l| l.0);
        let (first, last) = (j == 0, j + 1 == n);
        let vowel = marks.iter().find_map(|&m| short_vowel(m, persian));
        let after = match sounds.last() {
            Some(Sound::Vowel(v)) => Some(*v),
            _ => None,
        };
        // ي between an Arabic word's first two consonants: "ay" (ليل "layl", عيني "'ayni").
        let diphthong = !persian
            && j == 1
            && marks.is_empty()
            && !matches!(letters[0].0, 'ا' | 'ٱ' | 'آ' | 'أ' | 'إ')
            && next.is_some_and(|x| !is_vowel_letter(x));
        match c {
            'ا' | 'ٱ' => push_vowel(&mut sounds, if first { vowel.unwrap_or("a") } else if vowel == Some("an") { "an" } else { "a" }),
            'آ' => {
                if !first {
                    sounds.push(Sound::Consonant("'".to_string(), true));
                }
                push_vowel(&mut sounds, "a");
            }
            'أ' | 'إ' if first => push_vowel(&mut sounds, vowel.unwrap_or(if c == 'إ' { "i" } else { "a" })),
            // A hamza at the end of a word goes unsaid (شاء "sha").
            'ء' if last && marks.is_empty() => {}
            'أ' | 'إ' | 'ؤ' | 'ئ' | 'ء' => {
                push_consonant(&mut sounds, "'", marks, vowel);
                if vowel.is_none() && !marks.contains(&SUKUN) {
                    // The vowel the seat stands for, before a consonant (سائق "sa'iq").
                    let seat = match c {
                        'إ' => Some("i"),
                        'ؤ' => Some("u"),
                        'ئ' => Some("i"),
                        _ => None,
                    };
                    if let Some(v) = seat.filter(|_| c == 'إ' || next.is_some_and(|x| !is_vowel_letter(x))) {
                        push_vowel(&mut sounds, v);
                    }
                }
            }
            'ة' | 'ۃ' => {
                push_vowel(&mut sounds, "a");
                if let Some(v) = vowel {
                    sounds.push(Sound::Consonant("t".to_string(), true));
                    push_vowel(&mut sounds, v);
                }
            }
            'ى' => push_vowel(&mut sounds, if persian { "i" } else if vowel == Some("an") { "an" } else { "a" }),
            'ے' | 'ۓ' | 'ۀ' => push_vowel(&mut sounds, "e"),
            // Urdu's ھ breathes the consonant before it ("bh", "kh").
            'ھ' => match sounds.last_mut() {
                Some(Sound::Consonant(latin, _)) => latin.push('h'),
                _ => push_consonant(&mut sounds, "h", marks, vowel),
            },
            // A Persian or Urdu word's last ه after a consonant is a vowel (خانه "khane").
            'ه' | 'ہ' | 'ە' if persian && last && !first && marks.is_empty() && prev.is_some_and(|p| !is_vowel_letter(p)) => {
                push_vowel(&mut sounds, "e")
            }
            // Persian's خوا says no و ("khab", "khaham").
            'و' if persian && prev == Some('خ') && next == Some('ا') => {}
            // Persian's ای and او at a word's start: "i" and "u" (این "in", او "u").
            'و' | 'ي' | 'ی' if persian && j == 1 && prev == Some('ا') && marks.is_empty() && letters[0].1.is_empty() => {
                sounds.pop();
                push_vowel(&mut sounds, if c == 'و' { "u" } else { "i" });
            }
            'ي' if diphthong => push_vowel(&mut sounds, "ay"),
            'و' | 'ي' | 'ی' => {
                let (glide, long) = if c == 'و' { (if persian { "v" } else { "w" }, "u") } else { ("y", "i") };
                // A consonant at a word's start, with a vowel of its own, after a vowel it
                // doesn't lengthen, or before a vowel letter; a long vowel otherwise.
                let before_vowel = next.is_some_and(|x| matches!(x, 'ا' | 'ى' | 'ة' | 'آ') || (c != 'و' && x == 'و'))
                    || (c == 'و' && matches!(next, Some('ي' | 'ی')) && j + 2 == n);
                let glides = first || vowel.is_some() || marks.contains(&SHADDA) || matches!(after, Some(v) if v != long) || before_vowel;
                if glides {
                    push_consonant(&mut sounds, glide, marks, vowel)
                } else {
                    push_vowel(&mut sounds, long)
                }
            }
            _ => match arabic_consonant(c, persian) {
                Some(latin) => push_consonant(&mut sounds, latin, marks, vowel),
                None => sounds.push(Sound::Consonant(c.to_string(), true)),
            },
        }
        if marks.contains(&DAGGER_ALIF) {
            push_vowel(&mut sounds, "a");
        }
    }

    let mut out = String::new();
    let mut i = 0;
    while i < sounds.len() {
        if let Sound::Vowel(v) = &sounds[i] {
            out.push_str(v);
            i += 1;
            continue;
        }
        let start = i;
        while i < sounds.len() && matches!(sounds[i], Sound::Consonant(..)) {
            i += 1;
        }
        let voiced = syllable_vowels(i - start, start > 0, i < sounds.len());
        for (k, sound) in sounds[start..i].iter().enumerate() {
            if let Sound::Consonant(latin, written) = sound {
                out.push_str(latin);
                if !written && voiced.contains(&k) {
                    out.push('a');
                }
            }
        }
    }
    out
}

/// Which of a run of `n` consonants with no vowels written take an "a" after them, so each
/// syllable has a vowel: the last starts the next syllable when a vowel follows, the first
/// can end the syllable before, and the rest pair up as "CaC", with a lone "Ca" where the
/// count is odd: first at a word's end ("qamar"), last before a vowel ("madrasa").
fn syllable_vowels(n: usize, after_vowel: bool, before_vowel: bool) -> Vec<usize> {
    let end = if before_vowel { n - 1 } else { n };
    let mut voiced = Vec::new();
    if end == 0 {
        return voiced;
    }
    let mut k = if after_vowel && end % 2 == 1 { 1 } else { 0 };
    let odd = (end - k) % 2 == 1;
    if odd && !before_vowel {
        voiced.push(k);
        k += 1;
    }
    while k + 1 < end {
        voiced.push(k);
        k += 2;
    }
    if odd && before_vowel {
        voiced.push(k);
    }
    voiced
}

#[cfg(test)]
mod tests {
    use super::*;

    fn th(line: &str) -> String {
        thai(line).unwrap()
    }

    fn ar(line: &str) -> String {
        arabic(line).unwrap()
    }

    #[test]
    fn tells_the_scripts_apart() {
        assert!(is_thai("ฉันรักเธอ"));
        assert!(!is_thai("I love you so much ฉัน"));
        assert!(!is_thai("حبيبي"));
        assert!(is_arabic("يا ليل يا عين"));
        assert!(is_arabic("دوستت دارم"));
        assert!(!is_arabic("ฉันรักเธอ"));
        assert_eq!(thai("Hello"), None);
        assert_eq!(arabic("Hello"), None);
        assert_eq!(thai("حبيبي"), None);
    }

    #[test]
    fn reads_thai_lyrics() {
        assert_eq!(th("ฉันรักเธอ"), "Chan rak thoe");
        assert_eq!(th("คิดถึงเธอทุกวัน"), "Khit thueng thoe thuk wan");
        assert_eq!(th("ไม่เป็นไร"), "Mai pen rai");
        assert_eq!(th("ขอบคุณ"), "Khop khun");
        assert_eq!(th("สวัสดี"), "Sawat di");
        assert_eq!(th("หัวใจ"), "Hua chai");
        assert_eq!(th("เพลง"), "Phleng");
        assert_eq!(th("น้ำตา"), "Nam ta");
    }

    #[test]
    fn reads_thai_vowels_and_finals() {
        assert_eq!(th("ความรัก"), "Khwam rak");
        assert_eq!(th("มีความสุข"), "Mi khwam suk");
        assert_eq!(th("คนเดียว"), "Khon diao");
        assert_eq!(th("เหมือนเดิม"), "Muean doem");
        assert_eq!(th("เสียงของเธอ"), "Siang khong thoe");
        assert_eq!(th("แล้วเราก็"), "Laeo rao ko");
        assert_eq!(th("อยู่ที่ไหน"), "Yu thi nai");
        assert_eq!(th("ช่วยด้วย"), "Chuai duai");
        assert_eq!(th("เลย"), "Loei");
        assert_eq!(th("ประเทศไทย"), "Pra thet thai");
        assert_eq!(th("พรุ่งนี้"), "Phrung ni");
        assert_eq!(th("ทำไม"), "Tham mai");
        assert_eq!(th("หนึ่ง"), "Nueng");
        assert_eq!(th("รักแท้ไม่มีจริง"), "Rak thae mai mi ching");
    }

    #[test]
    fn reads_thai_unwritten_vowels() {
        assert_eq!(th("คน"), "Khon");
        assert_eq!(th("ขนม"), "Khanom");
        assert_eq!(th("สบาย"), "Sabai");
        assert_eq!(th("ตลอดไป"), "Talot pai");
        assert_eq!(th("เสมอ"), "Samoe");
        assert_eq!(th("หมด"), "Mot");
        assert_eq!(th("กรรม"), "Kam");
        assert_eq!(th("สวรรค์"), "Sawan");
    }

    #[test]
    fn reads_thai_marks_and_mixed_lines() {
        assert_eq!(th("ช้าๆ"), "Cha cha");
        assert_eq!(th("ช้า ๆ"), "Cha cha");
        assert_eq!(th("สบายๆ"), "Sabai sabai");
        assert_eq!(th("จันทร์"), "Chan");
        assert_eq!(th("ศาสตร์"), "Sat");
        assert_eq!(th("เกมส์"), "Kem");
        assert_eq!(th("ฉันรักเธอ (oh baby)"), "Chan rak thoe (oh baby)");
        assert_eq!(th("I love you ฉันรักเธอ!"), "I love you chan rak thoe!");
        assert_eq!(th("๑๒ เดือน"), "12 duean");
    }

    #[test]
    fn reads_arabic_lyrics() {
        assert_eq!(ar("حبيبي"), "Habibi");
        assert_eq!(ar("يا ليل يا عين"), "Ya layl ya 'ayn");
        assert_eq!(ar("أنا بحبك"), "Ana bahbak");
        assert_eq!(ar("إن شاء الله"), "In sha allah");
        assert_eq!(ar("قلبي"), "Qalbi");
        assert_eq!(ar("القمر"), "Al-qamar");
        assert_eq!(ar("الليل"), "Al-layl");
        assert_eq!(ar("على"), "'Ala");
        assert_eq!(ar("حلوة"), "Halwa");
        assert_eq!(ar("مدرسة"), "Madrasa");
        assert_eq!(ar("عيونك"), "'Ayunak");
        assert_eq!(ar("نور عيني"), "Nur 'ayni");
        assert_eq!(ar("انت عمري"), "Anta 'amri");
    }

    #[test]
    fn reads_arabic_vowel_marks() {
        assert_eq!(ar("حَبِيبِي"), "Habibi");
        assert_eq!(ar("قَلْبِي"), "Qalbi");
        assert_eq!(ar("شُكْرًا"), "Shukran");
        assert_eq!(ar("الشَّمْس"), "Ash-shams");
        assert_eq!(ar("حُبّ"), "Hubb");
        assert_eq!(ar("دائماً"), "Da'iman");
    }

    #[test]
    fn reads_arabic_punctuation_digits_and_forms() {
        assert_eq!(ar("ـحـبـيـبـي"), "Habibi");
        assert_eq!(ar("وين، ليش؟"), "Wayn, laysh?");
        assert_eq!(ar("٢٠٢٤ و ۱۴۰۳"), "2024 wa 1403");
        // Presentation forms: ﺣﺒﻴﺒﻲ, and the lam-alef ligature in ﻻ.
        assert_eq!(ar("\u{FEA3}\u{FE92}\u{FEF4}\u{FE92}\u{FEF2}"), "Habibi");
        assert_eq!(ar("\u{FEFB}"), "La");
        assert_eq!(ar("Oh حبيبي (yeah)"), "Oh habibi (yeah)");
    }

    #[test]
    fn reads_persian_and_urdu() {
        assert_eq!(ar("دوستت دارم"), "Dustat daram");
        assert_eq!(ar("تو را دوست دارم"), "Tu ra dust daram");
        assert_eq!(ar("زندگی"), "Zandagi");
        assert_eq!(ar("می‌خواهم"), "Mikhaham");
        assert_eq!(ar("چشم"), "Chasham");
        assert_eq!(ar("این خانه"), "In khane");
        assert_eq!(ar("دل ہے"), "Dal he");
    }
}
