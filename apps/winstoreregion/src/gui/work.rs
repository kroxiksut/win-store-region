//! Off-thread work that posts structured results back to the window.

use crate::gui::ids::{
    WM_APP_DEVICE_COMPATIBILITY, WM_APP_INSTALLER_DOWNLOADED, WM_APP_JOURNAL_DELETED,
    WM_APP_JOURNAL_LOADED, WM_APP_MARKET_ANSWERS, WM_APP_PRODUCT_RESOLVED, WM_APP_STUB_INSPECTED,
    WM_APP_UPDATES_SCANNED,
};
use crate::gui::install::now_utc;
use crate::gui::state::{
    DeviceCompatibilityUpdate, InstallerDownloadUpdate, JournalDeleteUpdate, MarketSurveyUpdate,
    ProductResolutionUpdate, StubInspectionUpdate, UpdatesScanUpdate,
};
use crate::platform::installer_download::download_store_installer;
use crate::platform::market_probe::{WinHttpMarketProber, current_windows_version};
use crate::platform::packaged::installed_store_applications;
use crate::platform::storage::Win32JournalStore;
use crate::platform::storage::save_updates_scan;
use crate::platform::stub::inspect_installer_stub;
use crate::platform::winget::resolver::WinGetComResolver;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use winstoreregion_core::{
    DeviceCompatibility, DeviceCompatibilityProber, JournalRecord, MarketAnswer,
    MarketAvailability, MarketCode, MarketProber, ResolveRequest, StoreProductId,
    StoreProductLookup, SurveyScope, UpdateCandidate, resolve_product,
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
        let update = Box::new(ProductResolutionUpdate {
            result: resolve_product(&WinGetComResolver, &request),
            request,
        });
        let raw = Box::into_raw(update);
        let posted = unsafe {
            PostMessageW(
                Some(HWND(window_handle as *mut c_void)),
                WM_APP_PRODUCT_RESOLVED,
                WPARAM(raw as usize),
                LPARAM(0),
            )
        };
        if posted.is_err() {
            unsafe { drop(Box::from_raw(raw)) };
        }
    });
}

pub(super) fn start_journal_load(window: HWND) {
    let window_handle = window.0 as usize;
    thread::spawn(move || {
        let result = Win32JournalStore::for_current_user().and_then(|store| store.load());
        let raw = Box::into_raw(Box::new(result));
        let posted = unsafe {
            PostMessageW(
                Some(HWND(window_handle as *mut c_void)),
                WM_APP_JOURNAL_LOADED,
                WPARAM(raw as usize),
                LPARAM(0),
            )
        };
        if posted.is_err() {
            unsafe { drop(Box::from_raw(raw)) };
        }
    });
}

pub(super) fn start_journal_delete(window: HWND, record: JournalRecord) {
    let window_handle = window.0 as usize;
    thread::spawn(move || {
        let update = Box::new(JournalDeleteUpdate {
            result: Win32JournalStore::for_current_user().and_then(|store| store.delete(&record)),
        });
        let raw = Box::into_raw(update);
        let posted = unsafe {
            PostMessageW(
                Some(HWND(window_handle as *mut c_void)),
                WM_APP_JOURNAL_DELETED,
                WPARAM(raw as usize),
                LPARAM(0),
            )
        };
        if posted.is_err() {
            unsafe { drop(Box::from_raw(raw)) };
        }
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
        let update = Box::new(JournalDeleteUpdate {
            result: Win32JournalStore::for_current_user().and_then(|store| store.replace(&[])),
        });
        let raw = Box::into_raw(update);
        let posted = unsafe {
            PostMessageW(
                Some(HWND(window_handle as *mut c_void)),
                WM_APP_JOURNAL_DELETED,
                WPARAM(raw as usize),
                LPARAM(0),
            )
        };
        if posted.is_err() {
            unsafe { drop(Box::from_raw(raw)) };
        }
    });
}

pub(super) fn start_stub_inspection(window: HWND, path: PathBuf) {
    let window_handle = window.0 as usize;
    thread::spawn(move || {
        let update = Box::new(StubInspectionUpdate {
            result: inspect_installer_stub(&path),
            path,
        });
        let raw = Box::into_raw(update);
        let posted = unsafe {
            PostMessageW(
                Some(HWND(window_handle as *mut c_void)),
                WM_APP_STUB_INSPECTED,
                WPARAM(raw as usize),
                LPARAM(0),
            )
        };
        if posted.is_err() {
            unsafe { drop(Box::from_raw(raw)) };
        }
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
pub(super) fn start_updates_scan(window: HWND, market: MarketCode) {
    let window_handle = window.0 as usize;
    thread::spawn(move || {
        let Ok(applications) = installed_store_applications() else {
            post_scan(window_handle, Vec::new(), 0, 0, true, true);
            return;
        };
        let total = applications.len();
        post_scan(window_handle, Vec::new(), 0, total, false, false);
        let device = current_windows_version();
        // The version is asked of a market that serves the product; the current
        // one, by definition of this list, does not.
        let Ok(lookup_market) = MarketCode::parse("US") else {
            return;
        };
        let applications = Arc::new(applications);
        let next = Arc::new(AtomicUsize::new(0));
        let done = Arc::new(AtomicUsize::new(0));
        let found: Arc<Mutex<Vec<UpdateCandidate>>> = Arc::new(Mutex::new(Vec::new()));
        let workers: Vec<_> = (0..SURVEY_CONCURRENCY.min(total.max(1)))
            .map(|_| {
                let applications = Arc::clone(&applications);
                let next = Arc::clone(&next);
                let done = Arc::clone(&done);
                let found = Arc::clone(&found);
                let market = market.clone();
                let lookup_market = lookup_market.clone();
                thread::spawn(move || {
                    // A session belongs to the thread that opened it.
                    let prober = WinHttpMarketProber::open();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(application) = applications.get(index) else {
                            return;
                        };
                        let candidate = prober.as_ref().map(|prober| {
                            let product_id = prober.product_for_family(&application.family_name);
                            let availability = product_id
                                .as_ref()
                                .map_or(MarketAvailability::Unknown, |product_id| {
                                    prober.probe(product_id, &market).availability
                                });
                            // The offered version is asked for only where it
                            // will be shown: a product this market serves is
                            // not listed, so asking about it would be traffic
                            // spent on an answer nobody sees.
                            // Both further questions are asked only about a
                            // product this market refused: for anything else
                            // the answers would never be shown.
                            let refused = availability == MarketAvailability::NotOffered;
                            let offered_elsewhere = product_id
                                .as_ref()
                                .filter(|_| refused)
                                .is_some_and(|product_id| {
                                    prober.probe(product_id, &lookup_market).availability
                                        == MarketAvailability::Offered
                                });
                            let offered_version = product_id
                                .as_ref()
                                .filter(|_| offered_elsewhere)
                                .and_then(|product_id| {
                                    prober.offered_version(product_id, &lookup_market, device)
                                });
                            UpdateCandidate {
                                application: application.clone(),
                                product_id,
                                availability,
                                offered_version,
                                offered_elsewhere,
                            }
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
                            found.clone()
                        };
                        post_scan(window_handle, listed, finished, total, false, false);
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
        // this application produces.
        save_updates_scan(&market, now_utc().as_str(), &listed);
        post_scan(window_handle, listed, total, total, true, false);
    });
}

/// Post one report from the updates scan, discarding it if the window has gone.
fn post_scan(
    window_handle: usize,
    candidates: Vec<UpdateCandidate>,
    done: usize,
    total: usize,
    finished: bool,
    unavailable: bool,
) {
    let raw = Box::into_raw(Box::new(UpdatesScanUpdate {
        candidates,
        done,
        total,
        finished,
        unavailable,
    }));
    let posted = unsafe {
        PostMessageW(
            Some(HWND(window_handle as *mut c_void)),
            WM_APP_UPDATES_SCANNED,
            WPARAM(raw as usize),
            LPARAM(0),
        )
    };
    if posted.is_err() {
        unsafe { drop(Box::from_raw(raw)) };
    }
}

/// Ask Microsoft for the Store installer of one product.
///
/// Off the UI thread like every other request. Nothing is decided here: the
/// file that arrives goes through the same admission gate as one the user
/// picked by hand, and only that gate may let it run.
pub(super) fn start_installer_download(window: HWND, product_id: StoreProductId) {
    let window_handle = window.0 as usize;
    thread::spawn(move || {
        let update = Box::new(InstallerDownloadUpdate {
            result: download_store_installer(&product_id),
        });
        let raw = Box::into_raw(update);
        let posted = unsafe {
            PostMessageW(
                Some(HWND(window_handle as *mut c_void)),
                WM_APP_INSTALLER_DOWNLOADED,
                WPARAM(raw as usize),
                LPARAM(0),
            )
        };
        if posted.is_err() {
            unsafe { drop(Box::from_raw(raw)) };
        }
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
        let raw = Box::into_raw(Box::new(DeviceCompatibilityUpdate {
            product_id,
            market,
            compatibility,
        }));
        let posted = unsafe {
            PostMessageW(
                Some(HWND(window_handle as *mut c_void)),
                WM_APP_DEVICE_COMPATIBILITY,
                WPARAM(raw as usize),
                LPARAM(0),
            )
        };
        if posted.is_err() {
            unsafe { drop(Box::from_raw(raw)) };
        }
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
    let raw = Box::into_raw(Box::new(MarketSurveyUpdate {
        generation,
        answers,
        finished,
        scope,
    }));
    let posted = unsafe {
        PostMessageW(
            Some(HWND(window_handle as *mut c_void)),
            WM_APP_MARKET_ANSWERS,
            WPARAM(raw as usize),
            LPARAM(0),
        )
    };
    if posted.is_err() {
        unsafe { drop(Box::from_raw(raw)) };
    }
}
