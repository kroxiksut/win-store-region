# Repository Structure

This document lists the versioned project structure. Build artifacts, temporary
captures, and runtime data are intentionally excluded; Cargo writes build output
to `C:\temp\WinStoreRegion`.

```text
.
├── .cargo/
│   └── config.toml                 Shared Cargo output and MSVC CRT settings
├── apps/
│   └── winstoreregion/             Single native Win32 executable crate
│       ├── app.manifest            asInvoker and PerMonitorV2 declarations
│       ├── build.rs                Embeds icon, manifest, and version metadata
│       └── src/
│           ├── main.rs             Entrypoint: argument classification only
│           ├── platform/           Narrow Win32 and WinRT adapters
│           │   ├── mod.rs          Submodule declarations and shared adapters
│           │   ├── diagnostic_log.rs  Size-rotated plain-text diagnostic log
│           │   ├── handoff.rs      One verified Store installer started, never observed
│           │   ├── installer_download.rs  Microsoft's own Store installer fetched by Product ID
│           │   ├── market_probe.rs  One market asked over WinHTTP where a product is offered
│           │   ├── packaged.rs     Packaged-app and deployment-event evidence
│           │   ├── prerequisites.rs  Read-only check of the packages an install needs
│           │   ├── region.rs       Windows Home Location reads and the single writer
│           │   ├── storage.rs      Atomic recovery-record and journal files
│           │   ├── store.rs        Store page opener
│           │   ├── stub.rs         Read-only installer-stub inspection
│           │   ├── uninstall.rs    Uninstall-registry evidence
│           │   └── winget/         WinGet COM API adapter
│           │       ├── mod.rs      Activation and the connected msstore catalogue
│           │       ├── backend.rs  Install, observe, cancel, and re-attach
│           │       ├── bindings.rs Generated projection of the WinGet interfaces
│           │       ├── metadata.rs Interface metadata placed beside the executable
│           │       └── resolver.rs Exact product resolution from the catalogue
│           └── gui/                Native Win32 presentation layer
│               ├── mod.rs          Submodule declarations and the entry point
│               ├── command.rs      Windows notifications into presentation state
│               ├── controls.rs     Child-control creation and static content
│               ├── diagnostic.rs   Failure text and its separate details block
│               ├── dialogs.rs      Modal dialogs, file picker, clipboard
│               ├── direction.rs    Which way the interface reads, and the mirrored layout
│               ├── dragdrop.rs     OLE drop target for an installer file
│               ├── handoff.rs      One Store-installer handoff off the UI thread
│               ├── ids.rs          Window class, control ids, private messages
│               ├── install.rs      One guarded installation run off the UI thread
│               ├── install_trace.rs  Diagnostic trace of one guarded installation
│               ├── journal.rs      Journal presentation
│               ├── layout.rs       DPI-aware control placement
│               ├── menu.rs         Application menu and its actions
│               ├── recovery.rs     Startup recovery inspection and notice
│               ├── render.rs       Presentation state into existing controls
│               ├── state.rs        Presentation state, context, lifetime guards
│               ├── strings.rs      Language selection and user-visible text
│               ├── window.rs       Window creation, message loop, close policy
│               └── work.rs         Off-thread work posting results to the window
├── assets/
│   ├── brand/                      Source branding artwork
│   └── icons/app/                  Win32 application icon source
├── crates/
│   └── winstoreregion-core/        GUI-independent domain crate
│       └── src/
│           ├── lib.rs              Crate facade: identity, modules, re-exports
│           ├── availability.rs     Where a product is offered, ahead of a region change
│           ├── diagnostic.rs       Closed set of user-facing failure causes
│           ├── input.rs            Store text input into a product identifier
│           ├── install.rs          Install model, backend contract, selection
│           ├── journal.rs          Operation-history records and their schema
│           ├── launch.rs           Argument classification into a launch surface
│           ├── log.rs              Diagnostic records and their plain-text form
│           ├── machine.rs          Guarded-operation state machine
│           ├── operation.rs        The next step a guarded operation should take
│           ├── prerequisite.rs     What must exist before an install is possible
│           ├── region.rs           GeoId, market, and the guarded region switch
│           ├── resolve.rs          Product resolution and the application card
│           ├── source.rs           Application source and Store stub inspection
│           ├── store_page.rs       Store page opening as a non-install action
│           ├── time.rs             Validated UTC timestamps
│           ├── test_support.rs     Deterministic fakes shared by module tests
│           ├── observe/            Evidence-based completion observation
│           │   ├── mod.rs          Submodule declarations and shared timestamps
│           │   ├── packaged.rs     Packaged-application completion evidence
│           │   ├── timeout.rs      Deadlines and user-directed resolution
│           │   └── win32.rs        Win32 completion evidence
│           └── recovery/           The recovery record and its decisions
│               ├── mod.rs          Submodule declarations
│               ├── record.rs       pending-restore.json and its wire schema
│               ├── startup.rs      Conflict-safe classification and execution
│               └── store.rs        Durable storage boundary for the record
├── .github/
│   ├── release-notes.md            Release text under the changelog section, {version} filled in
│   └── workflows/
│       ├── ci.yml                  Format, lints, tests, and a release build per architecture
│       └── release.yml             Tag-triggered build, changelog section, and GitHub release
├── Cargo.lock                      Locked Rust dependency graph
├── Cargo.toml                      Workspace manifest and shared lint policy
├── CHANGELOG.md                    What changed in each version, in English only
├── CLA.md                          Contributor licence agreement, in English
├── CONTRIBUTING.md                 How to contribute and the rules that bind, English
├── assets/screenshots/             One interface screenshot per language, `installation-<tag>.png`
├── lang/                           One TOML file per interface language
│   ├── ar.toml                     Arabic, read right to left, machine-drafted and unreviewed
│   ├── en.toml                     English interface text
│   ├── es-ES.toml                  European Spanish, machine-drafted and unreviewed
│   ├── fa.toml                     Persian, read right to left, machine-drafted and unreviewed
│   ├── ja.toml                     Japanese, machine-drafted and unreviewed
│   ├── ko.toml                     Korean, machine-drafted and unreviewed
│   ├── pt-BR.toml                  Brazilian Portuguese, machine-drafted and unreviewed
│   ├── ru.toml                     Russian interface text
│   ├── tr.toml                     Turkish, machine-drafted and unreviewed
│   ├── zh-CN.toml                  Simplified Chinese, machine-drafted and unreviewed
│   └── zh-TW.toml                  Traditional Chinese, machine-drafted and unreviewed
├── README.md                       Product documentation for users, in English
├── README.ar.md                    The same document in Arabic, machine-translated
├── README.es-ES.md                 The same document in European Spanish, machine-translated
├── README.fa.md                    The same document in Persian, machine-translated
├── README.ja.md                    The same document in Japanese, machine-translated
├── README.ko.md                    The same document in Korean, machine-translated
├── README.pt-BR.md                 The same document in Brazilian Portuguese, machine-translated
├── README.ru.md                    The same document in Russian
├── README.tr.md                    The same document in Turkish, machine-translated
├── README.zh-CN.md                 The same document in Simplified Chinese, machine-translated
├── README.zh-TW.md                 The same document in Traditional Chinese, machine-translated
├── SECURITY.md                     Threat model and private reporting, in English
├── rustfmt.toml                    Formatting policy
├── STRUCTURE.md                    This document
└── .gitignore                      Version-control exclusion rules
```

`winstoreregion-core` must never depend on the Win32 executable crate. The
application crate may depend on core and owns only native presentation and
platform-integration code.

Every module file states its subject in its own `//!` header. `AI_RULES.md`,
"Module Placement", owns the rule for deciding which file new code belongs in.

Files excluded by `.gitignore` are intentionally omitted from this document.
