//! Fetching a release's file from GitHub for an app to install: the file, and the checksum
//! beside it (`release.yml` makes `<file>.sha256`), which it must match. Installing is the
//! app's own business (the desktop replaces its program, Android hands the APK to the system).

/// Downloads `url` and checks it against `<url>.sha256`. The checksum is fetched first: no
/// checksum, no update. `progress` gets the bytes so far, and in all if GitHub said.
pub async fn fetch_verified(url: &str, progress: &(dyn Fn(u64, Option<u64>) + Send + Sync)) -> Result<Vec<u8>, String> {
    let sum = fetch_checksum(url).await?;
    let file = download(url, progress).await?;
    verify(&file, &sum)?;
    Ok(file)
}

async fn fetch_checksum(url: &str) -> Result<String, String> {
    let url = format!("{url}.sha256");
    let response = reqwest::get(&url).await.map_err(|e| format!("Couldn't reach GitHub: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("The release has no checksum to check the download against ({url}: {}).", response.status()));
    }
    response.text().await.map_err(|e| format!("Couldn't read the checksum: {e}"))
}

/// Whether the download is what the checksum file (`sha256sum`'s: the hash, then the
/// name) says.
pub fn verify(file: &[u8], checksum_file: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let expected = checksum_file.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    if hex::encode(Sha256::digest(file)) == expected {
        Ok(())
    } else {
        Err("The download doesn't match its checksum, so it wasn't installed.".into())
    }
}

async fn download(url: &str, progress: &(dyn Fn(u64, Option<u64>) + Send + Sync)) -> Result<Vec<u8>, String> {
    let mut response = reqwest::get(url).await.map_err(|e| format!("Couldn't reach GitHub: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("GitHub has no {url} ({})", response.status()));
    }
    let total = response.content_length();
    let mut bytes = Vec::with_capacity(total.unwrap_or(0).min(1 << 30) as usize);
    let mut last = std::time::Instant::now();
    while let Some(chunk) = response.chunk().await.map_err(|e| format!("The download stopped: {e}"))? {
        bytes.extend_from_slice(&chunk);
        // (A report per chunk would be a lot.)
        if last.elapsed().as_millis() > 100 {
            progress(bytes.len() as u64, total);
            last = std::time::Instant::now();
        }
    }
    if let Some(total) = total
        && total != bytes.len() as u64
    {
        return Err("The download was cut short.".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_must_match_its_checksum() {
        use sha2::{Digest, Sha256};
        let sum = hex::encode(Sha256::digest(b"archive"));
        assert!(verify(b"archive", &format!("{sum}  Acetate-v1.0.0-linux-x86_64.tar.gz\n")).is_ok());
        assert!(verify(b"archive", &sum.to_uppercase()).is_ok());
        assert!(verify(b"tampered", &format!("{sum}  x\n")).is_err());
        assert!(verify(b"archive", "").is_err());
    }
}
