//! Deterministic fakes shared by the module test suites.

use crate::input::StoreProductId;
use crate::install::{
    DetachOrCancelOutcome, InstallAttemptId, InstallBackend, InstallBackendCapability,
    InstallBackendError, InstallBackendKind, InstallClassification, InstallHandle,
    InstallObservation, InstallRequest,
};
use crate::machine::OperationStatePersistence;
use crate::recovery::record::{DurableOperationState, PendingRestore};
use crate::recovery::store::{RecoveryStore, RecoveryStoreError};
use crate::region::{
    GeoId, MarketCode, OperationId, OperationIdGenerator, PreparedRegionChange, Region,
    RegionChangeGuard, RegionReadError, RegionReader, RegionWriteError, RegionWriter,
};
use crate::resolve::{
    ProductResolver, ResolveFailure, ResolveRequest, ResolveRequestId, ResolverProduct,
    ResolverSource, resolve_product,
};
use crate::store_page::{StorePageOpenError, StorePageOpener, StorePageRequest};
use crate::time::UtcTimestamp;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TestGuardError {
    Save,
    Cleanup,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TestPersistenceError {
    Write,
}

#[derive(Default)]
pub(crate) struct RecordingPersistence {
    pub(crate) states: Vec<DurableOperationState>,
    pub(crate) fail_next: bool,
}

impl OperationStatePersistence for RecordingPersistence {
    type Error = TestPersistenceError;

    fn persist_state(&mut self, state: DurableOperationState) -> Result<(), Self::Error> {
        if self.fail_next {
            self.fail_next = false;
            return Err(TestPersistenceError::Write);
        }
        self.states.push(state);
        Ok(())
    }
}

pub(crate) struct SequenceReader {
    pub(crate) values: RefCell<Vec<Result<Region, RegionReadError>>>,
}

impl SequenceReader {
    pub(crate) fn from_geo_ids(geo_ids: impl IntoIterator<Item = GeoId>) -> Self {
        Self {
            values: RefCell::new(
                geo_ids
                    .into_iter()
                    .map(|geo_id| {
                        Ok(Region::new(geo_id, "Test region", None)
                            .expect("test regions have a display name"))
                    })
                    .collect(),
            ),
        }
    }
}

impl RegionReader for SequenceReader {
    fn current_region(&self) -> Result<Region, RegionReadError> {
        self.values.borrow_mut().remove(0)
    }
}

pub(crate) struct TestWriter {
    pub(crate) result: Result<(), RegionWriteError>,
    pub(crate) events: Rc<RefCell<Vec<&'static str>>>,
}

impl RegionWriter for TestWriter {
    fn set_region(&self, _target: GeoId) -> Result<(), RegionWriteError> {
        self.events.borrow_mut().push("write");
        self.result
    }
}

pub(crate) struct TestOperationIds;

impl OperationIdGenerator for TestOperationIds {
    fn next_operation_id(&mut self) -> OperationId {
        OperationId::new("test-operation").expect("non-empty test identifier")
    }
}

pub(crate) struct TestGuard {
    pub(crate) save_result: Result<(), TestGuardError>,
    pub(crate) cleanup_result: Result<(), TestGuardError>,
    pub(crate) events: Rc<RefCell<Vec<&'static str>>>,
}

impl RegionChangeGuard for TestGuard {
    type Error = TestGuardError;

    fn save_prepared(&mut self, _change: &PreparedRegionChange) -> Result<(), Self::Error> {
        self.events.borrow_mut().push("save");
        self.save_result
    }

    fn clear_after_original_read_back(
        &mut self,
        _change: &PreparedRegionChange,
    ) -> Result<(), Self::Error> {
        self.events.borrow_mut().push("cleanup");
        self.cleanup_result
    }
}

#[derive(Default)]
pub(crate) struct MemoryRecoveryStore {
    pub(crate) pending: Option<PendingRestore>,
}

impl RecoveryStore for MemoryRecoveryStore {
    fn publish(&mut self, pending: &PendingRestore) -> Result<(), RecoveryStoreError> {
        self.pending = Some(pending.clone());
        Ok(())
    }

    fn load(&self) -> Result<Option<PendingRestore>, RecoveryStoreError> {
        Ok(self.pending.clone())
    }

    fn clear_verified(&mut self, pending: &PendingRestore) -> Result<(), RecoveryStoreError> {
        if self.pending.as_ref() != Some(pending) {
            return Err(RecoveryStoreError::Verification);
        }

        self.pending = None;
        Ok(())
    }
}

pub(crate) struct FakeResolver {
    pub(crate) result: Result<ResolverProduct, ResolveFailure>,
}

impl ProductResolver for FakeResolver {
    fn resolve(&self, _request: &ResolveRequest) -> Result<ResolverProduct, ResolveFailure> {
        self.result.clone()
    }
}

pub(crate) struct FakeInstallBackend {
    pub(crate) kind: InstallBackendKind,
    pub(crate) capability: InstallBackendCapability,
    pub(crate) observation: InstallObservation,
    pub(crate) detach_outcome: DetachOrCancelOutcome,
    /// Whether an install of the requested product is still in flight.
    pub(crate) resumable: bool,
}

impl InstallBackend for FakeInstallBackend {
    fn kind(&self) -> InstallBackendKind {
        self.kind
    }

    fn capability(&self, _request: &InstallRequest) -> InstallBackendCapability {
        self.capability
    }

    fn start_install(
        &self,
        _request: &InstallRequest,
    ) -> Result<InstallHandle, InstallBackendError> {
        match self.capability {
            InstallBackendCapability::CanInstall { .. } => Ok(InstallHandle::new(
                self.kind,
                InstallAttemptId::new("fake-attempt").expect("non-empty attempt ID"),
            )),
            InstallBackendCapability::StorePageOnly
            | InstallBackendCapability::UnsupportedInstallKind
            | InstallBackendCapability::Unavailable => Err(InstallBackendError::Unavailable),
        }
    }

    fn resume(
        &self,
        _request: &InstallRequest,
    ) -> Result<Option<InstallHandle>, InstallBackendError> {
        Ok(self.resumable.then(|| {
            InstallHandle::new(
                self.kind,
                InstallAttemptId::new("fake-attempt").expect("non-empty attempt ID"),
            )
        }))
    }

    fn observe(&self, handle: &InstallHandle) -> Result<InstallObservation, InstallBackendError> {
        (handle.backend() == self.kind)
            .then_some(self.observation)
            .ok_or(InstallBackendError::InvalidHandle)
    }

    fn detach_or_cancel(
        &self,
        handle: &InstallHandle,
    ) -> Result<DetachOrCancelOutcome, InstallBackendError> {
        (handle.backend() == self.kind)
            .then_some(self.detach_outcome)
            .ok_or(InstallBackendError::InvalidHandle)
    }
}

pub(crate) struct FakeStorePageOpener {
    pub(crate) result: Result<(), StorePageOpenError>,
    pub(crate) opened_uri: RefCell<Option<String>>,
}

impl StorePageOpener for FakeStorePageOpener {
    fn open_store_page(&self, request: &StorePageRequest) -> Result<(), StorePageOpenError> {
        *self.opened_uri.borrow_mut() = Some(request.uri());
        self.result
    }
}

pub(crate) fn resolver_request() -> ResolveRequest {
    ResolveRequest {
        request_id: ResolveRequestId::new(41),
        product_id: StoreProductId::parse("9WZDNCRFJ3PZ").expect("valid product ID"),
        market: MarketCode::parse("us").expect("valid market"),
    }
}

pub(crate) fn resolver_product(request: &ResolveRequest) -> ResolverProduct {
    ResolverProduct {
        request_id: request.request_id,
        product_id: request.product_id.clone(),
        market: request.market.clone(),
        market_applied: true,
        display_name: "Test application".to_owned(),
        package_family_name: None,
        publisher: None,
        icon: None,
        install_classification: InstallClassification::unknown(),
        version: None,
        size_bytes: None,
        resolver_source: ResolverSource::WinGetMsStore,
    }
}

pub(crate) fn install_request_for(classification: InstallClassification) -> InstallRequest {
    let request = resolver_request();
    let mut response = resolver_product(&request);
    response.install_classification = classification;
    let product = resolve_product(
        &FakeResolver {
            result: Ok(response),
        },
        &request,
    )
    .expect("resolved product for backend request");
    InstallRequest::from_resolved_product(&product)
}

pub(crate) fn geo_id(value: i32) -> GeoId {
    GeoId::new(value).expect("positive test GeoId")
}

pub(crate) fn pending_restore_for(operation_id: &str) -> PendingRestore {
    PendingRestore::prepared(
        OperationId::new(operation_id).expect("non-empty operation identifier"),
        UtcTimestamp::parse("2026-08-15T12:34:56.123Z").expect("valid UTC timestamp"),
        geo_id(244),
        geo_id(840),
        "9WZDNCRFJ3PZ",
        "test-backend",
    )
    .expect("complete recovery record")
}

pub(crate) fn pending_restore_for_test() -> PendingRestore {
    pending_restore_for("recovery-test")
}
