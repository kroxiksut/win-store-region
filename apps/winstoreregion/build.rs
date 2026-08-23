//! Windows resources, and the interface text compiled from the language files.

#[cfg(windows)]
fn main() {
    use std::path::PathBuf;

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace_dir = manifest_dir.join("../..");
    let icon_path = workspace_dir.join("assets/icons/app/winstoreregion.ico");
    let manifest_path = manifest_dir.join("app.manifest");

    println!("cargo:rerun-if-changed={}", icon_path.display());
    println!("cargo:rerun-if-changed={}", manifest_path.display());

    languages::generate(&workspace_dir.join("lang"));

    let mut resources = winres::WindowsResource::new();
    resources.set_icon(&icon_path.to_string_lossy());
    resources.set_manifest_file(&manifest_path.to_string_lossy());
    resources.compile().expect("compile Windows resources");
}

#[cfg(not(windows))]
fn main() {}

/// Turning `lang/*.toml` into the tables the interface reads.
///
/// The generator exists so that a translation is a data contribution rather
/// than a code one, without giving up what the compiler already guarantees: a
/// language missing a key, holding an unknown key, or disagreeing about the
/// length of a list fails the build rather than shipping half-translated.
#[cfg(windows)]
mod languages {
    use std::collections::BTreeMap;
    use std::fmt::Write as _;
    use std::path::Path;

    /// One value a language file can hold for a key.
    #[derive(Debug, Eq, PartialEq)]
    enum Value {
        Text(String),
        List(Vec<String>),
    }

    /// One language file, already validated on its own.
    struct LanguageFile {
        /// Rust identifier for this language's variant.
        variant: String,
        /// BCP 47 tag, kept for the record and for future use.
        code: String,
        /// What the language chooser shows.
        name: String,
        /// Who translated this language, as they wish to be credited.
        authors: Vec<String>,
        /// Position in the chooser, and therefore the variant's index.
        order: i64,
        strings: BTreeMap<String, Value>,
        /// Where it came from, so a complaint can name the file.
        file: String,
    }

    pub(super) fn generate(directory: &Path) {
        println!("cargo:rerun-if-changed={}", directory.display());
        let mut languages: Vec<LanguageFile> = read_directory(directory);
        assert!(
            !languages.is_empty(),
            "no language files found in {}",
            directory.display()
        );
        languages.sort_by_key(|language| language.order);
        check_agreement(&languages);

        let reference = &languages[0];
        let mut generated = String::new();
        write_struct(&mut generated, reference);
        write_enum(&mut generated, &languages);
        write_tables(&mut generated, &languages);

        let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("out dir"))
            .join("languages.rs");
        std::fs::write(&out, generated).expect("write generated languages");
    }

    /// Read every `*.toml` in the directory, failing loudly on a bad one.
    fn read_directory(directory: &Path) -> Vec<LanguageFile> {
        let entries = std::fs::read_dir(directory)
            .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()));
        let mut languages = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "toml") {
                continue;
            }
            println!("cargo:rerun-if-changed={}", path.display());
            languages.push(read_file(&path));
        }
        languages
    }

    fn read_file(path: &Path) -> LanguageFile {
        let file = path.display().to_string();
        let text = std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{file}: {error}"));
        let document: toml::Value =
            toml::from_str(&text).unwrap_or_else(|error| panic!("{file}: {error}"));

        let field = |name: &str| -> String {
            document
                .get(name)
                .and_then(toml::Value::as_str)
                .unwrap_or_else(|| panic!("{file}: missing string field `{name}`"))
                .to_owned()
        };
        let variant = field("variant");
        assert!(
            variant
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
                && variant.starts_with(char::is_uppercase),
            "{file}: `variant` must be an upper-camel-case ASCII identifier, got `{variant}`"
        );

        let table = document
            .get("strings")
            .and_then(toml::Value::as_table)
            .unwrap_or_else(|| panic!("{file}: missing `[strings]` table"));
        let mut strings = BTreeMap::new();
        for (key, value) in table {
            let value = match value {
                toml::Value::String(text) => Value::Text(text.clone()),
                toml::Value::Array(items) => Value::List(
                    items
                        .iter()
                        .map(|item| {
                            item.as_str()
                                .unwrap_or_else(|| {
                                    panic!("{file}: `{key}` holds a non-string list item")
                                })
                                .to_owned()
                        })
                        .collect(),
                ),
                _ => panic!("{file}: `{key}` must be a string or a list of strings"),
            };
            strings.insert(key.clone(), value);
        }

        LanguageFile {
            variant,
            code: field("code"),
            name: field("name"),
            authors: document
                .get("authors")
                .and_then(toml::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|item| {
                            item.as_str()
                                .unwrap_or_else(|| panic!("{file}: `authors` holds a non-string"))
                                .to_owned()
                        })
                        .collect()
                })
                .unwrap_or_default(),
            order: document
                .get("order")
                .and_then(toml::Value::as_integer)
                .unwrap_or_else(|| panic!("{file}: missing integer field `order`")),
            strings,
            file,
        }
    }

    /// Every language must describe exactly the same keys, in the same shapes.
    ///
    /// This is the whole reason the generator exists. A key present in one
    /// language and absent from another would compile into a table with a hole,
    /// and the hole would surface as a blank caption for whoever chose that
    /// language — which is exactly the kind of thing nobody notices until a user
    /// reports it.
    fn check_agreement(languages: &[LanguageFile]) {
        let reference = &languages[0];
        for language in &languages[1..] {
            for key in reference.strings.keys() {
                assert!(
                    language.strings.contains_key(key),
                    "{}: missing key `{key}`, which {} defines",
                    language.file,
                    reference.file
                );
            }
            for key in language.strings.keys() {
                assert!(
                    reference.strings.contains_key(key),
                    "{}: unknown key `{key}`, which {} does not define",
                    language.file,
                    reference.file
                );
            }
            for (key, value) in &reference.strings {
                let other = &language.strings[key];
                match (value, other) {
                    (Value::Text(expected), Value::Text(actual)) => {
                        check_placeholders(&language.file, key, expected, actual);
                    }
                    (Value::List(expected), Value::List(actual)) => assert!(
                        expected.len() == actual.len(),
                        "{}: `{key}` has {} items, but {} has {}",
                        language.file,
                        actual.len(),
                        reference.file,
                        expected.len()
                    ),
                    _ => panic!(
                        "{}: `{key}` is a different shape from the one in {}",
                        language.file, reference.file
                    ),
                }
            }
        }
        let mut orders: Vec<i64> = languages.iter().map(|language| language.order).collect();
        orders.dedup();
        assert!(
            orders.len() == languages.len(),
            "two language files share the same `order`"
        );
    }

    /// A translation must carry the same `{placeholders}` as the original.
    ///
    /// This is the one thing about a translation that can be checked without
    /// reading the language. A lost placeholder is not a typo: it is a sentence
    /// that has quietly dropped the region, the file name or the version it was
    /// about, and it would ship looking perfectly well-formed.
    fn check_placeholders(file: &str, key: &str, expected: &str, actual: &str) {
        let names = |text: &str| -> Vec<String> {
            let mut found = Vec::new();
            let mut rest = text;
            while let Some(open) = rest.find('{') {
                rest = &rest[open + 1..];
                let Some(close) = rest.find('}') else { break };
                let name = &rest[..close];
                if !name.is_empty()
                    && name
                        .chars()
                        .all(|character| character.is_ascii_lowercase() || character == '_')
                {
                    found.push(name.to_owned());
                }
                rest = &rest[close + 1..];
            }
            found.sort();
            found.dedup();
            found
        };
        let expected = names(expected);
        let actual = names(actual);
        assert!(
            expected == actual,
            "{file}: `{key}` uses placeholders {actual:?}, but the reference uses {expected:?}"
        );
    }

    fn write_struct(generated: &mut String, reference: &LanguageFile) {
        generated.push_str(
            "/// Every user-visible string, generated from `lang/*.toml`.\n\
             #[derive(Clone, Copy)]\n\
             pub(super) struct Strings {\n",
        );
        for (key, value) in &reference.strings {
            match value {
                Value::Text(_) => {
                    let _ = writeln!(generated, "    pub(super) {key}: &'static str,");
                }
                Value::List(items) => {
                    let _ = writeln!(
                        generated,
                        "    pub(super) {key}: [&'static str; {}],",
                        items.len()
                    );
                }
            }
        }
        generated.push_str("}\n\n");
    }

    fn write_enum(generated: &mut String, languages: &[LanguageFile]) {
        generated.push_str(
            "/// Interface languages, in the order the chooser lists them.\n\
             #[derive(Clone, Copy, Debug, Eq, PartialEq)]\n\
             pub(super) enum Language {\n",
        );
        for language in languages {
            let _ = writeln!(generated, "    /// {}", language.code);
            let _ = writeln!(generated, "    {},", language.variant);
        }
        generated.push_str("}\n\nimpl Language {\n");

        // The first language is both index zero and the fallback, so it is
        // written once: an arm for it as well would be a duplicate body.
        generated.push_str(
            "    pub(super) const fn from_index(index: isize) -> Self {\n        match index {\n",
        );
        for (index, language) in languages.iter().enumerate().skip(1) {
            let _ = writeln!(
                generated,
                "            {index} => Self::{},",
                language.variant
            );
        }
        let _ = writeln!(
            generated,
            "            _ => Self::{},\n        }}\n    }}\n",
            languages[0].variant
        );

        generated
            .push_str("    pub(super) const fn index(self) -> usize {\n        match self {\n");
        for (index, language) in languages.iter().enumerate() {
            let _ = writeln!(
                generated,
                "            Self::{} => {index},",
                language.variant
            );
        }
        generated.push_str("        }\n    }\n}\n\n");

        generated.push_str(
            "/// What the language chooser shows, in the order of the variants.\n\
             pub(super) const LANGUAGE_NAMES: [&str; ",
        );
        let _ = write!(generated, "{}] = [", languages.len());
        for language in languages {
            let _ = write!(generated, "{:?}, ", language.name);
        }
        generated.push_str("];\n\n");

        // Credits are generated from the language files themselves, so a
        // language and the people who provided it cannot drift apart.
        generated.push_str(
            "/// Who translated each language, in the order of the variants.\n\
             pub(super) const TRANSLATION_CREDITS: [(&str, &str); ",
        );
        let _ = write!(generated, "{}] = [", languages.len());
        for language in languages {
            let _ = write!(
                generated,
                "({:?}, {:?}), ",
                language.name,
                language.authors.join(", ")
            );
        }
        generated.push_str("];\n\n");
    }

    fn write_tables(generated: &mut String, languages: &[LanguageFile]) {
        generated.push_str(
            "impl Language {\n    \
             /// One table per language; its length follows the string count.\n    \
             #[allow(clippy::too_many_lines)]\n    \
             pub(super) const fn strings(self) -> Strings {\n        match self {\n",
        );
        for language in languages {
            let _ = writeln!(
                generated,
                "            Self::{} => Strings {{",
                language.variant
            );
            for (key, value) in &language.strings {
                match value {
                    Value::Text(text) => {
                        let _ = writeln!(generated, "                {key}: {text:?},");
                    }
                    Value::List(items) => {
                        let _ = write!(generated, "                {key}: [");
                        for item in items {
                            let _ = write!(generated, "{item:?}, ");
                        }
                        generated.push_str("],\n");
                    }
                }
            }
            generated.push_str("            },\n");
        }
        generated.push_str("        }\n    }\n}\n");
    }
}
