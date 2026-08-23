# Contributing to WinStoreRegion

Thank you for considering a contribution. This document is short on ceremony
and specific about the few rules that actually matter here.

WinStoreRegion is a Windows-only Rust utility that installs a Microsoft Store
product while the current user's Windows Home Location is temporarily switched,
then restores it safely. Almost every rule below exists because that sentence
involves changing a real setting on someone else's machine.

## Before you write code

**Read `AGENTS.md` first.** It is the map of which document owns which rules;
every rule has exactly one home, and changing a rule means changing it where it
lives rather than restating it somewhere new.

**Specification before code.** Implementation proceeds only for work that has
been explicitly requested, after the relevant part of `TASKS_ru.md` has been
worked through. `TASKS_ru.md` is the product source of truth and is written in
Russian; where any other document contradicts it, it wins. Diagnostics, probes,
and experiments are exempt from this rule — you are always free to go and find
out what is true. The full statement lives in `AI_RULES.md`, "Specification
Before Code".

If you want to change behaviour, open an issue describing the behaviour first.
A pull request that arrives before the discussion is likely to be asking for a
decision that has already been made and written down somewhere.

## The recovery invariant

This is the one rule that is not open to convenience.

The application must never leave a user's Windows Home Location changed without
a durable record that says how to put it back. Concretely:

- a complete recovery record is published **before** Windows is touched;
- every region write is confirmed by reading the region back;
- the record is removed only after a read-back confirms the original region;
- a failure, a crash, or a killed process leaves the record in place, and the
  next start offers the region back.

The full statement is in `AI_RULES.md`, "Region and Installation Safety".
Nothing in this project may weaken it. A change that makes a happy path shorter
by skipping a read-back will not be merged.

## Saying only what is true

The application does not claim results it has not established. This shows up
everywhere in the code and it is deliberate:

- opening a Store page is not an installation;
- a backend accepting a request is not a completed install;
- an installer process exiting proves nothing at all;
- an unfinished check is never treated as permission.

If you add a state, an outcome, or a sentence in the interface, it has to mean
exactly what happened. "Probably installed" is not an outcome this product has.

## Development

Requirements: a stable Rust toolchain of at least the version in
`Cargo.toml` (`rust-version`) with the MSVC target, and Windows to run the tests
on. Build output is redirected by `.cargo/config.toml`, so the repository stays
clean, and the CRT is linked statically on every target because the product
ships as one portable executable with no DLLs beside it.

```
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
```

All three must be clean. Clippy runs with `pedantic` enabled at the workspace
level; do not silence a lint without a comment saying why the code is right.

`unsafe_code` is denied for the whole workspace. The core crate forbids it
outright. In the executable crate, `unsafe` is allowed only where a Win32 or COM
call needs it, one `#[allow(unsafe_code)]` at a time, kept as small as the call
and never spread across a whole module for convenience.

Tests that change the Windows region or install software are marked
`#[ignore]` and are expected to be run by hand on a disposable machine — never
on a machine you care about. Run them with:

```
cargo test --workspace -- --ignored
```

### Where new code goes

`AI_RULES.md`, "Module Placement", owns the rule. In short: a rule about one
subject belongs in that subject's module; a rule joining two subjects belongs in
the module that owns the decision. The core crate `winstoreregion-core` is
GUI-independent and must never depend on the executable crate; COM types,
`HRESULT` values, and localized text do not cross the platform boundary.

**Adding a file means adding it to `STRUCTURE.md`.** Every module states its
subject in its own `//!` header.

### Interface text

User-visible text lives in the language files under `lang/`, never in domain
code and — increasingly — not in the presentation code either. Adding a sentence
means adding a key to every language file; the build refuses a key that only one
language has, so a half-added string cannot compile.

Text must not carry internal type names, `HRESULT` values, or `Debug` output:
technical evidence belongs in the separate details block that the user can copy
for a report.

Every interface sentence lives in `lang/`. Nothing is written inline at its use
site, and nothing should be added there. The single exception is the command
line: `--help` and the two argument errors are English in every language,
because the arguments are read before a language has been chosen.

Text must also stay in words the reader can check. A status that names a writer,
a backend, a resolver or a GeoId is naming something the window never shows, so
it says nothing the user can act on. Name what it is to them — putting the region
back, the Store catalogue, a region code.

## Translations

A language is a file. Nothing else.

```
lang/ru.toml
lang/en.toml
lang/<your language>.toml
```

Copy an existing file, translate the values, set the header fields, and open a
pull request. There is no Rust to write: the build reads this directory and
generates the language list, the chooser entries and the tables. Add
`lang/zh.toml` and the application offers Chinese.

The header of each file:

```toml
variant = "Chinese"        # upper-camel-case identifier, unique per language
code = "zh-CN"             # BCP 47 tag
name = "中文 (zh-CN)"       # exactly what the language chooser shows
order = 2                  # position in the chooser
authors = ["your name"]    # how you want to be credited; see below
```

### How a translation is reviewed

**It is not.** A new language is approved without a linguistic review, because
nobody here can read it, and pretending otherwise would only add delay. Once it
is approved, the maintainer aims to publish a release that includes the
language.

What is checked is structure, and the build does it rather than a person:

- a key present in one language and missing from another fails the build;
- a key no other language defines fails the build;
- a list of a different length fails the build;
- a string whose `{placeholders}` differ from the original fails the build.

That last one is the only property of a translation that can be verified without
knowing the language, and it catches the failure that would otherwise be
invisible: a sentence that has quietly lost the region, file name or version it
was about.

Because there is no review, the responsibility moves to you, and one rule
carries most of it.

### The one rule

Many strings here are deliberately careful. They say that a completion was *not
proven*, that an answer came from a single market, that a set is incomplete,
that a file's identity is the user's claim rather than a checked fact.

Those hedges are the product. This application changes a Windows setting and
puts it back, and everything it says about what it did — and did not — establish
is what makes that safe to trust. A translation that turns "completion is not
proven" into "installed" is wrong, even though it reads better and no test will
catch it.

If a hedge is hard to render in your language, say so in the pull request. It
will be discussed. Dropping it silently is the one thing that is not acceptable.

### Remarks about a language that already ships

Because no translation is read by a reviewer, the ones already in the
application are the ones most likely to be wrong, and nobody here will notice.
If you read one of them and something is off — a mistranslation, a caption that
does not fit, a hedge that got lost, wording that is simply unnatural — **please
open an issue**. Name the language and the key; a suggested wording is welcome
but not required, and "this reads badly, I am not sure how to fix it" is a
useful report.

A pull request is welcome for the same thing. An issue is the lower bar, and the
lower bar is the point: a remark nobody sends is a defect that ships.

### Being credited

The `authors` field is how you are named. Put whatever you want shown: a name, a
handle, several people separated in the list. It appears in **Help → About**
inside the application, beside the language, and in the README.

Credits are generated from the language files themselves, so the credit and the
translation can never drift apart: the name travels with the file that earned
it. Removing your name is a pull request like any other.

If you would rather not be credited, leave `authors` empty. The About dialog
then says nothing about that language rather than showing an empty heading.

### Licence

A translation is a contribution like any other. It is accepted under
**GPL-3.0-or-later** and under no other licence, and the agreement in `CLA.md`
applies to it exactly as it does to code.

You keep the copyright in the text you wrote. The agreement grants no right to
relicense your work: if the project ever wanted to ship under different terms,
it would have to ask you, and every other contributor, individually.

The practical consequence is worth stating plainly, because a translation feels
lighter than code and is not: sending a language file is publishing your text
under a copyleft licence. Anyone may redistribute it, including inside modified
copies of this application, provided they keep it under the same terms.

### Layout

A language changes how wide things need to be. The captions were sized for
Russian and English, and one of them has already been found clipped at 125%
scaling. If you can run the application, check your language at 100% and 150%
and say in the pull request what you saw. If you cannot, say that instead — it
is useful to know that nobody has looked.

## Pull requests

- One subject per pull request. A refactor and a behaviour change in the same
  branch are two pull requests.
- Explain what you observed, not only what you changed. If a bug was involved,
  say how it reproduces.
- Comments should explain why the code is the way it is. The code already says
  what it does.
- Match the surrounding style rather than introducing your own.

## Licensing of contributions

The project is licensed under **GPL-3.0-or-later**, and contributions are
accepted under that licence and no other. Contributors are asked to agree to the
Contributor Licence Agreement in `CLA.md` before their first contribution is
merged; it is short, it grants no right to relicense your work, and you keep your
copyright.

## Security

Ordinary bugs, including ones with security-sounding names, belong in a public
issue like anything else. If you believe a finding is genuinely sensitive,
`SECURITY.md` explains how to report it privately instead.
