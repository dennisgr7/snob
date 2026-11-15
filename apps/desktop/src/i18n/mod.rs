//! The words a view shows, in the system's language: English, which the
//! code is written in and every other language falls back to, and Spanish.
//! Both are Fluent files under `locales/`, built into the app.
//!
//! **A view names a message, never writes a sentence**, so the same thing is
//! called the same everywhere and a screen reader is told the language the
//! words are in ([`Texts::language`]).

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

const ENGLISH: &str = include_str!("../../locales/en/snob.ftl");
const SPANISH: &str = include_str!("../../locales/es/snob.ftl");

/// The languages the app speaks, as BCP 47 tags and their messages. The
/// first is what the others fall back to.
const LANGUAGES: [(&str, &str); 2] = [("en", ENGLISH), ("es", SPANISH)];

type Bundle = FluentBundle<FluentResource>;

fn bundle(tag: &str, source: &str) -> Bundle {
    let language: LanguageIdentifier = tag.parse().expect("a language tag written here");
    let mut bundle = FluentBundle::new_concurrent(vec![language]);
    // No direction marks around what is put in: they would be read out, and
    // every name put in is made printable before it gets here.
    bundle.set_use_isolating(false);
    let resource = match FluentResource::try_new(source.to_string()) {
        Ok(resource) => resource,
        // What parsed is still used: one bad message must not empty a
        // language. The tests fail on any.
        Err((resource, _)) => resource,
    };
    let _ = bundle.add_resource(resource);
    bundle
}

/// The words of one language, with English under it.
pub struct Texts {
    language: &'static str,
    chosen: Option<Bundle>,
    english: Bundle,
}

impl Texts {
    /// The words for a system set to `locale` (`es-ES`, `en_US.UTF-8`):
    /// that language when the app speaks it, English otherwise.
    pub fn of(locale: &str) -> Self {
        let wanted = locale
            .split(['-', '_', '.'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let chosen = LANGUAGES
            .iter()
            .skip(1)
            .find(|(tag, _)| *tag == wanted)
            .copied();
        Self {
            language: chosen.map_or(LANGUAGES[0].0, |(tag, _)| tag),
            chosen: chosen.map(|(tag, source)| bundle(tag, source)),
            english: bundle(LANGUAGES[0].0, LANGUAGES[0].1),
        }
    }

    /// The language the words are in, as a BCP 47 tag.
    pub fn language(&self) -> &'static str {
        self.language
    }

    /// The message `id`.
    pub fn tr(&self, id: &str) -> String {
        self.tr_with(id, &[])
    }

    /// The message `id`, with `values` put in. **A message nobody wrote
    /// reads as its id**, which a test of every id a view names rules out.
    pub fn tr_with(&self, id: &str, values: &[(&str, &str)]) -> String {
        let mut args = FluentArgs::new();
        for (name, value) in values {
            args.set(*name, FluentValue::from(*value));
        }
        let said = |bundle: &Bundle| {
            let pattern = bundle.get_message(id)?.value()?;
            let mut errors = Vec::new();
            let text = bundle.format_pattern(pattern, Some(&args), &mut errors);
            errors.is_empty().then(|| text.into_owned())
        };
        self.chosen
            .as_ref()
            .and_then(said)
            .or_else(|| said(&self.english))
            .unwrap_or_else(|| id.to_string())
    }

    /// The message `id` around one value a view draws in its own style:
    /// what comes before it, and what comes after.
    pub fn around(&self, id: &str, name: &str, value: &str) -> (String, String) {
        let whole = self.tr_with(id, &[(name, value)]);
        match whole.split_once(value) {
            Some((before, after)) => (before.to_string(), after.to_string()),
            None => (whole, String::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    /// Every message of a Fluent file, with the variables it takes.
    fn messages(source: &str) -> BTreeMap<String, BTreeSet<String>> {
        let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut current: Option<String> = None;
        for line in source.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            if !line.starts_with(' ')
                && let Some((id, _)) = line.split_once(" = ")
            {
                current = Some(id.to_string());
                found.entry(id.to_string()).or_default();
            }
            let Some(id) = &current else { continue };
            let mut rest = line;
            while let Some(at) = rest.find('$') {
                let name: String = rest[at + 1..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .collect();
                found.entry(id.clone()).or_default().insert(name);
                rest = &rest[at + 1..];
            }
        }
        found
    }

    #[test]
    fn every_language_parses_whole() {
        for (tag, source) in LANGUAGES {
            let parsed = FluentResource::try_new(source.to_string());
            assert!(parsed.is_ok(), "{tag} has a message Fluent does not read");
            assert!(messages(source).len() > 50, "{tag} reads as almost empty");
        }
    }

    #[test]
    fn every_language_has_the_same_messages_with_the_same_variables() {
        let english = messages(ENGLISH);
        for (tag, source) in LANGUAGES.iter().skip(1) {
            let other = messages(source);
            let missing: Vec<_> = english
                .keys()
                .filter(|id| !other.contains_key(*id))
                .collect();
            let extra: Vec<_> = other
                .keys()
                .filter(|id| !english.contains_key(*id))
                .collect();
            assert!(missing.is_empty(), "{tag} lacks {missing:?}");
            assert!(extra.is_empty(), "{tag} has {extra:?}, which English lacks");
            for (id, variables) in &english {
                assert_eq!(&other[id], variables, "{tag}: the variables of {id}");
            }
        }
    }

    /// The ids a view names are found by reading the source: a message
    /// misspelled at its use would otherwise show as its id, in a release.
    #[test]
    fn every_message_a_view_names_exists() {
        let english = messages(ENGLISH);
        let mut named = BTreeSet::new();
        let mut files = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(path) = files.pop() {
            if path.is_dir() {
                files.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for call in ["tr(\"", "tr_with(\"", "around(\"", "Msg(\""] {
                let mut rest = text.as_str();
                while let Some(at) = rest.find(call) {
                    rest = &rest[at + call.len()..];
                    let id: String = rest.chars().take_while(|c| *c != '"').collect();
                    // This file's own examples are not messages.
                    if !id.starts_with("no-such") {
                        named.insert(id);
                    }
                }
            }
        }
        let unknown: Vec<_> = named
            .iter()
            .filter(|id| !english.contains_key(*id))
            .collect();
        assert!(unknown.is_empty(), "no message is called {unknown:?}");
    }

    #[test]
    fn the_language_follows_the_system_and_falls_back_to_english() {
        let spanish = "Empecemos."; // i18n-allow
        let english = "Let's get started.";
        for (locale, language, title) in [
            ("es-ES", "es", spanish),
            ("es_MX.UTF-8", "es", spanish),
            ("ES", "es", spanish),
            ("en-GB", "en", english),
            ("fr-FR", "en", english),
            ("", "en", english),
        ] {
            let texts = Texts::of(locale);
            assert_eq!(texts.language(), language, "{locale}");
            assert_eq!(texts.tr("start-title"), title, "{locale}");
        }
    }

    #[test]
    fn a_value_is_put_in_as_it_is_given() {
        let texts = Texts::of("es");
        let downloading = "Descargando actualizaci\u{f3}n \u{b7} 63 %"; // i18n-allow
        assert_eq!(
            texts.tr_with("status-downloading", &[("percent", "63")]),
            downloading
        );
        let before = "Te damos la bienvenida, "; // i18n-allow
        assert_eq!(
            texts.around("welcome", "account", "@me"),
            (before.to_string(), ".".to_string())
        );
        assert_eq!(Texts::of("en").tr("no-such-message"), "no-such-message");
    }
}
