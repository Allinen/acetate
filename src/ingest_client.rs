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

/// An Acetate server to send files to, logged in with a session token.
pub struct Server {
    url: String,
    client: reqwest::Client,
    token: String,
}

/// An error response's body.
#[derive(Deserialize)]
struct ErrorBody {
    error: String,
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
            let message = response.json::<ErrorBody>().await.map(|b| b.error).unwrap_or_default();
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

        // The audio is streamed from disk, not read into memory first.
        let file = tokio::fs::File::open(path).await.with_context(|| format!("opening {}", path.display()))?;
        let length = file.metadata().await?.len();
        let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file));
        form = form.part("file", Part::stream_with_length(body, length).file_name(options.uploaded_filename.clone()));

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
            Ok(body) => body.error,
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
}
