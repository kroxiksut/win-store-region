//! Fetching the Microsoft Store installer for one product, over `WinHTTP`.
//!
//! The host answers with the same signed stub a person receives from the Store
//! web page, needs no account, and does not depend on the region the machine is
//! in. That last fact is what makes this useful here — the file can be fetched while the
//! machine still holds the user's own region, and only its execution needs the
//! temporary one.
//!
//! Nothing in this module decides that the file may run. It downloads and names
//! a file; admission stays where it already is, in `core::source`, and the file
//! goes through exactly the same gate as one the user picked by hand.

use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use windows::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_OPEN_REQUEST_FLAGS, WINHTTP_QUERY_CONTENT_DISPOSITION, WINHTTP_QUERY_FLAG_NUMBER,
    WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetTimeouts,
};
use windows::core::{HSTRING, PCWSTR};
use winstoreregion_core::{APPLICATION_NAME, StoreProductId};

/// Host that serves the Store installer stub.
const HOST: &str = "get.microsoft.com";

/// Largest file this module will write.
///
/// The measured stub is 815 136 bytes and the older build was 1 462 848. Eight
/// megabytes leaves room for a larger one without letting a surprising answer
/// fill the disk.
const MAX_BYTES: usize = 8 * 1024 * 1024;

/// Size of one read from the response stream.
const READ_CHUNK_BYTES: usize = 64 * 1024;

/// Timeouts in milliseconds: resolve, connect, send, receive.
///
/// Longer than the market probe's, because this transfers a file rather than a
/// short answer, and a user who asked for a download is waiting on purpose.
const TIMEOUTS: (i32, i32, i32, i32) = (10_000, 10_000, 30_000, 60_000);

/// Directory the downloaded installers are kept in, under the app's own data.
const INSTALLER_DIRECTORY: &str = "installers";

/// Why an installer could not be obtained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallerDownloadError {
    /// `WinHTTP` could not be used at all.
    TransportUnavailable,
    /// The request was made and the host answered something other than success.
    Refused {
        /// HTTP status the host answered with, when there was one.
        status: Option<u32>,
    },
    /// The answer was larger than this module will write.
    TooLarge,
    /// The file could not be written to the per-user data directory.
    NotWritable,
}

/// One downloaded installer, ready to be inspected by the ordinary gate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DownloadedInstaller {
    /// Where the file was written.
    pub(crate) path: PathBuf,
    /// Product this file was asked for, kept so the window can say so.
    pub(crate) product_id: StoreProductId,
}

/// An open `WinHTTP` handle that closes itself.
struct Handle(*mut c_void);

impl Handle {
    fn new(raw: *mut c_void) -> Option<Self> {
        (!raw.is_null()).then_some(Self(raw))
    }

    const fn raw(&self) -> *mut c_void {
        self.0
    }
}

impl Drop for Handle {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

/// Fetch the Store installer for one product and write it under the user's data.
///
/// The file is written whole before it is named: a partial download that ends
/// with the right name would be offered to the admission gate as if it were a
/// complete file, and a truncated signature is not a refusal anybody could
/// explain.
#[allow(unsafe_code)]
pub(crate) fn download_store_installer(
    product_id: &StoreProductId,
) -> Result<DownloadedInstaller, InstallerDownloadError> {
    let (body, disposition) = fetch(product_id)?;
    let directory = installer_directory().ok_or(InstallerDownloadError::NotWritable)?;
    fs::create_dir_all(&directory).map_err(|_| InstallerDownloadError::NotWritable)?;
    let name = file_name(product_id, disposition.as_deref());
    let path = directory.join(&name);
    write_whole(&path, &body)?;
    Ok(DownloadedInstaller {
        path,
        product_id: product_id.clone(),
    })
}

/// Remove every installer this application has downloaded.
///
/// Nothing here is worth keeping. The file is fetched by Product ID, needs no
/// account and does not depend on the machine's region, so it can always be
/// obtained again — and a folder of Store installers is exactly the kind of
/// thing a user neither asked for nor knows to clean up.
///
/// Best-effort by design: a stub that is still running holds its own file open,
/// and Windows will not delete it. That is why this also runs at startup, when
/// the previous run's installer is no longer executing. A failure to delete
/// changes no outcome and is never reported as one.
pub(crate) fn sweep_downloaded_installers() {
    let Some(directory) = installer_directory() else {
        return;
    };
    sweep_directory(&directory);
}

/// Delete every file directly inside one directory, keeping the directory.
///
/// Only files, and only one level deep: this removes what this application
/// wrote and nothing else. A locked or unreadable entry is skipped.
fn sweep_directory(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_file()) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Where downloaded installers live.
fn installer_directory() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|base| {
        PathBuf::from(base)
            .join(winstoreregion_core::LOCAL_DATA_DIRECTORY)
            .join(INSTALLER_DIRECTORY)
    })
}

/// Replace whatever is at the path with exactly these bytes.
///
/// A previous download of the same product is overwritten rather than kept: two
/// downloads differ in bytes that identify nothing, so
/// keeping both would only accumulate copies of the same installer.
fn write_whole(path: &Path, body: &[u8]) -> Result<(), InstallerDownloadError> {
    // Written under a name nothing looks for and renamed into place afterwards.
    // The admission gate takes the file it finds at the final name, and a write
    // that stopped halfway would leave a partial installer sitting under
    // exactly that name — which is the one thing the comment above promises
    // cannot happen. The rename replaces an existing file in one operation.
    let staging = path.with_extension("partial");
    if fs::write(&staging, body).is_err() {
        let _ = fs::remove_file(&staging);
        return Err(InstallerDownloadError::NotWritable);
    }
    if fs::rename(&staging, path).is_err() {
        let _ = fs::remove_file(&staging);
        return Err(InstallerDownloadError::NotWritable);
    }
    Ok(())
}

/// The name to store the file under.
///
/// The host sends the name a person would have received from the Store page, so
/// that name is used when it is safe to. It is never trusted as a path: only a
/// bare file name is taken, and anything else falls back to the Product ID,
/// which is always a safe name because its own syntax is already validated.
fn file_name(product_id: &StoreProductId, disposition: Option<&str>) -> String {
    let fallback = || format!("{} Installer.exe", product_id.as_str());
    let Some(offered) = disposition.and_then(disposition_file_name) else {
        return fallback();
    };
    // A name with a separator in it is a path, and a path is not a name.
    if offered.is_empty()
        || offered.contains(['/', '\\', ':'])
        || offered.starts_with('.')
        || !offered.to_ascii_lowercase().ends_with(".exe")
    {
        return fallback();
    }
    offered
}

/// Pull the file name out of a `Content-Disposition` header.
///
/// Only the plain `filename="..."` form is read. The `filename*` form carries a
/// character set and percent-encoding, and decoding it to gain a prettier name
/// is not worth the surface: the plain form has always been present beside it.
fn disposition_file_name(header: &str) -> Option<String> {
    let start = header.to_ascii_lowercase().find("filename=")?;
    let rest = header.get(start + "filename=".len()..)?.trim_start();
    let unquoted = rest.strip_prefix('"').map_or_else(
        || rest.split(';').next().unwrap_or_default(),
        |quoted| quoted.split('"').next().unwrap_or_default(),
    );
    Some(unquoted.trim().to_owned())
}

/// Perform the request, returning status, body, and the disposition header.
#[allow(unsafe_code)]
fn fetch(product_id: &StoreProductId) -> Result<(Vec<u8>, Option<String>), InstallerDownloadError> {
    let unavailable = InstallerDownloadError::TransportUnavailable;
    let agent = HSTRING::from(APPLICATION_NAME);
    let session = Handle::new(unsafe {
        WinHttpOpen(
            PCWSTR(agent.as_ptr()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        )
    })
    .ok_or(unavailable)?;
    let (resolve, connect, send, receive) = TIMEOUTS;
    unsafe { WinHttpSetTimeouts(session.raw(), resolve, connect, send, receive) }
        .map_err(|_| unavailable)?;

    let host = HSTRING::from(HOST);
    let connection = Handle::new(unsafe {
        WinHttpConnect(
            session.raw(),
            PCWSTR(host.as_ptr()),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        )
    })
    .ok_or(unavailable)?;

    let verb = HSTRING::from("GET");
    let object = HSTRING::from(format!("/installer/download/{}", product_id.as_str()));
    let request = Handle::new(unsafe {
        WinHttpOpenRequest(
            connection.raw(),
            PCWSTR(verb.as_ptr()),
            PCWSTR(object.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_OPEN_REQUEST_FLAGS(WINHTTP_FLAG_SECURE.0),
        )
    })
    .ok_or(unavailable)?;

    unsafe { WinHttpSendRequest(request.raw(), None, None, 0, 0, 0) }
        .map_err(|_| InstallerDownloadError::Refused { status: None })?;
    unsafe { WinHttpReceiveResponse(request.raw(), std::ptr::null_mut()) }
        .map_err(|_| InstallerDownloadError::Refused { status: None })?;

    let mut status: u32 = 0;
    let mut status_size = u32::try_from(size_of::<u32>()).unwrap_or(4);
    unsafe {
        WinHttpQueryHeaders(
            request.raw(),
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(std::ptr::from_mut(&mut status).cast::<c_void>()),
            &raw mut status_size,
            std::ptr::null_mut(),
        )
    }
    .map_err(|_| InstallerDownloadError::Refused { status: None })?;

    // The status is answered before a single byte of the body is read. An error
    // page is a body too, and reading it to the cap would spend megabytes of
    // the user's connection on text nothing will look at.
    if status != 200 {
        return Err(InstallerDownloadError::Refused {
            status: Some(status),
        });
    }

    let disposition = header_text(&request, WINHTTP_QUERY_CONTENT_DISPOSITION);
    let body = read_body(&request)?;
    Ok((body, disposition))
}

/// Read one response header as text, or nothing when it is absent.
#[allow(unsafe_code)]
fn header_text(request: &Handle, header: u32) -> Option<String> {
    let mut size: u32 = 0;
    // The first call is expected to fail: it reports the size that is needed.
    let _ = unsafe {
        WinHttpQueryHeaders(
            request.raw(),
            header,
            PCWSTR::null(),
            None,
            &raw mut size,
            std::ptr::null_mut(),
        )
    };
    if size == 0 {
        return None;
    }
    let mut buffer = vec![0u16; (size as usize / 2) + 1];
    unsafe {
        WinHttpQueryHeaders(
            request.raw(),
            header,
            PCWSTR::null(),
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            &raw mut size,
            std::ptr::null_mut(),
        )
    }
    .ok()?;
    let end = buffer.iter().position(|unit| *unit == 0)?;
    Some(String::from_utf16_lossy(&buffer[..end]))
}

/// Read the whole body, refusing anything past the cap.
#[allow(unsafe_code)]
fn read_body(request: &Handle) -> Result<Vec<u8>, InstallerDownloadError> {
    let mut body = Vec::new();
    let mut chunk = vec![0u8; READ_CHUNK_BYTES];
    loop {
        let mut read: u32 = 0;
        if unsafe {
            WinHttpReadData(
                request.raw(),
                chunk.as_mut_ptr().cast::<c_void>(),
                u32::try_from(chunk.len()).unwrap_or(0),
                &raw mut read,
            )
        }
        .is_err()
        {
            return Err(InstallerDownloadError::Refused { status: None });
        }
        if read == 0 {
            return Ok(body);
        }
        let read = read as usize;
        if body.len() + read > MAX_BYTES {
            return Err(InstallerDownloadError::TooLarge);
        }
        body.extend_from_slice(&chunk[..read]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn product() -> StoreProductId {
        StoreProductId::parse("9WZDNCRFJ3L1").expect("valid Product ID")
    }

    #[test]
    fn the_name_the_host_offers_is_used_when_it_is_only_a_name() {
        // The shape the host actually answers with.
        let header = "attachment; filename=\"Hulu Installer.exe\"; \
                      filename*=UTF-8''Hulu%20Installer.exe";
        assert_eq!(
            file_name(&product(), Some(header)),
            "Hulu Installer.exe".to_owned()
        );
    }

    #[test]
    fn a_name_that_is_really_a_path_is_refused_in_favour_of_the_product_id() {
        for hostile in [
            "attachment; filename=\"..\\\\..\\\\Windows\\\\System32\\\\evil.exe\"",
            "attachment; filename=\"/etc/passwd.exe\"",
            "attachment; filename=\"C:evil.exe\"",
            "attachment; filename=\".hidden.exe\"",
            // Not an executable, so nothing here would run it anyway; the point
            // is that the offered name never decides what the file is called.
            "attachment; filename=\"notes.txt\"",
            "attachment; filename=\"\"",
        ] {
            assert_eq!(
                file_name(&product(), Some(hostile)),
                "9WZDNCRFJ3L1 Installer.exe".to_owned(),
                "accepted a name it should not have: {hostile}"
            );
        }
    }

    #[test]
    fn no_header_at_all_still_produces_a_usable_name() {
        assert_eq!(
            file_name(&product(), None),
            "9WZDNCRFJ3L1 Installer.exe".to_owned()
        );
        assert_eq!(
            file_name(&product(), Some("attachment")),
            "9WZDNCRFJ3L1 Installer.exe".to_owned()
        );
    }

    #[test]
    fn the_installer_directory_sits_under_the_applications_own_data() {
        let Some(directory) = installer_directory() else {
            return;
        };
        assert!(directory.ends_with(INSTALLER_DIRECTORY));
        assert!(
            directory
                .to_string_lossy()
                .contains(winstoreregion_core::LOCAL_DATA_DIRECTORY)
        );
    }

    #[test]
    fn the_sweep_removes_what_was_downloaded_and_keeps_the_folder() {
        let directory = std::env::temp_dir().join("winstoreregion-sweep-test");
        let _ = fs::create_dir_all(&directory);
        let file = directory.join("Some Installer.exe");
        fs::write(&file, b"not really an installer").expect("the scratch file is writable");
        assert!(file.exists());

        sweep_directory(&directory);

        assert!(!file.exists(), "the downloaded installer was kept");
        assert!(directory.exists(), "the folder itself was removed");
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn sweeping_a_folder_that_is_not_there_is_not_a_failure() {
        // The ordinary case on a machine that has never downloaded anything.
        sweep_directory(&std::env::temp_dir().join("winstoreregion-never-existed"));
    }

    #[test]
    #[ignore = "downloads a real installer from Microsoft over the network"]
    fn the_live_host_answers_with_a_signed_installer_for_a_product_id() {
        let downloaded = download_store_installer(&product()).expect("the host answered");
        let written = fs::metadata(&downloaded.path).expect("the file was written");
        assert!(written.len() > 100_000, "the file is implausibly small");
        assert_eq!(downloaded.product_id, product());
    }
}
