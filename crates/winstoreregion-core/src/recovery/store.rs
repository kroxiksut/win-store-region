//! Durable storage boundary for the one recovery record.

use crate::machine::OperationStatePersistence;
use crate::recovery::record::{DurableOperationState, PendingRestore};
use crate::region::{PreparedRegionChange, RegionChangeGuard};

/// A structured failure while reading, publishing, validating, or cleaning recovery state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryStoreError {
    /// The published recovery file could not be read.
    Read { os_error: Option<i32> },
    /// The temporary recovery file could not be written or synced.
    Write { os_error: Option<i32> },
    /// Windows did not publish the temporary file as the recovery file.
    Publish { os_error: Option<i32> },
    /// The published record did not match the record just written.
    Verification,
    /// JSON serialization unexpectedly failed before an I/O attempt.
    Serialization,
    /// The record is malformed, incomplete, or uses an unsupported schema.
    Schema(RecoverySchemaError),
    /// A validated record could not be safely removed.
    Cleanup { os_error: Option<i32> },
    /// A guard was paired with a different recovery record.
    RecordMismatch,
}

/// Why a recovery JSON document is unsafe to use automatically.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoverySchemaError {
    /// The bytes are not valid JSON.
    MalformedJson,
    /// A mandatory field is absent, invalid, or empty.
    IncompleteRecord,
    /// Another application version owns this schema version.
    UnknownSchema(u32),
}

/// Crash-safe storage for the one published pending recovery record.
pub trait RecoveryStore {
    /// Publish a complete record, replacing the previous record only after durability checks.
    ///
    /// # Errors
    ///
    /// Returns a structured write, publish, schema, or verification failure.
    fn publish(&mut self, pending: &PendingRestore) -> Result<(), RecoveryStoreError>;

    /// Read and validate the published record without treating temporary files as recovery state.
    ///
    /// # Errors
    ///
    /// Returns a structured read or schema failure. A missing published file is
    /// represented by `Ok(None)`.
    fn load(&self) -> Result<Option<PendingRestore>, RecoveryStoreError>;

    /// Remove one validated record after the caller has confirmed the original region.
    ///
    /// # Errors
    ///
    /// Returns a structured read, verification, schema, or cleanup failure.
    fn clear_verified(&mut self, pending: &PendingRestore) -> Result<(), RecoveryStoreError>;
}

/// Connects a complete [`PendingRestore`] to the 4.2 region-switch protocol.
pub struct PendingRestoreGuard<S> {
    store: S,
    pending: PendingRestore,
}

impl<S> PendingRestoreGuard<S> {
    /// Create a guard for the one complete recovery record of an operation.
    #[must_use]
    pub fn new(store: S, pending: PendingRestore) -> Self {
        Self { store, pending }
    }

    /// Return the current in-memory durable snapshot.
    #[must_use]
    pub fn pending(&self) -> &PendingRestore {
        &self.pending
    }

    /// Publish a complete replacement with a later durable state.
    ///
    /// # Errors
    ///
    /// Returns the recovery-store error and retains the prior in-memory state
    /// if publication or verification fails.
    pub fn update_state(&mut self, state: DurableOperationState) -> Result<(), RecoveryStoreError>
    where
        S: RecoveryStore,
    {
        let updated = self.pending.with_state(state);
        self.store.publish(&updated)?;
        self.pending = updated;
        Ok(())
    }

    /// Publish a complete replacement record for the same operation.
    ///
    /// Used when the operation learns something the record must carry, such as
    /// the package identity it will wait for. The in-memory copy changes only
    /// after the store confirms what it wrote.
    ///
    /// # Errors
    ///
    /// Returns the recovery-store error and keeps the prior record if
    /// publication or verification fails.
    pub fn replace_record(&mut self, replacement: PendingRestore) -> Result<(), RecoveryStoreError>
    where
        S: RecoveryStore,
    {
        self.store.publish(&replacement)?;
        self.pending = replacement;
        Ok(())
    }

    /// Consume the guard and return its storage adapter.
    #[must_use]
    pub fn into_store(self) -> S {
        self.store
    }
}

impl<S> RegionChangeGuard for PendingRestoreGuard<S>
where
    S: RecoveryStore,
{
    type Error = RecoveryStoreError;

    fn save_prepared(&mut self, change: &PreparedRegionChange) -> Result<(), Self::Error> {
        if self.pending.state() != DurableOperationState::Prepared
            || !self.pending.matches_change(change)
        {
            return Err(RecoveryStoreError::RecordMismatch);
        }
        self.store.publish(&self.pending)
    }

    fn clear_after_original_read_back(
        &mut self,
        change: &PreparedRegionChange,
    ) -> Result<(), Self::Error> {
        if !self.pending.matches_change(change) {
            return Err(RecoveryStoreError::RecordMismatch);
        }
        self.store.clear_verified(&self.pending)
    }
}

impl<S> OperationStatePersistence for PendingRestoreGuard<S>
where
    S: RecoveryStore,
{
    type Error = RecoveryStoreError;

    fn persist_state(&mut self, state: DurableOperationState) -> Result<(), Self::Error> {
        self.update_state(state)
    }
}
