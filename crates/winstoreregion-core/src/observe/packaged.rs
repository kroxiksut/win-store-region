//! Completion evidence for packaged Store applications.

use crate::input::StoreProductId;
use crate::install::{InstallClassification, InstallKind};
use crate::observe::ObservationTimestamp;

/// Stable Windows package family identity used to correlate a Store product.
///
/// A package full name changes with version and architecture, so it is kept as
/// diagnostic snapshot data rather than the identity used for completion.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PackageFamilyName(String);

impl PackageFamilyName {
    /// Parse a non-empty package family name returned by Windows.
    #[must_use]
    pub fn parse(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.trim().is_empty()).then_some(Self(value))
    }

    /// Return the platform-provided package family name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Product-specific package identity expected from a packaged Store install.
///
/// The pair is deliberately stronger than a display name, a package family, or
/// a Store event in isolation. It may only be used with a packaged
/// classification established by a structured platform source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpectedPackagedIdentity {
    product_id: StoreProductId,
    family_name: PackageFamilyName,
}

impl ExpectedPackagedIdentity {
    /// Build an expected identity only for an evidence-backed packaged product.
    ///
    /// # Errors
    ///
    /// Returns [`ExpectedPackagedIdentityError::WrongInstallKind`] unless the
    /// resolver supplied a packaged classification.
    pub fn new(
        classification: InstallClassification,
        product_id: StoreProductId,
        family_name: PackageFamilyName,
    ) -> Result<Self, ExpectedPackagedIdentityError> {
        if classification.kind() != InstallKind::Packaged {
            return Err(ExpectedPackagedIdentityError::WrongInstallKind);
        }

        Ok(Self {
            product_id,
            family_name,
        })
    }

    /// Return the exact Store product that supplied this identity.
    #[must_use]
    pub fn product_id(&self) -> &StoreProductId {
        &self.product_id
    }

    /// Return the stable package family expected after installation.
    #[must_use]
    pub fn family_name(&self) -> &PackageFamilyName {
        &self.family_name
    }
}

/// Why a packaged observation plan cannot be constructed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExpectedPackagedIdentityError {
    /// A packaged observer must not be applied to unknown or Win32 products.
    WrongInstallKind,
}

/// Status fields needed to decide whether a Windows package is healthy enough
/// to corroborate completion.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PackageStatusSnapshot {
    pub verify_is_ok: bool,
    pub not_available: bool,
    pub package_offline: bool,
    pub data_offline: bool,
    pub disabled: bool,
    pub needs_remediation: bool,
    pub license_issue: bool,
    pub modified: bool,
    pub tampered: bool,
    pub dependency_issue: bool,
    pub servicing: bool,
    pub deployment_in_progress: bool,
    pub is_partially_staged: bool,
}

impl PackageStatusSnapshot {
    /// Whether Windows reported a complete, usable package state.
    #[must_use]
    pub const fn is_healthy(self) -> bool {
        self.verify_is_ok
            && !self.not_available
            && !self.package_offline
            && !self.data_offline
            && !self.disabled
            && !self.needs_remediation
            && !self.license_issue
            && !self.modified
            && !self.tampered
            && !self.dependency_issue
            && !self.servicing
            && !self.deployment_in_progress
            && !self.is_partially_staged
    }
}

/// One packaged application observed for the current Windows user.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageSnapshotEntry {
    pub product_id: Option<StoreProductId>,
    pub family_name: PackageFamilyName,
    pub full_name: String,
    /// `None` means Windows did not expose a complete status for this package;
    /// it can never corroborate completion.
    pub status: Option<PackageStatusSnapshot>,
}

impl PackageSnapshotEntry {
    /// Whether this entry can be attributed to the expected Store product.
    ///
    /// The package family is the evidence that matters: the resolver supplies
    /// it for the exact product before installation. Windows writes it with its
    /// own capitalisation, so the comparison ignores case, as Windows does.
    ///
    /// A Store product identifier on the package is rare but decisive when it
    /// exists: a disagreeing one means this is a different product, while an
    /// absent one leaves the family name to speak alone.
    #[must_use]
    pub fn matches(&self, expected: &ExpectedPackagedIdentity) -> bool {
        let family_matches = self
            .family_name
            .as_str()
            .eq_ignore_ascii_case(expected.family_name().as_str());
        let product_agrees = self
            .product_id
            .as_ref()
            .is_none_or(|product_id| product_id == expected.product_id());
        family_matches && product_agrees
    }
}

/// Read-only baseline or follow-up snapshot of packaged applications.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageSnapshot {
    pub observed_at: ObservationTimestamp,
    pub entries: Vec<PackageSnapshotEntry>,
}

impl PackageSnapshot {
    /// Return whether the snapshot already contained the expected package.
    #[must_use]
    pub fn contains(&self, expected: &ExpectedPackagedIdentity) -> bool {
        self.entries.iter().any(|entry| entry.matches(expected))
    }

    /// Return the matching package entry, if any.
    #[must_use]
    pub fn matching_entry(
        &self,
        expected: &ExpectedPackagedIdentity,
    ) -> Option<&PackageSnapshotEntry> {
        self.entries.iter().find(|entry| entry.matches(expected))
    }
}

/// Corroborating `AppX` deployment event parsed by the platform adapter.
///
/// An event without the expected identity is intentionally unusable for
/// completion, even if it arrived during this operation's time window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppxDeploymentEvent {
    pub observed_at: ObservationTimestamp,
    pub product_id: Option<StoreProductId>,
    pub family_name: Option<PackageFamilyName>,
}

impl AppxDeploymentEvent {
    /// Whether this event belongs to the expected product and started no
    /// earlier than the package baseline.
    #[must_use]
    pub fn corroborates(
        &self,
        expected: &ExpectedPackagedIdentity,
        baseline: ObservationTimestamp,
    ) -> bool {
        self.observed_at >= baseline
            && self.product_id.as_ref() == Some(expected.product_id())
            && self.family_name.as_ref() == Some(expected.family_name())
    }
}

/// Result of evaluating one packaged observation without inventing success.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackagedObservation {
    /// No matching healthy package has appeared after the baseline yet.
    Installing,
    /// The expected package was already present before the request.
    AlreadyPresent,
    /// A new healthy expected package appeared; `AppX` evidence is retained as a
    /// corroborating fact but is not required for this result.
    Completed { appx_event_observed: bool },
}

/// Evaluate packaged-install evidence for exactly one expected Store product.
///
/// The expected package must be absent in the baseline and appear healthy in a
/// follow-up snapshot. Foreign packages and unrelated `AppX` events cannot affect
/// the outcome. A prior presence is explicit rather than being reported as a
/// fresh installation completion.
#[must_use]
pub fn evaluate_packaged_observation(
    expected: &ExpectedPackagedIdentity,
    baseline: &PackageSnapshot,
    follow_up: &PackageSnapshot,
    appx_events: &[AppxDeploymentEvent],
) -> PackagedObservation {
    if baseline.contains(expected) {
        return PackagedObservation::AlreadyPresent;
    }

    let Some(entry) = follow_up.matching_entry(expected) else {
        return PackagedObservation::Installing;
    };
    if !entry.status.is_some_and(PackageStatusSnapshot::is_healthy) {
        return PackagedObservation::Installing;
    }

    PackagedObservation::Completed {
        appx_event_observed: appx_events
            .iter()
            .any(|event| event.corroborates(expected, baseline.observed_at)),
    }
}

#[cfg(test)]
mod matching_tests {
    use super::*;
    use crate::install::InstallClassification;

    fn expected() -> ExpectedPackagedIdentity {
        ExpectedPackagedIdentity::new(
            InstallClassification::packaged_from_msstore_installer_type(),
            StoreProductId::parse("9NT1R1C2HH7J").expect("valid identifier"),
            PackageFamilyName::parse("openai.chatgpt-desktop_2p2nqsd0c76g0")
                .expect("valid family name"),
        )
        .expect("a packaged product has an expected identity")
    }

    fn entry(family: &str, product_id: Option<&str>) -> PackageSnapshotEntry {
        PackageSnapshotEntry {
            product_id: product_id.map(|id| StoreProductId::parse(id).expect("valid identifier")),
            family_name: PackageFamilyName::parse(family).expect("valid family name"),
            full_name: format!("{family}_full"),
            status: None,
        }
    }

    #[test]
    fn windows_capitalisation_of_a_family_name_still_matches() {
        // The catalogue reports the family in lower case; Windows reports the
        // publisher's own capitalisation for the same package.
        assert!(entry("OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0", None).matches(&expected()));
    }

    #[test]
    fn a_different_family_never_matches() {
        assert!(!entry("mozilla.firefox_n80bbvh6b1yt2", None).matches(&expected()));
    }

    #[test]
    fn a_disagreeing_product_identifier_wins_over_a_matching_family() {
        assert!(
            !entry("OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0", Some("9WZDNCRFJ3TJ"))
                .matches(&expected())
        );
        assert!(
            entry("OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0", Some("9NT1R1C2HH7J"))
                .matches(&expected())
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::StoreProductId;
    use crate::install::InstallClassification;
    use crate::observe::ObservationTimestamp;

    #[test]
    fn packaged_observer_requires_a_new_healthy_expected_product() {
        let product_id = StoreProductId::parse("9WZDNCRFJ3PZ").expect("valid product ID");
        let family_name =
            PackageFamilyName::parse("Contoso.App_1234567890abc").expect("valid package family");
        let expected = ExpectedPackagedIdentity::new(
            InstallClassification::packaged_from_observed_identity(),
            product_id.clone(),
            family_name.clone(),
        )
        .expect("packaged classification");
        let baseline = PackageSnapshot {
            observed_at: ObservationTimestamp::from_unix_millis(1_000),
            entries: vec![package_entry(
                Some(StoreProductId::parse("9NBLGGH4R315").expect("valid foreign product ID")),
                &PackageFamilyName::parse("Contoso.Other_1234567890abc")
                    .expect("valid foreign family"),
                healthy_package_status(),
            )],
        };
        let follow_up = PackageSnapshot {
            observed_at: ObservationTimestamp::from_unix_millis(2_000),
            entries: vec![package_entry(
                Some(product_id.clone()),
                &family_name,
                healthy_package_status(),
            )],
        };

        assert_eq!(
            evaluate_packaged_observation(&expected, &baseline, &follow_up, &[]),
            PackagedObservation::Completed {
                appx_event_observed: false
            }
        );
    }

    #[test]
    fn packaged_observer_rejects_unhealthy_or_already_present_packages() {
        let product_id = StoreProductId::parse("9WZDNCRFJ3PZ").expect("valid product ID");
        let family_name =
            PackageFamilyName::parse("Contoso.App_1234567890abc").expect("valid package family");
        let expected = ExpectedPackagedIdentity::new(
            InstallClassification::packaged_from_observed_identity(),
            product_id.clone(),
            family_name.clone(),
        )
        .expect("packaged classification");
        let baseline = PackageSnapshot {
            observed_at: ObservationTimestamp::from_unix_millis(1_000),
            entries: Vec::new(),
        };
        let partially_staged = PackageStatusSnapshot {
            is_partially_staged: true,
            ..healthy_package_status()
        };
        let unhealthy_follow_up = PackageSnapshot {
            observed_at: ObservationTimestamp::from_unix_millis(2_000),
            entries: vec![package_entry(
                Some(product_id.clone()),
                &family_name,
                partially_staged,
            )],
        };
        assert_eq!(
            evaluate_packaged_observation(&expected, &baseline, &unhealthy_follow_up, &[]),
            PackagedObservation::Installing
        );

        let already_present = PackageSnapshot {
            observed_at: baseline.observed_at,
            entries: vec![package_entry(
                Some(product_id.clone()),
                &family_name,
                healthy_package_status(),
            )],
        };
        assert_eq!(
            evaluate_packaged_observation(&expected, &already_present, &unhealthy_follow_up, &[]),
            PackagedObservation::AlreadyPresent
        );
    }

    #[test]
    fn packaged_observer_accepts_only_fresh_product_bound_appx_evidence() {
        let product_id = StoreProductId::parse("9WZDNCRFJ3PZ").expect("valid product ID");
        let family_name =
            PackageFamilyName::parse("Contoso.App_1234567890abc").expect("valid package family");
        let expected = ExpectedPackagedIdentity::new(
            InstallClassification::packaged_from_observed_identity(),
            product_id.clone(),
            family_name.clone(),
        )
        .expect("packaged classification");
        let baseline = PackageSnapshot {
            observed_at: ObservationTimestamp::from_unix_millis(1_000),
            entries: Vec::new(),
        };
        let follow_up = PackageSnapshot {
            observed_at: ObservationTimestamp::from_unix_millis(2_000),
            entries: vec![package_entry(
                Some(product_id.clone()),
                &family_name,
                healthy_package_status(),
            )],
        };
        let stale = AppxDeploymentEvent {
            observed_at: ObservationTimestamp::from_unix_millis(999),
            product_id: Some(product_id.clone()),
            family_name: Some(family_name.clone()),
        };
        let foreign = AppxDeploymentEvent {
            observed_at: ObservationTimestamp::from_unix_millis(1_500),
            product_id: Some(StoreProductId::parse("9NBLGGH4R315").expect("valid foreign ID")),
            family_name: Some(family_name.clone()),
        };
        let matching = AppxDeploymentEvent {
            observed_at: ObservationTimestamp::from_unix_millis(1_500),
            product_id: Some(product_id),
            family_name: Some(family_name),
        };

        assert_eq!(
            evaluate_packaged_observation(&expected, &baseline, &follow_up, &[stale, foreign]),
            PackagedObservation::Completed {
                appx_event_observed: false
            }
        );
        assert_eq!(
            evaluate_packaged_observation(&expected, &baseline, &follow_up, &[matching]),
            PackagedObservation::Completed {
                appx_event_observed: true
            }
        );
        assert_eq!(
            ExpectedPackagedIdentity::new(
                InstallClassification::win32_from_backend_report(),
                expected.product_id().clone(),
                expected.family_name().clone(),
            ),
            Err(ExpectedPackagedIdentityError::WrongInstallKind)
        );
    }

    fn healthy_package_status() -> PackageStatusSnapshot {
        PackageStatusSnapshot {
            verify_is_ok: true,
            not_available: false,
            package_offline: false,
            data_offline: false,
            disabled: false,
            needs_remediation: false,
            license_issue: false,
            modified: false,
            tampered: false,
            dependency_issue: false,
            servicing: false,
            deployment_in_progress: false,
            is_partially_staged: false,
        }
    }

    fn package_entry(
        product_id: Option<StoreProductId>,
        family_name: &PackageFamilyName,
        status: PackageStatusSnapshot,
    ) -> PackageSnapshotEntry {
        PackageSnapshotEntry {
            product_id,
            family_name: family_name.clone(),
            full_name: format!("{}_1.0.0.0_x64__test", family_name.as_str()),
            status: Some(status),
        }
    }
}
