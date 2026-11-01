use std::collections::BTreeMap;
use std::time::Duration;

use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method, Url, redirect};
use serde_json::{Map, Value, json};
use tokio::net::lookup_host;

use crate::registry::{NetworkPolicy, is_private_address};

use super::ExecutionError;

pub struct HttpRequestOptions<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: &'a BTreeMap<String, String>,
    pub body: Option<&'a Value>,
    pub timeout: Duration,
    pub policy: &'a NetworkPolicy,
    pub expected_status: &'a [u16],
    pub max_response_bytes: usize,
}

pub async fn request(options: HttpRequestOptions<'_>) -> Result<Value, ExecutionError> {
    let method = Method::from_bytes(options.method.as_bytes()).map_err(|_| {
        ExecutionError::new(
            "HTTP_METHOD",
            format!("invalid HTTP method `{}`", options.method),
        )
    })?;
    let url = Url::parse(options.url)
        .map_err(|error| ExecutionError::new("HTTP_URL", format!("invalid URL: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ExecutionError::new(
            "HTTP_URL",
            "only http:// and https:// URLs are supported",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ExecutionError::new(
            "HTTP_URL",
            "credentials in URLs are not allowed",
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| ExecutionError::new("HTTP_URL", "URL has no host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| ExecutionError::new("HTTP_URL", "URL has no port"))?;
    if !options.policy.allows(host, port) {
        return Err(ExecutionError::new(
            "PERMISSION_NETWORK",
            format!("network access to `{host}:{port}` is not permitted"),
        ));
    }

    let addresses: Vec<_> = lookup_host((host, port))
        .await
        .map_err(|error| {
            ExecutionError::new("NETWORK", format!("cannot resolve `{host}`: {error}"))
                .retryable(true)
        })?
        .collect();
    if addresses.is_empty() {
        return Err(
            ExecutionError::new("NETWORK", format!("`{host}` resolved to no addresses"))
                .retryable(true),
        );
    }
    if !options.policy.allows_private_resolution(host, port)
        && addresses
            .iter()
            .any(|address| is_private_address(address.ip()))
    {
        return Err(ExecutionError::new(
            "SSRF_PRIVATE",
            format!("host `{host}` resolves to a private or loopback address"),
        ));
    }

    let mut headers = HeaderMap::new();
    for (key, value) in options.headers {
        if key.eq_ignore_ascii_case("host")
            || key.eq_ignore_ascii_case("connection")
            || key.eq_ignore_ascii_case("content-length")
        {
            continue;
        }
        let name = HeaderName::from_bytes(key.as_bytes()).map_err(|_| {
            ExecutionError::new("HTTP_HEADERS", format!("invalid header name `{key}`"))
        })?;
        let value = HeaderValue::from_str(value).map_err(|_| {
            ExecutionError::new("HTTP_HEADERS", format!("invalid header value for `{key}`"))
        })?;
        headers.insert(name, value);
    }
    let body_bytes = match options.body {
        None => None,
        Some(Value::String(text)) => Some(text.clone().into_bytes()),
        Some(value) => Some(serde_json::to_vec(value).map_err(|error| {
            ExecutionError::new("HTTP_BODY", format!("cannot encode request body: {error}"))
        })?),
    };
    if body_bytes.is_some() && !headers.contains_key(CONTENT_TYPE) {
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    }

    let client = Client::builder()
        .redirect(redirect::Policy::none())
        .no_proxy()
        .timeout(options.timeout)
        .build()
        .map_err(|error| {
            ExecutionError::new("HTTP_CLIENT", format!("cannot create HTTP client: {error}"))
        })?;
    let mut request = client.request(method, url).headers(headers);
    if let Some(bytes) = body_bytes {
        request = request.body(bytes);
    }
    let mut response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            ExecutionError::new("TIMEOUT", format!("request timed out: {error}")).retryable(true)
        } else {
            ExecutionError::new("NETWORK", format!("request failed: {error}")).retryable(true)
        }
    })?;

    let status = response.status().as_u16();
    let mut response_headers = Map::new();
    for (name, value) in response.headers() {
        let key = name.as_str().to_string();
        let value = value.to_str().unwrap_or_default().to_string();
        match response_headers.get_mut(&key) {
            Some(Value::String(existing)) => {
                existing.push_str(", ");
                existing.push_str(&value);
            }
            _ => {
                response_headers.insert(key, Value::String(value));
            }
        }
    }

    let max = options.max_response_bytes.max(1);
    if response
        .content_length()
        .is_some_and(|length| length > max as u64)
    {
        return Err(ExecutionError::new(
            "HTTP_LIMIT",
            format!("response exceeds {max} bytes"),
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        if error.is_timeout() {
            ExecutionError::new("TIMEOUT", format!("response stalled: {error}")).retryable(true)
        } else {
            ExecutionError::new("NETWORK", format!("cannot read response: {error}")).retryable(true)
        }
    })? {
        if body.len().saturating_add(chunk.len()) > max {
            return Err(ExecutionError::new(
                "HTTP_LIMIT",
                format!("response exceeds {max} bytes"),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    let body_text = String::from_utf8_lossy(&body).to_string();
    let is_json = response_headers
        .get("content-type")
        .and_then(Value::as_str)
        .map(|value| value.to_ascii_lowercase().contains("json"))
        .unwrap_or(false);
    let body_value = if is_json {
        serde_json::from_str(&body_text).unwrap_or(Value::String(body_text.clone()))
    } else {
        let trimmed = body_text.trim_start();
        match serde_json::from_str(&body_text) {
            Ok(value) if trimmed.starts_with('{') || trimmed.starts_with('[') => value,
            _ => Value::String(body_text.clone()),
        }
    };

    let expected = if options.expected_status.is_empty() {
        (200..300).contains(&status)
    } else {
        options.expected_status.contains(&status)
    };
    if !expected {
        let code = if (300..400).contains(&status) {
            "HTTP_REDIRECT"
        } else if status >= 500 {
            "HTTP_5XX"
        } else if status >= 400 {
            "HTTP_4XX"
        } else {
            "HTTP_STATUS"
        };
        return Err(
            ExecutionError::new(code, format!("HTTP request returned status {status}"))
                .retryable((500..600).contains(&status))
                .with_details(json!({
                    "status": status,
                    "headers": response_headers,
                    "body": body_value,
                })),
        );
    }

    Ok(json!({
        "status": status,
        "headers": response_headers,
        "body": body_value,
    }))
}

pub fn encode_query(params: &Map<String, Value>) -> String {
    let mut parts = Vec::new();
    for (key, value) in params {
        let value = match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        parts.push(format!(
            "{}={}",
            percent_encode(key),
            percent_encode(&value)
        ));
    }
    parts.join("&")
}

fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
