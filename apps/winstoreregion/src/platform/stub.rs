//! Read-only inspection of a Store installer stub.
//!
//! This module never loads, launches, or executes the file it inspects; a test
//! asserts that no process-, shell-, or library-loading API appears here.

use crate::platform::storage::wide_path;
use sha2::{Digest, Sha256};
use std::ffi::CStr;
use std::fs::File;
use std::io::Read;
use std::mem::size_of;
use std::path::Path;
use std::time::UNIX_EPOCH;
use windows::Win32::Foundation::{
    CERT_E_REVOCATION_FAILURE, CRYPT_E_REVOCATION_OFFLINE, TRUST_E_BAD_DIGEST, TRUST_E_NOSIGNATURE,
};
use windows::Win32::Security::Cryptography::{
    CERT_CONTEXT, CERT_NAME_ISSUER_FLAG, CERT_NAME_SIMPLE_DISPLAY_TYPE, CERT_SHA256_HASH_PROP_ID,
    CTL_USAGE, CertGetCertificateContextProperty, CertGetEnhancedKeyUsage, CertGetNameStringW,
    szOID_PKIX_KP_CODE_SIGNING,
};
use windows::Win32::Security::WinTrust::{
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
    WTD_CHOICE_FILE, WTD_DISABLE_MD2_MD4, WTD_REVOCATION_CHECK_CHAIN, WTD_STATEACTION_CLOSE,
    WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvCertFromChain,
    WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
};
use windows::Win32::System::Time::FileTimeToSystemTime;
use windows::core::PCWSTR;
use winstoreregion_core::{
    SignatureStatus, SignerInfo, StoreStubStatus, StubFileIdentity, StubInspection,
    StubInspectionWarning, matches_microsoft_store_signer,
};

/// Structured failures from the read-only stub-inspection boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StubInspectionError {
    Io,
    ChangedDuringInspection,
}

/// Inspect a selected executable without loading or executing it.
///
/// The current format probe has intentionally not authorised a static Product
/// ID extractor yet. This function therefore never produces a Product ID or
/// `VerifiedStoreStub`; it establishes the identity and Authenticode facts
/// that such an extractor will be bound to.
pub(crate) fn inspect_installer_stub(path: &Path) -> Result<StubInspection, StubInspectionError> {
    let identity = read_file_identity(path)?;
    let verification = verify_authenticode(path);
    let after = read_file_identity(path)?;
    if identity != after {
        return Err(StubInspectionError::ChangedDuringInspection);
    }
    let mut warnings = vec![StubInspectionWarning::ProductIdNotAvailable];
    if verification.native_code != 0 {
        warnings.push(StubInspectionWarning::NativeVerificationCode(
            verification.native_code,
        ));
    }
    if verification.status == SignatureStatus::Trusted && verification.signer.is_none() {
        warnings.push(StubInspectionWarning::SignerDataUnavailable);
    }
    if verification.status == SignatureStatus::Trusted
        && verification
            .signer
            .as_ref()
            .is_some_and(|signer| !matches_microsoft_store_signer(signer))
    {
        warnings.push(StubInspectionWarning::UnexpectedSigner);
    }
    Ok(StubInspection {
        file_identity: identity,
        signature_status: verification.status,
        signer: verification.signer,
        store_stub_status: StoreStubStatus::NotStoreStub,
        product_id: None,
        warnings,
    })
}

fn read_file_identity(path: &Path) -> Result<StubFileIdentity, StubInspectionError> {
    let mut file = File::open(path).map_err(|_| StubInspectionError::Io)?;
    let before = file.metadata().map_err(|_| StubInspectionError::Io)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| StubInspectionError::Io)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let after = file.metadata().map_err(|_| StubInspectionError::Io)?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(StubInspectionError::ChangedDuringInspection);
    }
    let modified_unix_nanos = before
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos());
    Ok(StubFileIdentity {
        path: path.to_path_buf(),
        byte_len: before.len(),
        modified_unix_nanos,
        sha256: format!("{:x}", hasher.finalize()),
    })
}

struct AuthenticodeVerification {
    status: SignatureStatus,
    native_code: i32,
    signer: Option<SignerInfo>,
}

#[allow(unsafe_code)]
fn verify_authenticode(path: &Path) -> AuthenticodeVerification {
    let path = wide_path(path);
    let mut file_info = WINTRUST_FILE_INFO {
        cbStruct: u32::try_from(std::mem::size_of::<WINTRUST_FILE_INFO>()).unwrap_or(u32::MAX),
        pcwszFilePath: PCWSTR(path.as_ptr()),
        ..Default::default()
    };
    let mut data = WINTRUST_DATA {
        cbStruct: u32::try_from(std::mem::size_of::<WINTRUST_DATA>()).unwrap_or(u32::MAX),
        dwUIChoice: WTD_UI_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 {
            pFile: &raw mut file_info,
        },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_REVOCATION_CHECK_CHAIN | WTD_DISABLE_MD2_MD4,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let result = unsafe {
        WinVerifyTrust(
            windows::Win32::Foundation::HWND::default(),
            &raw mut action,
            (&raw mut data).cast(),
        )
    };
    let signer = if result == 0 {
        unsafe { signer_from_wintrust_state(data.hWVTStateData) }
    } else {
        None
    };
    // WinTrust owns the provider certificate context with this state. It is
    // released only by this close call, after signer data has been copied.
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    let _ = unsafe {
        WinVerifyTrust(
            windows::Win32::Foundation::HWND::default(),
            &raw mut action,
            (&raw mut data).cast(),
        )
    };
    let status = signature_status_from_wintrust_code(result);
    AuthenticodeVerification {
        status,
        native_code: result,
        signer,
    }
}

fn signature_status_from_wintrust_code(result: i32) -> SignatureStatus {
    if result == 0 {
        SignatureStatus::Trusted
    } else if result == TRUST_E_NOSIGNATURE.0 {
        SignatureStatus::Unsigned
    } else if result == CERT_E_REVOCATION_FAILURE.0 || result == CRYPT_E_REVOCATION_OFFLINE.0 {
        SignatureStatus::RevocationUnavailable
    } else if result == TRUST_E_BAD_DIGEST.0 {
        SignatureStatus::Invalid
    } else {
        SignatureStatus::VerificationUnavailable
    }
}

#[allow(unsafe_code)]
unsafe fn signer_from_wintrust_state(
    state: windows::Win32::Foundation::HANDLE,
) -> Option<SignerInfo> {
    let provider = unsafe { WTHelperProvDataFromStateData(state) };
    if provider.is_null() {
        return None;
    }
    let signer = unsafe { WTHelperGetProvSignerFromChain(provider, 0, false, 0) };
    if signer.is_null() {
        return None;
    }
    let certificate = unsafe { WTHelperGetProvCertFromChain(signer, 0) };
    if certificate.is_null() || unsafe { (*certificate).pCert.is_null() } {
        return None;
    }
    unsafe { signer_info_from_certificate((*certificate).pCert) }
}

#[allow(unsafe_code)]
unsafe fn signer_info_from_certificate(context: *const CERT_CONTEXT) -> Option<SignerInfo> {
    let info = unsafe { (*context).pCertInfo };
    if info.is_null() {
        return None;
    }
    let serial = unsafe {
        let serial = &(*info).SerialNumber;
        if serial.pbData.is_null() && serial.cbData != 0 {
            return None;
        }
        uppercase_hex_reversed(std::slice::from_raw_parts(
            serial.pbData,
            usize::try_from(serial.cbData).ok()?,
        ))
    };
    Some(SignerInfo {
        subject: unsafe { certificate_name(context, 0) }?,
        issuer: unsafe { certificate_name(context, CERT_NAME_ISSUER_FLAG) }?,
        serial_number: serial,
        sha256_thumbprint: unsafe { certificate_thumbprint(context) }?,
        valid_from: unsafe { certificate_time((*info).NotBefore) }?,
        valid_to: unsafe { certificate_time((*info).NotAfter) }?,
        has_code_signing_eku: unsafe { certificate_has_code_signing_eku(context) },
    })
}

#[allow(unsafe_code)]
unsafe fn certificate_name(context: *const CERT_CONTEXT, flags: u32) -> Option<String> {
    let length =
        unsafe { CertGetNameStringW(context, CERT_NAME_SIMPLE_DISPLAY_TYPE, flags, None, None) };
    if length == 0 {
        return None;
    }
    let mut buffer = vec![0_u16; usize::try_from(length).ok()?];
    let copied = unsafe {
        CertGetNameStringW(
            context,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            flags,
            None,
            Some(&mut buffer),
        )
    };
    if copied == 0 {
        return None;
    }
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..end]))
}

#[allow(unsafe_code)]
unsafe fn certificate_thumbprint(context: *const CERT_CONTEXT) -> Option<String> {
    let mut length = 0_u32;
    unsafe {
        CertGetCertificateContextProperty(context, CERT_SHA256_HASH_PROP_ID, None, &raw mut length)
    }
    .ok()?;
    let mut thumbprint = vec![0_u8; usize::try_from(length).ok()?];
    unsafe {
        CertGetCertificateContextProperty(
            context,
            CERT_SHA256_HASH_PROP_ID,
            Some(thumbprint.as_mut_ptr().cast()),
            &raw mut length,
        )
    }
    .ok()?;
    Some(uppercase_hex(&thumbprint))
}

#[allow(unsafe_code)]
unsafe fn certificate_time(time: windows::Win32::Foundation::FILETIME) -> Option<String> {
    let mut system_time = windows::Win32::Foundation::SYSTEMTIME::default();
    unsafe { FileTimeToSystemTime(&raw const time, &raw mut system_time) }.ok()?;
    Some(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        system_time.wYear,
        system_time.wMonth,
        system_time.wDay,
        system_time.wHour,
        system_time.wMinute,
        system_time.wSecond
    ))
}

#[allow(unsafe_code)]
unsafe fn certificate_has_code_signing_eku(context: *const CERT_CONTEXT) -> bool {
    let mut required = 0_u32;
    if unsafe { CertGetEnhancedKeyUsage(context, 0, None, &raw mut required) }.is_err()
        || required == 0
    {
        return false;
    }
    let word_count = usize::try_from(required)
        .ok()
        .and_then(|bytes| bytes.checked_add(size_of::<usize>() - 1))
        .map(|bytes| bytes / size_of::<usize>());
    let Some(word_count) = word_count else {
        return false;
    };
    let mut storage = vec![0_usize; word_count];
    let usage = storage.as_mut_ptr().cast::<CTL_USAGE>();
    if unsafe { CertGetEnhancedKeyUsage(context, 0, Some(usage), &raw mut required) }.is_err() {
        return false;
    }
    let count = unsafe { usize::try_from((*usage).cUsageIdentifier).unwrap_or_default() };
    if unsafe { (*usage).rgpszUsageIdentifier.is_null() } && count != 0 {
        return false;
    }
    let identifiers = unsafe { std::slice::from_raw_parts((*usage).rgpszUsageIdentifier, count) };
    identifiers.iter().any(|identifier| unsafe {
        !identifier.0.is_null()
            && CStr::from_ptr(identifier.0.cast())
                .to_bytes()
                .eq(szOID_PKIX_KP_CODE_SIGNING.as_bytes())
    })
}

fn uppercase_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut text = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        text.push(char::from(HEX[usize::from(*byte >> 4)]));
        text.push(char::from(HEX[usize::from(*byte & 0x0f)]));
    }
    text
}

fn uppercase_hex_reversed(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut text = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes.iter().rev() {
        text.push(char::from(HEX[usize::from(*byte >> 4)]));
        text.push(char::from(HEX[usize::from(*byte & 0x0f)]));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    #[test]
    fn inspection_hashes_the_test_executable_without_executing_or_classifying_it() {
        let executable = std::env::current_exe().expect("test executable path");
        let inspection = inspect_installer_stub(&executable).expect("read-only inspection");

        assert_eq!(inspection.file_identity.path, executable);
        assert_eq!(inspection.file_identity.sha256.len(), 64);
        assert!(inspection.product_id.is_none());
        assert_eq!(
            inspection.store_stub_status,
            winstoreregion_core::StoreStubStatus::NotStoreStub
        );
    }

    #[test]
    fn native_wintrust_codes_stay_structured_signature_statuses() {
        assert_eq!(
            signature_status_from_wintrust_code(0),
            winstoreregion_core::SignatureStatus::Trusted
        );
        assert_eq!(
            signature_status_from_wintrust_code(super::TRUST_E_NOSIGNATURE.0),
            winstoreregion_core::SignatureStatus::Unsigned
        );
        assert_eq!(
            signature_status_from_wintrust_code(super::TRUST_E_BAD_DIGEST.0),
            winstoreregion_core::SignatureStatus::Invalid
        );
        assert_eq!(
            signature_status_from_wintrust_code(super::CRYPT_E_REVOCATION_OFFLINE.0),
            winstoreregion_core::SignatureStatus::RevocationUnavailable
        );
        assert_eq!(
            signature_status_from_wintrust_code(-1),
            winstoreregion_core::SignatureStatus::VerificationUnavailable
        );
    }

    #[test]
    fn inspection_module_has_no_executable_loading_or_launch_api() {
        // The whole module is the inspection boundary now that stub
        // inspection owns its own file; no marker slicing is needed.
        let inspection_source = include_str!("stub.rs");
        let forbidden = [
            ["Create", "Process"].concat(),
            ["Shell", "Execute"].concat(),
            ["Load", "Library"].concat(),
        ];
        for forbidden in forbidden {
            assert!(
                !inspection_source.contains(&forbidden),
                "inspection boundary must not call {forbidden}"
            );
        }
    }

    #[test]
    fn probe_samples_are_trusted_microsoft_store_stubs_but_have_no_open_product_id_field() {
        let samples = [
            (
                "windows-app.exe",
                "79943BE5B57AE1330A649CD41B8D6ED852469A4A2216552672D211D7C6C9E3E0",
            ),
            (
                "whatsapp.exe",
                "97EC678A159B6FF971F11B8A9F85577614E555C2B8377765EDCE1D9F87FC360B",
            ),
        ];
        let root = PathBuf::from(r"C:\temp\WinStoreRegion\stub-probe");
        for (name, digest) in samples {
            let path = root.join(name);
            if !path.exists() {
                return;
            }
            let inspection = inspect_installer_stub(&path).expect("read-only probe");
            assert_eq!(inspection.file_identity.sha256.to_ascii_uppercase(), digest);
            assert_eq!(
                inspection.signature_status,
                winstoreregion_core::SignatureStatus::Trusted
            );
            let signer = inspection.signer.expect("trusted sample signer");
            assert!(winstoreregion_core::matches_microsoft_store_signer(&signer));
            assert!(inspection.product_id.is_none());
            assert_eq!(
                inspection.store_stub_status,
                winstoreregion_core::StoreStubStatus::NotStoreStub
            );
        }
    }

    #[test]
    fn ordinary_microsoft_signed_executable_is_not_a_store_stub() {
        let windows_directory = std::env::var_os("WINDIR").expect("Windows directory");
        let notepad = PathBuf::from(windows_directory).join(r"System32\notepad.exe");
        let inspection = inspect_installer_stub(&notepad).expect("read-only inspection");

        assert!(inspection.product_id.is_none());
        assert_eq!(
            inspection.store_stub_status,
            winstoreregion_core::StoreStubStatus::NotStoreStub
        );
    }
}
