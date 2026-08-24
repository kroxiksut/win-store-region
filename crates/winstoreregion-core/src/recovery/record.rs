//! The `pending-restore.json` record and its wire schema.

use crate::install::InstallClassification;
use crate::observe::packaged::PackageFamilyName;
use crate::recovery::store::{RecoverySchemaError, RecoveryStoreError};
use crate::region::{GeoId, OperationId, PreparedRegionChange};
use crate::time::UtcTimestamp;
use serde::{Deserialize, Serialize};

/// Version of the on-disk `pending-restore.json` schema understood by this release.
pub const PENDING_RESTORE_SCHEMA_VERSION: u32 = 1;

/// File name used for the one published recovery record.
pub const PENDING_RESTORE_FILE_NAME: &str = "pending-restore.json";

/// Persisted snapshot of a guarded region-changing operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingRestore {
    schema_version: u32,
    operation_id: OperationId,
    started_at_utc: UtcTimestamp,
    original_region: GeoId,
    temporary_region: GeoId,
    product_id: String,
    backend: String,
    install_classification: InstallClassification,
    /// Package identity the resolver named, once the operation has one.
    ///
    /// Written after the product is resolved under the temporary region. A
    /// restart that takes the install over needs it: without it, completion
    /// cannot be proven and the operation can only end as uncertain.
    package_family_name: Option<PackageFamilyName>,
    state: DurableOperationState,
}

impl PendingRestore {
    /// Build the first complete recovery record before the region writer runs.
    ///
    /// # Errors
    ///
    /// Returns [`PendingRestoreBuildError`] when a required persisted value is
    /// empty or the timestamp is not UTC RFC 3339 text.
    pub fn prepared(
        operation_id: OperationId,
        started_at_utc: UtcTimestamp,
        original_region: GeoId,
        temporary_region: GeoId,
        product_id: impl Into<String>,
        backend: impl Into<String>,
    ) -> Result<Self, PendingRestoreBuildError> {
        Self::new(
            operation_id,
            started_at_utc,
            original_region,
            temporary_region,
            product_id.into(),
            backend.into(),
            DurableOperationState::Prepared,
        )
    }

    fn new(
        operation_id: OperationId,
        started_at_utc: UtcTimestamp,
        original_region: GeoId,
        temporary_region: GeoId,
        product_id: String,
        backend: String,
        state: DurableOperationState,
    ) -> Result<Self, PendingRestoreBuildError> {
        if product_id.trim().is_empty() {
            return Err(PendingRestoreBuildError::MissingProductId);
        }
        if backend.trim().is_empty() {
            return Err(PendingRestoreBuildError::MissingBackend);
        }

        Ok(Self {
            schema_version: PENDING_RESTORE_SCHEMA_VERSION,
            operation_id,
            started_at_utc,
            original_region,
            temporary_region,
            product_id,
            backend,
            install_classification: InstallClassification::unknown(),
            package_family_name: None,
            state,
        })
    }

    /// Return a complete replacement record with the supplied durable state.
    #[must_use]
    pub fn with_state(&self, state: DurableOperationState) -> Self {
        Self {
            schema_version: self.schema_version,
            operation_id: self.operation_id.clone(),
            started_at_utc: self.started_at_utc.clone(),
            original_region: self.original_region,
            temporary_region: self.temporary_region,
            product_id: self.product_id.clone(),
            backend: self.backend.clone(),
            install_classification: self.install_classification,
            package_family_name: self.package_family_name.clone(),
            state,
        }
    }

    /// Publish the package identity this operation expects to appear.
    #[must_use]
    pub fn with_package_family_name(&self, package_family_name: PackageFamilyName) -> Self {
        Self {
            package_family_name: Some(package_family_name),
            ..self.clone()
        }
    }

    /// The package identity recorded for this operation, when one is known.
    #[must_use]
    pub const fn package_family_name(&self) -> Option<&PackageFamilyName> {
        self.package_family_name.as_ref()
    }

    /// Return a complete replacement record with a proven installation model.
    ///
    /// A caller may replace the initial `Unknown` only with a value produced by
    /// [`InstallClassification`]'s evidence-backed constructors.
    #[must_use]
    pub fn with_install_classification(
        &self,
        install_classification: InstallClassification,
    ) -> Self {
        Self {
            schema_version: self.schema_version,
            operation_id: self.operation_id.clone(),
            started_at_utc: self.started_at_utc.clone(),
            original_region: self.original_region,
            temporary_region: self.temporary_region,
            product_id: self.product_id.clone(),
            backend: self.backend.clone(),
            install_classification,
            package_family_name: self.package_family_name.clone(),
            state: self.state,
        }
    }

    /// Serialize the complete validated record as UTF-8 JSON.
    ///
    /// # Errors
    ///
    /// Returns [`RecoveryStoreError::Serialization`] if JSON serialization
    /// unexpectedly fails.
    pub fn to_json_bytes(&self) -> Result<Vec<u8>, RecoveryStoreError> {
        serde_json::to_vec(&PendingRestoreWire::from(self))
            .map_err(|_| RecoveryStoreError::Serialization)
    }

    /// Parse and validate a complete recovery record from UTF-8 JSON.
    ///
    /// # Errors
    ///
    /// Returns [`RecoveryStoreError::Schema`] for malformed JSON, an unknown
    /// schema, or a record missing a required validated value.
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self, RecoveryStoreError> {
        let value = serde_json::from_slice::<serde_json::Value>(bytes)
            .map_err(|_| RecoveryStoreError::Schema(RecoverySchemaError::MalformedJson))?;
        let version = value
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or(RecoveryStoreError::Schema(
                RecoverySchemaError::IncompleteRecord,
            ))?;
        if version != PENDING_RESTORE_SCHEMA_VERSION {
            return Err(RecoveryStoreError::Schema(
                RecoverySchemaError::UnknownSchema(version),
            ));
        }

        let wire = serde_json::from_value::<PendingRestoreWire>(value)
            .map_err(|_| RecoveryStoreError::Schema(RecoverySchemaError::IncompleteRecord))?;
        Self::try_from(wire)
            .map_err(|_| RecoveryStoreError::Schema(RecoverySchemaError::IncompleteRecord))
    }

    /// Return the operation identifier.
    #[must_use]
    pub fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    /// Return the timestamp captured before the guarded operation started.
    #[must_use]
    pub fn started_at_utc(&self) -> &UtcTimestamp {
        &self.started_at_utc
    }

    /// Return the recovered product identity for user-facing recovery UI.
    #[must_use]
    pub fn product_id(&self) -> &str {
        &self.product_id
    }

    /// Return the backend recorded for technical recovery details.
    #[must_use]
    pub fn backend(&self) -> &str {
        &self.backend
    }

    /// Return the persisted evidence-backed installation classification.
    #[must_use]
    pub const fn install_classification(&self) -> InstallClassification {
        self.install_classification
    }

    /// Return the original region used for safe restoration.
    #[must_use]
    pub const fn original_region(&self) -> GeoId {
        self.original_region
    }

    /// Return the temporary region used for safe conflict detection.
    #[must_use]
    pub const fn temporary_region(&self) -> GeoId {
        self.temporary_region
    }

    /// Return the currently durable operation state.
    #[must_use]
    pub const fn state(&self) -> DurableOperationState {
        self.state
    }

    /// Check whether two snapshots belong to the same recovery operation.
    #[must_use]
    pub fn has_same_recovery_identity(&self, other: &Self) -> bool {
        self.operation_id == other.operation_id
            && self.original_region == other.original_region
            && self.temporary_region == other.temporary_region
    }

    /// Check that this record describes exactly the supplied prepared change.
    pub(crate) fn matches_change(&self, change: &PreparedRegionChange) -> bool {
        self.operation_id == change.operation_id
            && self.original_region == change.original_region
            && self.temporary_region == change.target_region
    }
}

impl TryFrom<PendingRestoreWire> for PendingRestore {
    type Error = PendingRestoreBuildError;

    fn try_from(wire: PendingRestoreWire) -> Result<Self, Self::Error> {
        if wire.schema_version != PENDING_RESTORE_SCHEMA_VERSION {
            return Err(PendingRestoreBuildError::InvalidSchemaVersion);
        }
        let operation_id = OperationId::new(wire.operation_id)
            .ok_or(PendingRestoreBuildError::InvalidOperationId)?;
        let started_at_utc = UtcTimestamp::parse(wire.started_at_utc)
            .ok_or(PendingRestoreBuildError::InvalidStartedAtUtc)?;
        let original_region =
            GeoId::new(wire.original_region).ok_or(PendingRestoreBuildError::InvalidGeoId)?;
        let temporary_region =
            GeoId::new(wire.temporary_region).ok_or(PendingRestoreBuildError::InvalidGeoId)?;
        Self::new(
            operation_id,
            started_at_utc,
            original_region,
            temporary_region,
            wire.product_id,
            wire.backend,
            wire.state,
        )
        .map(|pending| pending.with_install_classification(wire.install_classification))
        .map(|pending| {
            match wire
                .package_family_name
                .as_deref()
                .and_then(PackageFamilyName::parse)
            {
                Some(family) => pending.with_package_family_name(family),
                None => pending,
            }
        })
    }
}

/// Data-validation failure while building a [`PendingRestore`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingRestoreBuildError {
    /// The record does not use the current recovery schema version.
    InvalidSchemaVersion,
    /// The persisted operation identifier is blank.
    InvalidOperationId,
    /// The persisted timestamp is not UTC RFC 3339 text.
    InvalidStartedAtUtc,
    /// A persisted `GeoId` is not a positive Windows value.
    InvalidGeoId,
    /// The persisted product identity is blank.
    MissingProductId,
    /// The selected installation backend is blank.
    MissingBackend,
}

#[derive(Deserialize, Serialize)]
struct PendingRestoreWire {
    schema_version: u32,
    operation_id: String,
    started_at_utc: String,
    original_region: i32,
    temporary_region: i32,
    product_id: String,
    backend: String,
    #[serde(default)]
    install_classification: InstallClassification,
    // Kept as text on the wire: the parsed form validates on the way back in.
    #[serde(default)]
    package_family_name: Option<String>,
    state: DurableOperationState,
}

impl From<&PendingRestore> for PendingRestoreWire {
    fn from(pending: &PendingRestore) -> Self {
        Self {
            schema_version: pending.schema_version,
            operation_id: pending.operation_id.as_str().to_owned(),
            started_at_utc: pending.started_at_utc.as_str().to_owned(),
            original_region: pending.original_region.value(),
            temporary_region: pending.temporary_region.value(),
            product_id: pending.product_id.clone(),
            backend: pending.backend.clone(),
            install_classification: pending.install_classification,
            package_family_name: pending
                .package_family_name
                .as_ref()
                .map(|family| family.as_str().to_owned()),
            state: pending.state,
        }
    }
}

/// Stable recovery milestones written by the operation state machine.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableOperationState {
    /// The recovery record was published before the region write.
    Prepared,
    /// Read-back confirmed the temporary region.
    RegionSwitched,
    /// A backend accepted the installation request.
    InstallRequested,
    /// Completion evidence is insufficient for automatic restoration.
    CompletionUncertain,
    /// The original region came back while the installation was still running.
    ///
    /// The record survives this milestone on purpose. The region is safe, but
    /// the installation has no outcome yet, and a restart must not read a safe
    /// region as a finished operation.
    RegionRestoredEarly,
    /// A verified restoration attempt is in progress.
    RestoringRegion,
    /// Read-back confirmed the original region.
    Restored,
}

impl DurableOperationState {
    /// Whether the operation still owes an outcome even with the region back.
    ///
    /// Only one milestone can hold both at once. `RegionRestoredEarly` says the
    /// original region was read back while the installation kept running, so a
    /// later reader that sees the safe region has learnt nothing about how the
    /// installation ended. `Restored` looks the same from the outside but is
    /// written after the observation is over, which is what makes it final.
    #[must_use]
    pub const fn outcome_is_still_open(self) -> bool {
        matches!(self, Self::RegionRestoredEarly)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::InstallClassification;
    use crate::recovery::store::{RecoverySchemaError, RecoveryStoreError};
    use crate::test_support::pending_restore_for_test;

    #[test]
    fn pending_restore_round_trips_complete_utf8_json() {
        let pending = pending_restore_for_test();

        let bytes = pending.to_json_bytes().expect("serialize pending record");
        let restored = PendingRestore::from_json_bytes(&bytes).expect("parse pending record");

        assert_eq!(restored, pending);
        assert!(std::str::from_utf8(&bytes).is_ok());
    }

    #[test]
    fn pending_restore_persists_classification_and_old_records_remain_unknown() {
        let pending = pending_restore_for_test()
            .with_install_classification(InstallClassification::win32_from_backend_report());
        let bytes = pending
            .to_json_bytes()
            .expect("serialize classified record");
        let restored = PendingRestore::from_json_bytes(&bytes).expect("parse classified record");

        assert_eq!(
            restored.install_classification(),
            pending.install_classification()
        );
        assert!(
            std::str::from_utf8(&bytes)
                .expect("UTF-8 JSON")
                .contains("win32_reported_by_backend")
        );

        let old_record = format!(
            r#"{{"schema_version":{PENDING_RESTORE_SCHEMA_VERSION},"operation_id":"old-record","started_at_utc":"2026-08-15T12:34:56Z","original_region":244,"temporary_region":840,"product_id":"p","backend":"b","state":"prepared"}}"#
        );
        let restored_old = PendingRestore::from_json_bytes(old_record.as_bytes())
            .expect("old record without classification remains compatible");
        assert_eq!(
            restored_old.install_classification(),
            InstallClassification::unknown()
        );
    }

    #[test]
    fn pending_restore_replaces_only_the_durable_state() {
        let prepared = pending_restore_for_test();
        let switched = prepared.with_state(DurableOperationState::RegionSwitched);

        assert_eq!(prepared.state(), DurableOperationState::Prepared);
        assert_eq!(switched.state(), DurableOperationState::RegionSwitched);
        assert_eq!(switched.operation_id(), prepared.operation_id());
        assert_eq!(switched.original_region(), prepared.original_region());
        assert_eq!(switched.temporary_region(), prepared.temporary_region());
    }

    #[test]
    fn pending_restore_rejects_malformed_and_unknown_schema_json() {
        assert_eq!(
            PendingRestore::from_json_bytes(b"{"),
            Err(RecoveryStoreError::Schema(
                RecoverySchemaError::MalformedJson
            ))
        );
        let unknown_schema = format!(
            r#"{{"schema_version":{},"operation_id":"recovery-test","started_at_utc":"2026-08-15T12:34:56Z","original_region":244,"temporary_region":840,"product_id":"p","backend":"b","state":"prepared"}}"#,
            PENDING_RESTORE_SCHEMA_VERSION + 1
        );

        assert_eq!(
            PendingRestore::from_json_bytes(unknown_schema.as_bytes()),
            Err(RecoveryStoreError::Schema(
                RecoverySchemaError::UnknownSchema(PENDING_RESTORE_SCHEMA_VERSION + 1)
            ))
        );
        assert_eq!(
            PendingRestore::from_json_bytes(br#"{"schema_version":1}"#),
            Err(RecoveryStoreError::Schema(
                RecoverySchemaError::IncompleteRecord
            ))
        );
    }
}
