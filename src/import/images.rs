//! Phase 6 — trophy images (`docs/specs/migration-import.md`).
//!
//! Local filenames are kept as-is when the file exists under the images
//! directory (missing files → NULL + report). CDN URLs are downloaded
//! best-effort with bounded concurrency; any failure (including running
//! offline) marks the URL expired and stores NULL — image handling can never
//! fail the import. Orphan disk files are listed for optional manual cleanup.

use super::report::{DownloadedImage, ExpiredImageUrl, ImportReport, MissingImageFile};
use super::{ImageSource, ImportOptions, PreparedGuild};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A pending CDN download, addressed back to its prepared trophy by index.
struct DownloadJob {
    guild: usize,
    trophy: usize,
    url: String,
    filename: String,
}

/// Resolves every prepared trophy's `image_source` into its final stored
/// `image` filename (or `None`), reporting all anomalies. Runs before the
/// import transaction so no network I/O happens inside it.
pub(super) async fn resolve(
    guilds: &mut [PreparedGuild],
    opts: &ImportOptions,
    report: &mut ImportReport,
) {
    let mut jobs: Vec<DownloadJob> = Vec::new();
    for (gi, guild) in guilds.iter_mut().enumerate() {
        for (ti, trophy) in guild.trophies.iter_mut().enumerate() {
            match trophy.image_source.take() {
                None => {}
                Some(ImageSource::Local(filename)) => {
                    if opts.images_dir.join(&filename).is_file() {
                        trophy.image = Some(filename);
                        report.local_images_kept += 1;
                    } else {
                        report.missing_image_files.push(MissingImageFile {
                            guild_id: trophy.guild_id,
                            legacy_id: trophy.legacy_id.clone(),
                            filename,
                        });
                    }
                }
                Some(ImageSource::Url(url)) => {
                    let filename =
                        format!("{}_{}.{}", trophy.guild_id, trophy.legacy_id, url_extension(&url));
                    jobs.push(DownloadJob { guild: gi, trophy: ti, url, filename });
                }
            }
        }
    }

    let succeeded = download_all(&jobs, opts).await;
    for (job, ok) in jobs.into_iter().zip(succeeded) {
        let trophy = &mut guilds[job.guild].trophies[job.trophy];
        if ok {
            report.downloaded_images.push(DownloadedImage {
                guild_id: trophy.guild_id,
                legacy_id: trophy.legacy_id.clone(),
                url: job.url,
                filename: job.filename.clone(),
            });
            trophy.image = Some(job.filename);
        } else {
            report.expired_image_urls.push(ExpiredImageUrl {
                guild_id: trophy.guild_id,
                legacy_id: trophy.legacy_id.clone(),
                url: job.url,
            });
        }
    }

    scan_orphan_disk_files(guilds, &opts.images_dir, report);
}

/// Best-effort concurrent downloads; returns per-job success flags aligned
/// with `jobs`. Never returns an error: every failure is just `false`.
async fn download_all(jobs: &[DownloadJob], opts: &ImportOptions) -> Vec<bool> {
    let mut succeeded = vec![false; jobs.len()];
    if jobs.is_empty() {
        return succeeded;
    }
    let client = match reqwest::Client::builder().timeout(opts.http_timeout).build() {
        Ok(client) => client,
        Err(err) => {
            log::warn!("HTTP client unavailable; marking all {} image URLs expired: {err}", jobs.len());
            return succeeded;
        }
    };
    if let Err(err) = std::fs::create_dir_all(&opts.images_dir) {
        log::warn!(
            "cannot create images dir {}; marking all image URLs expired: {err}",
            opts.images_dir.display()
        );
        return succeeded;
    }

    log::info!("downloading {} CDN trophy images (best-effort)", jobs.len());
    let concurrency = opts.download_concurrency.max(1);
    let mut set = tokio::task::JoinSet::new();
    let mut next = 0usize;
    loop {
        while set.len() < concurrency && next < jobs.len() {
            let client = client.clone();
            let url = jobs[next].url.clone();
            let dest = opts.images_dir.join(&jobs[next].filename);
            let idx = next;
            set.spawn(async move { (idx, download_one(&client, &url, &dest).await) });
            next += 1;
        }
        match set.join_next().await {
            None => break,
            Some(Err(join_err)) => log::warn!("image download task failed: {join_err}"),
            Some(Ok((idx, Err(err)))) => {
                log::debug!("image URL expired ({}): {err:#}", jobs[idx].url);
            }
            Some(Ok((idx, Ok(())))) => succeeded[idx] = true,
        }
    }
    succeeded
}

async fn download_one(client: &reqwest::Client, url: &str, dest: &PathBuf) -> anyhow::Result<()> {
    let bytes = client.get(url).send().await?.error_for_status()?.bytes().await?;
    std::fs::write(dest, &bytes)?;
    Ok(())
}

/// Lists files under the images dir referenced by no imported trophy.
fn scan_orphan_disk_files(guilds: &[PreparedGuild], images_dir: &Path, report: &mut ImportReport) {
    let referenced: HashSet<&str> = guilds
        .iter()
        .flat_map(|g| g.trophies.iter())
        .filter_map(|t| t.image.as_deref())
        .collect();
    let Ok(entries) = std::fs::read_dir(images_dir) else {
        return; // No images dir at all: nothing to list.
    };
    for entry in entries.flatten() {
        if !entry.path().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !referenced.contains(name.as_str()) {
            report.orphan_disk_files.push(name);
        }
    }
    report.orphan_disk_files.sort();
}

/// File extension for a downloaded CDN image, taken from the URL path
/// (query/fragment stripped); falls back to `png`.
fn url_extension(url: &str) -> &str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let file = path.rsplit('/').next().unwrap_or("");
    match file.rsplit_once('.') {
        Some((_, ext))
            if !ext.is_empty() && ext.len() <= 8 && ext.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            ext
        }
        _ => "png",
    }
}

#[cfg(test)]
mod tests {
    use super::url_extension;

    #[test]
    fn url_extension_strips_query_and_falls_back_to_png() {
        assert_eq!(url_extension("https://cdn.discordapp.com/a/b/pic.gif?ex=1&is=2"), "gif");
        assert_eq!(url_extension("https://cdn.discordapp.com/a/b/pic.PNG"), "PNG");
        assert_eq!(url_extension("https://cdn.discordapp.com/a/b/noext"), "png");
        assert_eq!(url_extension("https://cdn.discordapp.com/a/b/trailingdot."), "png");
        assert_eq!(url_extension("https://cdn.discordapp.com/a/b/pic.webp#frag"), "webp");
    }
}
