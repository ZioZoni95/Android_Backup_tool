//! Localizzazione: la lingua di sistema viene rilevata all'avvio.
//!
//! Il testo sorgente nel codice è in italiano e fa da chiave: `t!("testo")` restituisce
//! il testo così com'è in italiano, oppure la traduzione presa da `i18n_en.rs` per le altre
//! lingue. Una chiave senza traduzione ricade sull'italiano, quindi non si rompe nulla.
//! Per aggiungere una lingua: variante in `Lang`, rilevamento in `from_tag`, tabella dedicata.
//!
//! Segnaposto: `t!("annullati {n} job", n = 3)`; i nomi devono coincidere in tutte le lingue.

use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    It,
    En,
}

/// Lingua da un tag tipo `it_IT.UTF-8`, `it-IT`, `en_US`. Qualsiasi lingua non italiana → inglese.
pub fn from_tag(tag: &str) -> Option<Lang> {
    let t = tag.trim().to_ascii_lowercase();
    if t.is_empty() || t == "c" || t == "posix" {
        return None;
    }
    if t.starts_with("it") {
        Some(Lang::It)
    } else {
        Some(Lang::En)
    }
}

#[cfg(windows)]
fn os_locale() -> Option<String> {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buf = [0u16; 85];
    let n = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
    if n > 1 {
        Some(String::from_utf16_lossy(&buf[..(n as usize - 1)]))
    } else {
        None
    }
}

#[cfg(not(windows))]
fn os_locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| from_tag(v.split(':').next().unwrap_or("")).is_some())
        .map(|v| v.split(':').next().unwrap_or("").to_string())
}

fn detect() -> Lang {
    if let Ok(v) = std::env::var("ABT_LANG") {
        if let Some(l) = from_tag(&v) {
            return l;
        }
    }
    os_locale().and_then(|t| from_tag(&t)).unwrap_or(Lang::En)
}

/// Lingua corrente (rilevata una sola volta; `ABT_LANG=it|en` la forza).
pub fn lang() -> Lang {
    static L: OnceLock<Lang> = OnceLock::new();
    *L.get_or_init(detect)
}

fn en_map() -> &'static HashMap<&'static str, &'static str> {
    static M: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    M.get_or_init(|| crate::i18n_en::EN.iter().copied().collect())
}

/// Traduce una chiave senza segnaposto.
pub fn tr(key: &'static str) -> &'static str {
    match lang() {
        Lang::It => key,
        Lang::En => en_map().get(key).copied().unwrap_or(key),
    }
}

/// Traduce una chiave sostituendo i segnaposto `{nome}`.
pub fn tr_args(key: &'static str, args: &[(&str, String)]) -> String {
    let mut s = tr(key).to_string();
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

/// `t!("testo")` oppure `t!("testo {x}", x = valore)`.
#[macro_export]
macro_rules! t {
    ($key:literal) => {
        $crate::i18n::tr($key)
    };
    ($key:literal, $($name:ident = $val:expr),+ $(,)?) => {
        $crate::i18n::tr_args($key, &[$((stringify!($name), ($val).to_string())),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholders(s: &str) -> Vec<String> {
        let mut v = Vec::new();
        let mut rest = s;
        while let Some(i) = rest.find('{') {
            if let Some(j) = rest[i..].find('}') {
                v.push(rest[i + 1..i + j].to_string());
                rest = &rest[i + j + 1..];
            } else {
                break;
            }
        }
        v.sort();
        v
    }

    #[test]
    fn tags() {
        assert_eq!(from_tag("it_IT.UTF-8"), Some(Lang::It));
        assert_eq!(from_tag("it-IT"), Some(Lang::It));
        assert_eq!(from_tag("en_US.UTF-8"), Some(Lang::En));
        assert_eq!(from_tag("de_DE"), Some(Lang::En));
        assert_eq!(from_tag("C"), None);
        assert_eq!(from_tag(""), None);
    }

    #[test]
    fn catalog_is_consistent() {
        let mut seen = std::collections::HashSet::new();
        for (k, v) in crate::i18n_en::EN {
            assert!(seen.insert(*k), "chiave duplicata: {k}");
            assert_eq!(placeholders(k), placeholders(v), "segnaposto diversi: {k}");
            assert!(!v.is_empty(), "traduzione vuota: {k}");
        }
    }

    #[test]
    fn args_are_substituted() {
        assert_eq!(tr_args("x {n} y", &[("n", "3".into())]).replace("x 3 y", "ok"), "ok");
    }
}
