//! Off-thread work that posts structured results back to the window.

use crate::gui::ids::{
    WM_APP_DEVICE_COMPATIBILITY, WM_APP_INSTALLER_DOWNLOADED, WM_APP_JOURNAL_DELETED,
    WM_APP_JOURNAL_LOADED, WM_APP_MARKET_ANSWERS, WM_APP_PRODUCT_RESOLVED, WM_APP_RESUME_PROBED,
    WM_APP_STARTUP_CHECKED, WM_APP_STUB_INSPECTED, WM_APP_UPDATES_SCANNED, post_boxed,
};
use crate::gui::install::{ResumeProbe, now_utc, probe_resumable_install};
use crate::gui::state::{
    DeviceCompatibilityUpdate, InstallerDownloadUpdate, JournalDeleteUpdate, MarketSurveyUpdate,
    ProductResolutionUpdate, ResumeProbeUpdate, StartupChecksUpdate, StubInspectionUpdate,
    UpdatesScanUpdate,
};
use crate::platform::diagnostic_log::record;
use crate::platform::installer_download::{download_store_installer, sweep_downloaded_installers};
use crate::platform::market_probe::{WinHttpMarketProber, current_windows_version};
use crate::platform::packaged::installed_store_applications;
use crate::platform::prerequisites::check_prerequisites;
use crate::platform::region::Win32RegionReader;
use crate::platform::storage::Win32JournalStore;
use crate::platform::storage::{load_updates_scan, save_updates_scan};
use crate::platform::stub::inspect_installer_stub;
use crate::platform::winget::resolver::WinGetComResolver;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HWND;
use winstoreregion_core::{
    DeviceCompatibility, DeviceCompatibilityProber, InstalledStoreApplication, JournalRecord,
    LogEvent, LogEventCode, LogLevel, MarketAnswer, MarketAvailability, MarketCode, MarketProber,
    PackedVersion, PendingRestore, RegionReader, ResolveRequest, StoreProductId,
    StoreProductLookup, SurveyScope, UpdateCandidate, curated_markets, resolve_product,
};

/// How many markets are asked at the same time.
///
/// One market costs about eight tenths of a second, so a curated set asked one
/// at a time would keep a window waiting half a minute. Eight is enough to make
/// the wait short without turning a convenience into a burst of traffic.
const SURVEY_CONCURRENCY: usize = 8;

pub(super) fn start_product_resolution(window: HWND, request: ResolveRequest) {
    let window_handle = window.0 as usize;
    thread::spawn(move || {
        let update = ProductResolutionUpdate {
            result: resolve_product(&WinGetComResolver, &request),
            request,
        };
        post_boxed(window_handle, WM_APP_PRODUCT_RESOLVED, update);
    });
}

/// Run the startup checks that ask Windows something, off the UI thread.
///
/// Each of these asks something outside the process: the package manager for
/// what is installed, the disk for a remembered scan, the download folder for
/// files to remove. On the UI thread they held the window blank until they were
/// done, and an outgoing call from a UI thread services incoming messages while
/// the state is still borrowed by the handler that made it.
pub(super) fn start_startup_checks(window: HWND) {
    let window_handle = window.0 as usize;
    thread::spawn(move || {
        // Installers this application downloaded are never kept. The user did
        // not ask for a folder of Store stubs, cannot be expected to find it,
        // and the file can always be fetched again by Product ID. Startup is
        // where this works: nothing of ours is running the file.
        sweep_downloaded_installers();
        // A scan remembered from a previous run costs nothing to show and saves
        // the user a wait. It is labelled with when it was taken rather than
        // presented as current.
        let remembered_scan = Win32RegionReader
            .current_region()
            .ok()
            .and_then(|region| MarketCode::from_region(&region).ok())
            .and_then(|market| load_updates_scan(&market));
        let update = StartupChecksUpdate {
            prerequisites: check_prerequisites(),
            remembered_scan,
        };
        post_boxed(window_handle, WM_APP_STARTUP_CHECKED, update);
    });
}

/// Delete the installers this application downloaded, off the UI thread.
///
/// A file that is still held open by a running stub survives this; the sweep at
/// the next startup catches it.
pub(super) fn start_installer_sweep() {
    thread::spawn(sweep_downloaded_installers);
}

/// Ask whether the installation a recovery record names is still going.
///
/// The question activates an out-of-process package manager and asks it over
/// the network, which is why it is asked from here and not from the window.
pub(super) fn start_resume_probe(window: HWND, pending: PendingRestore) {
    let window_handle = window.0 as usize;
    thread::spawn(move || {
        let answer = probe_resumable_install(&pending);
        record(
            &LogEvent::new(LogLevel::Info, LogEventCode::ResumeProbed)
                .with_token("outcome", answer.probe.as_token())
                .with_flag("package_present", answer.package_present),
        );
        let update = ResumeProbeUpdate {
            resumable: answer.probe == ResumeProbe::InFlight,
            package_present: answer.package_present,
            pending,
        };
        post_boxed(window_handle, WM_APP_RESUME_PROBED, update);
    });
}

pub(super) fn start_journal_load(window: HWND) {
    let window_handle = window.0 as usize;
    thread::spawn(move || {
        let result = Win32JournalStore::for_current_user().and_then(|store| store.load());
        post_boxed(window_handle, WM_APP_JOURNAL_LOADED, result);
    });
}

pub(super) fn start_journal_delete(window: HWND, record: JournalRecord) {
    let window_handle = window.0 as usize;
    thread::spawn(move || {
        let update = JournalDeleteUpdate {
            result: Win32JournalStore::for_current_user().and_then(|store| store.delete(&record)),
        };
        post_boxed(window_handle, WM_APP_JOURNAL_DELETED, update);
    });
}

/// Remove every entry from the operation history.
///
/// Replacing the file with an empty history rather than deleting it: the store
/// publishes atomically, so a failure leaves the previous history intact
/// instead of a missing file the next start would have to interpret.
pub(super) fn start_journal_clear(window: HWND) {
    let window_handle = window.0 as usize;
    thread::spawn(move || {
        let update = JournalDeleteUpdate {
            result: Win32JournalStore::for_current_user().and_then(|store| store.replace(&[])),
        };
        post_boxed(window_handle, WM_APP_JOURNAL_DELETED, update);
    });
}

pub(super) fn start_stub_inspection(window: HWND, path: PathBuf) {
    let window_handle = window.0 as usize;
    thread::spawn(move || {
        let update = StubInspectionUpdate {
            result: inspect_installer_stub(&path),
            path,
        };
        post_boxed(window_handle, WM_APP_STUB_INSPECTED, update);
    });
}

/// Find installed Store applications this region's Store will not serve.
///
/// Two questions per application, both off the UI thread: who the package
/// family belongs to, and whether that product is offered in the market the
/// machine is in. Only a stated refusal is reported — silence from a market
/// says nothing, exactly as everywhere else in this product.
///
/// Applications the catalogue does not recognise are counted but not listed:
/// without a Product ID there is no operation this window could offer.
pub(super) fn start_updates_scan(window: HWND, market: MarketCode, generation: u64) {
    let window_handle = window.0 as usize;
    thread::spawn(move || {
        let Ok(applications) = installed_store_applications() else {
            post_scan(window_handle, generation, Vec::new(), 0, 0, true, true);
            return;
        };
        let total = applications.len();
        post_scan(
            window_handle,
            generation,
            Vec::new(),
            0,
            total,
            false,
            false,
        );
        let device = current_windows_version();
        // Reference markets, asked in the curated order until one offers the
        // product. A single reference market answers "offered nowhere else" for
        // an application that is alive in the next market on the list — exactly
        // the application this tab exists to find. The version is then asked of
        // the market that answered, because the current one, by definition of
        // this list, does not serve the product.
        let reference_markets = curated_markets();
        let applications = Arc::new(applications);
        let next = Arc::new(AtomicUsize::new(0));
        let done = Arc::new(AtomicUsize::new(0));
        let found: Arc<Mutex<Vec<UpdateCandidate>>> = Arc::new(Mutex::new(Vec::new()));
        // The scan has just reported "0 of total", so the clock starts now:
        // the next report is due one interval after that one, not immediately.
        let last_report = Arc::new(Mutex::new(Instant::now()));
        let workers: Vec<_> = (0..SURVEY_CONCURRENCY.min(total.max(1)))
            .map(|_| {
                let applications = Arc::clone(&applications);
                let next = Arc::clone(&next);
                let done = Arc::clone(&done);
                let found = Arc::clone(&found);
                let last_report = Arc::clone(&last_report);
                let market = market.clone();
                let reference_markets = reference_markets.clone();
                thread::spawn(move || {
                    // A session belongs to the thread that opened it.
                    let prober = WinHttpMarketProber::open();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(application) = applications.get(index) else {
                            return;
                        };
                        let candidate = prober.as_ref().map(|prober| {
                            survey_application(
                                prober,
                                application,
                                &market,
                                &reference_markets,
                                device,
                            )
                        });
                        let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
                        let listed = {
                            let mut found = found
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            if let Some(candidate) = candidate
                                && candidate.is_worth_listing()
                            {
                                found.push(candidate);
                            }
                            let mut last_report = last_report
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            // Only a report the window can keep up with is
                            // worth its copy. Every message costs a full
                            // render, and several scan threads finishing at
                            // once queued one render per application, each
                            // carrying a copy of everything found so far.
                            (last_report.elapsed() >= SCAN_REPORT_INTERVAL).then(|| {
                                *last_report = Instant::now();
                                found.clone()
                            })
                        };
                        if let Some(listed) = listed {
                            post_scan(
                                window_handle,
                                generation,
                                listed,
                                finished,
                                total,
                                false,
                                false,
                            );
                        }
                    }
                })
            })
            .collect();
        for worker in workers {
            let _ = worker.join();
        }
        let listed = {
            let found = found
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            found.clone()
        };
        // Remembered so the next run costs no network until the user asks for a
        // fresh answer. Written here, off the UI thread, like every other file
        // this application produces. A scan that cannot be dated is not saved:
        // the tab shows when an answer was taken, and an undated one would be
        // shown as taken at the epoch.
        if let Some(taken_at) = now_utc() {
            save_updates_scan(&market, taken_at.as_str(), &listed);
        }
        post_scan(window_handle, generation, listed, total, total, true, false);
    });
}

/// Ask both questions about one installed application.
///
/// The second and third questions are asked only about a product the current
/// market refused: for anything else the answers would never be shown, and
/// asking would be traffic spent on nothing.
fn survey_application(
    prober: &WinHttpMarketProber,
    application: &InstalledStoreApplication,
    market: &MarketCode,
    reference_markets: &[MarketCode],
    device: PackedVersion,
) -> UpdateCandidate {
    let product_id = prober.product_for_family(&application.family_name);
    let availability = product_id
        .as_ref()
        .map_or(MarketAvailability::Unknown, |product_id| {
            prober.probe(product_id, market).availability
        });
    let refused = availability == MarketAvailability::NotOffered;
    let offered_in = product_id
        .as_ref()
        .filter(|_| refused)
        .and_then(|product_id| {
            reference_markets.iter().find(|reference| {
                *reference != market
                    && prober.probe(product_id, reference).availability
                        == MarketAvailability::Offered
            })
        });
    let offered_version = product_id
        .as_ref()
        .zip(offered_in)
        .and_then(|(product_id, reference)| prober.offered_version(product_id, reference, device));
    UpdateCandidate {
        application: application.clone(),
        product_id,
        availability,
        offered_elsewhere: offered_in.is_some(),
        offered_version,
    }
}

/// Shortest gap between two progress reports from one scan.
///
/// The last report is never rate-limited: it is posted after every worker has
/// joined, so the window always ends up with the complete answer.
const SCAN_REPORT_INTERVAL: Duration = Duration::from_millis(100);

/// Post one report from the updates scan, discarding it if the window has gone.
fn post_scan(
    window_handle: usize,
    generation: u64,
    candidates: Vec<UpdateCandidate>,
    done: usize,
    total: usize,
    finished: bool,
    unavailable: bool,
) {
    post_boxed(
        window_handle,
        WM_APP_UPDATES_SCANNED,
        UpdatesScanUpdate {
            generation,
            candidates,
            done,
            total,
            finished,
            unavailable,
        },
    );
}

/// Ask Microsoft for the Store installer of one product.
///
/// Off the UI thread like every other request. Nothing is decided here: the
/// file that arrives goes through the same admission gate as one the user
/// picked by hand, and only that gate may let it run.
pub(super) fn start_installer_download(window: HWND, product_id: StoreProductId) {
    let window_handle = window.0 as usize;
    thread::spawn(move || {
        let update = InstallerDownloadUpdate {
            result: download_store_installer(&product_id),
        };
        post_boxed(window_handle, WM_APP_INSTALLER_DOWNLOADED, update);
    });
}

/// Ask the catalogue whether this device can be delivered this product.
///
/// One request, off the UI thread like every other. A failure of any kind
/// arrives as `Unknown`, which refuses nothing: the check may only stop an
/// installation when the catalogue actually stated that it cannot happen.
pub(super) fn start_device_compatibility_probe(
    window: HWND,
    product_id: StoreProductId,
    market: MarketCode,
) {
    let window_handle = window.0 as usize;
    thread::spawn(move || {
        let device = current_windows_version();
        let compatibility = WinHttpMarketProber::open()
            .map_or(DeviceCompatibility::Unknown, |prober| {
                prober.compatibility(&product_id, &market, device)
            });
        let update = DeviceCompatibilityUpdate {
            product_id,
            market,
            compatibility,
        };
        post_boxed(window_handle, WM_APP_DEVICE_COMPATIBILITY, update);
    });
}

/// Ask a set of markets where one product is offered.
///
/// Answers are posted one at a time so the window can show progress and narrow
/// its list as they arrive, and a final report closes the pass whether it
/// finished or was stopped. Cancellation is cooperative: a worker checks the
/// flag between markets and never abandons a request in flight.
pub(super) fn start_market_survey(
    window: HWND,
    product_id: StoreProductId,
    markets: Vec<MarketCode>,
    generation: u64,
    scope: SurveyScope,
    cancel: Arc<AtomicBool>,
) {
    let window_handle = window.0 as usize;
    thread::spawn(move || {
        let markets = Arc::new(markets);
        let next = Arc::new(AtomicUsize::new(0));
        let workers: Vec<_> = (0..SURVEY_CONCURRENCY.min(markets.len().max(1)))
            .map(|_| {
                let markets = Arc::clone(&markets);
                let next = Arc::clone(&next);
                let cancel = Arc::clone(&cancel);
                let product_id = product_id.clone();
                thread::spawn(move || {
                    // A session belongs to the thread that opened it, so each
                    // worker opens its own rather than sharing one handle.
                    let prober = WinHttpMarketProber::open();
                    loop {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(market) = markets.get(index) else {
                            return;
                        };
                        let answer = prober.as_ref().map_or_else(
                            || MarketAnswer::unknown(market.clone()),
                            |prober| prober.probe(&product_id, market),
                        );
                        post_answers(window_handle, generation, vec![answer], false, scope);
                    }
                })
            })
            .collect();
        for worker in workers {
            let _ = worker.join();
        }
        post_answers(window_handle, generation, Vec::new(), true, scope);
    });
}

/// Post one report from a survey pass, discarding it if the window has gone.
fn post_answers(
    window_handle: usize,
    generation: u64,
    answers: Vec<MarketAnswer>,
    finished: bool,
    scope: SurveyScope,
) {
    post_boxed(
        window_handle,
        WM_APP_MARKET_ANSWERS,
        MarketSurveyUpdate {
            generation,
            answers,
            finished,
            scope,
        },
    );
}
