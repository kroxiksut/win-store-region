//! Chosen application source and read-only Store stub inspection.

use crate::input::StoreProductId;
use std::path::{Path, PathBuf};

/// The one application-source kind currently selected in the UI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplicationSourceKind {
    /// A Store Product ID, URL, or URI supplied as text.
    StoreText,
    /// A local Microsoft Store installer stub selected as a file.
    InstallerFile,
}

/// A validated path to a candidate Store installer stub.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallerStubPath(PathBuf);

impl InstallerStubPath {
    /// Accept a local executable candidate without opening or executing it.
    ///
    /// # Errors
    ///
    /// Returns [`InstallerStubPathError`] when the path is empty or does not
    /// have the `.exe` extension supported by v0.1.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, InstallerStubPathError> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(InstallerStubPathError::EmptyPath);
        }
        let extension = path.extension().and_then(|extension| extension.to_str());
        if !extension.is_some_and(|extension| extension.eq_ignore_ascii_case("exe")) {
            return Err(InstallerStubPathError::UnsupportedExtension);
        }
        Ok(Self(path))
    }

    /// Return the unexecuted candidate path for the platform verification layer.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// A stable identity snapshot of a candidate inspected without executing it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StubFileIdentity {
    /// Path selected by the user.
    pub path: PathBuf,
    /// File size in bytes read from the same read-only handle as the digest.
    pub byte_len: u64,
    /// Last-write timestamp expressed as nanoseconds since the Unix epoch.
    pub modified_unix_nanos: u128,
    /// Lowercase SHA-256 digest of the bytes inspected.
    pub sha256: String,
}

/// Cryptographic status returned by the native Authenticode verifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignatureStatus {
    /// `WinVerifyTrust` returned exactly zero.
    Trusted,
    /// The file has no Authenticode signature.
    Unsigned,
    /// A signature was present but did not validate.
    Invalid,
    /// Windows could not complete revocation checking.
    RevocationUnavailable,
    /// Windows could not make a verification decision for another reason.
    VerificationUnavailable,
}

/// Human-readable signer metadata extracted from the signed message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignerInfo {
    pub subject: String,
    pub issuer: String,
    pub serial_number: String,
    pub sha256_thumbprint: String,
    pub valid_from: String,
    pub valid_to: String,
    pub has_code_signing_eku: bool,
}

/// Check the currently observed Microsoft Store signer family without pinning a
/// rotating leaf certificate thumbprint.
#[must_use]
pub fn matches_microsoft_store_signer(signer: &SignerInfo) -> bool {
    signer.has_code_signing_eku
        && signer.subject.eq_ignore_ascii_case("Microsoft Corporation")
        && signer
            .issuer
            .to_ascii_lowercase()
            .starts_with("microsoft marketplace ca ")
}

/// Result of the Store-specific classification, independent from Authenticode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreStubStatus {
    /// Signature, signer policy, documented structure, and exactly one ID agree.
    VerifiedStoreStub,
    /// A candidate ID exists but cannot be trusted as a Store stub.
    UntrustedProductCandidate,
    /// The inspected file is not a recognised Store web-installer format.
    NotStoreStub,
    /// Native inspection could not establish a safe classification.
    Indeterminate,
}

/// Structured non-fatal diagnostics for an installer-stub inspection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StubInspectionWarning {
    /// The source changed while it was being analysed; no handoff is permitted.
    FileChangedDuringInspection,
    /// A native API failed; the numeric status is retained only for diagnostics.
    NativeVerificationCode(i32),
    /// Static data contains no exactly-one documented Product ID.
    ProductIdNotAvailable,
    /// Windows validated the file but did not expose a signer certificate.
    SignerDataUnavailable,
    /// More than one syntactically valid ID was found in a recognised container.
    MultipleProductIds,
    /// The signer is valid cryptographically but outside the Store policy.
    UnexpectedSigner,
    /// A non-sensitive diagnostic suitable for the details UI.
    Detail(String),
}

/// Full read-only outcome for one selected installer candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StubInspection {
    pub file_identity: StubFileIdentity,
    pub signature_status: SignatureStatus,
    pub signer: Option<SignerInfo>,
    pub store_stub_status: StoreStubStatus,
    pub product_id: Option<StoreProductId>,
    pub warnings: Vec<StubInspectionWarning>,
}

/// Evidence used by the pure Store-stub classifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StubStructureEvidence {
    /// The Product ID comes from a versioned, documented Store-stub field.
    DocumentedProductIdField,
    /// A string-like candidate is diagnostic only and never verifies a stub.
    HeuristicOnly,
    /// The file did not expose a recognised Store-stub field.
    Absent,
}

/// Apply the conservative Store-stub policy to already collected evidence.
#[must_use]
pub fn classify_store_stub(
    signature_status: SignatureStatus,
    signer_matches_store_policy: bool,
    structure: StubStructureEvidence,
    product_ids: &[StoreProductId],
) -> StoreStubStatus {
    if product_ids.len() > 1 {
        return StoreStubStatus::Indeterminate;
    }
    let Some(_) = product_ids.first() else {
        return if structure == StubStructureEvidence::Absent {
            StoreStubStatus::NotStoreStub
        } else {
            StoreStubStatus::Indeterminate
        };
    };
    if signature_status == SignatureStatus::Trusted
        && signer_matches_store_policy
        && structure == StubStructureEvidence::DocumentedProductIdField
    {
        StoreStubStatus::VerifiedStoreStub
    } else {
        StoreStubStatus::UntrustedProductCandidate
    }
}

/// Backend name recorded for a Store-installer handoff.
///
/// A handoff installs nothing itself: it holds the temporary region and hands
/// the signed installer to Microsoft Store. The name keeps those recovery and
/// history records apart from the ones a real installation backend produced.
pub const STORE_INSTALLER_HANDOFF_BACKEND: &str = "store-installer-handoff";

/// What a handoff names where a Product ID would otherwise stand.
///
/// A Microsoft Store installer carries no readable Product ID, so the file
/// itself is the only identity this operation has. Nothing here is a product
/// identifier and nothing may be used as one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandoffFileIdentity {
    file_name: String,
    sha256: String,
}

impl HandoffFileIdentity {
    /// Rebuild an identity from values that were already validated once.
    ///
    /// Used when reading history back. Both parts are required: a name without
    /// a digest names nothing in particular, and a digest without a name is not
    /// something a person can recognise.
    #[must_use]
    pub fn new(file_name: impl Into<String>, sha256: impl Into<String>) -> Option<Self> {
        let file_name = file_name.into().trim().to_owned();
        let sha256 = sha256.into().trim().to_owned();
        (!file_name.is_empty() && !sha256.is_empty()).then_some(Self { file_name, sha256 })
    }

    /// Take the file name and digest of an already-inspected candidate.
    ///
    /// The path is deliberately dropped: a record and a log line carry what the
    /// file was, never where it lived.
    #[must_use]
    pub fn from_inspected_file(identity: &StubFileIdentity) -> Option<Self> {
        let file_name = identity
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::trim)
            .filter(|name| !name.is_empty())?
            .to_owned();
        let sha256 = identity.sha256.trim();
        (!sha256.is_empty()).then(|| Self {
            file_name,
            sha256: sha256.to_owned(),
        })
    }

    /// Return the file name shown to the user and written to history.
    #[must_use]
    pub fn file_name(&self) -> &str {
        &self.file_name
    }

    /// Return the digest of the exact bytes that were inspected.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// The one line a recovery record stores in place of a Product ID.
    ///
    /// The record schema requires an identity, and this operation has only
    /// this one. It is text for a person to read, never something to parse
    /// back into a product identifier.
    #[must_use]
    pub fn record_text(&self) -> String {
        format!("{} sha256:{}", self.file_name, self.sha256)
    }
}

/// Re-check an admitted file against the digest the user actually confirmed.
///
/// The confirmation happens before the region changes, and the launch happens
/// after; a file replaced in between never inherits that permission. This is
/// the rule the handoff operation runs immediately before executing anything.
///
/// # Errors
///
/// Returns [`HandoffRefusal`] naming the single condition that failed.
pub fn admit_confirmed_installer_handoff(
    inspection: &StubInspection,
    confirmed_sha256: &str,
) -> Result<HandoffFileIdentity, HandoffRefusal> {
    let identity = admit_installer_handoff(inspection)?;
    if !identity
        .sha256()
        .eq_ignore_ascii_case(confirmed_sha256.trim())
    {
        return Err(HandoffRefusal::FileDiffersFromConfirmed);
    }
    Ok(identity)
}

/// Why a selected file may not be handed to Microsoft Store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandoffRefusal {
    /// Authenticode did not end in a trusted verdict.
    SignatureNotTrusted,
    /// Windows validated the file but exposed no signer certificate.
    SignerUnavailable,
    /// The signature is cryptographically valid but the signer is not the Store's.
    SignerOutsideStorePolicy,
    /// The file changed or was replaced while it was being inspected.
    FileChangedDuringInspection,
    /// The inspection produced no usable file name or digest.
    FileIdentityIncomplete,
    /// The bytes on disk are no longer the ones the user was shown.
    ///
    /// The confirmation names a digest, so a file swapped between that moment
    /// and the launch is a different file and gets no permission from it.
    FileDiffersFromConfirmed,
}

/// Decide whether one inspected file may be executed at all.
///
/// This is the only gate that admits running foreign code in this product, so
/// it is deliberately narrow: a trusted Authenticode verdict plus a signer that
/// passes the Microsoft Store policy, on bytes that did not change while they
/// were read. Nothing else is enough, and no other evidence widens it.
///
/// The returned identity is the only way to obtain one, so a caller cannot
/// name a file in a recovery record or in history without having passed here.
///
/// # Errors
///
/// Returns [`HandoffRefusal`] naming the single condition that failed.
pub fn admit_installer_handoff(
    inspection: &StubInspection,
) -> Result<HandoffFileIdentity, HandoffRefusal> {
    if inspection
        .warnings
        .contains(&StubInspectionWarning::FileChangedDuringInspection)
    {
        return Err(HandoffRefusal::FileChangedDuringInspection);
    }
    if inspection.signature_status != SignatureStatus::Trusted {
        return Err(HandoffRefusal::SignatureNotTrusted);
    }
    let signer = inspection
        .signer
        .as_ref()
        .ok_or(HandoffRefusal::SignerUnavailable)?;
    if !matches_microsoft_store_signer(signer) {
        return Err(HandoffRefusal::SignerOutsideStorePolicy);
    }
    HandoffFileIdentity::from_inspected_file(&inspection.file_identity)
        .ok_or(HandoffRefusal::FileIdentityIncomplete)
}

/// Why a local candidate cannot be selected as an installer stub.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallerStubPathError {
    /// The user did not provide a path.
    EmptyPath,
    /// v0.1 accepts only executable Store installer stubs.
    UnsupportedExtension,
}

/// Draft application input that preserves both source variants across switching.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplicationSourceDraft {
    selected: ApplicationSourceKind,
    store_text: String,
    installer_file: Option<InstallerStubPath>,
}

impl ApplicationSourceDraft {
    /// Start with an optional Store-text prefill, for example from the command line.
    #[must_use]
    pub fn new(store_text: impl Into<String>) -> Self {
        Self {
            selected: ApplicationSourceKind::StoreText,
            store_text: store_text.into(),
            installer_file: None,
        }
    }

    /// Return the source selected for the next explicit validation step.
    #[must_use]
    pub const fn selected(&self) -> ApplicationSourceKind {
        self.selected
    }

    /// Select the Store-text source without clearing the selected file.
    pub fn select_store_text(&mut self) {
        self.selected = ApplicationSourceKind::StoreText;
    }

    /// Select the file source without clearing the Store-text draft.
    pub fn select_installer_file(&mut self) {
        self.selected = ApplicationSourceKind::InstallerFile;
    }

    /// Replace the Store-text draft without parsing or resolving it.
    pub fn set_store_text(&mut self, value: impl Into<String>) {
        self.store_text = value.into();
    }

    /// Return the raw Store-text draft for block 8 validation.
    #[must_use]
    pub fn store_text(&self) -> &str {
        &self.store_text
    }

    /// Set a local executable candidate and make the file source active.
    ///
    /// This operation only stores the path. It never reads, launches, or trusts
    /// the file; that belongs to the block 9 verification boundary.
    ///
    /// # Errors
    ///
    /// Returns [`InstallerStubPathError`] without changing either draft when
    /// the candidate is unsupported.
    pub fn select_installer_stub(
        &mut self,
        path: impl Into<PathBuf>,
    ) -> Result<(), InstallerStubPathError> {
        let path = InstallerStubPath::new(path)?;
        self.installer_file = Some(path);
        self.selected = ApplicationSourceKind::InstallerFile;
        Ok(())
    }

    /// Return the file draft without making it active.
    #[must_use]
    pub fn installer_file(&self) -> Option<&InstallerStubPath> {
        self.installer_file.as_ref()
    }

    /// Clear only the stored file draft and select the Store-text source.
    pub fn clear_installer_file(&mut self) {
        self.installer_file = None;
        self.selected = ApplicationSourceKind::StoreText;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::StoreInputCandidate;
    use std::path::PathBuf;

    #[test]
    fn application_source_keeps_inactive_draft_and_rejects_non_executable_files() {
        let mut source = ApplicationSourceDraft::new("9WZDNCRFJ3PZ");
        source
            .select_installer_stub(r"C:\Downloads\StoreInstaller.EXE")
            .expect("case-insensitive executable extension");

        assert_eq!(source.selected(), ApplicationSourceKind::InstallerFile);
        assert_eq!(source.store_text(), "9WZDNCRFJ3PZ");
        assert_eq!(
            source
                .installer_file()
                .expect("selected file is preserved")
                .as_path(),
            std::path::Path::new(r"C:\Downloads\StoreInstaller.EXE")
        );
        let selected_file = source
            .installer_file()
            .expect("selected file is preserved")
            .clone();
        assert_eq!(
            source.select_installer_stub(r"C:\Downloads\package.msix"),
            Err(InstallerStubPathError::UnsupportedExtension)
        );
        assert_eq!(source.selected(), ApplicationSourceKind::InstallerFile);
        assert_eq!(source.store_text(), "9WZDNCRFJ3PZ");
        assert_eq!(source.installer_file(), Some(&selected_file));

        source.select_store_text();
        source.set_store_text("https://apps.microsoft.com/detail/9WZDNCRFJ3PZ");
        assert_eq!(source.selected(), ApplicationSourceKind::StoreText);
        assert!(source.installer_file().is_some());

        source.clear_installer_file();
        assert_eq!(source.selected(), ApplicationSourceKind::StoreText);
        assert!(source.installer_file().is_none());
        assert_eq!(
            source.store_text(),
            "https://apps.microsoft.com/detail/9WZDNCRFJ3PZ"
        );
    }

    #[test]
    fn store_stub_classifier_requires_all_independent_evidence() {
        let product = StoreInputCandidate::parse("9wzdncrfj3pz")
            .expect("valid Product ID")
            .product_id()
            .clone();
        assert_eq!(
            classify_store_stub(
                SignatureStatus::Trusted,
                true,
                StubStructureEvidence::DocumentedProductIdField,
                std::slice::from_ref(&product),
            ),
            StoreStubStatus::VerifiedStoreStub
        );
        assert_eq!(
            classify_store_stub(
                SignatureStatus::Trusted,
                true,
                StubStructureEvidence::HeuristicOnly,
                std::slice::from_ref(&product),
            ),
            StoreStubStatus::UntrustedProductCandidate
        );
        assert_eq!(
            classify_store_stub(
                SignatureStatus::Invalid,
                false,
                StubStructureEvidence::DocumentedProductIdField,
                std::slice::from_ref(&product),
            ),
            StoreStubStatus::UntrustedProductCandidate
        );
        assert_eq!(
            classify_store_stub(
                SignatureStatus::Trusted,
                true,
                StubStructureEvidence::DocumentedProductIdField,
                &[product.clone(), product],
            ),
            StoreStubStatus::Indeterminate
        );
        assert_eq!(
            classify_store_stub(
                SignatureStatus::Trusted,
                true,
                StubStructureEvidence::Absent,
                &[],
            ),
            StoreStubStatus::NotStoreStub
        );
    }

    fn store_signer() -> SignerInfo {
        SignerInfo {
            subject: "Microsoft Corporation".to_owned(),
            issuer: "Microsoft Marketplace CA G 024".to_owned(),
            serial_number: "serial".to_owned(),
            sha256_thumbprint: "thumbprint".to_owned(),
            valid_from: "2026-01-01T00:00:00Z".to_owned(),
            valid_to: "2027-01-01T00:00:00Z".to_owned(),
            has_code_signing_eku: true,
        }
    }

    fn admissible_inspection() -> StubInspection {
        StubInspection {
            file_identity: StubFileIdentity {
                path: PathBuf::from(r"C:\Downloads\StoreInstaller.exe"),
                byte_len: 1_462_848,
                modified_unix_nanos: 0,
                sha256: "97ec678a".to_owned(),
            },
            signature_status: SignatureStatus::Trusted,
            signer: Some(store_signer()),
            store_stub_status: StoreStubStatus::NotStoreStub,
            product_id: None,
            warnings: vec![StubInspectionWarning::ProductIdNotAvailable],
        }
    }

    #[test]
    fn only_a_trusted_file_signed_by_the_store_may_ever_be_executed() {
        let admitted =
            admit_installer_handoff(&admissible_inspection()).expect("a Store-signed installer");
        assert_eq!(admitted.file_name(), "StoreInstaller.exe");
        assert_eq!(admitted.sha256(), "97ec678a");
        assert_eq!(admitted.record_text(), "StoreInstaller.exe sha256:97ec678a");

        // Every gate refuses on its own, whatever the rest of the evidence says.
        for (inspection, refusal) in [
            (
                StubInspection {
                    signature_status: SignatureStatus::Unsigned,
                    ..admissible_inspection()
                },
                HandoffRefusal::SignatureNotTrusted,
            ),
            (
                StubInspection {
                    signature_status: SignatureStatus::RevocationUnavailable,
                    ..admissible_inspection()
                },
                HandoffRefusal::SignatureNotTrusted,
            ),
            (
                StubInspection {
                    signer: None,
                    ..admissible_inspection()
                },
                HandoffRefusal::SignerUnavailable,
            ),
            (
                StubInspection {
                    signer: Some(SignerInfo {
                        issuer: "Unrelated CA".to_owned(),
                        ..store_signer()
                    }),
                    ..admissible_inspection()
                },
                HandoffRefusal::SignerOutsideStorePolicy,
            ),
            (
                StubInspection {
                    warnings: vec![StubInspectionWarning::FileChangedDuringInspection],
                    ..admissible_inspection()
                },
                HandoffRefusal::FileChangedDuringInspection,
            ),
            (
                StubInspection {
                    file_identity: StubFileIdentity {
                        sha256: String::new(),
                        ..admissible_inspection().file_identity
                    },
                    ..admissible_inspection()
                },
                HandoffRefusal::FileIdentityIncomplete,
            ),
        ] {
            assert_eq!(admit_installer_handoff(&inspection), Err(refusal));
        }
    }

    #[test]
    fn a_handoff_identity_never_carries_the_directory_the_file_came_from() {
        let identity = admit_installer_handoff(&admissible_inspection()).expect("admitted file");
        assert!(!identity.record_text().contains(r"C:\"));
        assert!(!identity.record_text().contains("Downloads"));
    }

    #[test]
    fn signer_policy_allows_marketplace_ca_rotation_without_pinning_leaf_hashes() {
        let signer = SignerInfo {
            subject: "Microsoft Corporation".to_owned(),
            issuer: "Microsoft Marketplace CA G 024".to_owned(),
            serial_number: "serial".to_owned(),
            sha256_thumbprint: "old-leaf-thumbprint".to_owned(),
            valid_from: "2026-01-01T00:00:00Z".to_owned(),
            valid_to: "2027-01-01T00:00:00Z".to_owned(),
            has_code_signing_eku: true,
        };
        assert!(matches_microsoft_store_signer(&signer));
        let rotated = SignerInfo {
            issuer: "Microsoft Marketplace CA G 999".to_owned(),
            sha256_thumbprint: "new-leaf-thumbprint".to_owned(),
            ..signer.clone()
        };
        assert!(matches_microsoft_store_signer(&rotated));
        let unexpected = SignerInfo {
            subject: "Microsoft Corporation".to_owned(),
            issuer: "Unrelated CA".to_owned(),
            ..signer
        };
        assert!(!matches_microsoft_store_signer(&unexpected));
    }
}
