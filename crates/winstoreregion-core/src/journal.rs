//! Durable, human-readable history of completed operations.

use crate::input::StoreProductId;
use crate::install::InstallKind;
use crate::observe::packaged::PackageFamilyName;
use crate::region::{GeoId, OperationId};
use crate::source::HandoffFileIdentity;
use crate::time::UtcTimestamp;
use serde::{Deserialize, Serialize};

/// File name used for the human-readable per-user operation history.
pub const JOURNAL_FILE_NAME: &str = "journal.json";

/// Version written by this release for each journal entry.
pub const JOURNAL_SCHEMA_VERSION: u32 = 2;

/// Earlier schema this release still reads.
///
/// Version 1 knew only product entries. Its records are complete under the
/// current rules, so an existing history keeps working instead of being
/// rejected as a whole the first time this version runs.
const JOURNAL_SCHEMA_VERSION_PRODUCT_ONLY: u32 = 1;

/// Durable user-visible result of one guarded operation.
///
/// These results deliberately distinguish a confirmed installation from an
/// operation that still requires recovery or a manual completion decision.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalResult {
    /// Installation and original-region restoration were both confirmed.
    Installed,
    /// The installer was started under the temporary region and took over.
    ///
    /// This is not an installation result and must never be shown as one. The
    /// application switched the region, launched the signed Microsoft Store
    /// installer, and the user finished the work in Microsoft Store. Whether
    /// anything was installed is not established by this operation.
    HandedOffToInstaller,
    /// Completion could not be established automatically; recovery remains required.
    CompletionUncertain,
    /// The operation ended without proving that recovery is no longer needed.
    RecoveryRequired,
    /// The operation failed before a confirmed installation result.
    Failed,
}

impl JournalResult {
    /// Whether this history entry must remain visually prominent in the journal.
    #[must_use]
    pub const fn requires_attention(self) -> bool {
        matches!(self, Self::CompletionUncertain | Self::RecoveryRequired)
    }
}

/// The product one history entry is about, as the catalogue described it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalProduct {
    product_id: StoreProductId,
    package_family_name: Option<PackageFamilyName>,
    install_kind: InstallKind,
    name: String,
    publisher: Option<String>,
    installed_version: Option<String>,
}

impl JournalProduct {
    /// Build the product half of an entry from already-confirmed facts.
    ///
    /// # Errors
    ///
    /// Returns [`JournalRecordBuildError`] when presentation data required for
    /// a useful, trustworthy history item is missing or blank.
    pub fn new(
        product_id: StoreProductId,
        package_family_name: Option<PackageFamilyName>,
        install_kind: InstallKind,
        name: impl Into<String>,
        publisher: Option<String>,
        installed_version: Option<String>,
    ) -> Result<Self, JournalRecordBuildError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(JournalRecordBuildError::MissingName);
        }
        if publisher
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(JournalRecordBuildError::InvalidPublisher);
        }
        if installed_version
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(JournalRecordBuildError::InvalidVersion);
        }
        Ok(Self {
            product_id,
            package_family_name,
            install_kind,
            name,
            publisher,
            installed_version,
        })
    }

    /// Return the exact Store Product ID.
    #[must_use]
    pub const fn product_id(&self) -> &StoreProductId {
        &self.product_id
    }

    /// Return the optional package family identity established by observation.
    #[must_use]
    pub const fn package_family_name(&self) -> Option<&PackageFamilyName> {
        self.package_family_name.as_ref()
    }

    /// Return the evidence-backed installation type.
    #[must_use]
    pub const fn install_kind(&self) -> InstallKind {
        self.install_kind
    }

    /// Return the resolved application name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return the optional publisher reported by the resolver.
    #[must_use]
    pub fn publisher(&self) -> Option<&str> {
        self.publisher.as_deref()
    }

    /// Return the optional installed version obtained from the observer.
    #[must_use]
    pub fn installed_version(&self) -> Option<&str> {
        self.installed_version.as_deref()
    }
}

/// What one history entry is about.
///
/// The two variants exist because a Microsoft Store installer file carries no
/// readable Product ID. An operation that hands such a file over therefore has
/// no product to name, and an entry that invented one would be a claim nothing
/// supports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JournalSubject {
    /// A product the catalogue named and resolved.
    Product(JournalProduct),
    /// A signed installer file that Microsoft Store took over.
    InstallerFile(HandoffFileIdentity),
}

impl JournalSubject {
    /// Return the product half, when this entry has one.
    #[must_use]
    pub const fn product(&self) -> Option<&JournalProduct> {
        match self {
            Self::Product(product) => Some(product),
            Self::InstallerFile(_) => None,
        }
    }

    /// Return the file this entry is about, when it is about a file.
    #[must_use]
    pub const fn installer_file(&self) -> Option<&HandoffFileIdentity> {
        match self {
            Self::InstallerFile(file) => Some(file),
            Self::Product(_) => None,
        }
    }

    /// The name this entry can be listed under.
    ///
    /// A product entry uses the catalogue name; a file entry uses the file
    /// name, which is the only name that exists for it.
    #[must_use]
    pub fn display_name(&self) -> &str {
        match self {
            Self::Product(product) => product.name(),
            Self::InstallerFile(file) => file.file_name(),
        }
    }
}

/// One immutable, human-readable journal entry.
///
/// The record stores only facts established by the operation layer.  In
/// particular, it never treats a PDP launch or an accepted backend request as
/// a confirmed installation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalRecord {
    schema_version: u32,
    operation_id: OperationId,
    subject: JournalSubject,
    region: GeoId,
    installed_at: UtcTimestamp,
    backend: String,
    result: JournalResult,
}

impl JournalRecord {
    /// Build a complete journal entry from already-confirmed operation facts.
    ///
    /// # Errors
    ///
    /// Returns [`JournalRecordBuildError`] when the backend name is blank.
    pub fn new(
        operation_id: OperationId,
        subject: JournalSubject,
        region: GeoId,
        installed_at: UtcTimestamp,
        backend: impl Into<String>,
        result: JournalResult,
    ) -> Result<Self, JournalRecordBuildError> {
        let backend = backend.into();
        if backend.trim().is_empty() {
            return Err(JournalRecordBuildError::MissingBackend);
        }
        Ok(Self {
            schema_version: JOURNAL_SCHEMA_VERSION,
            operation_id,
            subject,
            region,
            installed_at,
            backend,
            result,
        })
    }

    /// Return the immutable operation identifier.
    #[must_use]
    pub const fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    /// Return what this entry is about.
    #[must_use]
    pub const fn subject(&self) -> &JournalSubject {
        &self.subject
    }

    /// Return the exact Store Product ID, when this entry has one.
    ///
    /// A handoff entry has none: the file it names carries no readable product
    /// identifier, so every action that needs one is unavailable for it.
    #[must_use]
    pub const fn product_id(&self) -> Option<&StoreProductId> {
        match self.subject.product() {
            Some(product) => Some(product.product_id()),
            None => None,
        }
    }

    /// Return the name this entry is listed under.
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.subject.display_name()
    }

    /// Return the selected temporary region `GeoId`.
    #[must_use]
    pub const fn region(&self) -> GeoId {
        self.region
    }

    /// Return the completed or unresolved operation timestamp.
    #[must_use]
    pub const fn installed_at(&self) -> &UtcTimestamp {
        &self.installed_at
    }

    /// Return the backend name kept for diagnostics.
    #[must_use]
    pub fn backend(&self) -> &str {
        &self.backend
    }

    /// Return the durable, user-visible result.
    #[must_use]
    pub const fn result(&self) -> JournalResult {
        self.result
    }
}

impl TryFrom<JournalRecordWire> for JournalRecord {
    type Error = JournalRecordBuildError;

    fn try_from(wire: JournalRecordWire) -> Result<Self, Self::Error> {
        if wire.schema_version != JOURNAL_SCHEMA_VERSION
            && wire.schema_version != JOURNAL_SCHEMA_VERSION_PRODUCT_ONLY
        {
            return Err(JournalRecordBuildError::InvalidSchemaVersion);
        }
        let subject = subject_from_wire(&wire)?;
        let operation_id = OperationId::new(wire.operation_id)
            .ok_or(JournalRecordBuildError::InvalidOperationId)?;
        let region = GeoId::new(wire.region).ok_or(JournalRecordBuildError::InvalidGeoId)?;
        let installed_at = UtcTimestamp::parse(wire.installed_at)
            .ok_or(JournalRecordBuildError::InvalidTimestamp)?;
        Self::new(
            operation_id,
            subject,
            region,
            installed_at,
            wire.backend,
            wire.result,
        )
    }
}

/// Rebuild exactly one subject, refusing a record that names none or both.
fn subject_from_wire(wire: &JournalRecordWire) -> Result<JournalSubject, JournalRecordBuildError> {
    match (wire.product_id.as_deref(), wire.installer_file.as_ref()) {
        (Some(_), Some(_)) | (None, None) => Err(JournalRecordBuildError::AmbiguousSubject),
        (Some(product_id), None) => {
            let product_id = StoreProductId::parse(product_id)
                .map_err(|_| JournalRecordBuildError::InvalidProductId)?;
            let package_family_name = match wire.package_family_name.as_deref() {
                Some(value) => Some(
                    PackageFamilyName::parse(value)
                        .ok_or(JournalRecordBuildError::InvalidPackageFamilyName)?,
                ),
                None => None,
            };
            let name = wire
                .name
                .clone()
                .ok_or(JournalRecordBuildError::MissingName)?;
            JournalProduct::new(
                product_id,
                package_family_name,
                wire.install_kind.unwrap_or(InstallKind::Unknown),
                name,
                wire.publisher.clone(),
                wire.installed_version.clone(),
            )
            .map(JournalSubject::Product)
        }
        (None, Some(file)) => HandoffFileIdentity::new(file.name.clone(), file.sha256.clone())
            .map(JournalSubject::InstallerFile)
            .ok_or(JournalRecordBuildError::InvalidInstallerFile),
    }
}

/// Validation failure while building or reading a journal entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalRecordBuildError {
    /// The record used a schema this release does not understand.
    InvalidSchemaVersion,
    /// The record did not carry a valid operation ID.
    InvalidOperationId,
    /// The record named no subject at all, or named two.
    AmbiguousSubject,
    /// The record did not carry a valid Store Product ID.
    InvalidProductId,
    /// The optional package family name was blank.
    InvalidPackageFamilyName,
    /// The resolved application name was blank.
    MissingName,
    /// An optional publisher was present but blank.
    InvalidPublisher,
    /// The stored region was not a positive Windows `GeoId`.
    InvalidGeoId,
    /// The record timestamp was not RFC 3339 UTC.
    InvalidTimestamp,
    /// An optional installed version was present but blank.
    InvalidVersion,
    /// The handed-over file had no usable name or digest.
    InvalidInstallerFile,
    /// The backend name was blank.
    MissingBackend,
}

/// Failure while encoding or decoding the whole journal file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalFormatError {
    /// The JSON document was malformed or did not contain a journal array.
    MalformedJson,
    /// One entry was incomplete or violated the journal schema.
    InvalidRecord(JournalRecordBuildError),
    /// Serialization failed before an I/O operation started.
    Serialization,
}

/// Serialize records as stable, indented UTF-8 JSON for direct user inspection.
///
/// # Errors
///
/// Returns [`JournalFormatError::Serialization`] only if the JSON encoder fails.
pub fn journal_records_to_json(records: &[JournalRecord]) -> Result<Vec<u8>, JournalFormatError> {
    serde_json::to_vec_pretty(
        &records
            .iter()
            .map(JournalRecordWire::from)
            .collect::<Vec<_>>(),
    )
    .map_err(|_| JournalFormatError::Serialization)
}

/// Decode and validate every entry before exposing any journal history.
///
/// A malformed item rejects the complete file; callers must preserve it rather
/// than silently dropping history or overwriting it with a partial replacement.
///
/// # Errors
///
/// Returns [`JournalFormatError`] if the document is not a complete supported
/// journal array or any record fails its validation rules.
pub fn journal_records_from_json(bytes: &[u8]) -> Result<Vec<JournalRecord>, JournalFormatError> {
    let wires = serde_json::from_slice::<Vec<JournalRecordWire>>(bytes)
        .map_err(|_| JournalFormatError::MalformedJson)?;
    wires
        .into_iter()
        .map(|wire| JournalRecord::try_from(wire).map_err(JournalFormatError::InvalidRecord))
        .collect()
}

#[derive(Deserialize, Serialize)]
struct JournalRecordWire {
    schema_version: u32,
    operation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    product_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    package_family_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    install_kind: Option<InstallKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    publisher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    installed_version: Option<String>,
    /// Present instead of every product field when a file was handed over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    installer_file: Option<InstallerFileWire>,
    region: i32,
    installed_at: String,
    backend: String,
    result: JournalResult,
}

#[derive(Deserialize, Serialize)]
struct InstallerFileWire {
    name: String,
    sha256: String,
}

impl From<&JournalRecord> for JournalRecordWire {
    fn from(record: &JournalRecord) -> Self {
        let base = Self {
            schema_version: record.schema_version,
            operation_id: record.operation_id.as_str().to_owned(),
            product_id: None,
            package_family_name: None,
            install_kind: None,
            name: None,
            publisher: None,
            installed_version: None,
            installer_file: None,
            region: record.region.value(),
            installed_at: record.installed_at.as_str().to_owned(),
            backend: record.backend.clone(),
            result: record.result,
        };
        match &record.subject {
            JournalSubject::Product(product) => Self {
                product_id: Some(product.product_id.as_str().to_owned()),
                package_family_name: product
                    .package_family_name
                    .as_ref()
                    .map(|family| family.as_str().to_owned()),
                install_kind: Some(product.install_kind),
                name: Some(product.name.clone()),
                publisher: product.publisher.clone(),
                installed_version: product.installed_version.clone(),
                ..base
            },
            JournalSubject::InstallerFile(file) => Self {
                installer_file: Some(InstallerFileWire {
                    name: file.file_name().to_owned(),
                    sha256: file.sha256().to_owned(),
                }),
                ..base
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::StoreProductId;
    use crate::install::InstallKind;
    use crate::observe::packaged::PackageFamilyName;
    use crate::region::OperationId;
    use crate::source::HandoffFileIdentity;
    use crate::test_support::geo_id;
    use crate::time::UtcTimestamp;

    fn product_subject() -> JournalSubject {
        JournalSubject::Product(
            JournalProduct::new(
                StoreProductId::parse("9WZDNCRFJ3PZ").expect("Product ID"),
                Some(PackageFamilyName::parse("Contoso.App_1234567890abc").expect("family")),
                InstallKind::Packaged,
                "Contoso App",
                Some("Contoso".to_owned()),
                Some("1.0.0".to_owned()),
            )
            .expect("complete product"),
        )
    }

    #[test]
    fn journal_json_round_trip_is_indented_and_retains_confirmed_facts() {
        let record = JournalRecord::new(
            OperationId::new("journal-operation").expect("operation ID"),
            product_subject(),
            geo_id(244),
            UtcTimestamp::parse("2026-08-17T12:34:56Z").expect("timestamp"),
            "test-backend",
            JournalResult::Installed,
        )
        .expect("journal record");

        let json =
            journal_records_to_json(std::slice::from_ref(&record)).expect("serialize journal");
        assert!(
            std::str::from_utf8(&json)
                .expect("UTF-8 JSON")
                .contains("\n  {")
        );
        assert_eq!(journal_records_from_json(&json), Ok(vec![record]));
    }

    #[test]
    fn a_handed_over_file_is_recorded_by_name_and_digest_and_names_no_product() {
        let file = HandoffFileIdentity::new("StoreInstaller.exe", "abc123").expect("file identity");
        let record = JournalRecord::new(
            OperationId::new("handoff-operation").expect("operation ID"),
            JournalSubject::InstallerFile(file),
            geo_id(244),
            UtcTimestamp::parse("2026-08-21T09:00:00Z").expect("timestamp"),
            crate::source::STORE_INSTALLER_HANDOFF_BACKEND,
            JournalResult::HandedOffToInstaller,
        )
        .expect("journal record");

        assert!(record.product_id().is_none());
        assert_eq!(record.display_name(), "StoreInstaller.exe");
        assert!(!record.result().requires_attention());

        let json =
            journal_records_to_json(std::slice::from_ref(&record)).expect("serialize journal");
        let text = std::str::from_utf8(&json).expect("UTF-8 JSON");
        assert!(text.contains("\"installer_file\""));
        assert!(
            !text.contains("product_id"),
            "an entry with no product must not carry a product field at all"
        );
        assert_eq!(journal_records_from_json(&json), Ok(vec![record]));
    }

    #[test]
    fn version_one_history_is_still_readable_and_a_record_names_exactly_one_subject() {
        let version_one = format!(
            r#"[{{"schema_version":{JOURNAL_SCHEMA_VERSION_PRODUCT_ONLY},"operation_id":"operation","product_id":"9WZDNCRFJ3PZ","package_family_name":null,"install_kind":"packaged","name":"App","publisher":null,"region":244,"installed_at":"2026-08-17T12:34:56Z","installed_version":null,"backend":"backend","result":"installed"}}]"#
        );
        let records =
            journal_records_from_json(version_one.as_bytes()).expect("version 1 history is kept");
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].product_id().map(StoreProductId::as_str),
            Some("9WZDNCRFJ3PZ")
        );

        let both = r#"[{"schema_version":2,"operation_id":"operation","product_id":"9WZDNCRFJ3PZ","name":"App","install_kind":"packaged","installer_file":{"name":"a.exe","sha256":"abc"},"region":244,"installed_at":"2026-08-17T12:34:56Z","backend":"backend","result":"installed"}]"#;
        assert_eq!(
            journal_records_from_json(both.as_bytes()),
            Err(JournalFormatError::InvalidRecord(
                JournalRecordBuildError::AmbiguousSubject
            ))
        );

        let neither = r#"[{"schema_version":2,"operation_id":"operation","region":244,"installed_at":"2026-08-17T12:34:56Z","backend":"backend","result":"installed"}]"#;
        assert_eq!(
            journal_records_from_json(neither.as_bytes()),
            Err(JournalFormatError::InvalidRecord(
                JournalRecordBuildError::AmbiguousSubject
            ))
        );
    }

    #[test]
    fn journal_json_rejects_unknown_schema_and_incomplete_entries() {
        let unknown_schema = format!(
            r#"[{{"schema_version":{},"operation_id":"operation","product_id":"9WZDNCRFJ3PZ","package_family_name":null,"install_kind":"packaged","name":"App","publisher":null,"region":244,"installed_at":"2026-08-17T12:34:56Z","installed_version":null,"backend":"backend","result":"installed"}}]"#,
            JOURNAL_SCHEMA_VERSION + 1
        );
        assert_eq!(
            journal_records_from_json(unknown_schema.as_bytes()),
            Err(JournalFormatError::InvalidRecord(
                JournalRecordBuildError::InvalidSchemaVersion
            ))
        );
        assert_eq!(
            journal_records_from_json(br#"[{"schema_version":1}]"#),
            Err(JournalFormatError::MalformedJson)
        );
    }
}
