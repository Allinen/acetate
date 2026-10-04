//! `acetate ingest --server <url>`: sends each file to another Acetate over HTTP
//! (`POST /api/ingest`), with the same description a local import would use. So a folder
//! on one machine can be imported into a server elsewhere, with all the layouts and flags.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use reqwest::multipart::{Form, Part};
use serde::Deserialize;

use crate::ingest::IngestOptions;
use crate::ingest::describe::{self, Report};
use crate::ingest_command::IMPORTED;

/// An Acetate server to send files to, logged in with a session token.
pub struct Server {
    url: String,
    client: reqwest::Client,
    token: String,
}

/// Files bigger than this go in chunks of this size (`/api/uploads`), under proxies' caps
/// on a request's body (Cloudflare: 100 MB).
const CHUNK_BYTES: u64 = 50_000_000;

/// An error response's body.
#[derive(Deserialize)]
struct ErrorBody {
    message: String,
}

impl Server {
    /// Checks the URL and token right away (`GET /api/auth/me`), so a typo fails before
    /// the first file rather than on each.
    pub async fn connect(url: &str, token: &str) -> anyhow::Result<Server> {
        let client = reqwest::Client::builder()
            // Long: a big WAV over a slow link, and the server hashing and probing it.
            .timeout(Duration::from_secs(30 * 60))
            .build()
            .context("setting up the HTTP client")?;
        let server = Server { url: url.trim_end_matches('/').to_string(), client, token: token.to_string() };
        let response = server
            .client
            .get(format!("{}/api/auth/me", server.url))
            .bearer_auth(&server.token)
            .send()
            .await
            .with_context(|| format!("can't reach {}", server.url))?;
        match response.status() {
            status if status.is_success() => {
                let me: serde_json::Value = response.json().await?;
                println!("sending to {} as {}", server.url, me["username"].as_str().unwrap_or("?"));
                Ok(server)
            }
            reqwest::StatusCode::UNAUTHORIZED => anyhow::bail!(
                "{} doesn't accept ACETATE_TOKEN (expired or logged out?): log in again for a new one",
                server.url
            ),
            status => anyhow::bail!("{}/api/auth/me answered {status}: is it an Acetate?", server.url),
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// The ids of these groups on the server (`GET /api/groups`, which needs
    /// `manage_access` or `manage_users`), for `--restrict-to`.
    pub async fn group_ids(&self, names: &[String]) -> anyhow::Result<Vec<uuid::Uuid>> {
        #[derive(Deserialize)]
        struct Group {
            id: uuid::Uuid,
            name: String,
        }
        let response = self.client.get(format!("{}/api/groups", self.url)).bearer_auth(&self.token).send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let message = response.json::<ErrorBody>().await.map(|b| b.message).unwrap_or_default();
            anyhow::bail!("can't list the server's groups for --restrict-to ({status}: {message})");
        }
        let groups: Vec<Group> = response.json().await?;
        let mut ids = Vec::new();
        let mut unknown = Vec::new();
        for name in names {
            match groups.iter().find(|g| &g.name == name) {
                Some(group) => ids.push(group.id),
                None => unknown.push(name.as_str()),
            }
        }
        if !unknown.is_empty() {
            anyhow::bail!("unknown groups on {}: {}", self.url, unknown.join(", "));
        }
        Ok(ids)
    }

    /// Sends one file with its description. A refusal (bad tags, a source that doesn't fit)
    /// is `Skipped`, other failures `Failed`. An error stops the run: the token stopped
    /// working, or the file can't be read.
    pub(crate) async fn send(
        &self,
        path: &Path,
        options: &IngestOptions,
        dry_run: bool,
    ) -> anyhow::Result<super::ingest_command::Outcome> {
        use super::ingest_command::Outcome;

        // The same description the server would have built for a local import. Covers are
        // form parts, named in it.
        let mut description = describe::description_json(
            &options.origin,
            &options.status,
            &options.sources,
            &options.hints,
            options.restrict_to.as_deref(),
        );
        description["imported_from"] = serde_json::json!(options.local_path);
        description["volume"] = serde_json::json!(options.volume);
        let mut form = Form::new();
        let mut track_cover_names = Vec::new();
        for (n, bytes) in options.hints.track_covers.iter().enumerate() {
            let name = format!("track_cover_{}", n + 1);
            form = form.part(name.clone(), Part::bytes(bytes.clone()).file_name(name.clone()));
            track_cover_names.push(name);
        }
        description["track"]["covers"] = serde_json::json!(track_cover_names);
        if let Some(album) = &options.hints.album {
            let mut names = Vec::new();
            for (n, bytes) in album.covers.iter().enumerate() {
                let name = format!("album_cover_{}", n + 1);
                form = form.part(name.clone(), Part::bytes(bytes.clone()).file_name(name.clone()));
                names.push(name);
            }
            description["album"]["covers"] = serde_json::json!(names);
        }
        form = form.text("description", description.to_string());
        let part = match self.file_part(path).await? {
            Ok(part) => part,
            Err(failed) => return Ok(Outcome::Failed(failed)),
        };
        form = form.part("file", part.file_name(options.uploaded_filename.clone()));

        let response = self
            .client
            .post(format!("{}/api/ingest", self.url))
            .query(&[("dry_run", dry_run)])
            .bearer_auth(&self.token)
            .multipart(form)
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(err) => return Ok(Outcome::Failed(format!("sending it failed: {err}"))),
        };
        let status = response.status();
        if status.is_success() {
            let report: Report = response.json().await.context("reading the server's report")?;
            return Ok(Outcome::Done(report));
        }
        let message = match response.json::<ErrorBody>().await {
            Ok(body) => body.message,
            Err(_) => status.to_string(),
        };
        match status {
            reqwest::StatusCode::UNAUTHORIZED => {
                anyhow::bail!("{} stopped accepting ACETATE_TOKEN: {message}", self.url)
            }
            // The file or its description was refused; it's listed on the server's Review
            // page (on a real run).
            reqwest::StatusCode::UNPROCESSABLE_ENTITY | reqwest::StatusCode::BAD_REQUEST | reqwest::StatusCode::CONFLICT => {
                Ok(Outcome::Skipped(message))
            }
            _ => Ok(Outcome::Failed(format!("{status}: {message}"))),
        }
    }

    /// A video, as an extra linked to nothing yet (`POST /api/extras`, then its file), unless
    /// an extra has the same bytes already: asked first (`/api/extra-files/by-sha256`), so a
    /// video stored already isn't sent again. Errors as for `send`.
    pub(crate) async fn send_video(
        &self,
        path: &Path,
        restrict_to: Option<&[uuid::Uuid]>,
        dry_run: bool,
    ) -> anyhow::Result<super::ingest_command::Outcome> {
        use super::ingest_command::Outcome;

        let hashed = path.to_path_buf();
        let (sha256, _) = tokio::task::spawn_blocking(move || crate::storage::hash_file(&hashed))
            .await?
            .with_context(|| format!("reading {}", path.display()))?;
        let response = self.client.get(format!("{}/api/extra-files/by-sha256/{sha256}", self.url)).bearer_auth(&self.token).send().await?;
        match response.status() {
            status if status.is_success() => return Ok(Outcome::VideoStored(response.json().await?)),
            reqwest::StatusCode::NOT_FOUND => {}
            reqwest::StatusCode::UNAUTHORIZED => anyhow::bail!("{} stopped accepting ACETATE_TOKEN", self.url),
            status => return Ok(Outcome::Failed(format!("asking whether it's stored: {status}"))),
        }
        if dry_run {
            return Ok(Outcome::VideoAdded { extra_id: None, note: None });
        }

        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let title = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| name.clone());
        let mut body = serde_json::json!({ "name": title, "kind": "video", "reason": IMPORTED });
        if let Some(groups) = restrict_to {
            body["access"] = serde_json::json!({ "restricted": true, "groups": groups });
        }
        let response = self.client.post(format!("{}/api/extras", self.url)).bearer_auth(&self.token).json(&body).send().await?;
        let made: serde_json::Value = match self.answer(response).await? {
            Ok(response) => response.json().await?,
            Err(failed) => return Ok(Outcome::Failed(format!("making its extra: {failed}"))),
        };
        let extra_id: uuid::Uuid = serde_json::from_value(made["id"].clone()).context("reading the new extra's id")?;

        let part = match self.file_part(path).await? {
            Ok(part) => part,
            Err(failed) => return Ok(Outcome::Failed(format!("{failed} (its extra {extra_id} was made, without the file)"))),
        };
        let form = Form::new()
            .text("reason", IMPORTED)
            .text("imported_from", path.to_string_lossy().to_string())
            .part("file", part.file_name(name));
        let response = self.client.post(format!("{}/api/extras/{extra_id}/files", self.url)).bearer_auth(&self.token).multipart(form).send().await;
        let response = match response {
            Ok(response) => response,
            Err(err) => return Ok(Outcome::Failed(format!("sending it failed: {err} (its extra {extra_id} was made, without the file)"))),
        };
        let added: serde_json::Value = match self.answer(response).await? {
            Ok(response) => response.json().await?,
            Err(failed) => return Ok(Outcome::Failed(format!("{failed} (its extra {extra_id} was made, without the file)"))),
        };
        // Sent by someone else in the meantime.
        if let Some(already) = added["already"].get(0) {
            return Ok(Outcome::VideoStored(serde_json::from_value(already.clone())?));
        }
        Ok(Outcome::VideoAdded { extra_id: Some(extra_id), note: None })
    }

    /// A response, if it's a success; else what went wrong (an error if it's the token).
    async fn answer(&self, response: reqwest::Response) -> anyhow::Result<Result<reqwest::Response, String>> {
        let status = response.status();
        if status.is_success() {
            return Ok(Ok(response));
        }
        let message = response.json::<ErrorBody>().await.map(|b| b.message).unwrap_or_default();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            anyhow::bail!("{} stopped accepting ACETATE_TOKEN: {message}", self.url);
        }
        Ok(Err(format!("{status}: {message}")))
    }

    /// A file as a form part: streamed from disk, not read into memory first; a big one goes
    /// ahead in chunks, and the part names it. Err inside: sending it failed.
    async fn file_part(&self, path: &Path) -> anyhow::Result<Result<Part, String>> {
        let file = tokio::fs::File::open(path).await.with_context(|| format!("opening {}", path.display()))?;
        let length = file.metadata().await?.len();
        if length > CHUNK_BYTES {
            return Ok(match self.upload_in_chunks(file, length).await {
                Ok(id) => Ok(Part::text(id.to_string()).mime_str("application/vnd.acetate.upload")?),
                Err(err) => Err(format!("sending it failed: {err:#}")),
            });
        }
        let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file));
        Ok(Ok(Part::stream_with_length(body, length)))
    }

    /// Sends a file in chunks (`/api/uploads`); returns the upload's id. A chunk that
    /// fails is sent again from where the server got to, a few times.
    async fn upload_in_chunks(&self, mut file: tokio::fs::File, length: u64) -> anyhow::Result<uuid::Uuid> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};

        #[derive(Deserialize)]
        struct Started {
            id: uuid::Uuid,
        }
        #[derive(Deserialize)]
        struct Progress {
            size: u64,
        }
        let check = |response: reqwest::Response| async move {
            let status = response.status();
            if status.is_success() {
                return Ok(response);
            }
            let message = response.json::<ErrorBody>().await.map(|b| b.message).unwrap_or_default();
            anyhow::bail!("{status}: {message}")
        };
        let started: Started =
            check(self.client.post(format!("{}/api/uploads", self.url)).bearer_auth(&self.token).send().await?).await?.json().await?;
        let url = format!("{}/api/uploads/{}", self.url, started.id);
        let mut offset = 0;
        let mut tries = 0;
        while offset < length {
            let mut chunk = vec![0; CHUNK_BYTES.min(length - offset) as usize];
            file.seek(std::io::SeekFrom::Start(offset)).await?;
            file.read_exact(&mut chunk).await?;
            let sent = self.client.put(&url).query(&[("offset", offset)]).bearer_auth(&self.token).body(chunk).send().await;
            match sent {
                Ok(response) if response.status().is_success() => {
                    offset = response.json::<Progress>().await?.size;
                    tries = 0;
                }
                failed => {
                    tries += 1;
                    if tries > 3 {
                        match failed {
                            Ok(response) => check(response).await?,
                            Err(err) => return Err(err.into()),
                        };
                    }
                    // Go on from what the server has.
                    let progress: Progress =
                        check(self.client.get(&url).bearer_auth(&self.token).send().await?).await?.json().await?;
                    offset = progress.size;
                }
            }
        }
        Ok(started.id)
    }
}
