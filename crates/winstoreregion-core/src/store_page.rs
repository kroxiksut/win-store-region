//! Opening a Store product page as an explicit non-install action.

use crate::input::StoreProductId;
use crate::resolve::ResolvedProduct;

/// Observable milestones in the Store interaction lifecycle.
///
/// The values are facts for the future operation state machine, not permission
/// to infer a later milestone from an earlier one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreInstallLifecycleEvent {
    /// Windows accepted a request to open the product's Store PDP URI.
    StorePageOpened,
    /// An installation backend accepted an exact installation request.
    InstallRequested,
    /// A selected observer reported evidence for that request.
    InstallObserved,
    /// The observer supplied enough evidence to confirm completion.
    InstallCompleted,
}

impl StoreInstallLifecycleEvent {
    /// Whether this fact proves that a backend accepted an install request.
    #[must_use]
    pub const fn is_install_requested(self) -> bool {
        matches!(
            self,
            Self::InstallRequested | Self::InstallObserved | Self::InstallCompleted
        )
    }

    /// Whether this fact proves completion of the installation.
    #[must_use]
    pub const fn is_install_completed(self) -> bool {
        matches!(self, Self::InstallCompleted)
    }
}

/// Exact Store PDP request separated from installation backend input.
///
/// Opening this URI is a user-visible fallback action only. It carries neither a
/// temporary region nor a recovery obligation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorePageRequest {
    product_id: StoreProductId,
}

impl StorePageRequest {
    /// Build a PDP request from an exact, already-resolved product.
    #[must_use]
    pub fn from_resolved_product(product: &ResolvedProduct) -> Self {
        Self {
            product_id: product.product_id.clone(),
        }
    }

    /// Build a PDP request from an exact Product ID retained in local history.
    ///
    /// This remains a page-opening request only; it is not an installation
    /// request and carries no recovery or region information.
    #[must_use]
    pub fn from_product_id(product_id: StoreProductId) -> Self {
        Self { product_id }
    }

    /// Return the exact Product ID shown in the Store page URI.
    #[must_use]
    pub fn product_id(&self) -> &StoreProductId {
        &self.product_id
    }

    /// Build the canonical Microsoft Store PDP URI for this exact Product ID.
    #[must_use]
    pub fn uri(&self) -> String {
        format!(
            "ms-windows-store://pdp/?ProductId={}",
            self.product_id.as_str()
        )
    }
}

/// Structured reason why Windows could not accept a Store page launch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorePageOpenError {
    /// No registered handler accepted the PDP URI launch.
    LaunchRejected,
}

/// Narrow platform boundary for opening a Store PDP page.
///
/// This trait deliberately has no install, observation, cancellation, region,
/// or recovery methods. A successful call only means that Windows accepted the
/// URI launch request.
pub trait StorePageOpener {
    /// Request opening the exact Store PDP URI.
    ///
    /// # Errors
    ///
    /// Returns a structured launch failure without claiming installation
    /// progress, acceptance, observation, or completion.
    fn open_store_page(&self, request: &StorePageRequest) -> Result<(), StorePageOpenError>;
}

/// Open a PDP page and return its distinct non-install lifecycle fact.
///
/// # Errors
///
/// Returns [`StorePageOpenError`] without creating an install handle or
/// converting a failed/successful URI launch into an installation result.
pub fn open_store_page<O>(
    opener: &O,
    request: &StorePageRequest,
) -> Result<StoreInstallLifecycleEvent, StorePageOpenError>
where
    O: StorePageOpener,
{
    opener.open_store_page(request)?;
    Ok(StoreInstallLifecycleEvent::StorePageOpened)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::resolve_product;
    use crate::test_support::{
        FakeResolver, FakeStorePageOpener, resolver_product, resolver_request,
    };
    use std::cell::RefCell;

    #[test]
    fn opening_a_pdp_uri_is_a_distinct_non_install_lifecycle_fact() {
        let resolve_request = resolver_request();
        let product = resolve_product(
            &FakeResolver {
                result: Ok(resolver_product(&resolve_request)),
            },
            &resolve_request,
        )
        .expect("resolved product for PDP request");
        let request = StorePageRequest::from_resolved_product(&product);
        let opener = FakeStorePageOpener {
            result: Ok(()),
            opened_uri: RefCell::new(None),
        };

        let event = open_store_page(&opener, &request).expect("PDP launch accepted");

        assert_eq!(
            opener.opened_uri.into_inner(),
            Some("ms-windows-store://pdp/?ProductId=9WZDNCRFJ3PZ".to_owned())
        );
        assert_eq!(event, StoreInstallLifecycleEvent::StorePageOpened);
        assert!(!event.is_install_requested());
        assert!(!event.is_install_completed());
        assert!(!StoreInstallLifecycleEvent::InstallRequested.is_install_completed());
        assert!(StoreInstallLifecycleEvent::InstallCompleted.is_install_requested());
        assert!(StoreInstallLifecycleEvent::InstallCompleted.is_install_completed());
    }

    #[test]
    fn rejected_pdp_launch_never_becomes_an_install_result() {
        let request = StorePageRequest::from_resolved_product(
            &resolve_product(
                &FakeResolver {
                    result: Ok(resolver_product(&resolver_request())),
                },
                &resolver_request(),
            )
            .expect("resolved product for failed PDP request"),
        );
        let opener = FakeStorePageOpener {
            result: Err(StorePageOpenError::LaunchRejected),
            opened_uri: RefCell::new(None),
        };

        assert_eq!(
            open_store_page(&opener, &request),
            Err(StorePageOpenError::LaunchRejected)
        );
        assert!(opener.opened_uri.borrow().is_some());
    }
}
