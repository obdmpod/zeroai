use crate::providers::traits::Provider;
use async_trait::async_trait;
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// ── AWS Credentials ────────────────────────────────────────

struct AwsCredentials {
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
    region: String,
}

impl AwsCredentials {
    /// Resolve credentials from environment variables.
    /// Returns `None` if required vars (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`) are missing.
    fn from_env() -> Option<Self> {
        Self::from_parts(
            std::env::var("AWS_ACCESS_KEY_ID").ok().as_deref(),
            std::env::var("AWS_SECRET_ACCESS_KEY").ok().as_deref(),
            std::env::var("AWS_SESSION_TOKEN").ok().as_deref(),
            std::env::var("AWS_REGION")
                .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
                .ok()
                .as_deref(),
        )
    }

    /// Build credentials from explicit values (testable without env vars).
    fn from_parts(
        access_key_id: Option<&str>,
        secret_access_key: Option<&str>,
        session_token: Option<&str>,
        region: Option<&str>,
    ) -> Option<Self> {
        let access_key_id = access_key_id.map(str::trim).filter(|s| !s.is_empty())?;
        let secret_access_key = secret_access_key.map(str::trim).filter(|s| !s.is_empty())?;

        let session_token = session_token
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);

        let region = region
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("us-east-1");

        Some(Self {
            access_key_id: access_key_id.to_string(),
            secret_access_key: secret_access_key.to_string(),
            session_token,
            region: region.to_string(),
        })
    }
}

// ── Converse API types ─────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConverseRequest {
    messages: Vec<ConverseMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Vec<SystemContent>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inference_config: Option<InferenceConfig>,
}

#[derive(Debug, Serialize)]
struct ConverseMessage {
    role: String,
    content: Vec<ContentBlock>,
}

#[derive(Debug, Serialize)]
struct ContentBlock {
    text: String,
}

#[derive(Debug, Serialize)]
struct SystemContent {
    text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InferenceConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ConverseResponse {
    output: ConverseOutput,
}

#[derive(Debug, Deserialize)]
struct ConverseOutput {
    message: ConverseOutputMessage,
}

#[derive(Debug, Deserialize)]
struct ConverseOutputMessage {
    content: Vec<ResponseContentBlock>,
}

#[derive(Debug, Deserialize)]
struct ResponseContentBlock {
    text: String,
}

// ── SigV4 signing helpers ──────────────────────────────────

const SERVICE: &str = "bedrock";

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

type HmacSha256 = Hmac<Sha256>;

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac =
        HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn signing_key(secret: &str, date_stamp: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date_stamp.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

/// Encode a model ID for use in the URL path.
/// Bedrock model IDs may contain `:` which must be percent-encoded.
fn encode_model_id(model_id: &str) -> String {
    model_id.replace(':', "%3A")
}

/// Build the SigV4 `Authorization` header value.
///
/// Returns `(authorization_header, amz_date)`.
fn sign_request(
    creds: &AwsCredentials,
    method: &str,
    url: &reqwest::Url,
    body: &[u8],
    timestamp: &chrono::DateTime<chrono::Utc>,
) -> (String, String) {
    let amz_date = timestamp.format("%Y%m%dT%H%M%SZ").to_string();
    let date_stamp = timestamp.format("%Y%m%d").to_string();

    let host = url.host_str().unwrap_or_default();
    let canonical_uri = url.path();
    let canonical_querystring = url.query().unwrap_or("");

    // Build signed headers list (must be sorted)
    let mut signed_header_names: Vec<&str> = vec!["content-type", "host", "x-amz-date"];
    if creds.session_token.is_some() {
        signed_header_names.push("x-amz-security-token");
    }
    signed_header_names.sort();
    let signed_headers = signed_header_names.join(";");

    // Build canonical headers (must be sorted by name)
    let mut canonical_header_parts: Vec<String> = vec![
        format!("content-type:application/json"),
        format!("host:{host}"),
        format!("x-amz-date:{amz_date}"),
    ];
    if let Some(ref token) = creds.session_token {
        canonical_header_parts.push(format!("x-amz-security-token:{token}"));
    }
    canonical_header_parts.sort();
    let canonical_headers = canonical_header_parts
        .iter()
        .map(|h| format!("{h}\n"))
        .collect::<String>();

    let payload_hash = sha256_hex(body);

    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{canonical_querystring}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );

    let credential_scope = format!("{date_stamp}/{}/{SERVICE}/aws4_request", creds.region);

    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );

    let key = signing_key(&creds.secret_access_key, &date_stamp, &creds.region, SERVICE);
    let signature = hex::encode(hmac_sha256(&key, string_to_sign.as_bytes()));

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
        creds.access_key_id
    );

    (authorization, amz_date)
}

// ── BedrockProvider ────────────────────────────────────────

pub struct BedrockProvider {
    credentials: Option<AwsCredentials>,
    client: Client,
}

impl BedrockProvider {
    pub fn new() -> Self {
        Self {
            credentials: AwsCredentials::from_env(),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn endpoint(region: &str, model_id: &str) -> String {
        let encoded = encode_model_id(model_id);
        format!(
            "https://bedrock-runtime.{region}.amazonaws.com/model/{encoded}/converse"
        )
    }
}

#[async_trait]
impl Provider for BedrockProvider {
    async fn chat_with_system(
        &self,
        system_prompt: Option<&str>,
        message: &str,
        model: &str,
        temperature: f64,
    ) -> anyhow::Result<String> {
        let creds = self.credentials.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "AWS credentials not set. Set AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY environment variables."
            )
        })?;

        let request_body = ConverseRequest {
            messages: vec![ConverseMessage {
                role: "user".to_string(),
                content: vec![ContentBlock {
                    text: message.to_string(),
                }],
            }],
            system: system_prompt.map(|s| {
                vec![SystemContent {
                    text: s.to_string(),
                }]
            }),
            inference_config: Some(InferenceConfig {
                max_tokens: Some(4096),
                temperature: Some(temperature),
            }),
        };

        let body = serde_json::to_vec(&request_body)?;
        let url_str = Self::endpoint(&creds.region, model);
        let url: reqwest::Url = url_str.parse()?;

        let now = chrono::Utc::now();
        let (authorization, amz_date) = sign_request(creds, "POST", &url, &body, &now);

        let mut req = self
            .client
            .post(url_str)
            .header("content-type", "application/json")
            .header("x-amz-date", &amz_date)
            .header("Authorization", &authorization);

        if let Some(ref token) = creds.session_token {
            req = req.header("x-amz-security-token", token);
        }

        let response = req.body(body).send().await?;

        if !response.status().is_success() {
            return Err(super::api_error("Bedrock", response).await);
        }

        let converse_response: ConverseResponse = response.json().await?;

        converse_response
            .output
            .message
            .content
            .into_iter()
            .next()
            .map(|c| c.text)
            .ok_or_else(|| anyhow::anyhow!("No response from Bedrock"))
    }

    async fn warmup(&self) -> anyhow::Result<()> {
        if let Some(ref creds) = self.credentials {
            let url = format!(
                "https://bedrock-runtime.{}.amazonaws.com/",
                creds.region
            );
            let _ = self.client.head(&url).send().await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Credential resolution (via from_parts to avoid env var races) ──

    #[test]
    fn credentials_all_present() {
        let creds = AwsCredentials::from_parts(
            Some("AKIAIOSFODNN7EXAMPLE"),
            Some("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"),
            Some("FwoGZXIvYXdzEBYaDHqa0AP"),
            Some("us-west-2"),
        )
        .expect("should resolve credentials");
        assert_eq!(creds.access_key_id, "AKIAIOSFODNN7EXAMPLE");
        assert_eq!(creds.secret_access_key, "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY");
        assert_eq!(creds.session_token.as_deref(), Some("FwoGZXIvYXdzEBYaDHqa0AP"));
        assert_eq!(creds.region, "us-west-2");
    }

    #[test]
    fn credentials_missing_access_key() {
        assert!(AwsCredentials::from_parts(None, Some("secret"), None, None).is_none());
    }

    #[test]
    fn credentials_missing_secret_key() {
        assert!(AwsCredentials::from_parts(Some("AKIA..."), None, None, None).is_none());
    }

    #[test]
    fn credentials_empty_access_key() {
        assert!(AwsCredentials::from_parts(Some(""), Some("secret"), None, None).is_none());
    }

    #[test]
    fn credentials_empty_secret_key() {
        assert!(AwsCredentials::from_parts(Some("AKIA"), Some("  "), None, None).is_none());
    }

    #[test]
    fn credentials_default_region() {
        let creds = AwsCredentials::from_parts(
            Some("AKIAIOSFODNN7EXAMPLE"),
            Some("secret"),
            None,
            None,
        )
        .expect("should resolve");
        assert_eq!(creds.region, "us-east-1");
    }

    #[test]
    fn credentials_explicit_region() {
        let creds = AwsCredentials::from_parts(
            Some("AKIAIOSFODNN7EXAMPLE"),
            Some("secret"),
            None,
            Some("eu-west-1"),
        )
        .expect("should resolve");
        assert_eq!(creds.region, "eu-west-1");
    }

    #[test]
    fn credentials_session_token_optional() {
        let creds = AwsCredentials::from_parts(
            Some("AKIAIOSFODNN7EXAMPLE"),
            Some("secret"),
            None,
            Some("us-east-1"),
        )
        .expect("should resolve");
        assert!(creds.session_token.is_none());
    }

    #[test]
    fn credentials_trims_whitespace() {
        let creds = AwsCredentials::from_parts(
            Some("  AKIA  "),
            Some("  secret  "),
            Some("  token  "),
            Some("  us-west-2  "),
        )
        .expect("should resolve");
        assert_eq!(creds.access_key_id, "AKIA");
        assert_eq!(creds.secret_access_key, "secret");
        assert_eq!(creds.session_token.as_deref(), Some("token"));
        assert_eq!(creds.region, "us-west-2");
    }

    // ── Model ID encoding ──────────────────────────────────

    #[test]
    fn model_id_encoding_colon() {
        assert_eq!(
            encode_model_id("anthropic.claude-3-5-sonnet-20241022-v2:0"),
            "anthropic.claude-3-5-sonnet-20241022-v2%3A0"
        );
    }

    #[test]
    fn model_id_encoding_no_colon() {
        assert_eq!(
            encode_model_id("meta.llama3-1-70b-instruct-v1"),
            "meta.llama3-1-70b-instruct-v1"
        );
    }

    // ── Endpoint construction ──────────────────────────────

    #[test]
    fn endpoint_url_construction() {
        let url = BedrockProvider::endpoint("us-east-1", "anthropic.claude-3-5-sonnet-20241022-v2:0");
        assert_eq!(
            url,
            "https://bedrock-runtime.us-east-1.amazonaws.com/model/anthropic.claude-3-5-sonnet-20241022-v2%3A0/converse"
        );
    }

    // ── Converse request serialization ─────────────────────

    #[test]
    fn converse_request_without_system() {
        let req = ConverseRequest {
            messages: vec![ConverseMessage {
                role: "user".to_string(),
                content: vec![ContentBlock {
                    text: "hello".to_string(),
                }],
            }],
            system: None,
            inference_config: Some(InferenceConfig {
                max_tokens: Some(4096),
                temperature: Some(0.7),
            }),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("\"system\""), "system should be omitted when None");
        assert!(json.contains("hello"));
        assert!(json.contains("inferenceConfig"));
    }

    #[test]
    fn converse_request_with_system() {
        let req = ConverseRequest {
            messages: vec![ConverseMessage {
                role: "user".to_string(),
                content: vec![ContentBlock {
                    text: "hello".to_string(),
                }],
            }],
            system: Some(vec![SystemContent {
                text: "You are a helpful assistant".to_string(),
            }]),
            inference_config: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("You are a helpful assistant"));
        assert!(!json.contains("inferenceConfig"));
    }

    // ── Converse response deserialization ───────────────────

    #[test]
    fn converse_response_deserializes() {
        let json = r#"{
            "output": {
                "message": {
                    "role": "assistant",
                    "content": [{"text": "Hello there!"}]
                }
            },
            "stopReason": "end_turn",
            "usage": {"inputTokens": 10, "outputTokens": 5}
        }"#;
        let resp: ConverseResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.output.message.content.len(), 1);
        assert_eq!(resp.output.message.content[0].text, "Hello there!");
    }

    #[test]
    fn converse_response_multiple_blocks() {
        let json = r#"{
            "output": {
                "message": {
                    "role": "assistant",
                    "content": [{"text": "First"}, {"text": "Second"}]
                }
            }
        }"#;
        let resp: ConverseResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.output.message.content.len(), 2);
        assert_eq!(resp.output.message.content[0].text, "First");
        assert_eq!(resp.output.message.content[1].text, "Second");
    }

    // ── SigV4 signing ──────────────────────────────────────

    #[test]
    fn sigv4_signing_produces_valid_header() {
        let creds = AwsCredentials {
            access_key_id: "AKIDEXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: None,
            region: "us-east-1".to_string(),
        };

        let url: reqwest::Url = "https://bedrock-runtime.us-east-1.amazonaws.com/model/test/converse"
            .parse()
            .unwrap();
        let body = b"{}";
        let timestamp = chrono::DateTime::parse_from_rfc3339("2024-01-15T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        let (auth, amz_date) = sign_request(&creds, "POST", &url, body, &timestamp);

        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240115/us-east-1/bedrock/aws4_request"));
        assert!(auth.contains("SignedHeaders=content-type;host;x-amz-date"));
        assert!(auth.contains("Signature="));
        assert_eq!(amz_date, "20240115T120000Z");
    }

    #[test]
    fn sigv4_signing_with_session_token() {
        let creds = AwsCredentials {
            access_key_id: "AKIDEXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: Some("AQoDYXdzEJr...".to_string()),
            region: "us-west-2".to_string(),
        };

        let url: reqwest::Url = "https://bedrock-runtime.us-west-2.amazonaws.com/model/test/converse"
            .parse()
            .unwrap();
        let body = b"{\"messages\":[]}";
        let timestamp = chrono::DateTime::parse_from_rfc3339("2024-06-01T08:30:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        let (auth, _) = sign_request(&creds, "POST", &url, body, &timestamp);

        assert!(auth.contains("SignedHeaders=content-type;host;x-amz-date;x-amz-security-token"));
        assert!(auth.contains("us-west-2/bedrock/aws4_request"));
    }

    #[test]
    fn sigv4_deterministic() {
        let creds = AwsCredentials {
            access_key_id: "AKID".to_string(),
            secret_access_key: "SECRET".to_string(),
            session_token: None,
            region: "us-east-1".to_string(),
        };

        let url: reqwest::Url = "https://bedrock-runtime.us-east-1.amazonaws.com/model/test/converse"
            .parse()
            .unwrap();
        let body = b"test";
        let timestamp = chrono::DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        let (auth1, _) = sign_request(&creds, "POST", &url, body, &timestamp);
        let (auth2, _) = sign_request(&creds, "POST", &url, body, &timestamp);
        assert_eq!(auth1, auth2, "Same inputs must produce same signature");
    }

    // ── Provider error path ──────────────────────────────

    #[test]
    fn chat_errors_when_credentials_none() {
        // Directly construct a provider with no credentials to avoid env var races.
        let p = BedrockProvider {
            credentials: None,
            client: Client::new(),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(p.chat_with_system(
            None,
            "hello",
            "anthropic.claude-3-5-sonnet-20241022-v2:0",
            0.7,
        ));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("AWS credentials not set"),
            "Expected credentials error, got: {err}"
        );
    }
}
