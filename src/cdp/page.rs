//! A single Chrome target (tab) driven over CDP. Ports frappe/utils/pdf_generator/page.py.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use base64::Engine;
use serde_json::{Map, Value, json};
use tokio::time::timeout;

use super::connection::CdpClient;

#[derive(Clone)]
pub struct Interceptor {
    pub host_url: String,
    pub bench_sites_path: Option<String>,
    pub site_public_path: Option<String>,
    pub sid: Option<String>,
}

pub struct Page {
    client: Arc<CdpClient>,
    pub session_id: String,
    pub target_id: String,
    pub frame_id: String,
    pub is_print_designer: bool,
    pub options: Map<String, Value>,
    interceptor: Interceptor,
}

impl Page {
    pub async fn new(
        client: Arc<CdpClient>,
        browser_context_id: &str,
        is_print_designer: bool,
        interceptor: Interceptor,
    ) -> Result<Self> {
        let created = client
            .send(
                "Target.createTarget",
                json!({ "url": "", "browserContextId": browser_context_id }),
                None,
            )
            .await?;
        let target_id = created["targetId"].as_str().ok_or_else(|| anyhow!("no targetId"))?.to_string();

        let attached = client
            .send(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
                None,
            )
            .await?;
        let session_id = attached["sessionId"].as_str().ok_or_else(|| anyhow!("no sessionId"))?.to_string();

        let mut page = Self {
            client,
            session_id,
            target_id,
            frame_id: String::new(),
            is_print_designer,
            options: Map::new(),
            interceptor,
        };

        page.send("Page.enable", json!({})).await?;
        page.load_frame_id().await?;
        page.set_media_emulation("print").await?;
        page.set_cookies().await?;
        Ok(page)
    }

    async fn send(&self, method: &str, params: Value) -> Result<Value> {
        self.client.send(method, params, Some(&self.session_id)).await
    }

    async fn load_frame_id(&mut self) -> Result<()> {
        let tree = self.send("Page.getFrameTree", json!({})).await?;
        self.frame_id = tree["frameTree"]["frame"]["id"]
            .as_str()
            .ok_or_else(|| anyhow!("no frameId"))?
            .to_string();
        Ok(())
    }

    async fn set_media_emulation(&self, media: &str) -> Result<()> {
        self.send("Emulation.setEmulatedMedia", json!({ "media": media })).await?;
        Ok(())
    }

    async fn set_cookies(&self) -> Result<()> {
        let Some(sid) = self.interceptor.sid.clone() else {
            return Ok(());
        };
        let domain = host_domain(&self.interceptor.host_url);
        self.send("Network.enable", json!({})).await?;
        self.send(
            "Network.setCookie",
            json!({ "name": "sid", "value": sid, "domain": domain, "sameSite": "Strict" }),
        )
        .await?;
        self.send("Network.disable", json!({})).await?;
        Ok(())
    }

    /// Navigate the tab to the host origin (fulfilled by the interceptor with an empty 200),
    /// giving the document a base URL before content is injected.
    pub async fn set_tab_url(&self) -> Result<()> {
        self.enable_interception().await?;
        let host = self.interceptor.host_url.clone();
        self.navigate(&host, &["load"]).await
    }

    async fn navigate(&self, url: &str, wait_for: &[&str]) -> Result<()> {
        let waiter = self.begin_lifecycle_wait(wait_for).await?;
        self.send("Page.navigate", json!({ "url": url })).await?;
        waiter.await?;
        Ok(())
    }

    /// Inject the full document via setDocumentContent and wait for it to load.
    pub async fn set_content(&self, html: &str) -> Result<()> {
        let waiter = self.begin_lifecycle_wait(&["load", "DOMContentLoaded"]).await?;
        self.send(
            "Page.setDocumentContent",
            json!({ "frameId": self.frame_id, "html": html }),
        )
        .await?;
        waiter.await?;
        Ok(())
    }

    /// Enable Fetch interception and spawn a handler for this session's paused requests.
    async fn enable_interception(&self) -> Result<()> {
        let client = self.client.clone();
        let session_id = self.session_id.clone();
        let cfg = self.interceptor.clone();
        let mut events = self.client.subscribe();

        tokio::spawn(async move {
            loop {
                let msg = match events.recv().await {
                    Ok(m) => m,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                };
                if msg["method"] != "Fetch.requestPaused" {
                    continue;
                }
                if msg.get("sessionId").and_then(|v| v.as_str()) != Some(session_id.as_str()) {
                    continue;
                }
                let params = &msg["params"];
                let Some(request_id) = params["requestId"].as_str() else { continue };
                let url = params["request"]["url"].as_str().unwrap_or("");
                handle_paused(&client, &session_id, request_id, url, &cfg).await;
            }
        });

        self.client
            .send(
                "Fetch.enable",
                json!({ "patterns": [{ "urlPattern": "*" }] }),
                Some(&self.session_id),
            )
            .await?;
        Ok(())
    }

    pub async fn get_element_height(&self) -> Result<f64> {
        let selector = if self.is_print_designer { "body" } else { ".wrapper" };
        self.send("DOM.enable", json!({})).await?;
        let doc = self.send("DOM.getDocument", json!({})).await?;
        let root = doc["root"]["nodeId"].as_i64().ok_or_else(|| anyhow!("no root node"))?;
        let found = self
            .send("DOM.querySelector", json!({ "nodeId": root, "selector": selector }))
            .await?;
        let node_id = found["nodeId"].as_i64().unwrap_or(0);
        let height = if node_id != 0 {
            let box_model = self.send("DOM.getBoxModel", json!({ "nodeId": node_id })).await?;
            box_model["model"]["height"].as_f64().unwrap_or(0.0)
        } else {
            0.0
        };
        self.send("DOM.disable", json!({})).await?;
        Ok(height)
    }

    /// Inject the `@page { size; margin }` rule from this page's options (inches).
    async fn add_page_size_css(&self) -> Result<()> {
        let g = |k: &str| self.options.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let css = format!(
            "\n\t@page {{\n\t\tsize: {}in {}in;\n\t\tmargin: {}in {}in {}in {}in;\n\t}}\n",
            g("paperWidth"),
            g("paperHeight"),
            g("marginTop"),
            g("marginRight"),
            g("marginBottom"),
            g("marginLeft"),
        );
        self.send("DOM.enable", json!({})).await?;
        self.send("CSS.enable", json!({})).await?;
        let sheet = self.send("CSS.createStyleSheet", json!({ "frameId": self.frame_id })).await?;
        let sheet_id = sheet["styleSheetId"].as_str().ok_or_else(|| anyhow!("no styleSheetId"))?.to_string();
        self.send("CSS.setStyleSheetText", json!({ "styleSheetId": sheet_id, "text": css })).await?;
        self.send("CSS.disable", json!({})).await?;
        self.send("DOM.disable", json!({})).await?;
        Ok(())
    }

    pub async fn evaluate(&self, expression: &str, await_promise: bool) -> Result<Value> {
        self.send("Runtime.enable", json!({})).await?;
        let result = self
            .send(
                "Runtime.evaluate",
                json!({ "expression": expression, "awaitPromise": await_promise }),
            )
            .await?;
        self.send("Runtime.disable", json!({})).await?;
        Ok(result)
    }

    /// Print this page to a PDF and return the raw bytes.
    pub async fn generate_pdf(&self) -> Result<Vec<u8>> {
        self.add_page_size_css().await?;
        let result = self.send("Page.printToPDF", Value::Object(self.options.clone())).await?;
        let stream = result["stream"].as_str().ok_or_else(|| anyhow!("no printToPDF stream"))?.to_string();
        self.read_stream(&stream).await
    }

    async fn read_stream(&self, handle: &str) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        loop {
            let chunk = self
                .send("IO.read", json!({ "handle": handle, "offset": data.len(), "size": 262144 }))
                .await?;
            let raw = chunk["data"].as_str().unwrap_or("");
            if chunk["base64Encoded"].as_bool().unwrap_or(false) {
                data.extend(base64::engine::general_purpose::STANDARD.decode(raw)?);
            } else {
                data.extend_from_slice(raw.as_bytes());
            }
            if chunk["eof"].as_bool().unwrap_or(false) {
                break;
            }
        }
        self.send("IO.close", json!({ "handle": handle })).await?;
        Ok(data)
    }

    pub async fn close(&self) -> Result<()> {
        let _ = self.send("Fetch.disable", json!({})).await;
        self.client.send("Target.closeTarget", json!({ "targetId": self.target_id }), None).await?;
        Ok(())
    }

    /// Subscribe now, then return a future that resolves once all lifecycle events are seen.
    async fn begin_lifecycle_wait(
        &self,
        wait_for: &[&str],
    ) -> Result<impl std::future::Future<Output = Result<()>>> {
        self.send("Page.setLifecycleEventsEnabled", json!({ "enabled": true })).await?;
        let mut events = self.client.subscribe();
        let session_id = self.session_id.clone();
        let frame_id = self.frame_id.clone();
        let mut remaining: std::collections::HashSet<String> =
            wait_for.iter().map(|s| s.to_string()).collect();

        Ok(async move {
            let deadline = Duration::from_secs(60);
            timeout(deadline, async move {
                while !remaining.is_empty() {
                    let msg = match events.recv().await {
                        Ok(m) => m,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => bail!("event stream closed while waiting for load"),
                    };
                    if msg["method"] != "Page.lifecycleEvent" {
                        continue;
                    }
                    if msg.get("sessionId").and_then(|v| v.as_str()) != Some(session_id.as_str()) {
                        continue;
                    }
                    let p = &msg["params"];
                    if p["frameId"].as_str() == Some(frame_id.as_str())
                        && let Some(name) = p["name"].as_str() {
                            remaining.remove(name);
                        }
                }
                Ok(())
            })
            .await
            .map_err(|_| anyhow!("timed out waiting for page load"))?
        })
    }
}

async fn handle_paused(
    client: &Arc<CdpClient>,
    session_id: &str,
    request_id: &str,
    url: &str,
    cfg: &Interceptor,
) {
    let sid = Some(session_id);
    if url.starts_with(&cfg.host_url) {
        let after_host = &url[cfg.host_url.len()..];
        let path = after_host.split("?v").next().unwrap_or(after_host);
        let clean = percent_encoding::percent_decode_str(path).decode_utf8_lossy().into_owned();

        if path.is_empty() {
            // Host-root navigation stub: fulfill with an empty 200 body.
            let _ = client
                .send("Fetch.fulfillRequest", json!({ "requestId": request_id, "responseCode": 200 }), sid)
                .await;
            return;
        }

        if let Some((system_path, is_safe)) = resolve_local(&clean, cfg) {
            if is_safe && std::fs::metadata(&system_path).map(|m| m.is_file()).unwrap_or(false)
                && let Ok(bytes) = std::fs::read(&system_path) {
                    let body = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    let mut headers = Vec::new();
                    if path.ends_with(".svg") {
                        headers.push(json!({ "name": "Content-Type", "value": "image/svg+xml" }));
                    }
                    let _ = client
                        .send(
                            "Fetch.fulfillRequest",
                            json!({ "requestId": request_id, "responseCode": 200, "responseHeaders": headers, "body": body }),
                            sid,
                        )
                        .await;
                    return;
                }
            // In-scope path but missing/unsafe: deny, matching Frappe's guard.
            let _ = client
                .send(
                    "Fetch.failRequest",
                    json!({ "requestId": request_id, "errorReason": "AccessDenied" }),
                    sid,
                )
                .await;
            return;
        }
    }
    let _ = client
        .send("Fetch.continueRequest", json!({ "requestId": request_id }), sid)
        .await;
}

/// Resolve a host-relative path to an on-disk file, enforcing Frappe's scope guard.
/// Returns (resolved_path, is_within_scope). `None` when required paths aren't configured.
/// `assets/` resolves under `<bench>/sites/assets` (lexical, like os.path.abspath); everything
/// else resolves under the site public root (canonical, like os.path.realpath).
fn resolve_local(clean_path: &str, cfg: &Interceptor) -> Option<(std::path::PathBuf, bool)> {
    use std::path::Path;
    if let Some(rest) = clean_path.strip_prefix("assets/") {
        let sites = cfg.bench_sites_path.as_ref()?;
        let asset_root = lexical_normalize(&Path::new(sites).join("assets"));
        let resolved = lexical_normalize(&Path::new(sites).join("assets").join(rest));
        let safe = resolved.starts_with(&asset_root);
        Some((resolved, safe))
    } else {
        let public = cfg.site_public_path.as_ref()?;
        let public_root =
            std::fs::canonicalize(public).unwrap_or_else(|_| lexical_normalize(Path::new(public)));
        let joined = public_root.join(clean_path);
        let resolved = std::fs::canonicalize(&joined).unwrap_or_else(|_| lexical_normalize(&joined));
        let safe = resolved.starts_with(&public_root);
        Some((resolved, safe))
    }
}

/// Resolve `.` and `..` lexically (like os.path.abspath), without touching the filesystem.
fn lexical_normalize(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut stack: Vec<Component> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(stack.last(), Some(Component::Normal(_))) {
                    stack.pop();
                } else {
                    stack.push(component);
                }
            }
            other => stack.push(other),
        }
    }
    stack.iter().collect()
}

fn host_domain(host_url: &str) -> String {
    host_url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Interceptor {
        Interceptor {
            host_url: "http://localhost:8000/".into(),
            bench_sites_path: Some("/bench/sites".into()),
            site_public_path: Some("/bench/sites/site/public".into()),
            sid: None,
        }
    }

    // Mirrors frappe/tests/test_chrome_pdf_interceptor.py: in-scope asset resolves and is safe;
    // a path-traversal escape is flagged unsafe.
    #[test]
    fn assets_in_scope_are_safe() {
        let (path, safe) = resolve_local("assets/frappe/css/app.css", &cfg()).unwrap();
        assert!(safe);
        assert!(path.starts_with("/bench/sites/assets/"));
    }

    #[test]
    fn asset_path_traversal_is_denied() {
        let (_, safe) = resolve_local("assets/../../etc/passwd", &cfg()).unwrap();
        assert!(!safe);
    }

    #[test]
    fn host_domain_strips_scheme_and_port() {
        assert_eq!(host_domain("http://example.com:8000/"), "example.com");
    }
}
