//! High-level Elasticsearch operations.
//!
//! Wraps reqwest HTTP calls with anyhow error handling and Edgion-friendly signatures.
//! Covers: cluster health, index CRUD, document CRUD, search, and health status.

use anyhow::Result;
use reqwest::StatusCode;
use serde_json::Value;

use super::client::EsLinkClient;

// ============================================================================
// Health Status (shared type, compatible with Redis/Etcd)
// ============================================================================

/// Health status for admin API exposure.
#[derive(serde::Serialize, Debug)]
pub struct LinkSysHealth {
    pub name: String,
    pub system_type: String,
    pub connected: bool,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

// ============================================================================
// Cluster Operations
// ============================================================================

impl EsLinkClient {
    /// GET /_cluster/health — returns cluster status ("green"/"yellow"/"red").
    pub async fn cluster_health(&self) -> Result<String> {
        let endpoint = self.next_endpoint();
        let url = format!("{}/_cluster/health", endpoint);

        let resp = self
            .inner()
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ES cluster health request failed: {:?}", e))?;

        if !resp.status().is_success() {
            anyhow::bail!("ES cluster health returned status {}", resp.status());
        }

        let body: Value = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("ES cluster health parse error: {:?}", e))?;

        body["status"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("ES cluster health: missing 'status' field"))
    }

    /// GET /_cat/health?format=json — verbose health info.
    pub async fn cluster_health_verbose(&self) -> Result<Value> {
        let endpoint = self.next_endpoint();
        let url = format!("{}/_cat/health?format=json", endpoint);

        let resp = self
            .inner()
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ES cat health failed: {:?}", e))?;

        resp.json()
            .await
            .map_err(|e| anyhow::anyhow!("ES cat health parse error: {:?}", e))
    }
}

// ============================================================================
// Index Operations
// ============================================================================

impl EsLinkClient {
    /// PUT /{index} — create index with optional settings/mappings.
    pub async fn create_index(&self, index: &str, body: Option<&Value>) -> Result<()> {
        let endpoint = self.next_endpoint();
        let url = format!("{}/{}", endpoint, index);

        let mut req = self.inner().put(&url);
        if let Some(body) = body {
            req = req.json(body);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ES create index {}: {:?}", index, e))?;

        if resp.status() == StatusCode::BAD_REQUEST {
            let body: Value = resp.json().await.unwrap_or_default();
            let err_type = body["error"]["type"].as_str().unwrap_or("");
            if err_type == "resource_already_exists_exception" {
                tracing::debug!("ES index {} already exists", index);
                return Ok(());
            }
            anyhow::bail!("ES create index {}: {}", index, body);
        }

        if !resp.status().is_success() {
            anyhow::bail!("ES create index {}: status {}", index, resp.status());
        }

        Ok(())
    }

    /// Check if index exists (via GET /{index}/_settings and checking status code).
    ///
    /// Uses `_settings` endpoint which returns 404 for non-existent indices
    /// and 200 with settings for existing ones. More reliable than HEAD
    /// across ES versions and reqwest default headers.
    pub async fn index_exists(&self, index: &str) -> Result<bool> {
        let endpoint = self.next_endpoint();
        let url = format!("{}/{}/_settings", endpoint, index);

        let resp = self
            .inner()
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ES index exists {}: {:?}", index, e))?;

        Ok(resp.status().is_success())
    }

    /// DELETE /{index} — delete index.
    pub async fn delete_index(&self, index: &str) -> Result<()> {
        let endpoint = self.next_endpoint();
        let url = format!("{}/{}", endpoint, index);

        let resp = self
            .inner()
            .delete(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ES delete index {}: {:?}", index, e))?;

        if resp.status() == StatusCode::NOT_FOUND {
            tracing::debug!("ES index {} not found (already deleted)", index);
            return Ok(());
        }

        if !resp.status().is_success() {
            anyhow::bail!("ES delete index {}: status {}", index, resp.status());
        }

        Ok(())
    }
}

// ============================================================================
// Document Operations
// ============================================================================

impl EsLinkClient {
    /// POST /{index}/_doc — index a single document. Returns document ID.
    pub async fn index_doc(&self, index: &str, doc: &Value) -> Result<String> {
        let endpoint = self.next_endpoint();
        let url = format!("{}/{}/_doc", endpoint, index);

        let resp = self
            .inner()
            .post(&url)
            .json(doc)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ES index doc {}: {:?}", index, e))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("ES index doc {}: {}", index, body);
        }

        let body: Value = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("ES index doc response parse: {:?}", e))?;

        body["_id"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("ES index doc: missing _id in response"))
    }

    /// PUT /{index}/_doc/{id} — index a document with explicit ID.
    pub async fn index_doc_with_id(&self, index: &str, id: &str, doc: &Value) -> Result<()> {
        let endpoint = self.next_endpoint();
        let url = format!("{}/{}/_doc/{}", endpoint, index, id);

        let resp = self
            .inner()
            .put(&url)
            .json(doc)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ES index doc {}/{}: {:?}", index, id, e))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("ES index doc {}/{}: {}", index, id, body);
        }

        Ok(())
    }

    /// GET /{index}/_doc/{id} — get document by ID. Returns `_source` or None.
    pub async fn get_doc(&self, index: &str, id: &str) -> Result<Option<Value>> {
        let endpoint = self.next_endpoint();
        let url = format!("{}/{}/_doc/{}", endpoint, index, id);

        let resp = self
            .inner()
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ES get doc {}/{}: {:?}", index, id, e))?;

        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !resp.status().is_success() {
            anyhow::bail!("ES get doc {}/{}: status {}", index, id, resp.status());
        }

        let body: Value = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("ES get doc parse: {:?}", e))?;

        Ok(Some(body["_source"].clone()))
    }

    /// DELETE /{index}/_doc/{id} — delete document by ID.
    /// Returns true if the document was found and deleted, false if not found.
    pub async fn delete_doc(&self, index: &str, id: &str) -> Result<bool> {
        let endpoint = self.next_endpoint();
        let url = format!("{}/{}/_doc/{}", endpoint, index, id);

        let resp = self
            .inner()
            .delete(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ES delete doc {}/{}: {:?}", index, id, e))?;

        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(false);
        }

        if !resp.status().is_success() {
            anyhow::bail!("ES delete doc {}/{}: status {}", index, id, resp.status());
        }

        // ES returns {"result":"deleted"} on success, {"result":"not_found"} on miss
        let body: Value = resp.json().await.unwrap_or_default();
        let result = body["result"].as_str().unwrap_or("unknown");
        Ok(result == "deleted")
    }

    /// POST /{index}/_search — search documents.
    ///
    /// `query` is the ES query DSL (e.g., `{"match_all":{}}` or `{"match":{"field":"value"}}`).
    /// This method wraps it in `{"query": ...}` before sending.
    pub async fn search(&self, index: &str, query: &Value) -> Result<EsSearchResult> {
        let endpoint = self.next_endpoint();
        let url = format!("{}/{}/_search", endpoint, index);

        let search_body = serde_json::json!({"query": query});

        let resp = self
            .inner()
            .post(&url)
            .json(&search_body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ES search {}: {:?}", index, e))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("ES search {}: {}", index, body);
        }

        resp.json::<EsSearchResult>()
            .await
            .map_err(|e| anyhow::anyhow!("ES search response parse: {:?}", e))
    }

    /// POST /{index}/_refresh — force refresh (make recent writes visible to search).
    pub async fn refresh_index(&self, index: &str) -> Result<()> {
        let endpoint = self.next_endpoint();
        let url = format!("{}/{}/_refresh", endpoint, index);

        let resp = self
            .inner()
            .post(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ES refresh {}: {:?}", index, e))?;

        if !resp.status().is_success() {
            anyhow::bail!("ES refresh {}: status {}", index, resp.status());
        }

        Ok(())
    }

    /// POST /{index}/_count — count documents matching a query.
    pub async fn count(&self, index: &str, query: Option<&Value>) -> Result<u64> {
        let endpoint = self.next_endpoint();
        let url = format!("{}/{}/_count", endpoint, index);

        let mut req = self.inner().post(&url);
        if let Some(q) = query {
            req = req.json(q);
        } else {
            req = req.json(&serde_json::json!({"query": {"match_all": {}}}));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ES count {}: {:?}", index, e))?;

        if !resp.status().is_success() {
            anyhow::bail!("ES count {}: status {}", index, resp.status());
        }

        let body: Value = resp.json().await?;
        Ok(body["count"].as_u64().unwrap_or(0))
    }
}

// ============================================================================
// Search Result Types
// ============================================================================

/// Search result structure
#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct EsSearchResult {
    pub took: u64,
    pub timed_out: bool,
    pub hits: EsSearchHits,
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct EsSearchHits {
    pub total: EsSearchTotal,
    #[serde(default)]
    pub hits: Vec<EsSearchHit>,
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct EsSearchTotal {
    pub value: u64,
    pub relation: String,
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct EsSearchHit {
    #[serde(rename = "_index")]
    pub index: String,
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(rename = "_score")]
    pub score: Option<f64>,
    #[serde(rename = "_source")]
    pub source: Value,
}

// ============================================================================
// Health Check
// ============================================================================

impl EsLinkClient {
    /// Active health check via cluster health API.
    /// Returns latency in milliseconds.
    pub async fn ping(&self) -> Result<u64> {
        let start = std::time::Instant::now();
        let status = self.cluster_health().await?;
        let latency = start.elapsed().as_millis() as u64;
        self.set_healthy(true);
        tracing::debug!("ES [{}] ping: status={}, latency={}ms", self.name(), status, latency);
        Ok(latency)
    }

    /// Get detailed health status.
    pub async fn health_status(&self) -> LinkSysHealth {
        match self.ping().await {
            Ok(latency_ms) => LinkSysHealth {
                name: self.name().to_string(),
                system_type: "elasticsearch".to_string(),
                connected: true,
                latency_ms: Some(latency_ms),
                error: None,
            },
            Err(e) => {
                self.set_healthy(false);
                LinkSysHealth {
                    name: self.name().to_string(),
                    system_type: "elasticsearch".to_string(),
                    connected: false,
                    latency_ms: None,
                    error: Some(e.to_string()),
                }
            }
        }
    }
}
