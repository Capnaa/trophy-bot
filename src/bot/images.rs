//! Shared trophy-image pipeline (F3: validate BEFORE any persistence).
//!
//! Used by `/create` (batch C1) and reusable by `/edit` (batch C8, F6):
//! 1. [`validate`] the attachment *metadata* (content-type + declared size)
//!    before anything is downloaded or written;
//! 2. [`download`] the attachment via reqwest and save it under
//!    [`IMAGES_DIR`] as `{guild_id}_{trophy_uuid}.{ext}` (see [`filename`]);
//! 3. [`remove`] for best-effort cleanup when a later DB step fails.

use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use uuid::Uuid;

/// Directory where trophy images live (legacy `./images`, kept by the import).
pub const IMAGES_DIR: &str = "images";

/// Legacy limit: 1,000,000 bytes (decimal megabyte), kept for parity.
pub const MAX_IMAGE_BYTES: u32 = 1_000_000;

/// HTTP timeout for a single attachment download.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15);

/// Why an attachment was rejected before download. The caller maps each
/// variant to its own localized error message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    /// Content type is not png/jpg/jpeg/gif.
    UnsupportedType,
    /// Declared size exceeds [`MAX_IMAGE_BYTES`].
    TooLarge,
}

/// Validates attachment metadata and returns the canonical file extension.
///
/// The check uses the Discord-reported `content_type` (not the filename, F3:
/// the legacy extension-from-filename check was spoofable) and the declared
/// byte size. Accepted types: `image/png`, `image/jpeg` (+ the non-standard
/// `image/jpg`), `image/gif`; parameters after `;` are ignored.
pub fn validate(content_type: Option<&str>, size: u32) -> Result<&'static str, ImageError> {
    let normalized = content_type
        .map(|ct| ct.split(';').next().unwrap_or(ct).trim().to_ascii_lowercase());
    let ext = match normalized.as_deref() {
        Some("image/png") => "png",
        Some("image/jpeg") | Some("image/jpg") => "jpg",
        Some("image/gif") => "gif",
        _ => return Err(ImageError::UnsupportedType),
    };
    if size > MAX_IMAGE_BYTES {
        return Err(ImageError::TooLarge);
    }
    Ok(ext)
}

/// Stored filename for a trophy image: `{guild_id}_{trophy_uuid}.{ext}`.
/// UUID-based so it can never collide with legacy `{guild}_{legacy_id}.{ext}`
/// files kept by the import.
pub fn filename(guild_id: i64, trophy_id: Uuid, ext: &str) -> String {
    format!("{guild_id}_{trophy_id}.{ext}")
}

/// Downloads `url` and saves it under [`IMAGES_DIR`]/`filename`, returning the
/// bytes (so the caller can also attach them to the reply without re-reading).
///
/// Defense in depth: the *actual* downloaded size is re-checked against
/// [`MAX_IMAGE_BYTES`] — the declared attachment size already passed
/// [`validate`], but the two must agree.
pub async fn download(url: &str, filename: &str) -> anyhow::Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .context("build HTTP client")?;
    let bytes = client
        .get(url)
        .send()
        .await
        .context("request attachment")?
        .error_for_status()
        .context("attachment response status")?
        .bytes()
        .await
        .context("read attachment body")?;
    anyhow::ensure!(
        bytes.len() as u64 <= u64::from(MAX_IMAGE_BYTES),
        "downloaded image is {} bytes, over the {} byte limit",
        bytes.len(),
        MAX_IMAGE_BYTES
    );

    std::fs::create_dir_all(IMAGES_DIR).context("create images directory")?;
    let path = Path::new(IMAGES_DIR).join(filename);
    std::fs::write(&path, &bytes).with_context(|| format!("write {}", path.display()))?;
    Ok(bytes.to_vec())
}

/// Best-effort removal of a stored image (rollback path when the DB insert
/// fails after the file was already saved). Never fails: errors are logged.
pub fn remove(filename: &str) {
    let path = Path::new(IMAGES_DIR).join(filename);
    if let Err(err) = std::fs::remove_file(&path) {
        if err.kind() != std::io::ErrorKind::NotFound {
            log::warn!("failed to remove image {}: {err}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_supported_content_types() {
        assert_eq!(validate(Some("image/png"), 1), Ok("png"));
        assert_eq!(validate(Some("image/jpeg"), 1), Ok("jpg"));
        assert_eq!(validate(Some("image/jpg"), 1), Ok("jpg"));
        assert_eq!(validate(Some("image/gif"), 1), Ok("gif"));
    }

    #[test]
    fn validate_is_case_insensitive_and_ignores_parameters() {
        assert_eq!(validate(Some("IMAGE/PNG"), 1), Ok("png"));
        assert_eq!(validate(Some("image/gif; charset=binary"), 1), Ok("gif"));
        assert_eq!(validate(Some(" image/jpeg "), 1), Ok("jpg"));
    }

    #[test]
    fn validate_rejects_unsupported_or_missing_content_types() {
        assert_eq!(validate(Some("image/webp"), 1), Err(ImageError::UnsupportedType));
        assert_eq!(validate(Some("text/html"), 1), Err(ImageError::UnsupportedType));
        assert_eq!(validate(Some("application/octet-stream"), 1), Err(ImageError::UnsupportedType));
        assert_eq!(validate(None, 1), Err(ImageError::UnsupportedType));
    }

    #[test]
    fn validate_enforces_the_size_limit() {
        assert_eq!(validate(Some("image/png"), MAX_IMAGE_BYTES), Ok("png"));
        assert_eq!(
            validate(Some("image/png"), MAX_IMAGE_BYTES + 1),
            Err(ImageError::TooLarge)
        );
    }

    #[test]
    fn filename_embeds_guild_and_trophy_uuid() {
        let id = Uuid::now_v7();
        assert_eq!(filename(42, id, "png"), format!("42_{id}.png"));
    }

    #[test]
    fn remove_of_missing_file_is_a_silent_noop() {
        remove("definitely-not-there.png");
    }
}
