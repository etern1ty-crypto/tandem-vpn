//! Explicit HTTPS requests, capped response bodies and public-only resolution.
//! No telemetry, background downloads, proxy auto-discovery or redirect probes.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::io::{self, Read};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub const REPOSITORY: &str = "Flowseal/zapret-discord-youtube";
pub const MAX_ARCHIVE: u64 = 64 * 1024 * 1024;
static DNS_WORKERS: AtomicUsize = AtomicUsize::new(0);

struct DnsPermit;
impl Drop for DnsPermit {
    fn drop(&mut self) {
        DNS_WORKERS.fetch_sub(1, Ordering::SeqCst);
    }
}
#[derive(Clone)]
struct PublicResolver {
    timeout: Duration,
}
impl ureq::Resolver for PublicResolver {
    fn resolve(&self, netloc: &str) -> io::Result<Vec<SocketAddr>> {
        // A wedged OS resolver cannot create unbounded threads across retries.
        DNS_WORKERS
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                if n < 8 {
                    Some(n + 1)
                } else {
                    None
                }
            })
            .map_err(|_| io::Error::new(io::ErrorKind::WouldBlock, "DNS worker limit reached"))?;
        let permit = DnsPermit;
        let netloc = netloc.to_owned();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("tandem-dns".into())
            .spawn(move || {
                let _permit = permit;
                let result: io::Result<Vec<SocketAddr>> =
                    netloc.to_socket_addrs().map(|values| values.collect());
                let _ = send.send(result);
            })?;
        let addresses = receive
            .recv_timeout(self.timeout)
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "DNS resolution timed out"))??;
        if addresses.is_empty()
            || addresses
                .iter()
                .any(|addr| !crate::hosts::public_ip(addr.ip()))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Private, special or mixed DNS answers are forbidden",
            ));
        }
        Ok(addresses)
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(timeout)
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .timeout_write(timeout)
        .redirects(0)
        .resolver(PublicResolver {
            timeout: timeout.min(Duration::from_secs(8)),
        })
        .user_agent(concat!("TandemWorkbench/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn upstream_url(value: &str) -> Result<url::Url> {
    let url = url::Url::parse(value).map_err(|e| Error::Invalid(e.to_string()))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|p| p != 443)
        || url.fragment().is_some()
        || !matches!(
            url.host_str(),
            Some(
                "api.github.com"
                    | "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
    {
        return Err(Error::Security(
            "Download URL is outside the GitHub HTTPS allowlist".into(),
        ));
    }
    Ok(url)
}

fn fetch(url: &str, max_bytes: u64, seconds: u64) -> Result<Vec<u8>> {
    let mut url = upstream_url(url)?;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    for _ in 0..6 {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| Error::Timeout("Download deadline".into()))?;
        let response = agent(remaining)
            .get(url.as_str())
            .set(
                "Accept",
                if url.host_str() == Some("api.github.com") {
                    "application/vnd.github+json"
                } else {
                    "application/octet-stream"
                },
            )
            .call()
            .map_err(|e| Error::Operation(format!("HTTPS request failed: {e}")))?;
        if (300..400).contains(&response.status()) {
            let location = response
                .header("Location")
                .ok_or_else(|| Error::Operation("Redirect has no Location".into()))?;
            let next = url
                .join(location)
                .map_err(|e| Error::Invalid(e.to_string()))?;
            url = upstream_url(next.as_str())?;
            continue;
        }
        if response.status() != 200 {
            return Err(Error::Operation(format!(
                "Unexpected HTTP status {}",
                response.status()
            )));
        }
        if response
            .header("Content-Length")
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|length| length > max_bytes)
        {
            return Err(Error::Invalid("Remote response exceeds size limit".into()));
        }
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(max_bytes + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > max_bytes {
            return Err(Error::Invalid("Remote response exceeded size limit".into()));
        }
        if Instant::now() > deadline {
            return Err(Error::Timeout("Download deadline".into()));
        }
        return Ok(bytes);
    }
    Err(Error::Security("Too many download redirects".into()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    pub digest: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub draft: bool,
    pub prerelease: bool,
    pub assets: Vec<ReleaseAsset>,
}

pub fn release(tag: Option<&str>) -> Result<Release> {
    let suffix = if let Some(tag) = tag {
        crate::zapret::validate_tag(tag)?;
        format!("tags/{tag}")
    } else {
        "latest".into()
    };
    let url = format!("https://api.github.com/repos/{REPOSITORY}/releases/{suffix}");
    let result: Release = serde_json::from_slice(&fetch(&url, 1024 * 1024, 15)?)?;
    crate::zapret::validate_tag(&result.tag_name)?;
    if result.draft || result.prerelease {
        return Err(Error::Invalid(
            "Draft and prerelease engine bundles are not accepted".into(),
        ));
    }
    if tag.is_some_and(|tag| tag != result.tag_name) {
        return Err(Error::Security(
            "GitHub returned a different release tag".into(),
        ));
    }
    Ok(result)
}

pub fn select_asset(release: &Release) -> Result<&ReleaseAsset> {
    let prefix = format!(
        "https://github.com/{REPOSITORY}/releases/download/{}/",
        release.tag_name
    );
    let candidates: Vec<_> = release
        .assets
        .iter()
        .filter(|asset| {
            asset.name.starts_with("zapret-discord-youtube-") && asset.name.ends_with(".zip")
        })
        .collect();
    if candidates.len() != 1 {
        return Err(Error::Invalid("Release must have exactly one zapret-discord-youtube-*.zip asset; import a reviewed ZIP manually instead".into()));
    }
    let asset = candidates[0];
    if asset.size == 0
        || asset.size > MAX_ARCHIVE
        || !asset.browser_download_url.starts_with(&prefix)
    {
        return Err(Error::Security(
            "Unexpected release asset size or URL".into(),
        ));
    }
    upstream_url(&asset.browser_download_url)?;
    Ok(asset)
}

pub fn download(tag: &str, expected_sha256: &str) -> Result<Vec<u8>> {
    let expected = crate::files::validate_digest(expected_sha256)?;
    let release = release(Some(tag))?;
    let asset = select_asset(&release)?;
    if let Some(digest) = &asset.digest {
        if digest != &format!("sha256:{expected}") {
            return Err(Error::Security(
                "Trusted SHA-256 disagrees with the GitHub asset digest".into(),
            ));
        }
    }
    let bytes = fetch(&asset.browser_download_url, MAX_ARCHIVE, 120)?;
    if bytes.len() as u64 != asset.size || crate::files::sha256(&bytes) != expected {
        return Err(Error::Security(
            "Downloaded archive size/SHA-256 does not match the approved artifact".into(),
        ));
    }
    Ok(bytes)
}

#[derive(Debug, Clone, Serialize)]
pub struct TargetResult {
    pub url: String,
    pub reachable: bool,
    pub http_ok: bool,
    pub status: Option<u16>,
    pub elapsed_ms: u128,
    pub error: Option<String>,
}
fn probe(url: &str, timeout: u64) -> TargetResult {
    let started = Instant::now();
    let response = agent(Duration::from_secs(timeout)).get(url).call();
    let (reachable, status, error) = match response {
        Ok(response) => (true, Some(response.status()), None),
        Err(ureq::Error::Status(code, _)) => (true, Some(code), None),
        Err(ureq::Error::Transport(error)) => (false, None, Some(error.to_string())),
    };
    TargetResult {
        url: url.into(),
        reachable,
        http_ok: status.is_some_and(|code| (200..400).contains(&code)),
        status,
        elapsed_ms: started.elapsed().as_millis(),
        error,
    }
}

pub fn test_targets(config: &crate::config::Config) -> Result<Vec<TargetResult>> {
    config.validate()?;
    // Up to three workers; retain configured ordering. Redirects are not followed.
    std::thread::scope(|scope| {
        let mut jobs = Vec::new();
        for chunk in config.targets.chunks(4) {
            jobs.push(scope.spawn(move || {
                chunk
                    .iter()
                    .map(|target| probe(target, config.request_timeout_secs))
                    .collect::<Vec<_>>()
            }));
        }
        let mut results = Vec::new();
        for job in jobs {
            results.extend(
                job.join()
                    .map_err(|_| Error::Operation("Connectivity worker failed".into()))?,
            );
        }
        Ok(results)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn downloader_host_policy() {
        for value in [
            "http://github.com/file",
            "https://github.com.evil.test/file",
            "https://evil.test/file",
            "https://user@github.com/file",
            "https://github.com:8443/file",
        ] {
            assert!(upstream_url(value).is_err());
        }
    }
    #[test]
    fn ambiguous_assets_are_not_selected() {
        let mut release = Release {
            tag_name: "1.0".into(),
            draft: false,
            prerelease: false,
            assets: vec![],
        };
        assert!(select_asset(&release).is_err());
        let asset = ReleaseAsset {
            name: "zapret-discord-youtube-1.0.zip".into(),
            browser_download_url: format!(
                "https://github.com/{REPOSITORY}/releases/download/1.0/a.zip"
            ),
            size: 10,
            digest: None,
        };
        release.assets = vec![asset.clone()];
        assert!(select_asset(&release).is_ok());
        release.assets.push(asset);
        assert!(select_asset(&release).is_err());
    }
}
