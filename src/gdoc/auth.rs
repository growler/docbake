//! Google authentication for the `publish gdoc` and `auth gdoc` commands.
//!
//! Two authentication flows are supported:
//!
//! 1. **Service account** — a JSON key file from Google Cloud Console.
//!    The tool signs a JWT with RS256 and exchanges it for a short-lived
//!    access token.  No user interaction required.
//!
//! 2. **OAuth2 installed-app flow** — opens a browser for consent, listens
//!    on `127.0.0.1` for the callback, exchanges the authorization code
//!    for access + refresh tokens.  Refresh tokens are stored in the
//!    credential store so subsequent runs do not require browser interaction.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Google OAuth2 endpoints.
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";

/// Scope required for creating / updating files via the Drive API.
const DRIVE_FILE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";

/// Hardcoded OAuth2 client ID for the docbake desktop application.
///
/// This is not a secret — Google's security model for "Desktop" app type
/// relies on the redirect to `http://127.0.0.1` + PKCE, not on client
/// secret confidentiality.
///
/// Replace these with your own values from Google Cloud Console.
const OAUTH_CLIENT_ID: &str =
    "YOUR_CLIENT_ID.apps.googleusercontent.com";
const OAUTH_CLIENT_SECRET: &str = "YOUR_CLIENT_SECRET";

// ── Service account types ───────────────────────────────────────────────

/// Subset of a Google service account JSON key file.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct ServiceAccountKey {
    pub client_email: String,
    pub private_key: String,
    pub private_key_id: String,
    #[serde(default)]
    pub token_uri: String,
}

/// JWT claims for the service account assertion.
#[derive(Debug, Serialize)]
struct JwtClaims {
    iss: String,
    scope: String,
    aud: String,
    iat: u64,
    exp: u64,
}

// ── OAuth2 credential types ─────────────────────────────────────────────

/// Stored OAuth2 credential (refresh token + optional cached access token).
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct OAuthCredential {
    pub refresh_token: String,
    /// Cached access token (may be expired).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    /// Unix timestamp when `access_token` expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}

/// Response from Google's token endpoint.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    #[serde(default)]
    refresh_token: Option<String>,
    #[allow(dead_code)]
    token_type: String,
}

// ── Service account flow ────────────────────────────────────────────────

/// Exchange a service account key for a short-lived access token.
///
/// Constructs a JWT assertion signed with RS256, posts it to the Google
/// token endpoint, and returns the access token string.
pub(crate) fn service_account_access_token(key: &ServiceAccountKey) -> Result<(String, u64)> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock error")?
        .as_secs();

    let claims = JwtClaims {
        iss: key.client_email.clone(),
        scope: DRIVE_FILE_SCOPE.to_string(),
        aud: GOOGLE_TOKEN_URL.to_string(),
        iat: now,
        exp: now + 3600,
    };

    let header = jsonwebtoken::Header {
        alg: jsonwebtoken::Algorithm::RS256,
        kid: Some(key.private_key_id.clone()),
        ..Default::default()
    };

    let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(key.private_key.as_bytes())
        .context("invalid RSA private key in service account JSON")?;

    let assertion = jsonwebtoken::encode(&header, &claims, &encoding_key)
        .context("failed to sign JWT assertion")?;

    // Exchange JWT for access token.
    let agent = ureq::Agent::new_with_defaults();
    let form = format!(
        "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={assertion}"
    );
    let resp: TokenResponse = agent
        .post(GOOGLE_TOKEN_URL)
        .content_type("application/x-www-form-urlencoded")
        .send(form.as_bytes())
        .context("token exchange request failed")?
        .into_body()
        .read_json()
        .context("failed to parse token response")?;

    let expires_at = now + resp.expires_in;
    Ok((resp.access_token, expires_at))
}

// ── OAuth2 installed-app flow ───────────────────────────────────────────

/// Perform the OAuth2 authorization code flow with PKCE.
///
/// 1. Starts a local HTTP server on a random port.
/// 2. Opens the browser to Google's consent page.
/// 3. Waits for the redirect callback.
/// 4. Exchanges the authorization code for tokens.
///
/// Returns the full token response (access_token + refresh_token).
pub(crate) fn oauth2_authorization_flow() -> Result<OAuthCredential> {
    // Generate PKCE code verifier (43-128 chars, RFC 7636).
    let code_verifier = generate_code_verifier();
    let code_challenge = generate_code_challenge(&code_verifier);

    // Exchange code for tokens — agent reused below.
    let agent = ureq::Agent::new_with_defaults();

    // Start local server on a random port.
    let server = tiny_http::Server::http("127.0.0.1:0")
        .map_err(|e| anyhow::anyhow!("failed to start local server: {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .context("failed to get local server address")?
        .port();

    let redirect_uri = format!("http://127.0.0.1:{port}");
    let state = format!("{:016x}", rand_u64());

    let auth_url = format!(
        "{GOOGLE_AUTH_URL}?\
         client_id={OAUTH_CLIENT_ID}&\
         redirect_uri={redirect_uri}&\
         response_type=code&\
         scope={DRIVE_FILE_SCOPE}&\
         code_challenge={code_challenge}&\
         code_challenge_method=S256&\
         access_type=offline&\
         prompt=consent&\
         state={state}"
    );

    eprintln!("Opening browser for Google authorization...");
    eprintln!("If the browser does not open, visit:");
    eprintln!("  {auth_url}");

    if let Err(e) = open::that(&auth_url) {
        eprintln!("Warning: failed to open browser: {e}");
    }

    // Wait for the callback.
    let request = server
        .recv()
        .context("failed to receive callback from browser")?;

    let url = request.url().to_string();
    let response_html = "<!DOCTYPE html><html><body>\
        <h2>Authentication successful</h2>\
        <p>You can close this tab and return to the terminal.</p>\
        </body></html>";
    let response = tiny_http::Response::from_string(response_html).with_header(
        "Content-Type: text/html"
            .parse::<tiny_http::Header>()
            .unwrap(),
    );
    let _ = request.respond(response);

    // Parse the callback URL to extract code and state.
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or_default();

    let mut code = None;
    let mut returned_state = None;

    for param in query.split('&') {
        if let Some((key, value)) = param.split_once('=') {
            match key {
                "code" => code = Some(value.to_string()),
                "state" => returned_state = Some(value.to_string()),
                "error" => bail!("Google authorization error: {value}"),
                _ => {}
            }
        }
    }

    let code = code.context("no authorization code in callback")?;
    if returned_state.as_deref() != Some(&state) {
        bail!("OAuth state mismatch — possible CSRF attack");
    }

    // Exchange code for tokens.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock error")?
        .as_secs();

    let form = format!(
        "client_id={OAUTH_CLIENT_ID}&\
         client_secret={OAUTH_CLIENT_SECRET}&\
         code={code}&\
         code_verifier={code_verifier}&\
         grant_type=authorization_code&\
         redirect_uri={redirect_uri}"
    );
    let resp: TokenResponse = agent
        .post(GOOGLE_TOKEN_URL)
        .content_type("application/x-www-form-urlencoded")
        .send(form.as_bytes())
        .context("token exchange request failed")?
        .into_body()
        .read_json()
        .context("failed to parse token response")?;

    let refresh_token = resp
        .refresh_token
        .context("no refresh token in response (was access_type=offline set?)")?;

    let expires_at = now + resp.expires_in;

    Ok(OAuthCredential {
        refresh_token,
        access_token: Some(resp.access_token),
        expires_at: Some(expires_at),
    })
}

/// Refresh an access token using a stored refresh token.
pub(crate) fn refresh_access_token(refresh_token: &str) -> Result<(String, u64)> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock error")?
        .as_secs();

    let agent = ureq::Agent::new_with_defaults();
    let form = format!(
        "client_id={OAUTH_CLIENT_ID}&\
         client_secret={OAUTH_CLIENT_SECRET}&\
         refresh_token={refresh_token}&\
         grant_type=refresh_token"
    );
    let resp: TokenResponse = agent
        .post(GOOGLE_TOKEN_URL)
        .content_type("application/x-www-form-urlencoded")
        .send(form.as_bytes())
        .context("refresh token request failed")?
        .into_body()
        .read_json()
        .context("failed to parse refresh token response")?;

    let expires_at = now + resp.expires_in;
    Ok((resp.access_token, expires_at))
}

// ── PKCE helpers ────────────────────────────────────────────────────────

/// Generate a PKCE code verifier (RFC 7636 §4.1).
fn generate_code_verifier() -> String {
    // 32 random bytes → 43-char base64url string (well within 43–128 range).
    let mut bytes = [0u8; 32];
    // Use the OS random source for cryptographic randomness.
    #[cfg(unix)]
    {
        use std::io::Read;
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            let _ = f.read_exact(&mut bytes);
        }
    }
    #[cfg(windows)]
    {
        // Fallback: use multiple RandomState hashes for reasonable entropy.
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        for chunk in bytes.chunks_mut(8) {
            let val = RandomState::new().build_hasher().finish().to_le_bytes();
            for (dst, src) in chunk.iter_mut().zip(val.iter()) {
                *dst = *src;
            }
        }
    }
    base64url_encode(&bytes)
}

/// Generate a PKCE code challenge from a code verifier (RFC 7636 §4.2).
///
/// `challenge = BASE64URL(SHA256(verifier))`
fn generate_code_challenge(verifier: &str) -> String {
    // Use blake3 (already a dependency) to compute the SHA-256.
    // Wait — blake3 is not SHA-256. We need actual SHA-256 for PKCE.
    // jsonwebtoken pulls in sha2 transitively, so we can use that.
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(verifier.as_bytes());
    base64url_encode(&hash)
}

/// Base64url encode without padding (RFC 4648 §5).
fn base64url_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((data.len() * 4 + 2) / 3);
    let mut i = 0;
    while i + 2 < data.len() {
        let n = (data[i] as u32) << 16 | (data[i + 1] as u32) << 8 | data[i + 2] as u32;
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        out.push(TABLE[(n & 0x3f) as usize] as char);
        i += 3;
    }
    let remaining = data.len() - i;
    if remaining == 2 {
        let n = (data[i] as u32) << 16 | (data[i + 1] as u32) << 8;
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        // no padding
    } else if remaining == 1 {
        let n = (data[i] as u32) << 16;
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        // no padding
    }
    out
}

/// Simple non-cryptographic random u64 for state parameters.
fn rand_u64() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    RandomState::new().build_hasher().finish()
}
