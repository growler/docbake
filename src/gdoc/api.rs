//! Google Drive v3 REST API client for creating and updating Google Docs.
//!
//! Uses `ureq` for synchronous HTTP requests — no async runtime required.
//! Files are uploaded via the multipart/related protocol with automatic
//! conversion to native Google Docs format.

use std::io::Read;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// Google Drive API v3 upload endpoint.
const DRIVE_UPLOAD_URL: &str = "https://www.googleapis.com/upload/drive/v3/files";

/// Google Drive API v3 metadata endpoint.
const DRIVE_FILES_URL: &str = "https://www.googleapis.com/drive/v3/files";

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

    /// Build and send a multipart/related upload request.
    ///
    /// The body is the metadata part and the content-part headers, then
    /// `docx_data`, then the close boundary, read in sequence. The
    /// Content-Length header is the exact body length.
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
        // -- metadata part, then the headers of the content part
        let head = format!(
            "--{boundary}\r\n\
             Content-Type: application/json; charset=UTF-8\r\n\r\n\
             {metadata_json}\r\n\
             --{boundary}\r\n\
             Content-Type: {DOCX_MIME}\r\n\r\n"
        );
        // -- end of content part, then close boundary
        let tail = format!("\r\n--{boundary}--\r\n");
        // The reader must give exactly `content_length` bytes, thus both come
        // from the same three slices. With fewer bytes, ureq waits without end.
        let content_length = head.len() + docx_data.len() + tail.len();
        let mut body = head.as_bytes().chain(docx_data).chain(tail.as_bytes());

        let content_type = format!("multipart/related; boundary={boundary}");

        let req = match method {
            "PATCH" => self.agent.patch(url),
            _ => self.agent.post(url),
        };

        let resp = req
            .header("Authorization", &format!("Bearer {}", self.access_token))
            .header("Accept", "application/json")
            .content_type(&content_type)
            .header("Content-Length", content_length)
            .send(ureq::SendBody::from_reader(&mut body))
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
}

#[cfg(test)]
impl GoogleDriveClient {
    /// Export a Google Doc as `mime` and write the response body to `dest`.
    ///
    /// Returns the number of bytes written.
    fn export_to_file(&self, file_id: &str, mime: &str, dest: &std::path::Path) -> Result<u64> {
        let url = format!("{DRIVE_FILES_URL}/{file_id}/export");
        let req = self
            .agent
            .get(&url)
            .query("mimeType", mime)
            .header("Authorization", &format!("Bearer {}", self.access_token));
        let resp = req
            .config()
            .http_status_as_error(false)
            .build()
            .call()
            .with_context(|| format!("GET {url}"))?;

        let status = resp.status().as_u16();
        if status >= 400 {
            let err_body: String = resp.into_body().read_to_string().unwrap_or_default();
            bail!("Google Drive API export error (HTTP {status}): {err_body}");
        }

        let mut file = std::fs::File::create(dest)
            .with_context(|| format!("failed to create {}", dest.display()))?;
        std::io::copy(&mut resp.into_body().into_reader(), &mut file)
            .inspect_err(|_| {
                // Do not leave a part-written file.
                let _ = std::fs::remove_file(dest);
            })
            .with_context(|| format!("failed to write {}", dest.display()))
    }

    /// Delete a Drive file permanently. The file does not go to the trash.
    fn delete(&self, file_id: &str) -> Result<()> {
        let url = format!("{DRIVE_FILES_URL}/{file_id}");
        let req = self
            .agent
            .delete(&url)
            .header("Authorization", &format!("Bearer {}", self.access_token));
        let resp = req
            .config()
            .http_status_as_error(false)
            .build()
            .call()
            .with_context(|| format!("DELETE {url}"))?;

        let status = resp.status().as_u16();
        if status >= 400 {
            let err_body: String = resp.into_body().read_to_string().unwrap_or_default();
            bail!("Google Drive API delete error (HTTP {status}): {err_body}");
        }
        Ok(())
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

    /// Read one HTTP request: return the head (up to and with CRLFCRLF) as
    /// text and the body of `Content-Length` bytes.
    fn read_request(stream: &mut std::net::TcpStream) -> (String, Vec<u8>) {
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        while !buf.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).expect("read request head");
            buf.push(byte[0]);
        }
        let head = String::from_utf8(buf).expect("request head is UTF-8");
        let len: usize = header_values(&head, "content-length")
            .first()
            .expect("Content-Length header")
            .parse()
            .expect("Content-Length is a number");
        let mut body = vec![0u8; len];
        stream.read_exact(&mut body).expect("read request body");
        (head, body)
    }

    /// Return the values of all headers in `head` with the name `name`
    /// (case-insensitive).
    fn header_values<'a>(head: &'a str, name: &str) -> Vec<&'a str> {
        head.lines()
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .filter(|(k, _)| k.trim().eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim())
            .collect()
    }

    /// Accept one connection on `listener` in 10 s. Panic with the result of
    /// `upload` if the thread ends before it connects.
    fn accept_or_fail<T: std::fmt::Debug>(
        listener: &std::net::TcpListener,
        upload: std::thread::JoinHandle<T>,
    ) -> (std::net::TcpStream, std::thread::JoinHandle<T>) {
        use std::time::{Duration, Instant};

        listener.set_nonblocking(true).expect("set non-blocking");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    // An accepted socket can inherit non-blocking mode.
                    stream.set_nonblocking(false).expect("set blocking");
                    return (stream, upload);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("accept: {e}"),
            }
            if upload.is_finished() {
                panic!("upload ended before it connected: {:?}", upload.join());
            }
            assert!(Instant::now() < deadline, "no connection in 10 s");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn multipart_upload_sends_exact_body_and_content_length() {
        use std::io::Write;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!(
            "http://{}/path?query",
            listener.local_addr().expect("local address")
        );
        // Larger than the 128 KiB output buffer of ureq.
        let docx: Vec<u8> = (0..300_000).map(|i| (i % 251) as u8).collect();
        let metadata = serde_json::json!({ "mimeType": GOOGLE_DOCS_MIME });

        let upload = {
            let (url, metadata, docx) = (url.clone(), metadata.clone(), docx.clone());
            std::thread::spawn(move || {
                // No proxy from the environment.
                let config = ureq::Agent::config_builder().proxy(None).build();
                let client = GoogleDriveClient {
                    access_token: "test-token".into(),
                    agent: ureq::Agent::new_with_config(config),
                };
                client.multipart_upload("PATCH", &url, &metadata, &docx)
            })
        };

        let (mut stream, upload) = accept_or_fail(&listener, upload);
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .expect("set read timeout");
        let (head, body) = read_request(&mut stream);
        let reply = r#"{"id":"doc-1","name":"Test"}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\r\n{reply}",
            reply.len()
        )
        .expect("write response");

        let file = upload.join().expect("upload thread").expect("upload");
        assert_eq!(file.id, "doc-1");

        assert_eq!(head.lines().next(), Some("PATCH /path?query HTTP/1.1"));
        assert_eq!(header_values(&head, "authorization"), ["Bearer test-token"]);
        assert!(header_values(&head, "transfer-encoding").is_empty());
        let content_type = header_values(&head, "content-type");
        let boundary = content_type
            .first()
            .and_then(|v| v.strip_prefix("multipart/related; boundary="))
            .expect("multipart/related content type");

        let mut expected = Vec::new();
        expected.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        expected.extend_from_slice(b"Content-Type: application/json; charset=UTF-8\r\n\r\n");
        expected.extend_from_slice(format!(r#"{{"mimeType":"{GOOGLE_DOCS_MIME}"}}"#).as_bytes());
        expected.extend_from_slice(format!("\r\n--{boundary}\r\n").as_bytes());
        expected.extend_from_slice(format!("Content-Type: {DOCX_MIME}\r\n\r\n").as_bytes());
        expected.extend_from_slice(&docx);
        expected.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

        assert_eq!(
            header_values(&head, "content-length"),
            [expected.len().to_string()]
        );
        assert!(body == expected, "request body differs from expected body");
    }

    /// Upload `docs/research/docx-gdoc-probe/probe.docx` to My Drive with
    /// conversion to a Google Doc, the same as `publish gdoc`. Export the
    /// Google Doc as DOCX and PDF to `result/probe-import.*`. Delete the
    /// Google Doc at the end, also when an export fails.
    ///
    /// Needs stored Google Docs credentials and network access:
    /// `cargo test import_probe_and_export -- --ignored --nocapture`
    #[test]
    #[ignore = "uploads to Google Drive; needs credentials and network access"]
    fn import_probe_and_export() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/research/docx-gdoc-probe");
        let docx = std::fs::read(dir.join("probe.docx"))
            .expect("read probe.docx (run build.sh to make it)");
        let out = dir.join("result");
        std::fs::create_dir_all(&out).expect("create result directory");
        // Remove the files of an earlier run, so that a failed export does
        // not leave old files next to new ones.
        for name in ["probe-import.docx", "probe-import.pdf"] {
            match std::fs::remove_file(out.join(name)) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                    panic!("remove {name}: {err}")
                }
                _ => {}
            }
        }

        let token = crate::credentials::resolve_gdoc_access_token().expect("resolve access token");
        let client = GoogleDriveClient::new(&token);
        let file = client
            .create_doc("docbake DOCX import probe", &docx, None)
            .expect("upload probe.docx");
        println!("Google Doc: {}", google_docs_url(&file.id));

        let exported = [("docx", DOCX_MIME), ("pdf", "application/pdf")]
            .into_iter()
            .try_for_each(|(ext, mime)| -> Result<()> {
                let dest = out.join(format!("probe-import.{ext}"));
                let n = client.export_to_file(&file.id, mime, &dest)?;
                println!("wrote {} ({n} bytes)", dest.display());
                Ok(())
            });

        let deleted = client.delete(&file.id);
        println!("delete {}: {deleted:?}", file.id);
        exported.expect("export Google Doc");
        deleted.expect("delete Google Doc");
    }
}
