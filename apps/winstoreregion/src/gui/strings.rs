//! Language selection and every user-visible string.
//!
//! Nothing in this module is written by hand. The text lives in `lang/*.toml`,
//! one file per language, and the build turns those files into the tables below:
//! the `Strings` struct, the `Language` enum, and one table per language.
//!
//! That indirection buys one thing and gives up nothing. A translation becomes a
//! data contribution — a new file, no Rust — while the compiler keeps proving
//! what it proved before: a language missing a key, holding a key nobody else
//! has, or disagreeing about the length of a list fails the build. Nothing is
//! parsed at run time, and the strings are still `&'static str` compiled into
//! the executable.
//!
//! The tables are the whole translation surface: captions, statuses,
//! diagnostics and dialog text all come from them, and no interface sentence
//! is written at its use site. The one deliberate exception is the command
//! line, which is English in every language because the arguments are read
//! before a language has been chosen — see `AI_RULES.md`.

include!(concat!(env!("OUT_DIR"), "/languages.rs"));

/// Put runtime values into a template from the language files.
///
/// `format!` cannot be used with these strings: it needs a literal, and a
/// literal is exactly what a translated string is not. So a template carries
/// `{name}` placeholders and this fills them in.
///
/// An unknown placeholder is left standing rather than removed. It is visible,
/// it names itself, and a visible `{region}` in one caption is a smaller
/// failure than a sentence that quietly loses its subject.
pub(super) fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut filled = template.to_owned();
    for (name, value) in values {
        filled = filled.replace(&format!("{{{name}}}"), value);
    }
    filled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_contract_keeps_text_outside_operation_logic() {
        assert_eq!(Language::Russian.strings().tabs[0], "Установка");
        assert_eq!(Language::English.strings().tabs[0], "Installation");
    }

    #[test]
    fn a_template_takes_its_values_and_keeps_what_it_was_not_given() {
        assert_eq!(
            fill(
                "Product ID {id} принят, регион: {region}.",
                &[("id", "ABC"), ("region", "США")]
            ),
            "Product ID ABC принят, регион: США."
        );
        // A placeholder nobody filled stays visible instead of vanishing.
        assert_eq!(fill("нет {region}", &[("id", "ABC")]), "нет {region}");
        // A value appearing twice is filled twice.
        assert_eq!(fill("{a} и {a}", &[("a", "x")]), "x и x");
    }

    #[test]
    fn every_language_the_chooser_lists_is_one_the_window_can_select() {
        // The chooser and the variants are generated from the same files, so a
        // language can never appear in one and be missing from the other. The
        // count is deliberately not pinned: adding a language is adding a file,
        // and a test that had to be edited for it would be a tax on doing so.
        assert!(LANGUAGE_NAMES.len() >= 2);
        for index in 0..LANGUAGE_NAMES.len() {
            let language = Language::from_index(isize::try_from(index).expect("small index"));
            assert_eq!(language.index(), index);
        }
    }
}
