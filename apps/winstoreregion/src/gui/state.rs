//! Presentation state, window context, and their lifetime guards.

use crate::gui::handoff::HandoffStage;
use crate::gui::render::source_status;
use crate::gui::strings::{Language, TextDirection};
use crate::platform::installer_download::{DownloadedInstaller, InstallerDownloadError};
use crate::platform::region::Win32RegionReader;
use crate::platform::storage::JournalStoreError;
use crate::platform::stub::StubInspectionError;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HINSTANCE, HWND};
use windows::Win32::Graphics::Gdi::{DeleteObject, HBRUSH};
use windows::Win32::System::Ole::{IDropTarget, OleUninitialize};
use windows::Win32::UI::WindowsAndMessaging::HMENU;
use winstoreregion_core::{
    ApplicationSourceDraft, ApplicationSourceKind, AvailabilitySurvey, DeviceCompatibility,
    Diagnostic, GeoId, GuiLaunch, HandoffFileIdentity, InstallPhase, InstallProgress,
    InstallationOperationState, JournalRecord, MarketAnswer, MarketCode, OfferedProduct,
    PendingRestore, PrerequisiteReport, ProductResolutionSession, ProductResolutionState, Region,
    RegionReadError, ResolveFailure, ResolveRequest, ResolvedProduct, StoreInputCandidate,
    StoreProductId, StubInspection, SurveyScope, UpdateCandidate, admit_installer_handoff,
};

/// A modal call that is on the stack right now, and what arrived meanwhile.
///
/// A modal dialog runs a message loop of its own, so a result posted by a
/// worker thread is dispatched into the window procedure while an outer handler
/// still holds `&mut AppState`. Taking that borrow a second time is undefined
/// behaviour, so those messages are set aside here and posted again once the
/// dialog is gone. Nothing is lost and nothing is handled twice: the queue is
/// drained exactly once, when the outermost modal call returns.
///
/// Thread-local because the whole presentation layer lives on one thread: the
/// window, its message loop, and every modal call are the same thread by
/// construction.
mod modal {
    use std::cell::{Cell, RefCell};
    use std::ffi::c_void;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

    thread_local! {
        /// How many modal calls are on the stack; dialogs can open dialogs.
        static DEPTH: Cell<usize> = const { Cell::new(0) };
        /// Window, message, and payload of everything held back so far.
        static HELD: RefCell<Vec<(usize, u32, usize)>> = const { RefCell::new(Vec::new()) };
    }

    /// Whether an outer handler is inside a modal call right now.
    pub(in crate::gui) fn call_is_active() -> bool {
        DEPTH.with(Cell::get) > 0
    }

    /// Hold one posted message back until the modal call is over.
    pub(in crate::gui) fn hold(window: HWND, message: u32, wparam: WPARAM) {
        HELD.with(|held| {
            held.borrow_mut()
                .push((window.0 as usize, message, wparam.0));
        });
    }

    /// Marks a modal call for as long as this value is alive.
    ///
    /// Held by the function that owns the dialog, so the mark disappears on
    /// every path out of it, including a panic.
    pub(in crate::gui) struct Scope;

    impl Scope {
        pub(in crate::gui) fn enter() -> Self {
            DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
            Self
        }
    }

    impl Drop for Scope {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            let depth = DEPTH.with(|depth| {
                let next = depth.get().saturating_sub(1);
                depth.set(next);
                next
            });
            if depth > 0 {
                return;
            }
            // Back in the ordinary message loop: everything held back goes into
            // the queue again and is dispatched normally. A post can only fail
            // for a window that no longer exists, and by then the process is on
            // its way out.
            for (window, message, wparam) in HELD.with(RefCell::take) {
                let _ = unsafe {
                    PostMessageW(
                        Some(HWND(window as *mut c_void)),
                        message,
                        WPARAM(wparam),
                        LPARAM(0),
                    )
                };
            }
        }
    }
}

pub(super) use modal::Scope as ModalScope;
pub(super) use modal::{call_is_active as modal_call_is_active, hold as hold_until_modal_ends};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Tab {
    Installation,
    Journal,
    Updates,
}

impl Tab {
    pub(super) const fn from_index(index: isize) -> Option<Self> {
        match index {
            0 => Some(Self::Installation),
            1 => Some(Self::Journal),
            2 => Some(Self::Updates),
            _ => None,
        }
    }

    pub(super) const fn index(self) -> usize {
        match self {
            Self::Installation => 0,
            Self::Journal => 1,
            Self::Updates => 2,
        }
    }
}

/// What the Updates tab knows right now.
pub(super) enum UpdatesView {
    /// Nothing has been asked yet. The scan costs network, so it is not
    /// started by merely opening the tab.
    Idle,
    /// A scan is running; the counters are what it has finished.
    Scanning { done: usize, total: usize },
    /// The scan finished. An empty list is an answer, not a failure.
    Ready(Vec<UpdateCandidate>),
    /// Windows would not say what is installed.
    Unavailable,
}

pub(super) enum JournalView {
    Loading,
    Ready(Vec<JournalRecord>),
    Failed(JournalStoreError),
}

/// What this window knows about the operation it started.
///
/// Held only while the operation runs. The thread owns the work; this is the
/// presentation copy of its latest report.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct RunningOperation {
    /// The resolve this operation opened, awaiting its card.
    pub(super) resolve: Option<ResolveRequest>,
    /// Phase last reported by the backend.
    pub(super) phase: Option<InstallPhase>,
    /// Percentage last reported by the backend.
    pub(super) progress: Option<InstallProgress>,
    /// Whether the original region is already back in place.
    pub(super) region_restored_early: bool,
    /// Name of the product as resolved under the temporary region.
    pub(super) product_name: Option<String>,
}

/// What this window knows about the handoff it started.
///
/// Held from the moment the operation begins until the region is confirmed
/// back. A handoff has no progress to show, so this carries only the facts the
/// window has to state plainly: which file went over, and which region is
/// waiting to be put back.
pub(super) struct HandoffProgress {
    pub(super) stage: HandoffStage,
    /// The file that was handed over, once the gate admitted one.
    pub(super) file: Option<HandoffFileIdentity>,
    /// The region this operation will put back, once it has read it.
    pub(super) original_region: Option<GeoId>,
    /// The region the operation switched to.
    pub(super) temporary_region: Option<Region>,
}

pub(super) struct AppState {
    pub(super) language: Language,
    pub(super) selected_tab: Tab,
    pub(super) application_source: ApplicationSourceDraft,
    pub(super) resolution: ProductResolutionSession,
    pub(super) installation_backend_confirmed: bool,
    /// Whether startup recovery left a new guarded operation admissible.
    ///
    /// It stays `false` until recovery has actually been inspected, so a window
    /// that never reached that inspection cannot offer an installation.
    pub(super) recovery_admits_new_operation: bool,
    /// What the machine offers for an installation.
    ///
    /// Starts unchecked, so a window that never reached the check cannot offer
    /// an installation.
    pub(super) prerequisites: PrerequisiteReport,
    pub(super) temporary_regions: Vec<Region>,
    pub(super) temporary_region_input: String,
    pub(super) selected_temporary_region: Option<Region>,
    pub(super) stub_inspection: Option<StubInspection>,
    /// Why the selected file could not be inspected, when it could not.
    ///
    /// Held beside the inspection rather than folded into it: an absent
    /// inspection means the check is still running, and a check that failed is
    /// a different thing to say.
    pub(super) stub_inspection_error: Option<StubInspectionError>,
    pub(super) journal: JournalView,
    pub(super) selected_journal_index: Option<usize>,
    /// Read-only lifecycle fact supplied by the core orchestrator when one exists.
    pub(super) operation_state: Option<InstallationOperationState>,
    pub(super) operation_status: String,
    /// State of the operation this window started, while it is still running.
    pub(super) running_operation: Option<RunningOperation>,
    /// Why the last operation could go no further, when it could not.
    pub(super) operation_failure: Option<Diagnostic>,
    pub(super) current_region: Option<std::result::Result<Region, RegionReadError>>,
    /// What is known about where this product is offered, before any region changes.
    pub(super) availability: Option<MarketSurvey>,
    /// State of the handoff this window started, until its region is back.
    pub(super) handoff: Option<HandoffProgress>,
    /// Whether the region list shows every nation despite a narrowing survey.
    pub(super) show_all_regions: bool,
    /// Identifies survey passes so a superseded one's answers are discarded.
    pub(super) survey_passes: u64,
    /// The product whose Store installer is being fetched right now, if any.
    pub(super) installer_download: Option<StoreProductId>,
    /// What the Updates tab has learned, and how far it got.
    pub(super) updates: UpdatesView,
    /// Which scan the window is showing.
    ///
    /// A scan cannot be stopped once its workers are asking the network, so a
    /// second one started over the first is told apart by this number instead:
    /// reports from the older scan arrive and are dropped.
    pub(super) updates_scan_generation: u64,
    /// Which entry of the updates list the user is looking at.
    pub(super) selected_update_index: Option<usize>,
    /// When the shown scan was taken, for a scan remembered from a past run.
    pub(super) updates_taken: Option<String>,
    /// What the catalogue said it would deliver, and what it was asked about.
    ///
    /// The subject is kept beside the verdict because a verdict about another
    /// product or market is not an answer about this one, and a stale refusal
    /// would block an installation that nothing refused.
    pub(super) device_compatibility: Option<(StoreProductId, MarketCode, DeviceCompatibility)>,
}

/// A market survey and how far it has got.
///
/// The survey is a preliminary reference: it narrows the region list and builds
/// a card ahead of time, and it never admits an installation. The guarded
/// operation resolves the product again under the region it actually holds.
pub(super) struct MarketSurvey {
    pub(super) survey: AvailabilitySurvey,
    /// Markets answered so far out of those this pass asked about.
    pub(super) answered: usize,
    pub(super) total: usize,
    /// Set while a pass is running, so a second one is not started over it.
    pub(super) running: bool,
    /// Whether a pass has asked more than the one market a card needs.
    ///
    /// A single-market answer is a card, not a search, and the window must not
    /// report it as one: it says nothing about anywhere else.
    pub(super) searched: bool,
    /// Markets that answered `Offered` as of the last pass that ended.
    ///
    /// The list shown to the user is built from this rather than from the
    /// answers arriving right now. A growing set would rebuild the region box on
    /// every find, and rebuilding closes the dropdown and repaints it — which is
    /// what a user sees as flicker. One pass costs one rebuild.
    settled_offering: Vec<MarketCode>,
    /// Cleared to ask a running pass to stop; it is never waited on.
    cancel: Arc<AtomicBool>,
    /// Identifies the pass whose answers this state accepts.
    ///
    /// A pass outlives the input that started it, so answers from a superseded
    /// pass arrive at a window that has moved on and must be discarded.
    pub(super) generation: u64,
}

impl MarketSurvey {
    pub(super) fn new(product_id: StoreProductId, generation: u64, total: usize) -> Self {
        Self {
            survey: AvailabilitySurvey::new(product_id),
            answered: 0,
            total,
            running: true,
            searched: total > 1,
            settled_offering: Vec::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            generation,
        }
    }

    /// Begin another pass over the same product, keeping the answers already given.
    pub(super) fn restart(mut self, generation: u64, total: usize) -> Self {
        self.answered = 0;
        self.total = total;
        self.running = true;
        self.searched = self.searched || total > 1;
        self.cancel = Arc::new(AtomicBool::new(false));
        self.generation = generation;
        self
    }

    /// Take the answers of a pass that has ended into the list shown.
    ///
    /// Called once per pass, which is what keeps the region box from being
    /// rebuilt on every arriving answer.
    pub(super) fn settle(&mut self) {
        self.settled_offering = self.survey.offering_markets();
    }

    /// A flag a worker reads to learn that nobody wants its answers any more.
    pub(super) fn cancellation(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    /// Ask the running pass to stop.
    pub(super) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// The regions a search found, out of everything Windows knows.
    ///
    /// Only markets that answered `Offered` remain: a list still holding two
    /// hundred unasked nations would not be an answer to "where can this be
    /// installed". Nothing is hidden by that — the incomplete-set note, the
    /// second pass, and the show-all switch all lead back to the full list.
    ///
    /// A search that found nothing narrows nothing: an empty list would leave
    /// the user nowhere to go.
    pub(super) fn narrows(&self, regions: &[Region]) -> Option<Vec<Region>> {
        if !self.searched {
            return None;
        }
        let offering = &self.settled_offering;
        if offering.is_empty() {
            return None;
        }
        let narrowed: Vec<Region> = regions
            .iter()
            .filter(|region| {
                MarketCode::from_region(region).is_ok_and(|market| offering.contains(&market))
            })
            .cloned()
            .collect();
        (!narrowed.is_empty()).then_some(narrowed)
    }

    /// The product one market offered, when that market is the selected one.
    pub(super) fn offer_in(&self, region: &Region) -> Option<&OfferedProduct> {
        let market = MarketCode::from_region(region).ok()?;
        self.survey.answer_for(&market)?.offered.as_ref()
    }
}

/// Temporary region offered before the user picks one.
///
/// The United States is the market a region-gated Store product is most often
/// offered in, so it is the answer that saves the most people a search through
/// every nation Windows knows. It is a starting point, not a decision: the
/// list stays free to change.
const DEFAULT_TEMPORARY_GEO_ID: i32 = 244;

impl AppState {
    pub(super) fn new(launch: GuiLaunch) -> Self {
        let language = Language::Russian;
        let initial_input = launch.initial_input.unwrap_or_default();
        let application_source = ApplicationSourceDraft::new(initial_input);
        let mut state = Self {
            language,
            selected_tab: Tab::Installation,
            application_source,
            resolution: ProductResolutionSession::default(),
            installation_backend_confirmed: false,
            recovery_admits_new_operation: false,
            prerequisites: PrerequisiteReport::unchecked(),
            temporary_regions: Win32RegionReader::available_national_regions(),
            temporary_region_input: String::new(),
            selected_temporary_region: None,
            // Filled in below from the region list this state was built with.
            stub_inspection: None,
            stub_inspection_error: None,
            journal: JournalView::Loading,
            selected_journal_index: None,
            operation_state: None,
            operation_status: language.strings().status.to_owned(),
            running_operation: None,
            operation_failure: None,
            current_region: None,
            availability: None,
            handoff: None,
            show_all_regions: false,
            survey_passes: 0,
            device_compatibility: None,
            installer_download: None,
            updates: UpdatesView::Idle,
            updates_scan_generation: 0,
            selected_update_index: None,
            updates_taken: None,
        };
        state.selected_temporary_region = state
            .temporary_regions
            .iter()
            .find(|region| region.geo_id.value() == DEFAULT_TEMPORARY_GEO_ID)
            .cloned();
        if let Ok(candidate) = StoreInputCandidate::parse(state.application_source.store_text()) {
            state
                .resolution
                .accept_syntax(candidate.product_id().clone());
        }
        state.operation_status = source_status(&state);
        state
    }

    /// Whether the install command may be offered at all.
    ///
    /// Pending recovery and missing prerequisites are hard gates: each blocks a
    /// new operation on its own, regardless of how complete the draft is.
    /// What the catalogue said about this exact product in this exact market.
    ///
    /// A verdict recorded for anything else is not an answer to the question
    /// being asked now, and silence is the default: nothing is refused on a
    /// question nobody answered.
    /// Whether this is the exact question the window is asking right now.
    ///
    /// An answer to anything else belongs to a product or region the user has
    /// since moved on from. It is not merely useless: stored, it would replace
    /// the answer that is current with one nothing will ask for again.
    pub(super) fn asks_about(&self, product_id: &StoreProductId, market: &MarketCode) -> bool {
        let (Some(current), Some(region)) = (
            self.accepted_product_id(),
            self.selected_temporary_region.as_ref(),
        ) else {
            return false;
        };
        *current == *product_id
            && MarketCode::from_region(region).is_ok_and(|current| current == *market)
    }

    pub(super) fn device_verdict(&self) -> DeviceCompatibility {
        let Some((asked_product, asked_market, verdict)) = self.device_compatibility.as_ref()
        else {
            return DeviceCompatibility::Unknown;
        };
        let (Some(product_id), Some(region)) = (
            self.accepted_product_id(),
            self.selected_temporary_region.as_ref(),
        ) else {
            return DeviceCompatibility::Unknown;
        };
        let Ok(market) = MarketCode::from_region(region) else {
            return DeviceCompatibility::Unknown;
        };
        if asked_product == product_id && *asked_market == market {
            *verdict
        } else {
            DeviceCompatibility::Unknown
        }
    }

    pub(super) fn install_is_available(&self) -> bool {
        self.recovery_admits_new_operation
            // a product the catalogue will not deliver to
            // this device fails after the region has already been switched. The
            // region is not touched for an answer that is already known.
            && !self.device_verdict().blocks_installation()
            && !self.handoff_blocks_new_operation()
            && self.prerequisites.admits_installation()
            // A parsed identifier, not a card: the catalogue is regional at
            // search time, so a product offered only elsewhere cannot be
            // resolved before its region is in force.
            && self.accepted_product_id().is_some()
            // Without a chosen temporary region there is nothing to switch to.
            && self.selected_temporary_region.is_some()
            && self.running_operation.is_none()
    }

    /// Whether a handoff this window started still owns the machine.
    ///
    /// A handoff that has not put the region back leaves a published recovery
    /// record, and startup recovery only ran once, at startup. Without this the
    /// window would offer a second operation over the first one's record.
    pub(super) fn handoff_blocks_new_operation(&self) -> bool {
        self.handoff
            .as_ref()
            .is_some_and(|handoff| handoff.stage.blocks_new_operation())
    }

    /// Whether the handoff command may be offered at all.
    ///
    /// The gates match the installation ones except for two facts: the source
    /// is a file rather than text, and the machine only has to have Microsoft
    /// Store, because nothing here asks a backend.
    pub(super) fn handoff_is_available(&self) -> bool {
        self.application_source.selected() == ApplicationSourceKind::InstallerFile
            && self.recovery_admits_new_operation
            && !self.handoff_blocks_new_operation()
            && self.prerequisites.admits_handoff()
            && self.admitted_installer_file().is_some()
            && self.selected_temporary_region.is_some()
            && self.running_operation.is_none()
    }

    /// Whether the primary command has anything to start at all.
    pub(super) fn primary_command_is_available(&self) -> bool {
        self.install_is_available() || self.handoff_is_available()
    }

    /// Whether a second attempt through the Store installer may be offered.
    ///
    /// Only after an operation through the backend that installed nothing, and
    /// only for a link or Product ID: a file source is already this path. The
    /// download itself needs no region, so nothing about the region is required
    /// here — only a product to ask about.
    ///
    /// A device the catalogue refuses is not offered a second way to fail.
    pub(super) fn store_installer_download_is_available(&self) -> bool {
        self.running_operation.is_none()
            && self.installer_download.is_none()
            && !self.region_restore_is_available()
            && self.application_source.selected() == ApplicationSourceKind::StoreText
            && self.accepted_product_id().is_some()
            && !self.device_verdict().blocks_installation()
            && (self.operation_failure.is_some()
                || self.operation_state == Some(InstallationOperationState::CompletionUncertain))
    }

    /// The updates entry the user is looking at, when the scan produced one.
    pub(super) fn selected_update(&self) -> Option<&UpdateCandidate> {
        let UpdatesView::Ready(candidates) = &self.updates else {
            return None;
        };
        candidates.get(self.selected_update_index?)
    }

    /// Whether the region can be put back from the window right now.
    pub(super) fn region_restore_is_available(&self) -> bool {
        self.handoff
            .as_ref()
            .is_some_and(|handoff| handoff.stage.offers_region_restore())
    }

    /// The inspected file core would allow to run, when there is one.
    ///
    /// Core owns the rule; this only asks it about the inspection the window is
    /// currently showing, so a window can never widen the gate by itself.
    pub(super) fn admitted_installer_file(&self) -> Option<HandoffFileIdentity> {
        let inspection = self.stub_inspection.as_ref()?;
        // The inspection must still be about the file the user has selected.
        let selected = self.application_source.installer_file()?;
        if inspection.file_identity.path != selected.as_path() {
            return None;
        }
        admit_installer_handoff(inspection).ok()
    }

    /// The identifier the region search may use, whichever source is active.
    ///
    /// The search is a reference and nothing more: it installs nothing, admits
    /// nothing, and never opens the install gate. That is what makes it safe to
    /// answer from the preserved link draft while a file is the chosen source —
    /// a file names no product, so the only identifier that exists then is the
    /// one the user typed.
    ///
    /// What it cannot do is tie that identifier to the file. That the two are
    /// the same application is the user's claim, and the window says so rather
    /// than presenting the found regions as a fact about the file.
    pub(super) fn searchable_product_id(&self) -> Option<StoreProductId> {
        if let Some(product_id) = self.accepted_product_id() {
            return Some(product_id.clone());
        }
        StoreInputCandidate::parse(self.application_source.store_text())
            .ok()
            .map(|candidate| candidate.product_id().clone())
    }

    /// The exact identifier the user supplied, once it parses.
    ///
    /// This is what an operation needs before the region changes. A card is not
    /// required: under the home region a product offered elsewhere cannot be
    /// resolved at all, which is the case the product exists for.
    pub(super) fn accepted_product_id(&self) -> Option<&StoreProductId> {
        match self.resolution.state() {
            ProductResolutionState::SyntaxAccepted { product_id } => Some(product_id),
            ProductResolutionState::Resolving { request }
            | ProductResolutionState::Failed { request, .. } => Some(&request.product_id),
            ProductResolutionState::Resolved { product } => Some(&product.product_id),
            ProductResolutionState::Idle => None,
        }
    }

    pub(super) fn resolved_product(&self) -> Option<&ResolvedProduct> {
        match self.resolution.state() {
            ProductResolutionState::Resolved { product } => Some(product),
            _ => None,
        }
    }

    /// Regions the window offers, narrowed by a survey unless the user opted out.
    pub(super) fn offered_regions(&self) -> Vec<Region> {
        if self.show_all_regions {
            return self.temporary_regions.clone();
        }
        self.availability
            .as_ref()
            .and_then(|availability| availability.narrows(&self.temporary_regions))
            .unwrap_or_else(|| self.temporary_regions.clone())
    }

    /// Whether a survey is currently hiding regions from the list.
    pub(super) fn regions_are_narrowed(&self) -> bool {
        !self.show_all_regions && self.offered_regions().len() < self.temporary_regions.len()
    }

    /// The product the selected region's market offers, when one was found.
    pub(super) fn offered_product(&self) -> Option<&OfferedProduct> {
        let region = self.selected_temporary_region.as_ref()?;
        self.availability.as_ref()?.offer_in(region)
    }

    pub(super) fn selected_journal_record(&self) -> Option<&JournalRecord> {
        let JournalView::Ready(records) = &self.journal else {
            return None;
        };
        self.selected_journal_index
            .and_then(|index| records.get(index))
    }
}

pub(super) enum UiEvent {
    User(UiAction),
    StateUpdate(UiUpdate),
}

pub(super) enum UiAction {
    SelectLanguage(Language),
    SelectTab(Tab),
    SetStoreText(String),
    SelectTemporaryRegion(Option<Region>),
    SelectSource(ApplicationSourceKind),
}

/// A view-model update supplied by the future core state machine.
pub(super) enum UiUpdate {
    OperationStatus(String),
    // The production orchestrator is intentionally not connected until its
    // writer and backend gates are proven; this is its one-way UI boundary.
    #[allow(dead_code)]
    OperationState(InstallationOperationState),
    CurrentRegion(std::result::Result<Region, RegionReadError>),
    JournalLoaded(std::result::Result<Vec<JournalRecord>, JournalStoreError>),
}

/// What the startup checks found once they were done asking Windows.
///
/// They run together on one thread because they run once, at the same moment,
/// and none of them decides anything: each answers a question the window would
/// otherwise have asked while it was still blank.
pub(super) struct StartupChecksUpdate {
    pub(super) prerequisites: PrerequisiteReport,
    /// A scan remembered from a previous run, and when it was taken.
    pub(super) remembered_scan: Option<(String, Vec<UpdateCandidate>)>,
}

/// Whether the installation a recovery record names is still going.
pub(super) struct ResumeProbeUpdate {
    /// The record the question was asked about.
    pub(super) pending: PendingRestore,
    /// Set when the package manager still has that installation in flight.
    pub(super) resumable: bool,
    /// Set when the identity the record was waiting for is installed now.
    ///
    /// Read only once the installation is over: it is what turns a record left
    /// without an outcome into a history entry that can say "installed".
    pub(super) package_present: bool,
}

pub(super) struct StubInspectionUpdate {
    pub(super) path: PathBuf,
    pub(super) result: std::result::Result<StubInspection, StubInspectionError>,
}

pub(super) struct ProductResolutionUpdate {
    pub(super) request: ResolveRequest,
    pub(super) result: std::result::Result<ResolvedProduct, ResolveFailure>,
}

pub(super) struct JournalDeleteUpdate {
    pub(super) result: std::result::Result<(), JournalStoreError>,
}

/// One report from the scan behind the Updates tab.
pub(super) struct UpdatesScanUpdate {
    /// Which scan this report belongs to.
    pub(super) generation: u64,
    /// Candidates gathered so far, replacing what the window held.
    pub(super) candidates: Vec<UpdateCandidate>,
    /// How many applications have been asked about, out of how many.
    pub(super) done: usize,
    pub(super) total: usize,
    /// Set on the last report of a scan.
    pub(super) finished: bool,
    /// Set when Windows would not say what is installed.
    pub(super) unavailable: bool,
}

/// The Store installer this window asked Microsoft for, or why it did not come.
pub(super) struct InstallerDownloadUpdate {
    pub(super) result: std::result::Result<DownloadedInstaller, InstallerDownloadError>,
}

/// What the catalogue answered about delivering one product to this device.
pub(super) struct DeviceCompatibilityUpdate {
    pub(super) product_id: StoreProductId,
    pub(super) market: MarketCode,
    pub(super) compatibility: DeviceCompatibility,
}

/// Answers one market-survey pass has produced since the last report.
pub(super) struct MarketSurveyUpdate {
    /// The pass these answers belong to; a superseded one is discarded.
    pub(super) generation: u64,
    pub(super) answers: Vec<MarketAnswer>,
    /// Set on the last report of a pass, whether it finished or was stopped.
    pub(super) finished: bool,
    /// How much of the world this pass covered, once it finished.
    pub(super) scope: SurveyScope,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FileSelectionError {
    EmptyPath,
    UnsupportedExtension,
    NotAFile,
    LinkOrDevicePath,
    MultipleFiles,
}

pub(super) struct Controls {
    pub(super) brand_badge: HWND,
    pub(super) title: HWND,
    pub(super) subtitle: HWND,
    pub(super) language_label: HWND,
    pub(super) language: HWND,
    pub(super) tabs: HWND,
    pub(super) tab_content: HWND,
    pub(super) journal_list: HWND,
    pub(super) journal_details: HWND,
    pub(super) journal_open_store: HWND,
    pub(super) journal_repeat: HWND,
    pub(super) journal_delete: HWND,
    pub(super) journal_copy_id: HWND,
    pub(super) journal_clear: HWND,
    pub(super) updates_list: HWND,
    pub(super) updates_details: HWND,
    pub(super) updates_refresh: HWND,
    pub(super) updates_open: HWND,
    pub(super) source_panel: HWND,
    pub(super) source_title: HWND,
    pub(super) source_hint: HWND,
    pub(super) source_link: HWND,
    pub(super) source_file: HWND,
    pub(super) input_label: HWND,
    pub(super) input: HWND,
    pub(super) file_path: HWND,
    pub(super) source_action: HWND,
    pub(super) clear_file: HWND,
    pub(super) drop_overlay: HWND,
    pub(super) app_card: HWND,
    pub(super) region_panel: HWND,
    pub(super) current_region_label: HWND,
    pub(super) current_region: HWND,
    pub(super) temporary_region_label: HWND,
    pub(super) temporary_region: HWND,
    pub(super) find_region: HWND,
    pub(super) check_remaining: HWND,
    pub(super) show_all_regions: HWND,
    pub(super) availability_status: HWND,
    pub(super) status: HWND,
    /// Installation progress, shown only while an installation is running.
    pub(super) progress: HWND,
    pub(super) install: HWND,
    pub(super) restore: HWND,
    pub(super) open_store_page: HWND,
    pub(super) details: HWND,
}

/// Window resources that live for the whole window, separate from `AppState`.
///
/// A modal call re-enters the window procedure, so colouring, sizing, and
/// painting run while an outer handler still holds `&mut AppState`. Those
/// re-entrant messages read only this structure, and it lives in its own
/// allocation, so a shared borrow taken during a modal loop can never alias
/// that exclusive borrow. Nothing here may be mutated while a modal call is
/// possible: `controls`, `menu`, and `drop_target` are published once during
/// creation and taken back only in `WM_NCDESTROY`.
pub(super) struct WindowChrome {
    pub(super) instance: HINSTANCE,
    pub(super) app_icon: windows::Win32::UI::WindowsAndMessaging::HICON,
    pub(super) _single_instance: SingleInstanceGuard,
    pub(super) drop_target: Option<IDropTarget>,
    pub(super) brand_brush: HBRUSH,
    pub(super) status_brush: HBRUSH,
    /// Pen-coloured brush for the panel outlines drawn by the window itself.
    pub(super) frame_brush: HBRUSH,
    /// Interior mutability: the menu is replaced on a language change, which
    /// must not require an exclusive borrow of chrome.
    pub(super) menu: Cell<Option<HMENU>>,
    /// Text last written into each control, keyed by its window handle.
    ///
    /// Interior mutability for the same reason as the menu: rendering takes a
    /// shared borrow of chrome. Handles live as long as the window, so the map
    /// is bounded by the number of controls.
    pub(super) last_text: RefCell<std::collections::HashMap<usize, String>>,
    /// Client size and scaling the last layout was computed for.
    ///
    /// `layout_controls` reads nothing but those two values, so running it again
    /// for the same pair moves every control to where it already is and then
    /// invalidates the whole client area for nothing. A forty-market search
    /// reports each answer separately, and each report re-rendered the window:
    /// forty full repaints in a few seconds, which is exactly what a user sees
    /// as flicker.
    pub(super) last_layout: Cell<Option<(i32, i32, i32)>>,
    /// Regions currently listed in the temporary-region box, by `GeoId`.
    ///
    /// Interior mutability for the same reason as the menu. The list is refilled
    /// only when its contents actually change: rebuilding it on every render
    /// would close the dropdown under the user and lose the selection.
    pub(super) listed_regions: RefCell<Vec<i32>>,
    /// Language those region labels were built in, for the same reason the
    /// headings need one: the set of regions can stay identical while every
    /// label in it has to be rewritten.
    pub(super) listed_region_language: Cell<Option<Language>>,
    /// Rows currently in the updates table, for the same reason.
    pub(super) listed_updates: RefCell<Vec<[String; 4]>>,
    /// Language the table headings were last written in.
    ///
    /// A list-view column keeps the text it was created with; unlike every
    /// other caption, nothing rewrites it on a render. Remembering the language
    /// they were written in is what lets a language change reach them, and
    /// keeps every other render from rewriting four headings for nothing.
    pub(super) heading_language: Cell<Option<Language>>,
    /// Which way the window is currently laid out.
    ///
    /// Not the language: three languages read left to right and switching
    /// between them must not turn the window around and repaint every child.
    /// What matters here is the direction, and it changes on the one language
    /// change that crosses between the two.
    pub(super) layout_direction: Cell<Option<TextDirection>>,
    pub(super) controls: Option<Controls>,
}

/// The two pointers handed to `CreateWindowExW` and split by `WM_NCCREATE`.
pub(super) struct WindowBootstrap {
    pub(super) chrome: *mut WindowChrome,
    pub(super) state: *mut AppState,
}

/// Sole owner of both window allocations for the whole message loop.
///
/// The window merely borrows them through its window words, and `WM_NCDESTROY`
/// revokes those borrows. Window creation can fail after `WM_NCCREATE` has
/// already published the pointers, so a second owner in that path would free
/// allocations that `WM_NCDESTROY` has seen.
pub(super) struct WindowContextOwner {
    chrome: *mut WindowChrome,
    state: *mut AppState,
    bootstrap: *mut WindowBootstrap,
}

impl WindowContextOwner {
    pub(super) fn new(chrome: WindowChrome, state: AppState) -> Self {
        let chrome = Box::into_raw(Box::new(chrome));
        let state = Box::into_raw(Box::new(state));
        Self {
            chrome,
            state,
            bootstrap: Box::into_raw(Box::new(WindowBootstrap { chrome, state })),
        }
    }

    pub(super) fn as_create_parameter(&self) -> *const c_void {
        self.bootstrap.cast::<c_void>()
    }
}

impl Drop for WindowContextOwner {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe {
            drop(Box::from_raw(self.bootstrap));
            drop(Box::from_raw(self.state));
            drop(Box::from_raw(self.chrome));
        }
    }
}

impl Drop for WindowChrome {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.brand_brush.into());
            let _ = DeleteObject(self.status_brush.into());
            let _ = DeleteObject(self.frame_brush.into());
        }
    }
}

/// Keeps the per-logon-session ownership claim alive for the GUI lifetime.
pub(super) struct SingleInstanceGuard(pub(super) HANDLE);

pub(super) struct OleGuard;

impl Drop for SingleInstanceGuard {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

impl Drop for OleGuard {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        unsafe { OleUninitialize() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winstoreregion_core::{GeoId, IsoAlpha2};
    use winstoreregion_core::{
        GuiLaunch, InstallClassification, MarketCode, PrerequisiteState, ResolveRequest,
        ResolvedProduct, ResolverSource, SignatureStatus, SignerInfo, StoreProductId,
        StoreStubStatus, StubFileIdentity,
    };

    #[test]
    fn launch_input_is_kept_in_presentation_state() {
        let state = AppState::new(GuiLaunch {
            initial_input: Some("9WZDNCRFJ3PZ".to_owned()),
        });

        assert_eq!(state.application_source.store_text(), "9WZDNCRFJ3PZ");
        assert_eq!(state.selected_tab.index(), Tab::Installation.index());
        assert!(!state.install_is_available());
        // The status names the identifier it accepted and the condition still
        // missing; it must never claim readiness the gate has not granted.
        assert!(state.operation_status.contains("9WZDNCRFJ3PZ"));
        assert!(!state.operation_status.contains("Можно запускать"));
    }

    #[test]
    fn the_store_installer_is_offered_only_after_an_operation_that_installed_nothing() {
        let mut state = AppState::new(GuiLaunch {
            initial_input: Some("9WZDNCRFJ3L1".to_owned()),
        });

        // Nothing has run yet, so there is nothing to offer a second way about.
        assert!(!state.store_installer_download_is_available());

        state.operation_state = Some(InstallationOperationState::CompletionUncertain);
        assert!(state.store_installer_download_is_available());

        // A product the catalogue refuses to deliver here is not offered a
        // second way to fail: the installer would ask the same services.
        state.device_compatibility = Some((
            StoreProductId::parse("9WZDNCRFJ3L1").expect("valid Product ID"),
            MarketCode::from_region(
                state
                    .selected_temporary_region
                    .as_ref()
                    .expect("a region is selected by default"),
            )
            .expect("the default region has a market"),
            DeviceCompatibility::OtherDeviceOnly,
        ));
        assert!(!state.store_installer_download_is_available());
        assert!(!state.install_is_available());

        state.device_compatibility = None;
        assert!(state.store_installer_download_is_available());

        // While the file is being fetched the offer is not repeated.
        state.installer_download =
            Some(StoreProductId::parse("9WZDNCRFJ3L1").expect("valid Product ID"));
        assert!(!state.store_installer_download_is_available());
    }

    #[test]
    fn a_new_state_offers_the_united_states_before_the_user_chooses() {
        let state = AppState::new(GuiLaunch::default());
        // The list comes from Windows itself, so it is only asserted about when
        // Windows supplied one at all.
        if state.temporary_regions.is_empty() {
            return;
        }
        assert_eq!(
            state
                .selected_temporary_region
                .as_ref()
                .map(|region| region.geo_id.value()),
            Some(DEFAULT_TEMPORARY_GEO_ID),
            "a region-gated product is most often offered in the United States"
        );
    }

    #[test]
    fn pending_recovery_blocks_the_install_command_on_its_own() {
        let mut state = AppState::new(GuiLaunch {
            initial_input: Some("9WZDNCRFJ3PZ".to_owned()),
        });
        // Grant every other precondition so only the recovery gate can decide.
        state.installation_backend_confirmed = true;
        state.prerequisites = PrerequisiteReport::new(
            PrerequisiteState::Satisfied,
            PrerequisiteState::Satisfied,
            PrerequisiteState::Satisfied,
        );
        let product_id = StoreProductId::parse("9WZDNCRFJ3PZ").expect("valid product identifier");
        let market = MarketCode::parse("US").expect("valid market");
        let request = state.resolution.begin(product_id, market);
        let resolved = resolved_product_for_test(&request);
        assert!(state.resolution.complete(&request, Ok(resolved)));
        assert!(state.resolution.has_fresh_resolution());

        assert!(
            !state.install_is_available(),
            "recovery has not been inspected yet, so install must stay unavailable"
        );

        state.recovery_admits_new_operation = true;
        // A fresh state already offers a default region; this case is about the
        // gate itself, so the choice is taken away again.
        state.selected_temporary_region = None;
        assert!(
            !state.install_is_available(),
            "without a temporary region there is nothing to switch to"
        );

        state.selected_temporary_region = Some(
            Region::new(
                GeoId::new(244).expect("valid GeoId"),
                "United States",
                IsoAlpha2::parse("us"),
            )
            .expect("complete region"),
        );
        assert!(state.install_is_available());

        state.recovery_admits_new_operation = false;
        assert!(
            !state.install_is_available(),
            "a pending recovery record must block a new operation by itself"
        );
    }

    #[test]
    fn an_unmet_prerequisite_blocks_the_install_command_on_its_own() {
        let mut state = AppState::new(GuiLaunch::default());
        // Grant everything except the prerequisites.
        state.installation_backend_confirmed = true;
        state.recovery_admits_new_operation = true;
        state.selected_temporary_region = Some(
            Region::new(
                GeoId::new(244).expect("valid GeoId"),
                "United States",
                IsoAlpha2::parse("us"),
            )
            .expect("complete region"),
        );
        let request = state.resolution.begin(
            StoreProductId::parse("9WZDNCRFJ3PZ").expect("valid product identifier"),
            MarketCode::parse("US").expect("valid market"),
        );
        let resolved = resolved_product_for_test(&request);
        assert!(state.resolution.complete(&request, Ok(resolved)));

        assert!(
            !state.install_is_available(),
            "an unchecked machine must not offer an installation"
        );

        state.prerequisites = PrerequisiteReport::new(
            PrerequisiteState::Missing,
            PrerequisiteState::Satisfied,
            PrerequisiteState::Satisfied,
        );
        assert!(!state.install_is_available());

        state.prerequisites = PrerequisiteReport::new(
            PrerequisiteState::Satisfied,
            PrerequisiteState::Unknown,
            PrerequisiteState::Satisfied,
        );
        assert!(
            !state.install_is_available(),
            "an unfinished check must never become permission"
        );

        state.prerequisites = PrerequisiteReport::new(
            PrerequisiteState::Satisfied,
            PrerequisiteState::Satisfied,
            PrerequisiteState::Satisfied,
        );
        assert!(state.install_is_available());
    }

    #[test]
    fn a_parsed_identifier_is_enough_because_the_card_may_only_exist_elsewhere() {
        let mut state = AppState::new(GuiLaunch::default());
        state.recovery_admits_new_operation = true;
        state.prerequisites = PrerequisiteReport::new(
            PrerequisiteState::Satisfied,
            PrerequisiteState::Satisfied,
            PrerequisiteState::Satisfied,
        );
        state.selected_temporary_region = Some(
            Region::new(
                GeoId::new(244).expect("valid GeoId"),
                "United States",
                IsoAlpha2::parse("us"),
            )
            .expect("complete region"),
        );

        assert!(
            !state.install_is_available(),
            "there is no product to install yet"
        );

        state
            .resolution
            .accept_syntax(StoreProductId::parse("9NT1R1C2HH7J").expect("valid identifier"));
        assert!(
            state.install_is_available(),
            "a product offered only in the chosen region cannot be resolved before switching to it"
        );

        state.running_operation = Some(RunningOperation::default());
        assert!(!state.install_is_available(), "one operation at a time");
    }

    fn resolved_product_for_test(request: &ResolveRequest) -> ResolvedProduct {
        ResolvedProduct {
            request_id: request.request_id,
            product_id: request.product_id.clone(),
            market: request.market.clone(),
            display_name: "Contoso App".to_owned(),
            package_family_name: None,
            publisher: None,
            icon: None,
            install_classification: InstallClassification::unknown(),
            version: None,
            size_bytes: None,
            resolver_source: ResolverSource::WinGetMsStore,
        }
    }

    #[test]
    fn executable_launch_input_is_not_treated_as_a_file_source() {
        let executable = std::env::current_exe().expect("test executable path");
        let state = AppState::new(GuiLaunch {
            initial_input: Some(executable.to_string_lossy().into_owned()),
        });

        assert_eq!(
            state.application_source.store_text(),
            executable.to_string_lossy()
        );
        assert!(!state.install_is_available());
    }

    #[test]
    fn source_switch_keeps_store_draft_when_file_draft_is_cleared() {
        let mut state = AppState::new(GuiLaunch {
            initial_input: Some("9WZDNCRFJ3PZ".to_owned()),
        });
        let executable = std::env::current_exe().expect("test executable path");
        state
            .application_source
            .select_installer_stub(executable)
            .expect("an executable path is a supported draft");

        assert_eq!(
            state.application_source.selected(),
            winstoreregion_core::ApplicationSourceKind::InstallerFile
        );
        assert_eq!(state.application_source.store_text(), "9WZDNCRFJ3PZ");

        state.application_source.clear_installer_file();
        assert_eq!(
            state.application_source.selected(),
            winstoreregion_core::ApplicationSourceKind::StoreText
        );
        assert_eq!(state.application_source.store_text(), "9WZDNCRFJ3PZ");
    }

    /// A window with every handoff gate open except the file's own verdict.
    fn state_ready_for_handoff() -> (AppState, std::path::PathBuf) {
        let mut state = AppState::new(GuiLaunch::default());
        let executable = std::env::current_exe().expect("test executable path");
        state
            .application_source
            .select_installer_stub(executable.clone())
            .expect("an executable path is a supported draft");
        state.recovery_admits_new_operation = true;
        state.prerequisites = PrerequisiteReport::new(
            PrerequisiteState::Missing,
            // Only Microsoft Store matters for a handoff; nothing here asks a
            // backend, so App Installer and the COM metadata are irrelevant.
            PrerequisiteState::Satisfied,
            PrerequisiteState::Unknown,
        );
        state.selected_temporary_region = Some(
            Region::new(
                GeoId::new(244).expect("valid GeoId"),
                "United States",
                IsoAlpha2::parse("us"),
            )
            .expect("complete region"),
        );
        (state, executable)
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

    fn inspection_of(path: &std::path::Path, signer: Option<SignerInfo>) -> StubInspection {
        StubInspection {
            file_identity: StubFileIdentity {
                path: path.to_path_buf(),
                byte_len: 1024,
                modified_unix_nanos: 0,
                sha256: "abc123".to_owned(),
            },
            signature_status: SignatureStatus::Trusted,
            signer,
            store_stub_status: StoreStubStatus::NotStoreStub,
            product_id: None,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn the_region_search_falls_back_to_the_link_draft_while_a_file_is_the_source() {
        let mut state = AppState::new(GuiLaunch {
            initial_input: Some("9WZDNCRFJ3L1".to_owned()),
        });
        assert_eq!(
            state
                .searchable_product_id()
                .as_ref()
                .map(StoreProductId::as_str),
            Some("9WZDNCRFJ3L1")
        );

        let executable = std::env::current_exe().expect("test executable path");
        state
            .application_source
            .select_installer_stub(executable)
            .expect("an executable path is a supported draft");
        state.resolution.invalidate();

        // A file names no product, so nothing is accepted for an installation.
        assert!(state.accepted_product_id().is_none());
        // The search is a reference and installs nothing, so it may still ask
        // about whatever the user left in the preserved link draft.
        assert_eq!(
            state
                .searchable_product_id()
                .as_ref()
                .map(StoreProductId::as_str),
            Some("9WZDNCRFJ3L1"),
            "the region search must survive switching to a file"
        );

        state.application_source.set_store_text("");
        assert!(
            state.searchable_product_id().is_none(),
            "with nothing typed there is nothing to ask about"
        );
    }

    #[test]
    fn a_file_the_store_did_not_sign_is_never_offered_for_a_handoff() {
        let (mut state, executable) = state_ready_for_handoff();

        // Every other gate is open, so only the file's own verdict can decide.
        state.stub_inspection = Some(inspection_of(&executable, Some(store_signer())));
        assert!(state.handoff_is_available());
        assert!(state.primary_command_is_available());

        state.stub_inspection = Some(StubInspection {
            signature_status: SignatureStatus::Unsigned,
            ..inspection_of(&executable, Some(store_signer()))
        });
        assert!(
            !state.handoff_is_available(),
            "an untrusted signature must block the handoff on its own"
        );

        state.stub_inspection = Some(inspection_of(
            &executable,
            Some(SignerInfo {
                issuer: "Unrelated CA".to_owned(),
                ..store_signer()
            }),
        ));
        assert!(
            !state.handoff_is_available(),
            "a signer outside the Store policy must block the handoff on its own"
        );

        state.stub_inspection = Some(inspection_of(&executable, None));
        assert!(!state.handoff_is_available());

        // An inspection about some other file grants nothing about this one.
        state.stub_inspection = Some(inspection_of(
            &executable.with_file_name("other-installer.exe"),
            Some(store_signer()),
        ));
        assert!(
            !state.handoff_is_available(),
            "the verdict must be about the file that is actually selected"
        );

        state.stub_inspection = None;
        assert!(!state.handoff_is_available());
    }

    #[test]
    fn a_handoff_holding_the_region_blocks_a_second_operation_and_offers_the_way_back() {
        let (mut state, executable) = state_ready_for_handoff();
        state.stub_inspection = Some(inspection_of(&executable, Some(store_signer())));
        assert!(!state.region_restore_is_available());

        state.handoff = Some(HandoffProgress {
            stage: HandoffStage::HandedOff,
            file: None,
            original_region: None,
            temporary_region: None,
        });
        assert!(state.handoff_blocks_new_operation());
        assert!(!state.handoff_is_available(), "one operation at a time");
        assert!(!state.install_is_available());
        assert!(state.region_restore_is_available());

        state.handoff = Some(HandoffProgress {
            stage: HandoffStage::Restored,
            file: None,
            original_region: None,
            temporary_region: None,
        });
        assert!(!state.handoff_blocks_new_operation());
        assert!(state.handoff_is_available());
        assert!(
            !state.region_restore_is_available(),
            "a region already back has nothing left to restore"
        );
    }

    #[test]
    fn a_posted_result_waits_for_the_outermost_modal_call_to_return() {
        // Thread-local by design, and one test thread is one window's worth of
        // state, so this observes exactly what the window procedure would.
        assert!(!modal_call_is_active());
        {
            let _outer = ModalScope::enter();
            assert!(modal_call_is_active());
            {
                // A dialog can open a dialog; the mark has to survive the inner
                // one going away.
                let _inner = ModalScope::enter();
                assert!(modal_call_is_active());
            }
            assert!(
                modal_call_is_active(),
                "the outer dialog is still up, so results must still wait"
            );
            // The window this names no longer exists, so the re-post fails and
            // is dropped; what matters here is that the queue is emptied and
            // nothing is handled while the borrow is held.
            hold_until_modal_ends(
                HWND(std::ptr::null_mut()),
                0x8001,
                windows::Win32::Foundation::WPARAM(0),
            );
        }
        assert!(
            !modal_call_is_active(),
            "the ordinary message loop is back and results may be handled again"
        );
    }

    #[test]
    fn every_tab_the_strip_shows_is_a_tab_the_window_can_select() {
        // A tab that cannot be selected must not be offered at all.
        assert_eq!(Tab::from_index(0), Some(Tab::Installation));
        assert_eq!(Tab::from_index(1), Some(Tab::Journal));
        assert_eq!(Tab::from_index(2), Some(Tab::Updates));
        assert!(Tab::from_index(3).is_none());
        assert_eq!(Tab::Updates.index(), 2);
    }
}
