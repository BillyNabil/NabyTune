//! Lyrics in Latin letters, worked out on this machine: no service to ask, so no limits,
//! no waiting and no network. (Google's translate endpoint, which the web player used, turns
//! clients away with HTTP 429 more often than not.)
//!
//! - Japanese: IPADIC, a MeCab dictionary built into the binary, splits a line into words
//!   and gives each its reading in context (kanji included); the readings go to romaji, with
//!   endings and auxiliaries joined to their word ("yokatta", not "yokat ta").
//! - Korean: Hangul syllables by the Revised Romanization, carrying a final consonant over a
//!   silent ㅇ ("saranghae", "eopseo").
//! - Chinese: pinyin with tone marks, for songs with no kana at all.
//! - Cyrillic, Greek, Devanagari, Thai and Arabic script have their own readers. Other
//!   scripts use character-based transliteration when a mapping is available.

use std::borrow::Cow;

use anyhow::{anyhow, Result};
use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use pinyin::ToPinyin;
use unicode_normalization::UnicodeNormalization;
use unicode_script::{Script, UnicodeScript};
use wana_kana::ConvertJapanese;

/// Lines already written in Latin letters need no romanisation.
pub fn is_latin(text: &str) -> bool {
    !text.chars().any(needs_reading)
}

fn needs_reading(c: char) -> bool {
    match c.script() {
        Script::Latin | Script::Common | Script::Inherited => false,
        _ => c.is_alphabetic() || c.is_numeric() || unicode_normalization::char::is_combining_mark(c),
    }
}

fn is_kana(c: char) -> bool {
    matches!(c, '\u{3041}'..='\u{309F}' | '\u{30A0}'..='\u{30FF}' | '\u{31F0}'..='\u{31FF}' | '\u{FF66}'..='\u{FF9D}')
}

fn is_han(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}' | '々')
}

fn is_hangul(c: char) -> bool {
    matches!(c, '\u{AC00}'..='\u{D7A3}')
}

/// Each line in Latin letters, or None where it already is (or has nothing to read).
// Thai, and Arabic script (Arabic, Persian, Urdu).
#[path = "romaji_more.rs"]
mod more;

pub fn romanize(lines: &[String]) -> Result<Vec<Option<String>>> {
    // Kana in another line can identify a line of otherwise ambiguous Han characters.
    let japanese = lines.iter().any(|line| line.nfkc().any(is_kana));
    let mut segmenter = None;
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        let original = line.trim();
        let mut reading: String = original.nfkc().collect();
        if is_latin(&reading) && reading == original {
            out.push(None);
            continue;
        }

        if reading.chars().any(is_hangul) {
            reading = korean(&reading);
        }
        if reading.chars().any(is_kana) || (japanese && !original.nfkc().any(is_hangul) && reading.chars().any(is_han)) {
            if segmenter.is_none() {
                let dictionary = load_dictionary("embedded://ipadic").map_err(|e| anyhow!("Japanese dictionary: {e}"))?;
                segmenter = Some(Segmenter::new(Mode::Normal, dictionary, None));
            }
            reading = japanese_line(segmenter.as_ref().expect("Japanese segmenter"), &reading)?;
        } else if reading.chars().any(is_han) {
            reading = chinese(&reading);
        }
        if reading.chars().any(|c| c.script() == Script::Cyrillic) {
            reading = reading.chars().map(cyrillic).collect();
        }
        if reading.chars().any(|c| c.script() == Script::Greek) {
            reading = reading.chars().map(greek).collect();
        }
        if reading.chars().any(|c| c.script() == Script::Devanagari) {
            reading = devanagari(&reading);
        }
        if let Some(thai) = more::thai(&reading) {
            reading = thai;
        }
        if let Some(arabic) = more::arabic(&reading) {
            reading = arabic;
        }
        if !is_latin(&reading) {
            reading = approximate_reading(&reading);
        }
        let reading = reading.trim();
        out.push((!reading.is_empty() && reading != original).then(|| capitalise(reading)));
    }
    Ok(out)
}

/// Character-based fallback for scripts without a dedicated phonetic reader.
fn approximate_reading(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for c in text.chars() {
        if needs_reading(c) {
            if let Some(latin) = deunicode::deunicode_char(c) {
                output.push_str(latin);
                continue;
            }
        }
        output.push(c);
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Full-width punctuation as its plain counterpart, or None for a character to drop.
fn punctuation(c: char) -> Option<char> {
    Some(match c {
        '、' | '，' => ',',
        '。' | '．' => '.',
        '！' => '!',
        '？' => '?',
        '：' => ':',
        '；' => ';',
        '（' => '(',
        '）' => ')',
        '～' | '〜' => '~',
        '・' | '「' | '」' | '『' | '』' | '【' | '】' => return None,
        '　' => ' ',
        c => c,
    })
}

/// Romaji from kana, with the long-vowel mark as a doubled vowel ("tōto" → "tooto").
fn kana_to_romaji(kana: &str) -> String {
    let romaji = kana.to_romaji();
    let mut out = String::with_capacity(romaji.len());
    for c in romaji.chars() {
        if c == '-' || c == 'ー' {
            match out.chars().last().filter(|v| "aeiou".contains(*v)) {
                Some(v) => out.push(v),
                None => out.push('-'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn japanese_line(segmenter: &Segmenter, text: &str) -> Result<String> {
    let mut tokens = segmenter.segment(Cow::Borrowed(text)).map_err(|e| anyhow!("{e}"))?;
    // Words as kana (or as written, for Latin letters and digits), with whether each one
    // hangs on the word before it.
    let mut words: Vec<String> = Vec::new();
    // What the word before was: endings join verbs and adjectives, not nouns or Latin letters.
    let mut previous = String::new();
    for token in tokens.iter_mut() {
        let surface = token.surface.to_string();
        let details: Vec<String> = token.details().iter().map(|s| s.to_string()).collect();
        let field = |i: usize| details.get(i).map(String::as_str).filter(|s| *s != "*");
        let (pos, sub) = (field(0).unwrap_or(""), field(1).unwrap_or(""));

        if pos == "記号" || surface.chars().all(|c| !c.is_alphanumeric() && !is_kana(c) && !is_han(c)) {
            // Closing marks end the word before; opening ones start the next ("itai, (oh)").
            for mark in surface.chars().filter_map(punctuation).filter(|c| !c.is_whitespace()) {
                match words.last_mut() {
                    Some(word) if !"([{\"".contains(mark) && !word.ends_with(['(', '[', '{']) => word.push(mark),
                    Some(word) if word.ends_with(['(', '[', '{']) => word.push(mark),
                    _ => words.push(mark.to_string()),
                }
            }
            previous = pos.to_string();
            continue;
        }
        // Particles by how they sound (は "wa", へ "e", を "o"); everything else by its reading.
        let reading = if pos == "助詞" { field(8).or(field(7)) } else { field(7) };
        let mut kana = match reading {
            // IPADIC sounds を as ヲ, which romanises as "wo".
            Some("ヲ") if pos == "助詞" => "オ".to_string(),
            Some(r) if r.chars().all(|c| is_kana(c) || c == 'ー') => r.to_string(),
            // Not in the dictionary: kana stays as written; anything else as it is.
            _ => surface.clone(),
        };
        // A word with は grown on (本当は, read ホントウハ but said ホントーワ): the particle
        // stands apart, as "wa".
        let mut particle = None;
        if pos != "助詞" && kana.chars().count() > 1 && kana.ends_with('ハ') && field(8).is_some_and(|p| p.ends_with('ワ')) {
            kana.pop();
            particle = Some("ワ".to_string());
        }
        // "yokatta", "shiteiku", but "tokubetsu desu", "kanzen na": the copula (です, だ and
        // its forms) stands apart, as does any ending after a noun.
        let copula = matches!(field(6), Some("です" | "だ"));
        let after_inflected = matches!(previous.as_str(), "動詞" | "形容詞" | "助動詞");
        let opened = words.last().is_some_and(|w| w.ends_with(['(', '[', '{']));
        let hangs = !words.is_empty()
            && (opened
                || (pos == "助動詞" && after_inflected && !copula)
                || (pos == "動詞" && matches!(sub, "非自立" | "接尾"))
                || (pos == "形容詞" && sub == "非自立")
                || (pos == "名詞" && sub == "接尾")
                || (pos == "助詞" && sub == "接続助詞" && matches!(surface.as_str(), "て" | "で" | "ば"))
                || kana.starts_with(['ッ', 'っ', 'ー', 'ャ', 'ュ', 'ョ', 'ゃ', 'ゅ', 'ょ']));
        match words.last_mut() {
            Some(word) if hangs => word.push_str(&kana),
            _ => words.push(kana),
        }
        words.extend(particle);
        previous = pos.to_string();
    }
    let line = words.iter().map(|w| kana_to_romaji(w)).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    Ok(capitalise(&line))
}

fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn chinese(text: &str) -> String {
    let mut out = String::new();
    for (c, p) in text.chars().zip(text.to_pinyin()) {
        match p {
            Some(p) => {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str(p.with_tone());
            }
            None => {
                if let Some(c) = punctuation(c) {
                    out.push(c);
                }
            }
        }
    }
    capitalise(out.trim())
}

/// A letter in its Latin spelling, keeping its case: `map` gives the lower-case one.
fn cased(c: char, map: fn(char) -> Option<&'static str>) -> String {
    let lower = c.to_lowercase().next().unwrap_or(c);
    match map(lower) {
        Some(latin) if c != lower => {
            let mut chars = latin.chars();
            chars.next().map(|f| f.to_uppercase().chain(chars).collect()).unwrap_or_default()
        }
        Some(latin) => latin.to_string(),
        None => c.to_string(),
    }
}

/// Cyrillic (Russian, Ukrainian, Belarusian, Bulgarian) as English speakers spell it.
fn cyrillic(c: char) -> String {
    cased(c, |c| {
        Some(match c {
            'а' => "a", 'б' => "b", 'в' => "v", 'г' => "g", 'ґ' => "g", 'д' => "d", 'е' => "e", 'ё' => "yo",
            'є' => "ye", 'ж' => "zh", 'з' => "z", 'и' => "i", 'і' => "i", 'ї' => "yi", 'й' => "y", 'к' => "k",
            'л' => "l", 'м' => "m", 'н' => "n", 'о' => "o", 'п' => "p", 'р' => "r", 'с' => "s", 'т' => "t",
            'у' => "u", 'ў' => "w", 'ф' => "f", 'х' => "kh", 'ц' => "ts", 'ч' => "ch", 'ш' => "sh", 'щ' => "shch",
            'ъ' => "", 'ы' => "y", 'ь' => "", 'э' => "e", 'ю' => "yu", 'я' => "ya",
            _ => return None,
        })
    })
}

/// Greek, accents and all, as it is read today.
fn greek(c: char) -> String {
    cased(c, |c| {
        Some(match c {
            'α' | 'ά' | 'ἀ' | 'ἁ' | 'ὰ' | 'ᾶ' => "a", 'β' => "v", 'γ' => "g", 'δ' => "d", 'ε' | 'έ' | 'ἐ' | 'ἑ' | 'ὲ' => "e",
            'ζ' => "z", 'η' | 'ή' | 'ἠ' | 'ἡ' | 'ὴ' | 'ῆ' => "i", 'θ' => "th", 'ι' | 'ί' | 'ϊ' | 'ΐ' | 'ἰ' | 'ἱ' | 'ὶ' | 'ῖ' => "i",
            'κ' => "k", 'λ' => "l", 'μ' => "m", 'ν' => "n", 'ξ' => "x", 'ο' | 'ό' | 'ὀ' | 'ὁ' | 'ὸ' => "o", 'π' => "p",
            'ρ' | 'ῥ' => "r", 'σ' | 'ς' => "s", 'τ' => "t", 'υ' | 'ύ' | 'ϋ' | 'ΰ' | 'ὐ' | 'ὑ' | 'ὺ' | 'ῦ' => "y", 'φ' => "f",
            'χ' => "ch", 'ψ' => "ps", 'ω' | 'ώ' | 'ὠ' | 'ὡ' | 'ὼ' | 'ῶ' => "o",
            _ => return None,
        })
    })
}

/// Hindi (Devanagari): each consonant carries an "a" unless a vowel sign or virama says
/// otherwise; a word's last "a" goes unsaid, as Hindi drops it.
fn devanagari(text: &str) -> String {
    let consonant = |c: char| -> Option<&'static str> {
        Some(match c {
            'क' => "k", 'ख' => "kh", 'ग' => "g", 'घ' => "gh", 'ङ' => "ng", 'च' => "ch", 'छ' => "chh", 'ज' => "j", 'झ' => "jh",
            'ञ' => "ny", 'ट' => "t", 'ठ' => "th", 'ड' => "d", 'ढ' => "dh", 'ण' => "n", 'त' => "t", 'थ' => "th", 'द' => "d",
            'ध' => "dh", 'न' => "n", 'प' => "p", 'फ' => "ph", 'ब' => "b", 'भ' => "bh", 'म' => "m", 'य' => "y", 'र' => "r",
            'ल' => "l", 'व' => "v", 'श' => "sh", 'ष' => "sh", 'स' => "s", 'ह' => "h", 'ळ' => "l",
            // The dotted (nukta) letters, when written as one character each.
            '\u{0958}' => "q", '\u{0959}' => "kh", '\u{095A}' => "gh", '\u{095B}' => "z", '\u{095C}' => "r", '\u{095D}' => "rh", '\u{095E}' => "f",
            _ => return None,
        })
    };
    let vowel = |c: char| -> Option<&'static str> {
        Some(match c {
            'अ' => "a", 'आ' => "aa", 'इ' => "i", 'ई' => "ee", 'उ' => "u", 'ऊ' => "oo", 'ऋ' => "ri", 'ए' => "e", 'ऐ' => "ai",
            'ओ' => "o", 'औ' => "au",
            _ => return None,
        })
    };
    let sign = |c: char| -> Option<&'static str> {
        Some(match c {
            'ा' => "aa", 'ि' => "i", 'ी' => "ee", 'ु' => "u", 'ू' => "oo", 'ृ' => "ri", 'े' => "e", 'ै' => "ai", 'ो' => "o", 'ौ' => "au",
            _ => return None,
        })
    };
    let mut out = String::new();
    let mut word: Vec<Unit> = Vec::new();
    for c in text.chars() {
        if let Some(latin) = consonant(c) {
            word.push(Unit::Consonant(latin, None));
        } else if c == '\u{093C}' {
            // Nukta: a dot under the consonant before; its sound is close enough already.
        } else if let Some(latin) = sign(c) {
            match word.last_mut() {
                Some(Unit::Consonant(_, v)) => *v = Some(latin),
                _ => word.push(Unit::Vowel(latin)),
            }
        } else if c == '\u{094D}' {
            // Virama: no vowel after this consonant.
            if let Some(Unit::Consonant(_, v)) = word.last_mut() {
                *v = Some("");
            }
        } else if c == 'ं' || c == 'ँ' {
            word.push(Unit::Mark('n'));
        } else if c == 'ः' {
            word.push(Unit::Mark('h'));
        } else if let Some(latin) = vowel(c) {
            word.push(Unit::Vowel(latin));
        } else {
            spell_hindi_word(&word, &mut out);
            word.clear();
            match c {
                '।' | '॥' => out.push('.'),
                '०'..='९' => out.push(char::from(b'0' + (c as u32 - '०' as u32) as u8)),
                _ => out.push(c),
            }
        }
    }
    spell_hindi_word(&word, &mut out);
    out
}

/// A piece of a Devanagari word: a consonant with its vowel sign (None: the unwritten "a";
/// "": none, after a virama), a vowel letter, or a nasal or breath mark.
enum Unit {
    Consonant(&'static str, Option<&'static str>),
    Vowel(&'static str),
    Mark(char),
}

/// One Hindi word in Latin letters, with Hindi's own dropped "a"s: a word's last, and one
/// between a sounded vowel and a consonant that has its own (करता "karta", तुमसे "tumse").
/// A long "aa" at the very end is written "a", as usual in romanised Hindi.
fn spell_hindi_word(word: &[Unit], out: &mut String) {
    let n = word.len();
    // Whether each unwritten "a" is said.
    let mut said = vec![true; n];
    if n > 1 {
        if let Unit::Consonant(_, None) = word[n - 1] {
            said[n - 1] = false;
        }
    }
    let sounded = |i: usize, said: &[bool]| match &word[i] {
        Unit::Consonant(_, None) => said[i],
        Unit::Consonant(_, Some(v)) => !v.is_empty(),
        Unit::Vowel(_) => true,
        Unit::Mark(_) => false,
    };
    for i in (1..n.saturating_sub(1)).rev() {
        if let Unit::Consonant(_, None) = word[i] {
            let next_has_vowel = matches!(word[i + 1], Unit::Consonant(..)) && sounded(i + 1, &said);
            if sounded(i - 1, &said) && next_has_vowel {
                said[i] = false;
            }
        }
    }
    for (i, unit) in word.iter().enumerate() {
        let last = i + 1 == n;
        match unit {
            Unit::Consonant(c, vowel) => {
                out.push_str(c);
                match vowel {
                    None if said[i] => out.push('a'),
                    Some("aa") if last => out.push('a'),
                    Some(v) => out.push_str(v),
                    None => {}
                }
            }
            Unit::Vowel(v) => out.push_str(if last && *v == "aa" { "a" } else { v }),
            Unit::Mark(m) => out.push(*m),
        }
    }
}

const INITIALS: [&str; 19] = ["g", "kk", "n", "d", "tt", "r", "m", "b", "pp", "s", "ss", "", "j", "jj", "ch", "k", "t", "p", "h"];
const VOWELS: [&str; 21] =
    ["a", "ae", "ya", "yae", "eo", "e", "yeo", "ye", "o", "wa", "wae", "oe", "yo", "u", "wo", "we", "wi", "yu", "eu", "ui", "i"];
/// A final consonant at a word's end or before another consonant.
const FINALS: [&str; 28] =
    ["", "k", "k", "k", "n", "n", "n", "t", "l", "k", "m", "l", "l", "l", "p", "l", "m", "p", "p", "t", "t", "ng", "t", "t", "k", "t", "p", "t"];
/// The same final, carried over a silent ㅇ onto the next vowel.
const CARRIED: [&str; 28] = [
    "", "g", "kk", "ks", "n", "nj", "n", "d", "r", "lg", "lm", "lb", "ls", "lt", "lp", "r", "m", "b", "ps", "s", "ss", "ng", "j", "ch", "k", "t", "p", "",
];

fn korean(text: &str) -> String {
    let parts = |c: char| {
        let n = c as u32 - 0xAC00;
        ((n / 588) as usize, ((n % 588) / 28) as usize, (n % 28) as usize)
    };
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !is_hangul(c) {
            if let Some(c) = punctuation(c) {
                out.push(c);
            }
            continue;
        }
        let (initial, vowel, last) = parts(c);
        let start = i.checked_sub(1).map(|j| chars[j]).filter(|p| is_hangul(*p))
            .map_or(INITIALS[initial], |prev| korean_boundary(parts(prev).2, initial, vowel).1);
        out.push_str(start);
        out.push_str(VOWELS[vowel]);
        let end = chars.get(i + 1).filter(|&&next| is_hangul(next)).map_or(FINALS[last], |&next| {
            let (initial, vowel, _) = parts(next);
            korean_boundary(last, initial, vowel).0
        });
        out.push_str(end);
    }
    capitalise(&out)
}

/// Sound changes between adjacent Hangul syllables within a written word.
fn korean_boundary(tail: usize, onset: usize, vowel: usize) -> (&'static str, &'static str) {
    let final_sound = FINALS[tail];
    let initial_sound = INITIALS[onset];
    match (tail, final_sound, onset, vowel) {
        (7, _, 11, 20) => ("", "j"),
        (25, _, 11, 20) => ("", "ch"),
        (27, _, 0 | 3 | 7 | 12, _) => ("", match onset { 0 => "k", 3 => "t", 7 => "p", _ => "ch" }),
        (4 | 8, _, 5, _) => ("l", "l"),
        (8, _, 2, _) => ("l", "l"),
        (0, _, 5, _) => ("", "r"),
        (_, "ng" | "m", 5, _) => (final_sound, "n"),
        (_, "k", 2 | 6 | 5, _) => ("ng", if onset == 5 { "n" } else { initial_sound }),
        (_, "p", 2 | 6 | 5, _) => ("m", if onset == 5 { "n" } else { initial_sound }),
        (_, "t", 2 | 6, _) => ("n", initial_sound),
        (_, _, 11, _) => (CARRIED[tail], ""),
        _ => (final_sound, initial_sound),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_japanese_in_context() {
        let lines = vec!["夢ならばどれほどよかったでしょう".to_string(), "忘れた物を取りに帰るように".to_string(), "Hello".to_string()];
        let got = romanize(&lines).unwrap();
        assert_eq!(got[2], None);
        let first = got[0].clone().unwrap();
        assert!(first.starts_with("Yume"), "{first}");
        assert!(first.contains("yokatta"), "{first}");
        let second = got[1].clone().unwrap();
        assert!(second.starts_with("Wasureta mono o tori ni kaeru"), "{second}");
    }

    #[test]
    fn places_brackets_and_particles() {
        let lines = vec!["温かくて 痛い、(oh)".to_string(), "本当は辛いはずなのにな".to_string()];
        let got: Vec<String> = romanize(&lines).unwrap().into_iter().map(Option::unwrap).collect();
        assert!(got[0].ends_with("itai, (oh)"), "{}", got[0]);
        assert!(got[1].starts_with("Hontou wa"), "{}", got[1]);
    }

    #[test]
    fn keeps_the_copula_apart() {
        let lines = vec!["はいはいあの子は特別です".to_string(), "嘘でもそれは完全なアイ".to_string(), "お星様の引き立て役Bです".to_string()];
        let got: Vec<String> = romanize(&lines).unwrap().into_iter().map(Option::unwrap).collect();
        assert!(got[0].ends_with("tokubetsu desu"), "{}", got[0]);
        assert!(got[1].contains("kanzen na"), "{}", got[1]);
        assert!(got[2].ends_with("B desu"), "{}", got[2]);
    }

    #[test]
    fn reads_cyrillic_greek_and_hindi() {
        let lines = vec!["Я люблю тебя".to_string(), "Σ' αγαπώ".to_string(), "मैं तुमसे प्यार करता हूँ".to_string(), "Щастя є".to_string()];
        let got: Vec<String> = romanize(&lines).unwrap().into_iter().map(Option::unwrap).collect();
        assert_eq!(got[0], "Ya lyublyu tebya");
        assert_eq!(got[1], "S' agapo");
        assert_eq!(got[2], "Main tumse pyaar karta hoon");
        assert_eq!(got[3], "Shchastya ye");
    }

    #[test]
    fn reads_korean() {
        assert_eq!(korean("사랑해"), "Saranghae");
        assert_eq!(korean("없어"), "Eopseo");
        assert_eq!(korean("서울"), "Seoul");
        assert_eq!(korean("말리"), "Malli");
        assert_eq!(korean("국물"), "Gungmul");
        assert_eq!(korean("같이"), "Gachi");
        assert_eq!(korean("신라"), "Silla");
        assert_eq!(korean("좋고"), "Joko");
    }

    #[test]
    fn reads_decomposed_hangul_and_mixed_scripts() {
        let decomposed: String = "한국어".nfd().collect();
        let lines = [format!("Hey, {decomposed}!"), "안녕, мир".into(), "I love you ฉัน".into(), "Привет أحبك".into()];
        let converted = romanize(&lines).unwrap();
        assert_eq!(converted[0].as_deref(), Some("Hey, hangugeo!"));
        assert_eq!(converted[1].as_deref(), Some("Annyeong, mir"));
        assert_eq!(converted[2].as_deref(), Some("I love you chan"));
        assert!(converted[3].as_deref().is_some_and(is_latin));
    }

    #[test]
    fn expands_script_coverage_without_rewriting_existing_latin() {
        let lines = ["שלום".into(), "გამარჯობა".into(), "বাংলা".into(), "Tiếng Việt".into()];
        let converted = romanize(&lines).unwrap();
        assert!(converted[..3].iter().all(|line| line.as_deref().is_some_and(is_latin)));
        assert_eq!(converted[3], None);
    }

    #[test]
    fn reads_chinese_without_kana() {
        let got = romanize(&["我爱你".to_string()]).unwrap();
        assert_eq!(got[0].as_deref(), Some("Wǒ ài nǐ"));
    }
}
