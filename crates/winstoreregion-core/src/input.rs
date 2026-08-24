//! Parsing of Store text input into a normalised product identifier.

/// A normalised Microsoft Store product identifier awaiting resolver verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreProductId(String);

impl StoreProductId {
    /// Parse a syntactically valid Product ID without claiming that it exists.
    ///
    /// # Errors
    ///
    /// Returns [`StoreInputError`] for an empty, too long, or non-alphanumeric value.
    pub fn parse(value: &str) -> Result<Self, StoreInputError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(StoreInputError::Empty);
        }
        if value.len() > 64 || !value.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            return Err(StoreInputError::InvalidProductId);
        }
        Ok(Self(value.to_ascii_uppercase()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The supported textual forms that resolve to one Store Product ID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreInputCandidate {
    ProductId(StoreProductId),
    AppsMicrosoftComUrl(StoreProductId),
    StoreUri(StoreProductId),
}

impl StoreInputCandidate {
    /// Parse a Product ID, canonical apps.microsoft.com URL, or Store URI.
    ///
    /// # Errors
    ///
    /// Returns [`StoreInputError`] when the text does not encode one supported Product ID.
    pub fn parse(value: &str) -> Result<Self, StoreInputError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(StoreInputError::Empty);
        }
        if value
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
        {
            return Err(StoreInputError::InvalidProductId);
        }
        if value.contains('#') {
            return Err(StoreInputError::UnsupportedLocation);
        }
        if let Some(rest) = strip_prefix_ascii_case(value, "https://apps.microsoft.com/") {
            return extract_product_id(rest).map(Self::AppsMicrosoftComUrl);
        }
        if let Some(rest) = strip_prefix_ascii_case(value, "ms-windows-store://") {
            return extract_product_id(rest).map(Self::StoreUri);
        }
        if value.contains("://") {
            return Err(StoreInputError::UnsupportedLocation);
        }
        StoreProductId::parse(value).map(Self::ProductId)
    }

    #[must_use]
    pub fn product_id(&self) -> &StoreProductId {
        match self {
            Self::ProductId(id) | Self::AppsMicrosoftComUrl(id) | Self::StoreUri(id) => id,
        }
    }
}

/// Syntax error while parsing a Store text input; not a resolver result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreInputError {
    Empty,
    InvalidProductId,
    MissingProductId,
    UnsupportedLocation,
}

fn strip_prefix_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))
        .map(|_| &value[prefix.len()..])
}

fn extract_product_id(value: &str) -> Result<StoreProductId, StoreInputError> {
    let query = value.split_once('?').map_or("", |(_, query)| query);
    if let Some(product) = query.split('&').find_map(|item| {
        item.split_once('=')
            .filter(|(key, _)| key.eq_ignore_ascii_case("productid"))
            .map(|(_, value)| value)
    }) {
        return StoreProductId::parse(product);
    }
    let path = value.split('?').next().unwrap_or_default();
    let parts: Vec<_> = path.split('/').filter(|part| !part.is_empty()).collect();
    if let Some(detail_index) = parts
        .iter()
        .position(|part| part.eq_ignore_ascii_case("detail"))
    {
        // Not simply the last segment: a page of the product — its reviews, its
        // system requirements — is a segment too, and taking it produced a
        // product named REVIEWS out of an address the user pasted intact. Only
        // a segment shaped like a catalogue identifier is accepted, which also
        // steps over the slug in `/detail/example-app/9WZDNCRFJ3PZ`.
        return parts
            .get(detail_index + 1..)
            .and_then(|tail| tail.iter().rev().find(|part| is_catalogue_identifier(part)))
            .map_or(Err(StoreInputError::MissingProductId), |product| {
                StoreProductId::parse(product)
            });
    }
    Err(StoreInputError::MissingProductId)
}

/// Length of a Microsoft Store catalogue identifier.
const CATALOGUE_IDENTIFIER_LENGTH: usize = 12;

/// Whether a path segment has the shape of a Store catalogue identifier.
///
/// The shape is not a promise that the product exists — only the catalogue can
/// say that. It is enough to tell an identifier from the other segments a
/// product address carries, which is all this parser has to decide.
fn is_catalogue_identifier(part: &str) -> bool {
    part.len() == CATALOGUE_IDENTIFIER_LENGTH
        && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_input_normalizes_all_supported_text_forms_without_resolving_them() {
        assert_eq!(
            StoreInputCandidate::parse("abc1")
                .unwrap()
                .product_id()
                .as_str(),
            "ABC1"
        );
        assert_eq!(
            StoreInputCandidate::parse("https://apps.microsoft.com/detail/9wzdncrfj3pz")
                .unwrap()
                .product_id()
                .as_str(),
            "9WZDNCRFJ3PZ"
        );
        assert_eq!(
            StoreInputCandidate::parse(
                "https://apps.microsoft.com/detail/example-app/9wzdncrfj3pz"
            )
            .unwrap()
            .product_id()
            .as_str(),
            "9WZDNCRFJ3PZ"
        );
        assert_eq!(
            StoreInputCandidate::parse("ms-windows-store://pdp/?productid=9wzdncrfj3pz")
                .unwrap()
                .product_id()
                .as_str(),
            "9WZDNCRFJ3PZ"
        );
        assert_eq!(
            StoreInputCandidate::parse("https://www.apps.microsoft.com/detail/9WZDNCRFJ3PZ"),
            Err(StoreInputError::UnsupportedLocation)
        );
        assert_eq!(
            StoreInputCandidate::parse("ms-windows-store://pdp/"),
            Err(StoreInputError::MissingProductId)
        );
        assert_eq!(
            StoreInputCandidate::parse(
                "HTTPS://APPS.MICROSOFT.COM/detail/9wzdncrfj3pz?hl=en-us&gl=US"
            )
            .unwrap()
            .product_id()
            .as_str(),
            "9WZDNCRFJ3PZ"
        );
        assert_eq!(
            StoreInputCandidate::parse(
                "https://apps.microsoft.com/store/detail/example/ignored?productid=abc123"
            )
            .unwrap()
            .product_id()
            .as_str(),
            "ABC123"
        );
    }

    #[test]
    fn a_page_of_the_product_is_not_the_product() {
        // Pasting the address of a subpage is ordinary; reading its name as the
        // product turned that into two false diagnostics, one about a product
        // that does not exist and one about the region it was not found in.
        assert_eq!(
            StoreInputCandidate::parse("https://apps.microsoft.com/detail/9WZDNCRFJ3PZ/reviews")
                .unwrap()
                .product_id()
                .as_str(),
            "9WZDNCRFJ3PZ"
        );
        assert_eq!(
            StoreInputCandidate::parse(
                "https://apps.microsoft.com/detail/example-app/9wzdncrfj3pz/system-requirements"
            )
            .unwrap()
            .product_id()
            .as_str(),
            "9WZDNCRFJ3PZ"
        );
        // Nothing in this address is shaped like an identifier, and inventing
        // one out of the slug is what this rule exists to stop.
        assert_eq!(
            StoreInputCandidate::parse("https://apps.microsoft.com/detail/example-app"),
            Err(StoreInputError::MissingProductId)
        );
    }

    #[test]
    fn store_input_rejects_unsupported_or_ambiguous_text() {
        assert_eq!(
            StoreInputCandidate::parse("https://apps.microsoft.com/detail/9WZDNCRFJ3PZ#fragment"),
            Err(StoreInputError::UnsupportedLocation)
        );
        assert_eq!(
            StoreInputCandidate::parse("https://apps.microsoft.com.evil/detail/9WZDNCRFJ3PZ"),
            Err(StoreInputError::UnsupportedLocation)
        );
        assert_eq!(
            StoreInputCandidate::parse("AB CD"),
            Err(StoreInputError::InvalidProductId)
        );
        assert_eq!(
            StoreInputCandidate::parse("A".repeat(65).as_str()),
            Err(StoreInputError::InvalidProductId)
        );
    }
}
