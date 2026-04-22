use anyhow::{bail, Context, Result};
use rand::{rngs::OsRng, RngCore};
use serde_json::{json, Value};

/// Metadata about a Confluence page.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct PageInfo {
    pub id: String,
    pub title: String,
    pub version: u32,
}

/// Metadata about a Confluence attachment.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct AttachmentInfo {
    pub id: String,
    /// Filename (e.g. `"screenshot.png"`).
    pub title: String,
    /// The `comment` field — used to store the content hash.
    pub comment: String,
}

/// Blocking HTTP client for the Confluence Server 7.x REST API.
pub(crate) struct ConfluenceClient {
    /// Base URL without trailing slash (e.g. `https://confluence.example.com`).
    base_url: String,
    /// Personal Access Token (Bearer).
    token: String,
    agent: ureq::Agent,
}

impl ConfluenceClient {
    pub fn new(base_url: &str, token: &str) -> Self {
        let agent =
            ureq::Agent::new_with_config(ureq::Agent::config_builder().https_only(true).build());
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            agent,
        }
    }

    // ── helpers ──────────────────────────────────────────────────────────

    fn api_url(&self, path: &str) -> String {
        format!("{}/rest/api{}", self.base_url, path)
    }

    /// Send a GET request, return parsed JSON.
    fn get_json(&self, path: &str) -> Result<Value> {
        let url = self.api_url(path);
        let resp = self
            .agent
            .get(&url)
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Accept", "application/json")
            .call()
            .with_context(|| format!("GET {url}"))?;
        let body: Value = resp
            .into_body()
            .read_json()
            .with_context(|| format!("reading JSON from GET {url}"))?;
        Ok(body)
    }

    /// Send a POST with JSON body, return parsed JSON response.
    fn post_json(&self, path: &str, body: &Value) -> Result<Value> {
        let url = self.api_url(path);
        let resp = self
            .agent
            .post(&url)
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Accept", "application/json")
            .content_type("application/json")
            .send_json(body)
            .with_context(|| format!("POST {url}"))?;
        let resp_body: Value = resp
            .into_body()
            .read_json()
            .with_context(|| format!("reading JSON from POST {url}"))?;
        Ok(resp_body)
    }

    /// Send a PUT with JSON body, return parsed JSON response.
    fn put_json(&self, path: &str, body: &Value) -> Result<Value> {
        let url = self.api_url(path);
        let resp = self
            .agent
            .put(&url)
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Accept", "application/json")
            .content_type("application/json")
            .send_json(body)
            .with_context(|| format!("PUT {url}"))?;
        let resp_body: Value = resp
            .into_body()
            .read_json()
            .with_context(|| format!("reading JSON from PUT {url}"))?;
        Ok(resp_body)
    }

    // ── page operations ─────────────────────────────────────────────────

    /// Find a page by exact title within a space.
    ///
    /// Returns `None` when no page matches.
    pub fn find_page(&self, space: &str, title: &str) -> Result<Option<PageInfo>> {
        let encoded_title = percent_encode(title);
        let path = format!(
            "/content?spaceKey={}&title={}&expand=version",
            percent_encode(space),
            encoded_title
        );
        let json = self.get_json(&path)?;
        let results = json["results"]
            .as_array()
            .context("expected `results` array in search response")?;
        if results.is_empty() {
            return Ok(None);
        }
        Ok(Some(parse_page_info(&results[0])?))
    }

    /// Find a child page by title under a given parent.
    pub fn find_child_by_title(&self, parent_id: &str, title: &str) -> Result<Option<PageInfo>> {
        let path = format!("/content/{}/child/page?expand=version&limit=250", parent_id);
        let json = self.get_json(&path)?;
        let results = json["results"]
            .as_array()
            .context("expected `results` array in child page response")?;
        for page in results {
            let page_title = page["title"].as_str().unwrap_or_default();
            if page_title == title {
                return Ok(Some(parse_page_info(page)?));
            }
        }
        Ok(None)
    }

    /// Get a page by its content ID.
    ///
    /// This is used when the caller already has a numeric page ID
    /// (e.g. from a `id:12345` reference in `confluence.path`).
    pub fn get_page_by_id(&self, page_id: &str) -> Result<PageInfo> {
        let path = format!("/content/{}?expand=version", page_id);
        let json = self.get_json(&path)?;
        parse_page_info(&json)
    }

    /// Get the page body in storage format and current version.
    pub fn get_page_body(&self, page_id: &str) -> Result<(String, u32)> {
        let path = format!("/content/{}?expand=body.storage,version", page_id);
        let json = self.get_json(&path)?;
        let body = json["body"]["storage"]["value"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let version = json["version"]["number"].as_u64().unwrap_or(1) as u32;
        Ok((body, version))
    }

    /// Create a new page.  Returns the created page info.
    pub fn create_page(
        &self,
        space: &str,
        title: &str,
        body: &str,
        parent_id: Option<&str>,
    ) -> Result<PageInfo> {
        let mut payload = json!({
            "type": "page",
            "title": title,
            "space": { "key": space },
            "body": {
                "storage": {
                    "value": body,
                    "representation": "storage"
                }
            }
        });
        if let Some(pid) = parent_id {
            payload["ancestors"] = json!([{ "id": pid }]);
        }
        let resp = self.post_json("/content", &payload)?;
        parse_page_info(&resp)
    }

    /// Update an existing page (bumps version to `current_version + 1`).
    pub fn update_page(
        &self,
        page_id: &str,
        title: &str,
        body: &str,
        current_version: u32,
    ) -> Result<PageInfo> {
        let payload = json!({
            "type": "page",
            "title": title,
            "version": { "number": current_version + 1 },
            "body": {
                "storage": {
                    "value": body,
                    "representation": "storage"
                }
            }
        });
        let path = format!("/content/{}", page_id);
        let resp = self.put_json(&path, &payload)?;
        parse_page_info(&resp)
    }

    // ── page property operations ────────────────────────────────────────

    /// Get a page property by key.  Returns `None` if not found.
    pub fn get_page_property(&self, page_id: &str, key: &str) -> Result<Option<String>> {
        let path = format!("/content/{}/property/{}", page_id, key);
        let url = self.api_url(&path);
        let result = self
            .agent
            .get(&url)
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Accept", "application/json")
            .call();

        match result {
            Ok(resp) => {
                let body: Value = resp.into_body().read_json()?;
                let hash = body["value"]["hash"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                Ok(Some(hash))
            }
            Err(ureq::Error::StatusCode(404)) => Ok(None),
            Err(e) => Err(e).with_context(|| format!("getting property '{key}' on page {page_id}")),
        }
    }

    /// Set a page property.  Creates it if absent, updates if present.
    pub fn set_page_property(&self, page_id: &str, key: &str, hash: &str) -> Result<()> {
        // Try to get the existing property to determine create vs update.
        let path = format!("/content/{}/property/{}", page_id, key);
        let url = self.api_url(&path);
        let existing = self
            .agent
            .get(&url)
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Accept", "application/json")
            .call();

        match existing {
            Ok(resp) => {
                // Property exists — update (PUT) with incremented version.
                let body: Value = resp.into_body().read_json()?;
                let version = body["version"]["number"].as_u64().unwrap_or(1) as u32;
                let payload = json!({
                    "key": key,
                    "value": { "hash": hash },
                    "version": { "number": version + 1 }
                });
                self.put_json(&path, &payload)?;
            }
            Err(ureq::Error::StatusCode(404)) => {
                // Property doesn't exist — create (POST).
                let create_path = format!("/content/{}/property", page_id);
                let payload = json!({
                    "key": key,
                    "value": { "hash": hash }
                });
                self.post_json(&create_path, &payload)?;
            }
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("checking property '{key}' on page {page_id}"));
            }
        }
        Ok(())
    }

    // ── attachment operations ───────────────────────────────────────────

    /// List all attachments on a page.
    pub fn list_attachments(&self, page_id: &str) -> Result<Vec<AttachmentInfo>> {
        let mut all = Vec::new();
        let mut start = 0usize;
        loop {
            let path = format!(
                "/content/{}/child/attachment?start={}&limit=100",
                page_id, start
            );
            let json = self.get_json(&path)?;
            let results = json["results"]
                .as_array()
                .context("expected `results` array in attachment response")?;
            if results.is_empty() {
                break;
            }
            for att in results {
                all.push(parse_attachment_info(att)?);
            }
            let size = json["size"].as_u64().unwrap_or(0) as usize;
            start += size;
            let total = json["_links"]["next"].as_str();
            if total.is_none() || size == 0 {
                break;
            }
        }
        Ok(all)
    }

    /// Upload a new attachment to a page (POST).
    ///
    /// The `comment` field is used to store a content hash for change
    /// detection on subsequent publishes.
    pub fn create_attachment(
        &self,
        page_id: &str,
        filename: &str,
        data: &[u8],
        comment: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/rest/api/content/{}/child/attachment",
            self.base_url, page_id
        );
        self.send_attachment_multipart("POST", &url, filename, data, comment)
            .with_context(|| format!("creating attachment '{filename}' on page {page_id}"))
    }

    /// Update an existing attachment's data (POST to .../data endpoint).
    ///
    /// Confluence Server 7.x requires posting to the attachment's own
    /// `/data` endpoint to update the file content.
    pub fn update_attachment(
        &self,
        page_id: &str,
        attachment_id: &str,
        filename: &str,
        data: &[u8],
        comment: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/rest/api/content/{}/child/attachment/{}/data",
            self.base_url, page_id, attachment_id
        );
        self.send_attachment_multipart("POST", &url, filename, data, comment)
            .with_context(|| format!("updating attachment '{filename}' on page {page_id}"))
    }

    /// Build and send a multipart/form-data attachment upload.
    fn send_attachment_multipart(
        &self,
        method: &str,
        url: &str,
        filename: &str,
        data: &[u8],
        comment: &str,
    ) -> Result<()> {
        let boundary = format!("----docbake{:016x}", random_u64());
        let mut body = Vec::new();

        // -- file part
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");

        // -- comment part
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(b"Content-Disposition: form-data; name=\"comment\"\r\n\r\n");
        body.extend_from_slice(comment.as_bytes());
        body.extend_from_slice(b"\r\n");

        // -- minorEdit part
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(b"Content-Disposition: form-data; name=\"minorEdit\"\r\n\r\n");
        body.extend_from_slice(b"true\r\n");

        // -- close
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

        let content_type = format!("multipart/form-data; boundary={boundary}");

        let req = match method {
            "POST" => self.agent.post(url),
            "PUT" => self.agent.put(url),
            _ => self.agent.post(url),
        };

        let resp = req
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("X-Atlassian-Token", "nocheck")
            .header("Accept", "application/json")
            .content_type(&content_type)
            .send(body.as_slice())?;

        let status = resp.status().as_u16();
        if status >= 400 {
            let err_body: String = resp.into_body().read_to_string().unwrap_or_default();
            bail!("attachment upload failed (HTTP {}): {}", status, err_body);
        }
        Ok(())
    }
}

// ── private helpers ─────────────────────────────────────────────────────

fn parse_page_info(value: &Value) -> Result<PageInfo> {
    let id = value["id"]
        .as_str()
        .context("page JSON missing `id`")?
        .to_string();
    let title = value["title"]
        .as_str()
        .context("page JSON missing `title`")?
        .to_string();
    let version = value["version"]["number"].as_u64().unwrap_or(1) as u32;
    Ok(PageInfo { id, title, version })
}

fn parse_attachment_info(value: &Value) -> Result<AttachmentInfo> {
    let id = value["id"]
        .as_str()
        .context("attachment JSON missing `id`")?
        .to_string();
    let title = value["title"]
        .as_str()
        .context("attachment JSON missing `title`")?
        .to_string();
    let comment = value["metadata"]["comment"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    Ok(AttachmentInfo { id, title, comment })
}

/// Minimal percent-encoding for query parameter values.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => {
                out.push('%');
                out.push(char::from(HEX[(b >> 4) as usize]));
                out.push(char::from(HEX[(b & 0xf) as usize]));
            }
        }
    }
    out
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

fn random_u64() -> u64 {
    let mut rng = OsRng;
    rng.next_u64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_only_client_rejects_plain_http() {
        let client = ConfluenceClient::new("http://127.0.0.1:1", "token");
        let err = client
            .get_page_by_id("123")
            .expect_err("HTTP requests must be rejected in HTTPS-only mode");
        let msg = format!("{err:#}").to_ascii_lowercase();
        assert!(msg.contains("https") || msg.contains("tls"));
    }
}
