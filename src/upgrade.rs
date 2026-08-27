use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{fs, io::Read, os::unix::fs::PermissionsExt, path::Path};
use tempfile::NamedTempFile;

const RELEASE_API: &str = "https://api.github.com/repos/lucashutch/limitwatch/releases/latest";
const BINARY_ASSET: &str = "limitwatch-linux-x86_64";
const CHECKSUM_ASSET: &str = "limitwatch-linux-x86_64.sha256";

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

pub fn run(current_version: &str) -> Result<()> {
    if !cfg!(target_os = "linux") || !cfg!(target_arch = "x86_64") {
        bail!("LimitWatch upgrades are currently supported on Linux x86_64 only")
    }

    let install_path =
        std::env::current_exe().context("could not determine the LimitWatch executable path")?;
    let client = Client::builder()
        .user_agent(format!("limitwatch/{current_version}"))
        .build()
        .context("could not create the GitHub client")?;
    let release: Release = client
        .get(RELEASE_API)
        .send()
        .context("could not check GitHub for a LimitWatch release")?
        .error_for_status()
        .context("GitHub returned an error while checking for a LimitWatch release")?
        .json()
        .context("GitHub returned an invalid release response")?;

    let latest = release.tag_name.trim_start_matches('v');
    let current = current_version.trim_start_matches('v');
    if !release_is_newer(latest, current) {
        println!("LimitWatch {current} is already up to date.");
        return Ok(());
    }

    let binary_url = asset_url(&release, BINARY_ASSET)?;
    let checksum_url = asset_url(&release, CHECKSUM_ASSET)?;
    println!("Downloading LimitWatch {latest}...");
    let binary = client
        .get(binary_url)
        .send()
        .context("could not download the LimitWatch release")?
        .error_for_status()
        .context("GitHub returned an error while downloading the LimitWatch release")?;
    let checksum = client
        .get(checksum_url)
        .send()
        .context("could not download the release checksum")?
        .error_for_status()
        .context("GitHub returned an error while downloading the release checksum")?
        .text()
        .context("could not read the release checksum")?;

    let expected = checksum
        .split_whitespace()
        .next()
        .filter(|value| value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit()))
        .context("the release checksum is invalid")?;
    let mut temp = NamedTempFile::new_in(
        install_path
            .parent()
            .context("the LimitWatch executable has no parent directory")?,
    )
    .context("could not create a temporary file beside the LimitWatch executable")?;
    let mut response = binary;
    response
        .copy_to(temp.as_file_mut())
        .context("could not save the downloaded LimitWatch release")?;
    temp.as_file_mut()
        .sync_all()
        .context("could not flush the downloaded LimitWatch release")?;

    let actual = sha256(temp.path()).context("could not verify the downloaded release")?;
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("downloaded LimitWatch release checksum did not match GitHub")
    }
    temp.as_file()
        .set_permissions(fs::Permissions::from_mode(0o755))
        .context("could not make the downloaded LimitWatch release executable")?;
    temp.persist(&install_path)
        .map_err(|error| error.error)
        .with_context(|| format!("could not replace {}", install_path.display()))?;
    println!("Updated LimitWatch to {latest}.");
    Ok(())
}

fn asset_url<'a>(release: &'a Release, name: &str) -> Result<&'a str> {
    release
        .assets
        .iter()
        .find(|asset| asset.name == name)
        .map(|asset| asset.browser_download_url.as_str())
        .with_context(|| format!("the GitHub release is missing {name}"))
}

fn release_is_newer(latest: &str, current: &str) -> bool {
    match (version_parts(latest), version_parts(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => latest != current,
    }
}

fn version_parts(version: &str) -> Option<[u64; 3]> {
    let mut parts = version.split('-').next()?.split('.');
    Some([
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ])
}

fn sha256(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn selects_expected_release_assets() {
        let release = Release {
            tag_name: "v1.2.3".into(),
            assets: vec![Asset {
                name: BINARY_ASSET.into(),
                browser_download_url: "https://example.test/binary".into(),
            }],
        };
        assert_eq!(
            asset_url(&release, BINARY_ASSET).unwrap(),
            "https://example.test/binary"
        );
        assert!(asset_url(&release, CHECKSUM_ASSET).is_err());
    }

    #[test]
    fn only_newer_versions_are_installed() {
        assert!(release_is_newer("1.2.3", "1.2.2"));
        assert!(!release_is_newer("1.2.3", "1.2.3-4-gabcdef"));
        assert!(!release_is_newer("1.2.3", "1.3.0"));
        assert!(release_is_newer("preview", "development"));
        assert!(!release_is_newer("preview", "preview"));
        assert_eq!(version_parts("1.2.3-beta.1"), Some([1, 2, 3]));
        assert_eq!(version_parts("1.2"), None);
        assert_eq!(version_parts("1.two.3"), None);
    }

    #[test]
    fn computes_a_file_checksum() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"limitwatch").unwrap();
        file.flush().unwrap();

        assert_eq!(
            sha256(file.path()).unwrap(),
            "bed080aea1af111ba365b76457de8cbc735d389997fc041cdb21903deec0a725"
        );
    }
}
