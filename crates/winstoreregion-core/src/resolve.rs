//! Product resolution and the card shown before any region change.

use crate::input::StoreProductId;
use crate::install::InstallClassification;
use crate::observe::packaged::PackageFamilyName;
use crate::region::MarketCode;

/// Monotonic identifier for one product-resolution request.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ResolveRequestId(u64);

impl ResolveRequestId {
    /// Construct an identifier supplied by the GUI/core request coordinator.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the value used only for request correlation.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

/// Normalised resolver input, independent from URL/URI presentation details.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolveRequest {
    pub request_id: ResolveRequestId,
    pub product_id: StoreProductId,
    pub market: MarketCode,
}

/// An icon descriptor that can be rendered without inventing product identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProductIcon {
    /// A local, application-owned placeholder.
    LocalPlaceholder,
}

/// Structured backend selected for a successful resolver result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolverSource {
    /// `WinGet`'s structured Microsoft Store catalogue integration.
    WinGetMsStore,
}

/// Product fields reported by the platform adapter before core validates them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolverProduct {
    pub request_id: ResolveRequestId,
    pub product_id: StoreProductId,
    pub market: MarketCode,
    pub market_applied: bool,
    pub display_name: String,
    /// Package identity the catalogue named for this product, when it names one.
    ///
    /// It arrives before installation, which is what lets completion be proven
    /// against the identity that was asked for rather than against whatever
    /// appeared meanwhile.
    pub package_family_name: Option<PackageFamilyName>,
    pub publisher: Option<String>,
    pub icon: Option<ProductIcon>,
    pub install_classification: InstallClassification,
    pub version: Option<String>,
    pub size_bytes: Option<u64>,
    pub resolver_source: ResolverSource,
}

/// A resolver result that passed the exact-identity and display-data checks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedProduct {
    pub request_id: ResolveRequestId,
    pub product_id: StoreProductId,
    pub market: MarketCode,
    pub display_name: String,
    /// Package identity to observe once installation is under way.
    pub package_family_name: Option<PackageFamilyName>,
    pub publisher: Option<String>,
    pub icon: Option<ProductIcon>,
    pub install_classification: InstallClassification,
    pub version: Option<String>,
    pub size_bytes: Option<u64>,
    pub resolver_source: ResolverSource,
}

/// User-meaningful failure of a product resolver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolveError {
    ResolverUnavailable,
    SourceUnavailable,
    UnsupportedMarket,
    ProductNotFoundInMarket,
    NetworkUnavailable,
    PolicyDenied,
    InvalidResponse,
    ProductIdMismatch,
    Cancelled,
}

/// Non-localised diagnostic retained separately from resolver UI text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolverDiagnostic {
    /// Native or COM failure code, when the platform supplied one.
    pub native_code: Option<i32>,
}

/// A structured resolver failure with optional technical diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolveFailure {
    pub error: ResolveError,
    pub diagnostic: Option<ResolverDiagnostic>,
}

impl ResolveFailure {
    /// Construct a user-meaningful failure without platform diagnostics.
    #[must_use]
    pub const fn new(error: ResolveError) -> Self {
        Self {
            error,
            diagnostic: None,
        }
    }
}

/// Narrow, GUI-independent boundary for a structured product resolver.
pub trait ProductResolver {
    /// Resolve one already-normalised ID for one explicitly selected market.
    ///
    /// The caller runs this synchronous operation on a worker thread. Platform
    /// implementations must not return COM/WinRT values or console text.
    ///
    /// # Errors
    ///
    /// Returns a structured resolver failure without UI text or platform types.
    fn resolve(&self, request: &ResolveRequest) -> Result<ResolverProduct, ResolveFailure>;
}

/// Validate a structured adapter result against the original request.
///
/// # Errors
///
/// Returns [`ResolveError::InvalidResponse`] for missing required data or a
/// backend that did not explicitly apply the requested market, and
/// [`ResolveError::ProductIdMismatch`] for any correlation or identity mismatch.
pub fn resolve_product<R>(
    resolver: &R,
    request: &ResolveRequest,
) -> Result<ResolvedProduct, ResolveFailure>
where
    R: ProductResolver,
{
    let response = resolver.resolve(request)?;
    if response.request_id != request.request_id || response.product_id != request.product_id {
        return Err(ResolveFailure::new(ResolveError::ProductIdMismatch));
    }
    if response.market != request.market
        || !response.market_applied
        || response.display_name.trim().is_empty()
    {
        return Err(ResolveFailure::new(ResolveError::InvalidResponse));
    }
    Ok(ResolvedProduct {
        request_id: response.request_id,
        product_id: response.product_id,
        market: response.market,
        display_name: response.display_name,
        package_family_name: response.package_family_name,
        publisher: response.publisher,
        icon: response.icon,
        install_classification: response.install_classification,
        version: response.version,
        size_bytes: response.size_bytes,
        resolver_source: response.resolver_source,
    })
}

/// GUI-independent state that correlates asynchronous resolver results.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProductResolutionState {
    Idle,
    SyntaxAccepted {
        product_id: StoreProductId,
    },
    Resolving {
        request: ResolveRequest,
    },
    Resolved {
        product: ResolvedProduct,
    },
    Failed {
        request: ResolveRequest,
        failure: ResolveFailure,
    },
}

/// Generates requests and discards stale resolver completions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductResolutionSession {
    next_request_id: u64,
    state: ProductResolutionState,
}

impl Default for ProductResolutionSession {
    fn default() -> Self {
        Self {
            next_request_id: 1,
            state: ProductResolutionState::Idle,
        }
    }
}

impl ProductResolutionSession {
    /// Return the current presentation-safe resolution state.
    #[must_use]
    pub const fn state(&self) -> &ProductResolutionState {
        &self.state
    }

    /// Forget every previous request and accept only the new syntactic input.
    pub fn accept_syntax(&mut self, product_id: StoreProductId) {
        self.state = ProductResolutionState::SyntaxAccepted { product_id };
    }

    /// Invalidate a request when its source draft, ID, or market changes.
    pub fn invalidate(&mut self) {
        self.state = ProductResolutionState::Idle;
    }

    /// Start a request that a UI worker may pass to a platform adapter.
    #[must_use]
    pub fn begin(&mut self, product_id: StoreProductId, market: MarketCode) -> ResolveRequest {
        let request = ResolveRequest {
            request_id: ResolveRequestId::new(self.next_request_id),
            product_id,
            market,
        };
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.state = ProductResolutionState::Resolving {
            request: request.clone(),
        };
        request
    }

    /// Apply a worker result only when it belongs to the still-active request.
    ///
    /// Returns `false` for stale results, leaving the current state unchanged.
    pub fn complete(
        &mut self,
        request: &ResolveRequest,
        result: Result<ResolvedProduct, ResolveFailure>,
    ) -> bool {
        let ProductResolutionState::Resolving { request: active } = &self.state else {
            return false;
        };
        if active != request {
            return false;
        }
        self.state = match result {
            Ok(product)
                if product.request_id == request.request_id
                    && product.product_id == request.product_id
                    && product.market == request.market =>
            {
                ProductResolutionState::Resolved { product }
            }
            Ok(_) => ProductResolutionState::Failed {
                request: request.clone(),
                failure: ResolveFailure::new(ResolveError::ProductIdMismatch),
            },
            Err(failure) => ProductResolutionState::Failed {
                request: request.clone(),
                failure,
            },
        };
        true
    }

    /// Whether this boundary has a current resolved identity.
    #[must_use]
    pub const fn has_fresh_resolution(&self) -> bool {
        matches!(self.state, ProductResolutionState::Resolved { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::StoreProductId;
    use crate::install::InstallClassification;
    use crate::test_support::{FakeResolver, resolver_product, resolver_request};

    #[test]
    fn resolver_requires_exact_id_requested_market_and_a_display_name() {
        let request = resolver_request();
        let resolver = FakeResolver {
            result: Ok(resolver_product(&request)),
        };
        let product = resolve_product(&resolver, &request).expect("complete exact response");
        assert_eq!(product.product_id, request.product_id);
        assert_eq!(product.market, request.market);
        assert_eq!(
            product.install_classification,
            InstallClassification::unknown()
        );
        assert_eq!(product.version, None);
        assert_eq!(product.size_bytes, None);

        let mut mismatched = resolver_product(&request);
        mismatched.product_id = StoreProductId::parse("9NKSQGP7F2NH").expect("valid mismatch ID");
        assert_eq!(
            resolve_product(
                &FakeResolver {
                    result: Ok(mismatched)
                },
                &request
            ),
            Err(ResolveFailure::new(ResolveError::ProductIdMismatch))
        );

        let mut no_market_confirmation = resolver_product(&request);
        no_market_confirmation.market_applied = false;
        assert_eq!(
            resolve_product(
                &FakeResolver {
                    result: Ok(no_market_confirmation),
                },
                &request
            ),
            Err(ResolveFailure::new(ResolveError::InvalidResponse))
        );

        let mut empty_name = resolver_product(&request);
        empty_name.display_name = "  ".to_owned();
        assert_eq!(
            resolve_product(
                &FakeResolver {
                    result: Ok(empty_name)
                },
                &request
            ),
            Err(ResolveFailure::new(ResolveError::InvalidResponse))
        );
    }

    #[test]
    fn all_resolver_failures_cross_the_boundary_without_native_text() {
        for error in [
            ResolveError::ResolverUnavailable,
            ResolveError::SourceUnavailable,
            ResolveError::UnsupportedMarket,
            ResolveError::ProductNotFoundInMarket,
            ResolveError::NetworkUnavailable,
            ResolveError::PolicyDenied,
            ResolveError::InvalidResponse,
            ResolveError::ProductIdMismatch,
            ResolveError::Cancelled,
        ] {
            let request = resolver_request();
            assert_eq!(
                resolve_product(
                    &FakeResolver {
                        result: Err(ResolveFailure::new(error)),
                    },
                    &request
                ),
                Err(ResolveFailure::new(error))
            );
        }
    }

    #[test]
    fn resolution_session_ignores_stale_results_and_only_accepts_a_current_card() {
        let mut session = ProductResolutionSession::default();
        let first = resolver_request();
        session.accept_syntax(first.product_id.clone());
        let active = session.begin(first.product_id.clone(), first.market.clone());
        let stale = ResolveRequest {
            request_id: ResolveRequestId::new(active.request_id.value() + 1),
            product_id: active.product_id.clone(),
            market: active.market.clone(),
        };
        let stale_product = ResolvedProduct {
            request_id: stale.request_id,
            product_id: stale.product_id.clone(),
            market: stale.market.clone(),
            display_name: "Stale".to_owned(),
            package_family_name: None,
            publisher: None,
            icon: None,
            install_classification: InstallClassification::unknown(),
            version: None,
            size_bytes: None,
            resolver_source: ResolverSource::WinGetMsStore,
        };
        assert!(!session.complete(&stale, Ok(stale_product)));
        assert!(matches!(
            session.state(),
            ProductResolutionState::Resolving { .. }
        ));
        session.invalidate();
        assert!(!session.has_fresh_resolution());

        let second = session.begin(first.product_id, first.market);
        let second_product = ResolvedProduct {
            request_id: second.request_id,
            product_id: second.product_id.clone(),
            market: second.market.clone(),
            display_name: "Current".to_owned(),
            package_family_name: None,
            publisher: None,
            icon: None,
            install_classification: InstallClassification::unknown(),
            version: None,
            size_bytes: None,
            resolver_source: ResolverSource::WinGetMsStore,
        };
        assert!(session.complete(&second, Ok(second_product)));
        assert!(session.has_fresh_resolution());
    }
}
