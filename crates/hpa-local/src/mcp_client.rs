use std::collections::BTreeMap;

use reqwest::Url;
use serde_json::{Value, json};

use crate::db::McpServerRow;
use crate::mcp::{Era, LEGACY_VERSION, MODERN_VERSION, McpClient};

#[derive(Debug, Clone)]
pub struct ExternalServer {
    pub id: String,
    pub name: String,
    pub transport: String,
    pub command: Vec<String>,
    pub url: Option<String>,
    pub allowed_tools: Vec<String>,
    pub network: Vec<String>,
    pub credential_env: Option<String>,
}

impl ExternalServer {
    pub fn from_row(row: &McpServerRow) -> Result<Self, String> {
        let command = serde_json::from_str::<Vec<String>>(&row.command_json)
            .map_err(|error| format!("invalid command config: {error}"))?;
        let allowed_tools =
            serde_json::from_str::<Vec<String>>(&row.allowed_tools_json).unwrap_or_default();
        let network = serde_json::from_str::<Vec<String>>(&row.network_json).unwrap_or_default();
        Ok(Self {
            id: row.id.clone(),
            name: row.name.clone(),
            transport: row.transport.clone(),
            command,
            url: row.url.clone(),
            allowed_tools,
            network,
            credential_env: row.credential_env.clone(),
        })
    }

    fn allows_tool(&self, tool: &str) -> bool {
        self.allowed_tools.is_empty() || self.allowed_tools.iter().any(|name| name == tool)
    }
}

pub async fn probe(server: &ExternalServer) -> Result<Value, String> {
    match server.transport.as_str() {
        "stdio" => probe_stdio(server).await,
        "http" => probe_http(server).await,
        other => Err(format!("unsupported transport `{other}`")),
    }
}

pub async fn call_tool(
    server: &ExternalServer,
    tool: &str,
    arguments: Value,
) -> Result<Value, String> {
    if !server.allows_tool(tool) {
        return Err(format!("tool `{tool}` is not allowed for this server"));
    }
    match server.transport.as_str() {
        "stdio" => {
            let mut client = stdio_client(server)?;
            client.initialize()?;
            client.call_tool(tool, arguments)
        }
        "http" => {
            let mut client = HttpExternal::connect(server).await?;
            client.call_tool(tool, arguments).await
        }
        other => Err(format!("unsupported transport `{other}`")),
    }
}

async fn probe_stdio(server: &ExternalServer) -> Result<Value, String> {
    let mut client = stdio_client(server)?;
    let initialized = client.initialize()?;
    let tools = client.list_tools()?;
    let resources = client
        .list_resources()
        .unwrap_or_else(|_| json!({"resources": []}));
    let prompts = client
        .list_prompts()
        .unwrap_or_else(|_| json!({"prompts": []}));
    Ok(probe_result(
        server,
        "legacy",
        initialized,
        tools,
        resources,
        prompts,
    ))
}

async fn probe_http(server: &ExternalServer) -> Result<Value, String> {
    let mut client = HttpExternal::connect(server).await?;
    let initialized = client.initialized.clone();
    let tools = client.list_tools().await?;
    let resources = client
        .list_resources()
        .await
        .unwrap_or_else(|_| json!({"resources": []}));
    let prompts = client
        .list_prompts()
        .await
        .unwrap_or_else(|_| json!({"prompts": []}));
    Ok(probe_result(
        server,
        if client.era == Era::Modern {
            "modern"
        } else {
            "legacy"
        },
        initialized,
        tools,
        resources,
        prompts,
    ))
}

fn probe_result(
    server: &ExternalServer,
    era: &str,
    initialized: Value,
    tools: Value,
    resources: Value,
    prompts: Value,
) -> Value {
    let tools = tools
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter(|tool| {
                    tool.get("name")
                        .and_then(Value::as_str)
                        .is_none_or(|name| server.allows_tool(name))
                })
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "serverId": server.id,
        "name": server.name,
        "transport": server.transport,
        "era": era,
        "serverInfo": initialized.get("serverInfo").cloned().unwrap_or(Value::Null),
        "tools": tools,
        "resources": resources.get("resources").cloned().unwrap_or_else(|| json!([])),
        "prompts": prompts.get("prompts").cloned().unwrap_or_else(|| json!([])),
    })
}

fn stdio_client(server: &ExternalServer) -> Result<McpClient, String> {
    let mut env = BTreeMap::new();
    if let Some(name) = &server.credential_env {
        let value = std::env::var(name)
            .map_err(|_| format!("credential environment variable `{name}` is not set"))?;
        env.insert(name.clone(), value);
    }
    McpClient::spawn_with_env(&server.command, &env)
}

struct HttpExternal {
    client: reqwest::Client,
    url: Url,
    token: Option<String>,
    era: Era,
    session: Option<String>,
    next_id: u64,
    initialized: Value,
}

impl HttpExternal {
    async fn connect(server: &ExternalServer) -> Result<Self, String> {
        let raw = server
            .url
            .as_deref()
            .ok_or_else(|| "HTTP MCP server requires a url".to_string())?;
        let url = validate_url(raw, &server.network).await?;
        let token = match &server.credential_env {
            Some(name) => Some(
                std::env::var(name)
                    .map_err(|_| format!("credential environment variable `{name}` is not set"))?,
            ),
            None => None,
        };
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|error| error.to_string())?;
        let mut external = Self {
            client,
            url,
            token,
            era: Era::Modern,
            session: None,
            next_id: 1,
            initialized: Value::Null,
        };
        match external.discover().await {
            Ok(result) => {
                external.initialized = result;
                external.era = Era::Modern;
                Ok(external)
            }
            Err(error) if error.starts_with("UNAUTHORIZED") => Err(error),
            Err(_) => {
                external.era = Era::Legacy;
                let initialized = external.initialize_legacy().await?;
                external.initialized = initialized;
                Ok(external)
            }
        }
    }

    async fn discover(&mut self) -> Result<Value, String> {
        let params = json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": MODERN_VERSION,
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {"name": "hpa-mcp-client", "version": env!("CARGO_PKG_VERSION")},
            }
        });
        let response = self
            .post("server/discover", &params, Some("server/discover"), None)
            .await?;
        if response.status == 401 || response.status == 403 {
            return Err(format!("UNAUTHORIZED: HTTP {}", response.status));
        }
        if !response.status.is_success() {
            return Err(format!("discover failed with HTTP {}", response.status));
        }
        if response.body.get("result").is_none() {
            return Err("discover returned no result".to_string());
        }
        Ok(response.body["result"].clone())
    }

    async fn initialize_legacy(&mut self) -> Result<Value, String> {
        let params = json!({
            "protocolVersion": LEGACY_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "hpa-mcp-client", "version": env!("CARGO_PKG_VERSION")},
        });
        let response = self.post_legacy("initialize", &params).await?;
        if response.status == 401 || response.status == 403 {
            return Err(format!("UNAUTHORIZED: HTTP {}", response.status));
        }
        if !response.status.is_success() {
            return Err(format!("initialize failed with HTTP {}", response.status));
        }
        if let Some(session) = response.session {
            self.session = Some(session);
        }
        let initialized = response
            .body
            .get("result")
            .cloned()
            .ok_or_else(|| "initialize returned no result".to_string())?;
        let _ = self
            .notify_legacy("notifications/initialized", json!({}))
            .await;
        Ok(initialized)
    }

    async fn list_tools(&mut self) -> Result<Value, String> {
        self.request("tools/list", json!({}), None).await
    }

    async fn list_resources(&mut self) -> Result<Value, String> {
        self.request("resources/list", json!({}), None).await
    }

    async fn list_prompts(&mut self) -> Result<Value, String> {
        self.request("prompts/list", json!({}), None).await
    }

    async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, String> {
        self.request(
            "tools/call",
            json!({"name": name, "arguments": arguments}),
            Some(name),
        )
        .await
    }

    async fn request(
        &mut self,
        method: &str,
        mut params: Value,
        name: Option<&str>,
    ) -> Result<Value, String> {
        let response = if self.era == Era::Modern {
            if !params.is_object() {
                params = json!({});
            }
            params["_meta"] = json!({
                "io.modelcontextprotocol/protocolVersion": MODERN_VERSION,
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {"name": "hpa-mcp-client", "version": env!("CARGO_PKG_VERSION")},
            });
            self.post(method, &params, Some(method), name).await?
        } else {
            self.post_legacy(method, &params).await?
        };
        if response.status == 401 || response.status == 403 {
            return Err(format!("UNAUTHORIZED: HTTP {}", response.status));
        }
        if !response.status.is_success() {
            return Err(format!("MCP request failed with HTTP {}", response.status));
        }
        if let Some(error) = response.body.get("error") {
            return Err(format!("MCP error: {error}"));
        }
        Ok(response.body.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn post(
        &mut self,
        method: &str,
        params: &Value,
        method_header: Option<&str>,
        name_header: Option<&str>,
    ) -> Result<HttpResponse, String> {
        let id = self.next_id;
        self.next_id += 1;
        let mut request = self
            .client
            .post(self.url.clone())
            .header("content-type", "application/json")
            .header("mcp-protocol-version", MODERN_VERSION);
        if let Some(token) = &self.token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        if let Some(method_header) = method_header {
            request = request.header("mcp-method", method_header);
        }
        if let Some(name_header) = name_header {
            request = request.header("mcp-name", name_header);
        }
        if let Some(session) = &self.session {
            request = request.header("mcp-session-id", session);
        }
        let response = request
            .json(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .send()
            .await
            .map_err(|error| format!("MCP HTTP request failed: {error}"))?;
        parse_response(response).await
    }

    async fn post_legacy(&mut self, method: &str, params: &Value) -> Result<HttpResponse, String> {
        let id = self.next_id;
        self.next_id += 1;
        let mut request = self
            .client
            .post(self.url.clone())
            .header("content-type", "application/json");
        if let Some(token) = &self.token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        if let Some(session) = &self.session {
            request = request.header("mcp-session-id", session);
        }
        let response = request
            .json(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .send()
            .await
            .map_err(|error| format!("MCP HTTP request failed: {error}"))?;
        parse_response(response).await
    }

    async fn notify_legacy(&mut self, method: &str, params: Value) -> Result<(), String> {
        let mut request = self
            .client
            .post(self.url.clone())
            .header("content-type", "application/json");
        if let Some(token) = &self.token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        if let Some(session) = &self.session {
            request = request.header("mcp-session-id", session);
        }
        let response = request
            .json(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "notification failed with HTTP {}",
                response.status()
            ));
        }
        Ok(())
    }
}

struct HttpResponse {
    status: reqwest::StatusCode,
    session: Option<String>,
    body: Value,
}

async fn parse_response(response: reqwest::Response) -> Result<HttpResponse, String> {
    let status = response.status();
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = response.json::<Value>().await.unwrap_or_else(|_| json!({}));
    Ok(HttpResponse {
        status,
        session,
        body,
    })
}

async fn validate_url(raw: &str, network: &[String]) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|error| format!("invalid MCP URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("MCP URL must use http or https".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("MCP URL must not contain userinfo".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "MCP URL has no host".to_string())?;
    let port = url.port_or_known_default().unwrap_or(80);
    if is_loopback_host(host) {
        return Ok(url);
    }
    if !network.iter().any(|rule| rule_matches(rule, host, port)) {
        return Err(format!(
            "MCP host `{host}:{port}` is not in the configured network allowlist"
        ));
    }
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| format!("cannot resolve MCP host `{host}`: {error}"))?;
    for address in addresses {
        let ip = address.ip();
        if ip.to_string() == "169.254.169.254" {
            return Err("link-local metadata addresses are blocked".to_string());
        }
    }
    Ok(url)
}

fn is_loopback_host(host: &str) -> bool {
    host == "localhost"
        || host == "::1"
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

fn rule_matches(rule: &str, host: &str, port: u16) -> bool {
    if rule == "*" {
        return true;
    }
    let (rule_host, rule_port) = match rule.rsplit_once(':') {
        Some((host, port)) if port.parse::<u16>().is_ok() => {
            (host, Some(port.parse::<u16>().unwrap_or(0)))
        }
        _ => (rule, None),
    };
    if let Some(rule_port) = rule_port
        && rule_port != port
    {
        return false;
    }
    if let Some(suffix) = rule_host.strip_prefix("*.") {
        host.ends_with(suffix) && host.len() > suffix.len()
    } else {
        rule_host == host
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_rules_are_exact_or_wildcard() {
        assert!(rule_matches("mcp.example.com", "mcp.example.com", 443));
        assert!(rule_matches("*.example.com", "mcp.example.com", 443));
        assert!(!rule_matches("*.example.com", "example.com", 443));
        assert!(rule_matches(
            "mcp.example.com:8443",
            "mcp.example.com",
            8443
        ));
        assert!(!rule_matches(
            "mcp.example.com:8443",
            "mcp.example.com",
            443
        ));
        assert!(rule_matches("*", "anything", 1));
    }

    #[tokio::test]
    async fn remote_hosts_require_allowlist() {
        let error = validate_url("http://mcp.example.com/mcp", &[])
            .await
            .expect_err("host must be blocked");
        assert!(error.contains("allowlist"), "{error}");
        let error = validate_url(
            "http://169.254.169.254/latest",
            &["169.254.169.254".to_string()],
        )
        .await
        .expect_err("metadata address must be blocked");
        assert!(error.contains("metadata"), "{error}");
        assert!(validate_url("http://127.0.0.1:8787/mcp", &[]).await.is_ok());
    }
}
