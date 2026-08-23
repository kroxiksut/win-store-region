//! Completion evidence for Win32 Store applications.

use crate::input::StoreProductId;
use crate::install::{InstallClassification, InstallKind, InstallObservation, InstallPhase};
use crate::observe::ObservationTimestamp;

/// Windows uninstall-registry hive/view from which a Win32 entry was read.
///
/// The scope is part of an entry's identity: the same subkey text in two
/// registry views does not identify the same installation record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UninstallRegistryScope {
    /// `HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall`.
    CurrentUser,
    /// The 64-bit view below `HKLM`.
    LocalMachine64,
    /// The 32-bit view below `HKLM`.
    LocalMachine32,
}

/// One read-only uninstall-registry record.
///
/// `key_name`, display data, and version are diagnostics only. They must never
/// be used to attribute a Store installation to a product: only an explicit
/// Store Product ID can establish that link.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UninstallRegistryEntry {
    pub scope: UninstallRegistryScope,
    pub key_name: String,
    pub display_name: Option<String>,
    pub publisher: Option<String>,
    pub display_version: Option<String>,
    pub store_product_id: Option<StoreProductId>,
}

impl UninstallRegistryEntry {
    /// Whether this record has the same registry identity as another snapshot
    /// entry. Value changes do not turn an existing uninstall record into a
    /// fresh installation.
    #[must_use]
    pub fn has_same_registry_identity(&self, other: &Self) -> bool {
        self.scope == other.scope && self.key_name == other.key_name
    }

    /// Whether this explicit Product ID and optional publisher attribute the
    /// record to the expected Win32 Store product.
    #[must_use]
    pub fn matches(&self, expected: &ExpectedWin32UninstallEvidence) -> bool {
        self.store_product_id.as_ref() == Some(expected.product_id())
            && expected
                .publisher()
                .is_none_or(|publisher| self.publisher.as_deref() == Some(publisher))
    }
}

/// Read-only baseline or follow-up snapshot of the Win32 uninstall registry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UninstallRegistrySnapshot {
    pub observed_at: ObservationTimestamp,
    pub entries: Vec<UninstallRegistryEntry>,
}

impl UninstallRegistrySnapshot {
    /// Whether the matching record was present before this install attempt.
    #[must_use]
    pub fn contains(&self, expected: &ExpectedWin32UninstallEvidence) -> bool {
        self.entries.iter().any(|entry| entry.matches(expected))
    }

    /// Return a product-bound record that did not exist in the baseline.
    #[must_use]
    pub fn new_matching_entry(
        &self,
        expected: &ExpectedWin32UninstallEvidence,
        baseline: &Self,
    ) -> Option<&UninstallRegistryEntry> {
        self.entries.iter().find(|entry| {
            entry.matches(expected)
                && !baseline
                    .entries
                    .iter()
                    .any(|before| entry.has_same_registry_identity(before))
        })
    }
}

/// Product-bound criteria for a Win32 uninstall-registry observation.
///
/// The Product ID is mandatory. Publisher is an additional exact constraint
/// only when the resolver supplied it; display strings are deliberately absent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpectedWin32UninstallEvidence {
    product_id: StoreProductId,
    publisher: Option<String>,
}

impl ExpectedWin32UninstallEvidence {
    /// Construct criteria only for a product explicitly classified as Win32.
    ///
    /// # Errors
    ///
    /// Returns [`ExpectedWin32UninstallEvidenceError::WrongInstallKind`] for
    /// unknown or packaged products, and rejects a whitespace-only publisher.
    pub fn new(
        classification: InstallClassification,
        product_id: StoreProductId,
        publisher: Option<String>,
    ) -> Result<Self, ExpectedWin32UninstallEvidenceError> {
        if classification.kind() != InstallKind::Win32 {
            return Err(ExpectedWin32UninstallEvidenceError::WrongInstallKind);
        }
        let publisher = match publisher {
            Some(value) if value.trim().is_empty() => {
                return Err(ExpectedWin32UninstallEvidenceError::BlankPublisher);
            }
            Some(value) => Some(value),
            None => None,
        };
        Ok(Self {
            product_id,
            publisher,
        })
    }

    /// Return the exact Store Product ID required in the uninstall record.
    #[must_use]
    pub fn product_id(&self) -> &StoreProductId {
        &self.product_id
    }

    /// Return the optional publisher constraint from the resolver.
    #[must_use]
    pub fn publisher(&self) -> Option<&str> {
        self.publisher.as_deref()
    }
}

/// Why Win32 uninstall-registry criteria could not be constructed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExpectedWin32UninstallEvidenceError {
    /// The uninstall observer applies only to an evidence-backed Win32 product.
    WrongInstallKind,
    /// A supplied publisher cannot be blank because that would weaken matching.
    BlankPublisher,
}

/// Read-only backend lifecycle fact supplied with a registry follow-up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Win32BackendLifecycle {
    /// The backend has not reported its terminal state yet.
    Running,
    /// The backend ended; its exit code is retained only as supplementary fact.
    Finished { exit_code: Option<i32> },
}

/// Result of evaluating Win32 uninstall-registry evidence.
///
/// `ProductBoundRegistryEvidence` is intentionally not a generic successful
/// installation state until the VM experiments define a sufficient criterion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Win32RegistryObservation {
    /// No completion evidence yet while the backend is still active.
    Installing,
    /// The expected product was already registered before this attempt.
    AlreadyPresent,
    /// A new uninstall record explicitly naming the expected Store Product ID
    /// (and publisher, when known) appeared after the baseline.
    ProductBoundRegistryEvidence { backend_exit_code: Option<i32> },
    /// The backend ended without a new, product-bound registry record.
    CompletionUncertain { backend_exit_code: Option<i32> },
}

impl Win32RegistryObservation {
    /// Convert the current Win32 evidence into the generic install state.
    ///
    /// Until the required VM observations establish a sufficient Win32
    /// criterion, even a product-bound uninstall record remains conservative
    /// `CompletionUncertain`; this prevents the UI from claiming success early.
    #[must_use]
    pub const fn as_install_observation(self) -> InstallObservation {
        match self {
            Self::Installing => InstallObservation::Installing {
                phase: InstallPhase::Installing,
                progress: None,
            },
            Self::AlreadyPresent
            | Self::ProductBoundRegistryEvidence { .. }
            | Self::CompletionUncertain { .. } => InstallObservation::CompletionUncertain,
        }
    }
}

/// Evaluate before/after uninstall-registry snapshots for one Win32 product.
///
/// A successful backend exit code cannot compensate for absent product-bound
/// registry evidence. Equally, an existing matching record is never passed off
/// as a fresh completion.
#[must_use]
pub fn evaluate_win32_registry_observation(
    expected: &ExpectedWin32UninstallEvidence,
    baseline: &UninstallRegistrySnapshot,
    follow_up: &UninstallRegistrySnapshot,
    backend: Win32BackendLifecycle,
) -> Win32RegistryObservation {
    if baseline.contains(expected) {
        return Win32RegistryObservation::AlreadyPresent;
    }
    if follow_up.new_matching_entry(expected, baseline).is_some() {
        return Win32RegistryObservation::ProductBoundRegistryEvidence {
            backend_exit_code: match backend {
                Win32BackendLifecycle::Running => None,
                Win32BackendLifecycle::Finished { exit_code } => exit_code,
            },
        };
    }
    match backend {
        Win32BackendLifecycle::Running => Win32RegistryObservation::Installing,
        Win32BackendLifecycle::Finished { exit_code } => {
            Win32RegistryObservation::CompletionUncertain {
                backend_exit_code: exit_code,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::StoreProductId;
    use crate::install::{InstallClassification, InstallObservation};
    use crate::observe::ObservationTimestamp;

    #[test]
    fn win32_registry_observer_requires_new_exact_product_bound_evidence() {
        let product_id = StoreProductId::parse("9WZDNCRFJ3PZ").expect("valid product ID");
        let expected = ExpectedWin32UninstallEvidence::new(
            InstallClassification::win32_from_backend_report(),
            product_id,
            Some("Contoso Ltd.".to_owned()),
        )
        .expect("Win32 classification");
        let baseline = uninstall_snapshot(Vec::new());
        let follow_up = uninstall_snapshot(vec![uninstall_entry(
            UninstallRegistryScope::LocalMachine64,
            "Contoso.StoreApp",
            Some("9WZDNCRFJ3PZ"),
            Some("Contoso Ltd."),
        )]);

        let observation = evaluate_win32_registry_observation(
            &expected,
            &baseline,
            &follow_up,
            Win32BackendLifecycle::Finished { exit_code: Some(0) },
        );
        assert_eq!(
            observation,
            Win32RegistryObservation::ProductBoundRegistryEvidence {
                backend_exit_code: Some(0)
            }
        );
        assert_eq!(
            observation.as_install_observation(),
            InstallObservation::CompletionUncertain,
            "VM work must establish whether this candidate is sufficient"
        );
    }

    #[test]
    fn win32_registry_observer_rejects_foreign_existing_and_exit_code_only_evidence() {
        let product_id = StoreProductId::parse("9WZDNCRFJ3PZ").expect("valid product ID");
        let expected = ExpectedWin32UninstallEvidence::new(
            InstallClassification::win32_from_backend_report(),
            product_id,
            Some("Contoso Ltd.".to_owned()),
        )
        .expect("Win32 classification");
        let baseline = uninstall_snapshot(Vec::new());
        let foreign_follow_up = uninstall_snapshot(vec![
            uninstall_entry(
                UninstallRegistryScope::LocalMachine64,
                "Foreign.WithSamePublisher",
                Some("9NBLGGH4R315"),
                Some("Contoso Ltd."),
            ),
            uninstall_entry(
                UninstallRegistryScope::LocalMachine32,
                "NoProductId",
                None,
                Some("Contoso Ltd."),
            ),
        ]);
        assert_eq!(
            evaluate_win32_registry_observation(
                &expected,
                &baseline,
                &foreign_follow_up,
                Win32BackendLifecycle::Finished { exit_code: Some(0) },
            ),
            Win32RegistryObservation::CompletionUncertain {
                backend_exit_code: Some(0)
            }
        );
        assert_eq!(
            evaluate_win32_registry_observation(
                &expected,
                &baseline,
                &foreign_follow_up,
                Win32BackendLifecycle::Running,
            ),
            Win32RegistryObservation::Installing
        );

        let already_present = uninstall_snapshot(vec![uninstall_entry(
            UninstallRegistryScope::CurrentUser,
            "Contoso.StoreApp",
            Some("9WZDNCRFJ3PZ"),
            Some("Contoso Ltd."),
        )]);
        assert_eq!(
            evaluate_win32_registry_observation(
                &expected,
                &already_present,
                &already_present,
                Win32BackendLifecycle::Finished { exit_code: Some(0) },
            ),
            Win32RegistryObservation::AlreadyPresent
        );
    }

    #[test]
    fn win32_registry_observer_is_limited_to_classified_win32_products() {
        let product_id = StoreProductId::parse("9WZDNCRFJ3PZ").expect("valid product ID");
        assert_eq!(
            ExpectedWin32UninstallEvidence::new(
                InstallClassification::unknown(),
                product_id.clone(),
                None,
            ),
            Err(ExpectedWin32UninstallEvidenceError::WrongInstallKind)
        );
        assert_eq!(
            ExpectedWin32UninstallEvidence::new(
                InstallClassification::packaged_from_observed_identity(),
                product_id,
                None,
            ),
            Err(ExpectedWin32UninstallEvidenceError::WrongInstallKind)
        );
        assert_eq!(
            ExpectedWin32UninstallEvidence::new(
                InstallClassification::win32_from_backend_report(),
                StoreProductId::parse("9WZDNCRFJ3PZ").expect("valid product ID"),
                Some("   ".to_owned()),
            ),
            Err(ExpectedWin32UninstallEvidenceError::BlankPublisher)
        );
    }

    fn uninstall_snapshot(entries: Vec<UninstallRegistryEntry>) -> UninstallRegistrySnapshot {
        UninstallRegistrySnapshot {
            observed_at: ObservationTimestamp::from_unix_millis(2_000),
            entries,
        }
    }

    fn uninstall_entry(
        scope: UninstallRegistryScope,
        key_name: &str,
        product_id: Option<&str>,
        publisher: Option<&str>,
    ) -> UninstallRegistryEntry {
        UninstallRegistryEntry {
            scope,
            key_name: key_name.to_owned(),
            display_name: Some("Unrelated display text".to_owned()),
            publisher: publisher.map(str::to_owned),
            display_version: Some("1.0".to_owned()),
            store_product_id: product_id.map(|value| {
                StoreProductId::parse(value).expect("test product ID is syntactically valid")
            }),
        }
    }
}
