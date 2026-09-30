//! Ingress JWT validation for enterprise IdP-issued application credentials.
//!
//! Supports HS256 shared secret, RS256 PEM public key, a local JWKS file, or a
//! remote JWKS URL fetched at startup with refresh-on-unknown-kid for key rotation.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tracing::warn;

use crate::config::ServerConfig;

const MAX_JWKS_BYTES: usize = 1_048_576;
const JWKS_FETCH_TIMEOUT: Duration = Duration::from_secs(15);
const JWKS_REFRESH_MIN_INTERVAL: Duration = Duration::from_secs(60);
/// Short cooldown after a failed refresh so one IdP hiccup cannot lock out
/// unknown-kid validations for the full success interval.
const JWKS_REFRESH_FAILURE_BACKOFF: Duration = Duration::from_secs(5);

#[derive(Clone)]
enum JwksRefreshSource {
    Url(String),
    File(String),
}

#[derive(Clone)]
struct RefreshableJwksCache {
    keys: Arc<RwLock<HashMap<String, (Algorithm, DecodingKey)>>>,
    last_refresh_attempt: Arc<RwLock<Option<(Instant, bool)>>>,
    source: JwksRefreshSource,
}

impl RefreshableJwksCache {
    fn new(source: JwksRefreshSource, keys: HashMap<String, (Algorithm, DecodingKey)>) -> Self {
        Self {
            keys: Arc::new(RwLock::new(keys)),
            last_refresh_attempt: Arc::new(RwLock::new(None)),
            source,
        }
    }

    fn lookup(&self, kid: &str) -> Option<(Algorithm, DecodingKey)> {
        self.keys
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .get(kid)
            .cloned()
    }

    fn try_refresh(&self) -> bool {
        let mut last = self
            .last_refresh_attempt
            .write()
            .unwrap_or_else(|error| error.into_inner());
        // Success cools down for the full interval; a failed refresh only
        // cools down for the short failure backoff so one IdP hiccup cannot
        // lock out unknown-kid validations for a full minute.
        let (previous_interval, failed) = match *last {
            Some((at, previous_failed)) => (
                if previous_failed {
                    JWKS_REFRESH_FAILURE_BACKOFF
                } else {
                    JWKS_REFRESH_MIN_INTERVAL
                },
                previous_failed,
            ),
            None => (JWKS_REFRESH_MIN_INTERVAL, false),
        };
        if let Some((at, _)) = *last {
            if at.elapsed() < previous_interval {
                return false;
            }
        }
        *last = Some((Instant::now(), failed));
        drop(last);

        // Runtime refresh always enforces HTTPS for URL sources to prevent
        // MITM attacks that could replace public keys during key rotation.
        let refreshed = match &self.source {
            JwksRefreshSource::Url(url) => fetch_jwks_url(url, true),
            JwksRefreshSource::File(path) => load_jwks_file(path),
        };
        match refreshed {
            Ok(keys) => {
                *self.keys.write().unwrap_or_else(|error| error.into_inner()) = keys;
                *self
                    .last_refresh_attempt
                    .write()
                    .unwrap_or_else(|error| error.into_inner()) =
                    Some((Instant::now(), false));
                true
            }
            Err(error) => {
                warn!(error = %error, "ingress jwks refresh failed");
                *self
                    .last_refresh_attempt
                    .write()
                    .unwrap_or_else(|error| error.into_inner()) =
                    Some((Instant::now(), true));
                false
            }
        }
    }
}

/// Verified tenant/user identity extracted from an ingress JWT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedIngressIdentity {
    pub tenant_id: String,
    pub user_id: String,
}

#[derive(Debug, Deserialize)]
struct IngressJwtClaims {
    sub: String,
    #[serde(rename = "tenant_id")]
    tenant_id: Option<String>,
    #[serde(rename = "user_id")]
    user_id: Option<String>,
}

#[derive(Clone)]
pub struct IngressJwtValidator {
    issuer: Option<String>,
    audience: Option<String>,
    hs256_secret: Option<String>,
    rsa_decoding_key: Option<DecodingKey>,
    jwks_cache: Option<RefreshableJwksCache>,
}

impl IngressJwtValidator {
    pub fn from_config(config: &ServerConfig) -> Result<Self, String> {
        let issuer = config
            .ingress_jwt_issuer
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let audience = config
            .ingress_jwt_audience
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        let (hs256_secret, rsa_decoding_key, jwks_cache) = if let Some(path) = config
            .ingress_jwt_jwks_file
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            let keys = load_jwks_file(path)?;
            (
                None,
                None,
                Some(RefreshableJwksCache::new(
                    JwksRefreshSource::File(path.to_string()),
                    keys,
                )),
            )
        } else if let Some(url) = config
            .ingress_jwt_jwks_url
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            let require_https = config.is_production_kernel_profile();
            let keys = fetch_jwks_url(url, require_https)?;
            (
                None,
                None,
                Some(RefreshableJwksCache::new(
                    JwksRefreshSource::Url(url.to_string()),
                    keys,
                )),
            )
        } else if config.ingress_jwt_algorithm.eq_ignore_ascii_case("rs256") {
            let pem = config
                    .ingress_jwt_rsa_public_key_pem
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        "SDKWORK_KERNEL_INGRESS_JWT_RSA_PUBLIC_KEY_PEM is required for RS256 ingress JWT"
                            .to_string()
                    })?;
            (
                None,
                Some(
                    DecodingKey::from_rsa_pem(pem.as_bytes())
                        .map_err(|error| format!("invalid ingress RSA public key PEM: {error}"))?,
                ),
                None,
            )
        } else {
            let secret = config
                .ingress_jwt_secret
                .as_deref()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    "SDKWORK_KERNEL_INGRESS_JWT_SECRET is required for HS256 ingress JWT"
                        .to_string()
                })?
                .to_string();
            (Some(secret), None, None)
        };

        Ok(Self {
            issuer,
            audience,
            hs256_secret,
            rsa_decoding_key,
            jwks_cache,
        })
    }

    pub fn validate(&self, token: &str) -> Result<VerifiedIngressIdentity, StatusCode> {
        let (algorithm, decoding_key) = self.resolve_decoding_key(token)?;
        let mut validation = Validation::new(algorithm);
        validation.validate_exp = true;
        validation.validate_nbf = true;
        if let Some(issuer) = &self.issuer {
            validation.set_issuer(&[issuer.as_str()]);
        }
        if let Some(audience) = &self.audience {
            validation.set_audience(&[audience.as_str()]);
        }

        let token_data = decode::<IngressJwtClaims>(token, &decoding_key, &validation)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;

        identity_from_claims(token_data.claims)
    }

    fn resolve_decoding_key(&self, token: &str) -> Result<(Algorithm, DecodingKey), StatusCode> {
        if let Some(secret) = &self.hs256_secret {
            return Ok((
                Algorithm::HS256,
                DecodingKey::from_secret(secret.as_bytes()),
            ));
        }
        if let Some(decoding_key) = &self.rsa_decoding_key {
            return Ok((Algorithm::RS256, decoding_key.clone()));
        }
        if let Some(cache) = &self.jwks_cache {
            let header = decode_header(token).map_err(|_| StatusCode::UNAUTHORIZED)?;
            let kid = header.kid.ok_or(StatusCode::UNAUTHORIZED)?;
            if let Some(entry) = cache.lookup(&kid) {
                return Ok(entry);
            }
            if cache.try_refresh() {
                if let Some(entry) = cache.lookup(&kid) {
                    return Ok(entry);
                }
            }
            return Err(StatusCode::UNAUTHORIZED);
        }
        Err(StatusCode::SERVICE_UNAVAILABLE)
    }
}

fn identity_from_claims(claims: IngressJwtClaims) -> Result<VerifiedIngressIdentity, StatusCode> {
    let tenant_id = claims
        .tenant_id
        .filter(|value| !value.is_empty())
        .ok_or(StatusCode::FORBIDDEN)?;
    let user_id = claims
        .user_id
        .filter(|value| !value.is_empty())
        .unwrap_or(claims.sub);
    if user_id.is_empty() {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(VerifiedIngressIdentity { tenant_id, user_id })
}

fn jwk_algorithm(jwk: &jsonwebtoken::jwk::Jwk) -> Result<Algorithm, String> {
    match &jwk.algorithm {
        AlgorithmParameters::RSA(_) => Ok(Algorithm::RS256),
        other => Err(format!("unsupported ingress JWKS algorithm: {other:?}")),
    }
}

fn load_jwks_file(path: &str) -> Result<HashMap<String, (Algorithm, DecodingKey)>, String> {
    let raw = fs::read_to_string(Path::new(path))
        .map_err(|error| format!("failed to read ingress JWKS file {path}: {error}"))?;
    parse_jwks_json(&raw)
        .map_err(|error| format!("failed to parse ingress JWKS file {path}: {error}"))
}

fn fetch_jwks_url(
    url: &str,
    require_https: bool,
) -> Result<HashMap<String, (Algorithm, DecodingKey)>, String> {
    let trimmed = url.trim();
    if require_https && !trimmed.starts_with("https://") {
        return Err(format!(
            "ingress JWKS URL must use https:// in production: {trimmed}"
        ));
    }
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return Err(format!(
            "ingress JWKS URL must use http:// or https:// scheme: {trimmed}"
        ));
    }

    let agent = ureq::AgentBuilder::new()
        .timeout(JWKS_FETCH_TIMEOUT)
        .redirects(3)
        .build();
    let response = agent
        .get(trimmed)
        .call()
        .map_err(|error| format!("failed to fetch ingress JWKS URL {trimmed}: {error}"))?;
    if response.status() != 200 {
        return Err(format!(
            "ingress JWKS URL {trimmed} returned HTTP {}",
            response.status()
        ));
    }

    let mut raw = String::new();
    response
        .into_reader()
        .take(MAX_JWKS_BYTES as u64 + 1)
        .read_to_string(&mut raw)
        .map_err(|error| format!("failed to read ingress JWKS URL body {trimmed}: {error}"))?;
    if raw.len() > MAX_JWKS_BYTES {
        return Err(format!(
            "ingress JWKS URL {trimmed} response exceeds {MAX_JWKS_BYTES} bytes"
        ));
    }

    parse_jwks_json(&raw)
        .map_err(|error| format!("failed to parse ingress JWKS URL {trimmed}: {error}"))
}

fn parse_jwks_json(raw: &str) -> Result<HashMap<String, (Algorithm, DecodingKey)>, String> {
    let jwks: JwkSet =
        serde_json::from_str(raw).map_err(|error| format!("invalid JWKS JSON: {error}"))?;
    let mut keys = HashMap::new();
    for jwk in jwks.keys {
        let Some(kid) = jwk.common.key_id.clone() else {
            warn!("skipping ingress JWKS entry without kid");
            continue;
        };
        let algorithm = jwk_algorithm(&jwk)?;
        let decoding_key = DecodingKey::from_jwk(&jwk)
            .map_err(|error| format!("unsupported ingress JWKS key {kid}: {error}"))?;
        keys.insert(kid, (algorithm, decoding_key));
    }
    if keys.is_empty() {
        return Err("ingress JWKS document contains no usable keys".to_string());
    }
    Ok(keys)
}

/// Validate an ingress JWT using a startup-loaded validator.
pub fn validate_ingress_jwt(
    validator: &IngressJwtValidator,
    token: &str,
) -> Result<VerifiedIngressIdentity, StatusCode> {
    validator.validate(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use std::io::Write;

    fn hs256_config(secret: &str) -> ServerConfig {
        ServerConfig {
            ingress_auth_mode: "jwt".to_string(),
            ingress_jwt_secret: Some(secret.to_string()),
            ingress_jwt_issuer: Some("sdkwork-kernel".to_string()),
            ingress_jwt_audience: Some("internal-api".to_string()),
            ..ServerConfig::default()
        }
    }

    #[test]
    fn validates_hs256_tenant_and_user_claims() {
        let config = hs256_config("test-secret");
        let validator = IngressJwtValidator::from_config(&config).expect("validator");
        let exp = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize;
        let claims = serde_json::json!({
            "sub": "1",
            "tenant_id": "100001",
            "user_id": "1",
            "exp": exp,
            "iss": "sdkwork-kernel",
            "aud": "internal-api",
        });
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(b"test-secret"),
        )
        .expect("jwt encode");

        let identity = validator.validate(&token).expect("jwt should validate");
        assert_eq!(
            identity,
            VerifiedIngressIdentity {
                tenant_id: "100001".to_string(),
                user_id: "1".to_string(),
            }
        );
    }

    #[test]
    fn rejects_token_with_future_nbf() {
        let config = hs256_config("test-secret");
        let validator = IngressJwtValidator::from_config(&config).expect("validator");
        let nbf = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize;
        let exp = (chrono::Utc::now() + chrono::Duration::hours(2)).timestamp() as usize;
        let claims = serde_json::json!({
            "sub": "1",
            "tenant_id": "100001",
            "user_id": "1",
            "nbf": nbf,
            "exp": exp,
            "iss": "sdkwork-kernel",
            "aud": "internal-api",
        });
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(b"test-secret"),
        )
        .expect("jwt encode");

        assert_eq!(validator.validate(&token), Err(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn rejects_missing_tenant_claim() {
        let config = hs256_config("test-secret");
        let validator = IngressJwtValidator::from_config(&config).expect("validator");
        let exp = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize;
        let claims = serde_json::json!({
            "sub": "1",
            "exp": exp,
            "iss": "sdkwork-kernel",
            "aud": "internal-api",
        });
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(b"test-secret"),
        )
        .expect("jwt encode");
        assert_eq!(validator.validate(&token), Err(StatusCode::FORBIDDEN));
    }

    #[test]
    fn loads_rs256_pem_public_key() {
        let pem = include_str!("../tests/fixtures/ingress_jwt_rs256_public.pem");
        let config = ServerConfig {
            ingress_auth_mode: "jwt".to_string(),
            ingress_jwt_algorithm: "rs256".to_string(),
            ingress_jwt_rsa_public_key_pem: Some(pem.to_string()),
            ingress_jwt_issuer: Some("sdkwork-kernel".to_string()),
            ingress_jwt_audience: Some("internal-api".to_string()),
            ..ServerConfig::default()
        };
        let validator = IngressJwtValidator::from_config(&config).expect("validator");
        let private_pem = include_str!("../tests/fixtures/ingress_jwt_rs256_private.pem");
        let exp = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize;
        let claims = serde_json::json!({
            "sub": "1",
            "tenant_id": "100001",
            "user_id": "1",
            "exp": exp,
            "iss": "sdkwork-kernel",
            "aud": "internal-api",
        });
        let token = encode(
            &Header::new(Algorithm::RS256),
            &claims,
            &EncodingKey::from_rsa_pem(private_pem.as_bytes()).expect("private key"),
        )
        .expect("jwt encode");
        let identity = validator
            .validate(&token)
            .expect("rs256 jwt should validate");
        assert_eq!(identity.tenant_id, "100001");
    }

    #[test]
    fn loads_jwks_file_by_kid() {
        let mut file = tempfile::NamedTempFile::new().expect("temp jwks");
        write!(
            file,
            "{}",
            include_str!("../tests/fixtures/ingress_jwt_jwks.json")
        )
        .expect("write jwks");
        let config = ServerConfig {
            ingress_auth_mode: "jwt".to_string(),
            ingress_jwt_jwks_file: Some(file.path().to_string_lossy().into_owned()),
            ingress_jwt_issuer: Some("sdkwork-kernel".to_string()),
            ingress_jwt_audience: Some("internal-api".to_string()),
            ..ServerConfig::default()
        };
        let validator = IngressJwtValidator::from_config(&config).expect("validator");
        let private_pem = include_str!("../tests/fixtures/ingress_jwt_rs256_private.pem");
        let exp = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize;
        let claims = serde_json::json!({
            "sub": "1",
            "tenant_id": "100001",
            "user_id": "1",
            "exp": exp,
            "iss": "sdkwork-kernel",
            "aud": "internal-api",
        });
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("ingress-test-key".to_string());
        let token = encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(private_pem.as_bytes()).expect("private key"),
        )
        .expect("jwt encode");
        let identity = validator
            .validate(&token)
            .expect("jwks jwt should validate");
        assert_eq!(identity.tenant_id, "100001");
    }

    #[test]
    fn fetches_jwks_url_at_startup() {
        use std::io::Write;
        use std::net::TcpListener;
        use std::sync::mpsc;
        use std::thread;

        // Acquire the env lock and clear production profile env vars to
        // prevent parallel test interference. Other tests set
        // `SDKWORK_KERNEL_PROFILE_ID` to a `.production` profile, which would
        // make `is_production_kernel_profile()` return true and reject the
        // HTTP localhost JWKS URL used by this test.
        let _lock = crate::testing::env::lock();
        let _profile = crate::testing::env::VarGuard::set("SDKWORK_KERNEL_PROFILE_ID", None);
        let _environment = crate::testing::env::VarGuard::set("SDKWORK_KERNEL_ENVIRONMENT", None);

        let jwks_body = include_str!("../tests/fixtures/ingress_jwt_jwks.json").to_string();
        let (ready_tx, ready_rx) = mpsc::channel();
        thread::spawn(move || {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind test jwks listener");
            ready_tx
                .send(listener.local_addr().expect("listener addr"))
                .expect("ready signal");
            for _ in 0..3 {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut request_buf = [0u8; 2048];
                    let _ = stream.read(&mut request_buf);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        jwks_body.len(),
                        jwks_body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();
                }
            }
        });

        let addr = ready_rx
            .recv()
            .expect("jwks test server should be listening");
        let config = ServerConfig {
            ingress_auth_mode: "jwt".to_string(),
            ingress_jwt_jwks_url: Some(format!("http://{addr}")),
            ingress_jwt_issuer: Some("sdkwork-kernel".to_string()),
            ingress_jwt_audience: Some("internal-api".to_string()),
            ..ServerConfig::default()
        };
        let validator = IngressJwtValidator::from_config(&config).expect("validator");
        let private_pem = include_str!("../tests/fixtures/ingress_jwt_rs256_private.pem");
        let exp = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize;
        let claims = serde_json::json!({
            "sub": "1",
            "tenant_id": "100001",
            "user_id": "1",
            "exp": exp,
            "iss": "sdkwork-kernel",
            "aud": "internal-api",
        });
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("ingress-test-key".to_string());
        let token = encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(private_pem.as_bytes()).expect("private key"),
        )
        .expect("jwt encode");
        let identity = validator
            .validate(&token)
            .expect("jwks url jwt should validate");
        assert_eq!(identity.tenant_id, "100001");
    }

    #[test]
    fn rejects_http_jwks_url_in_production() {
        let config = ServerConfig {
            environment: "production".to_string(),
            ingress_auth_mode: "jwt".to_string(),
            ingress_jwt_jwks_url: Some("http://idp.example.com/jwks".to_string()),
            ..ServerConfig::default()
        };
        let result = IngressJwtValidator::from_config(&config);
        assert!(result.is_err(), "production JWKS URL must require https://");
    }

    #[test]
    fn refreshes_jwks_file_on_unknown_kid() {
        let mut file = tempfile::NamedTempFile::new().expect("temp jwks");
        let stale_jwks = include_str!("../tests/fixtures/ingress_jwt_jwks.json")
            .replace("ingress-test-key", "stale-kid");
        write!(file, "{stale_jwks}").expect("write stale jwks");
        let config = ServerConfig {
            ingress_auth_mode: "jwt".to_string(),
            ingress_jwt_jwks_file: Some(file.path().to_string_lossy().into_owned()),
            ingress_jwt_issuer: Some("sdkwork-kernel".to_string()),
            ingress_jwt_audience: Some("internal-api".to_string()),
            ..ServerConfig::default()
        };
        let validator = IngressJwtValidator::from_config(&config).expect("validator");
        let path = file.path().to_string_lossy().into_owned();
        fs::write(
            &path,
            include_str!("../tests/fixtures/ingress_jwt_jwks.json"),
        )
        .expect("write rotated jwks");

        let private_pem = include_str!("../tests/fixtures/ingress_jwt_rs256_private.pem");
        let exp = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize;
        let claims = serde_json::json!({
            "sub": "1",
            "tenant_id": "100001",
            "user_id": "1",
            "exp": exp,
            "iss": "sdkwork-kernel",
            "aud": "internal-api",
        });
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("ingress-test-key".to_string());
        let token = encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(private_pem.as_bytes()).expect("private key"),
        )
        .expect("jwt encode");
        let identity = validator
            .validate(&token)
            .expect("jwks refresh should load rotated kid");
        assert_eq!(identity.tenant_id, "100001");
    }
}
