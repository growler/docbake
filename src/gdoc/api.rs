//! Google Drive v3 REST API client for creating and updating Google Docs.
//!
//! Uses `ureq` for synchronous HTTP requests — no async runtime required.
//! Files are uploaded via the multipart/related protocol with automatic
//! conversion to native Google Docs format.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// Google Drive API v3 upload endpoint.
const DRIVE_UPLOAD_URL: &str = "https://www.googleapis.com/upload/drive/v3/files";

/// Google Drive API v3 metadata endpoint.
const DRIVE_FILES_URL: &str = "https://www.googleapis.com/drive/v3/files";

/// Google Docs API v1 document endpoint.
const DOCS_DOCUMENTS_URL: &str = "https://docs.googleapis.com/v1/documents";

/// Google Docs MIME type (target format for import conversion).
const GOOGLE_DOCS_MIME: &str = "application/vnd.google-apps.document";

/// DOCX MIME type (source format for upload).
const DOCX_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

/// Metadata about a created/updated Google Drive file.
#[derive(Debug, Deserialize)]
pub(crate) struct DriveFile {
    pub id: String,
    #[allow(dead_code)]
    pub name: Option<String>,
    #[serde(rename = "mimeType")]
    #[allow(dead_code)]
    pub mime_type: Option<String>,
    #[serde(rename = "webViewLink")]
    pub web_view_link: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GDocDocument {
    body: Option<GDocBody>,
}

#[derive(Debug, Deserialize)]
struct GDocBody {
    content: Vec<GDocStructuralElement>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GDocStructuralElement {
    start_index: Option<i32>,
    end_index: Option<i32>,
    paragraph: Option<GDocParagraph>,
    table_of_contents: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GDocParagraph {
    elements: Option<Vec<GDocParagraphElement>>,
    paragraph_style: Option<GDocParagraphStyle>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GDocParagraphElement {
    text_run: Option<GDocTextRun>,
}

#[derive(Debug, Deserialize)]
struct GDocTextRun {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GDocParagraphStyle {
    named_style_type: Option<String>,
}

/// Blocking HTTP client for the Google Drive v3 API.
pub(crate) struct GoogleDriveClient {
    access_token: String,
    agent: ureq::Agent,
}

impl GoogleDriveClient {
    pub fn new(access_token: &str) -> Self {
        Self {
            access_token: access_token.to_string(),
            agent: ureq::Agent::new_with_defaults(),
        }
    }

    /// Create a new Google Doc by uploading DOCX content with import conversion.
    ///
    /// The DOCX data is uploaded as `multipart/related` with file metadata
    /// specifying `mimeType = application/vnd.google-apps.document`.
    /// Google Drive automatically converts the DOCX to native Google Docs format.
    ///
    /// If `folder_id` is provided, the file is created in that folder.
    pub fn create_doc(
        &self,
        name: &str,
        docx_data: &[u8],
        folder_id: Option<&str>,
    ) -> Result<DriveFile> {
        let mut metadata = serde_json::json!({
            "name": name,
            "mimeType": GOOGLE_DOCS_MIME,
        });
        if let Some(fid) = folder_id {
            metadata["parents"] = serde_json::json!([fid]);
        }

        self.multipart_upload(
            "POST",
            &format!("{DRIVE_UPLOAD_URL}?uploadType=multipart&fields=id,name,mimeType,webViewLink"),
            &metadata,
            docx_data,
        )
    }

    /// Update an existing Google Doc by replacing its content with new DOCX.
    ///
    /// The entire document content is replaced.  Metadata (name, folder)
    /// is preserved unless explicitly changed in the metadata part.
    pub fn update_doc(&self, file_id: &str, docx_data: &[u8]) -> Result<DriveFile> {
        let metadata = serde_json::json!({
            "mimeType": GOOGLE_DOCS_MIME,
        });

        self.multipart_upload(
            "PATCH",
            &format!(
                "{DRIVE_UPLOAD_URL}/{file_id}?uploadType=multipart&fields=id,name,mimeType,webViewLink"
            ),
            &metadata,
            docx_data,
        )
    }

    /// Get file metadata by ID.
    #[allow(dead_code)]
    pub fn get_file(&self, file_id: &str) -> Result<DriveFile> {
        let url = format!("{DRIVE_FILES_URL}/{file_id}?fields=id,name,mimeType,webViewLink");
        let resp = self
            .agent
            .get(&url)
            .header("Authorization", &format!("Bearer {}", self.access_token))
            .header("Accept", "application/json")
            .call()
            .with_context(|| format!("GET {url}"))?;

        let file: DriveFile = resp
            .into_body()
            .read_json()
            .context("failed to parse file metadata")?;
        Ok(file)
    }

    /// Replace any existing TOC with a fresh native Google Docs TOC block.
    pub fn ensure_native_toc(&self, file_id: &str) -> Result<()> {
        let doc = self.get_document(file_id)?;
        let content = doc
            .body
            .map(|b| b.content)
            .context("Google Docs document body is missing")?;

        let mut toc_start: Option<i32> = None;
        let mut toc_end: Option<i32> = None;
        let mut first_heading1_start: Option<i32> = None;

        for element in &content {
            let Some(start) = element.start_index else {
                continue;
            };
            let end = element.end_index.unwrap_or(start);

            if element.table_of_contents.is_some() {
                toc_start = Some(start);
                toc_end = Some(end);
                continue;
            }

            if let Some(paragraph) = &element.paragraph {
                if first_heading1_start.is_none()
                    && paragraph
                        .paragraph_style
                        .as_ref()
                        .and_then(|s| s.named_style_type.as_deref())
                        == Some("HEADING_1")
                {
                    first_heading1_start = Some(start);
                }

                if toc_start.is_none() {
                    let text = paragraph
                        .elements
                        .as_ref()
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|it| it.text_run.as_ref())
                                .filter_map(|tr| tr.content.as_deref())
                                .collect::<String>()
                        })
                        .unwrap_or_default();
                    if text.contains("Table of Contents") || text.contains("Содержание") {
                        toc_start = Some(start);
                        toc_end = Some(end);
                    }
                }
            }
        }

        let (delete_range, insert_index) = match (toc_start, toc_end, first_heading1_start) {
            (Some(start), Some(end), _) if end > start => (Some((start, end)), start),
            (Some(start), _, Some(h1)) if h1 > start => (Some((start, h1)), start),
            (Some(start), _, _) => (None, start),
            (None, _, _) => (None, 1),
        };

        let mut requests = Vec::new();
        if let Some((start, end)) = delete_range {
            requests.push(serde_json::json!({
                "deleteContentRange": {
                    "range": {
                        "startIndex": start,
                        "endIndex": end
                    }
                }
            }));
        }
        requests.push(serde_json::json!({
            "insertTableOfContents": {
                "location": {
                    "index": insert_index
                }
            }
        }));

        let url = format!("{DOCS_DOCUMENTS_URL}/{file_id}:batchUpdate");
        let payload = serde_json::json!({ "requests": requests });

        let req = self
            .agent
            .post(&url)
            .header("Authorization", &format!("Bearer {}", self.access_token))
            .header("Accept", "application/json");
        let resp = req
            .config()
            .http_status_as_error(false)
            .build()
            .send_json(payload)
            .with_context(|| format!("POST {url}"))?;

        let status = resp.status().as_u16();
        if status >= 400 {
            let err_body: String = resp.into_body().read_to_string().unwrap_or_default();
            bail!("Google Docs API batchUpdate error (HTTP {status}): {err_body}");
        }

        Ok(())
    }

    /// Build and send a multipart/related upload request.
    fn multipart_upload(
        &self,
        method: &str,
        url: &str,
        metadata: &serde_json::Value,
        docx_data: &[u8],
    ) -> Result<DriveFile> {
        let boundary = format!("docbake_gdoc_{:016x}", rand_u64());

        let metadata_json =
            serde_json::to_string(metadata).context("failed to serialize file metadata")?;

        // Build multipart/related body (RFC 2387).
        let mut body = Vec::with_capacity(metadata_json.len() + docx_data.len() + 256);

        // -- metadata part
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(b"Content-Type: application/json; charset=UTF-8\r\n\r\n");
        body.extend_from_slice(metadata_json.as_bytes());
        body.extend_from_slice(b"\r\n");

        // -- content part
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(format!("Content-Type: {DOCX_MIME}\r\n\r\n").as_bytes());
        body.extend_from_slice(docx_data);
        body.extend_from_slice(b"\r\n");

        // -- close boundary
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

        let content_type = format!("multipart/related; boundary={boundary}");

        let req = match method {
            "PATCH" => self.agent.patch(url),
            _ => self.agent.post(url),
        };

        let resp = req
            .header("Authorization", &format!("Bearer {}", self.access_token))
            .header("Accept", "application/json")
            .content_type(&content_type)
            .send(body.as_slice())
            .with_context(|| format!("{method} {url}"))?;

        let status = resp.status().as_u16();
        if status >= 400 {
            let err_body: String = resp.into_body().read_to_string().unwrap_or_default();
            bail!("Google Drive API error (HTTP {status}): {err_body}");
        }

        let file: DriveFile = resp
            .into_body()
            .read_json()
            .context("failed to parse Google Drive response")?;
        Ok(file)
    }

    fn get_document(&self, file_id: &str) -> Result<GDocDocument> {
        let url = format!("{DOCS_DOCUMENTS_URL}/{file_id}");
        let req = self
            .agent
            .get(&url)
            .header("Authorization", &format!("Bearer {}", self.access_token))
            .header("Accept", "application/json");
        let resp = req
            .config()
            .http_status_as_error(false)
            .build()
            .call()
            .with_context(|| format!("GET {url}"))?;

        let status = resp.status().as_u16();
        if status >= 400 {
            let err_body: String = resp.into_body().read_to_string().unwrap_or_default();
            bail!("Google Docs API error (HTTP {status}): {err_body}");
        }

        let doc: GDocDocument = resp
            .into_body()
            .read_json()
            .context("failed to parse Google Docs document response")?;
        Ok(doc)
    }
}

/// Build a Google Docs URL from a document ID.
pub(crate) fn google_docs_url(doc_id: &str) -> String {
    format!("https://docs.google.com/document/d/{doc_id}/edit")
}

/// Extract a Google Doc ID from a URL.
///
/// Recognises patterns:
/// - `https://docs.google.com/document/d/{ID}/...`
/// - Raw ID (no slashes)
pub(crate) fn parse_doc_id(url_or_id: &str) -> &str {
    if let Some(rest) = url_or_id
        .strip_prefix("https://docs.google.com/document/d/")
        .or_else(|| url_or_id.strip_prefix("http://docs.google.com/document/d/"))
    {
        rest.split('/').next().unwrap_or(rest)
    } else {
        url_or_id
    }
}

/// Extract a Google Drive folder ID from a URL.
///
/// Recognises patterns:
/// - `https://drive.google.com/drive/folders/{ID}...`
/// - `https://drive.google.com/drive/u/0/folders/{ID}...`
/// - Raw ID (no slashes)
pub(crate) fn parse_folder_id(url_or_id: &str) -> &str {
    // Try to find "/folders/" in the URL.
    if let Some(idx) = url_or_id.find("/folders/") {
        let after = &url_or_id[idx + 9..]; // skip "/folders/"
                                           // The ID ends at the next '/', '?', or '#' (or end of string).
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        &after[..end]
    } else {
        url_or_id
    }
}

/// Simple non-cryptographic random u64 for multipart boundary generation.
fn rand_u64() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    RandomState::new().build_hasher().finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_doc_id_from_url() {
        assert_eq!(
            parse_doc_id("https://docs.google.com/document/d/1AbCdEfGhIjK/edit"),
            "1AbCdEfGhIjK"
        );
    }

    #[test]
    fn parse_doc_id_raw() {
        assert_eq!(parse_doc_id("1AbCdEfGhIjK"), "1AbCdEfGhIjK");
    }

    #[test]
    fn parse_folder_id_from_url() {
        assert_eq!(
            parse_folder_id("https://drive.google.com/drive/folders/1XyZ_AbC"),
            "1XyZ_AbC"
        );
    }

    #[test]
    fn parse_folder_id_with_user_prefix() {
        assert_eq!(
            parse_folder_id("https://drive.google.com/drive/u/0/folders/1XyZ_AbC?resourcekey=xyz"),
            "1XyZ_AbC"
        );
    }

    #[test]
    fn parse_folder_id_raw() {
        assert_eq!(parse_folder_id("1XyZ_AbC"), "1XyZ_AbC");
    }
}
