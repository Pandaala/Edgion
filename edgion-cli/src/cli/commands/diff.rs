mod manifest;

use super::super::client::EdgionClient;
use anyhow::{anyhow, bail, ensure, Context, Result};
use edgion_resources::{resource::is_kind_cluster_scoped, ResourceKind};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value;
use std::fmt;
use std::io::{Read, Write};
use std::path::PathBuf;

// Matches the Controller's bounded JSON response limit.
const MAX_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug)]
pub struct Differences;

impl fmt::Display for Differences {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("source differences found")
    }
}
impl std::error::Error for Differences {}

pub(crate) async fn execute(client: &EdgionClient, paths: &[PathBuf]) -> Result<()> {
    let mut lines = Vec::new();
    let mut different = false;
    for (index, path) in paths.iter().enumerate() {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .and_then(|file| file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes))
            .map_err(|_| anyhow!("file {}: cannot read manifest", index + 1))?;
        ensure!(bytes.len() <= MAX_BYTES, "file {}: manifest too large", index + 1);
        let start = lines.len();
        // deserialize_any represents both an empty document and explicit null as unit.
        // Advance a second cursor only when we need the original root scalar spelling.
        let mut spellings = yaml_serde::Deserializer::from_slice(&bytes).enumerate();
        for (index, document) in yaml_serde::Deserializer::from_slice(&bytes).enumerate() {
            let number = lines.len() + 1;
            let manifest::Manifest(local) = manifest::Manifest::deserialize(document)
                .map_err(|_| anyhow!("document {number}: invalid YAML manifest"))?;
            if local.is_null() {
                let empty = spellings
                    .find(|(i, _)| *i == index)
                    .and_then(|(_, scalar)| String::deserialize(scalar).ok())
                    .is_some_and(|spelling| spelling.is_empty());
                ensure!(empty, "document {number}: expected a resource, found null");
                continue;
            }
            let outcome = compare(client, local)
                .await
                .with_context(|| format!("document {number}"))?;
            different |= outcome != "equal";
            lines.push(format!("document {number}: {outcome}"));
        }
        ensure!(lines.len() > start, "file {}: empty manifest", index + 1);
    }

    let mut out = std::io::stdout().lock();
    for line in lines {
        writeln!(out, "{line}")?;
    }
    out.flush()?;
    if different {
        return Err(Differences.into());
    }
    Ok(())
}

fn valid_segment(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value != "."
        && !value.contains("..")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

async fn compare(client: &EdgionClient, mut local: Value) -> Result<String> {
    let kind = local["kind"]
        .as_str()
        .and_then(ResourceKind::from_kind_name)
        .filter(|kind| *kind != ResourceKind::Unspecified)
        .context("invalid resource kind")?;
    let metadata = local
        .get_mut("metadata")
        .and_then(Value::as_object_mut)
        .context("invalid metadata")?;
    let namespace = match (is_kind_cluster_scoped(kind), metadata.get("namespace")) {
        (false, None | Some(Value::Null)) => {
            metadata.insert("namespace".into(), Value::from("default"));
            Some("default")
        }
        (true, None | Some(Value::Null)) => {
            metadata.remove("namespace");
            None
        }
        (false, Some(Value::String(ns))) if valid_segment(ns, 63) => Some(ns.as_str()),
        _ => bail!("invalid namespace for resource scope"),
    };
    let name = metadata
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| valid_segment(name, 253))
        .context("invalid resource name")?;
    let mut response = client
        .get(kind.as_str(), namespace, name)
        .await
        .map_err(|_| anyhow!("Controller connection failed"))?;
    match response.status() {
        StatusCode::NOT_FOUND => {
            ensure!(
                response
                    .headers()
                    .get(edgion_resources::constants::SOURCE_NOT_FOUND_HEADER)
                    .is_some_and(|value| value == "true"),
                "unconfirmed source absence (HTTP 404)"
            );
            ensure!(
                response
                    .chunk()
                    .await
                    .map_err(|_| anyhow!("source read interrupted"))?
                    .is_none(),
                "unexpected HTTP 404 response"
            );
            return Ok("added".into());
        }
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            bail!("permission denied (HTTP {})", response.status().as_u16())
        }
        StatusCode::OK => {}
        status => bail!("source read failed (HTTP {})", status.as_u16()),
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| anyhow!("source read interrupted"))? {
        ensure!(chunk.len() <= MAX_BYTES - body.len(), "source response too large");
        body.extend_from_slice(&chunk);
    }
    let remote: Value = serde_json::from_slice(&body).map_err(|_| anyhow!("invalid source JSON"))?;
    ensure!(
        remote
            .get("kind")
            .is_none_or(|value| value.as_str() == Some(kind.as_str()))
            && remote.pointer("/metadata/name").and_then(Value::as_str) == Some(name)
            && match remote.pointer("/metadata/namespace") {
                None | Some(Value::Null) => namespace.is_none(),
                Some(Value::String(value)) => Some(value.as_str()) == namespace,
                _ => false,
            },
        "invalid source identity"
    );
    summary(local, remote)
}

fn summary(mut local: Value, mut remote: Value) -> Result<String> {
    // Split off metadata so arbitrary fields at either level remain comparable.
    fn take_metadata(document: &mut Value) -> Result<serde_json::Map<String, Value>> {
        let object = document.as_object_mut().context("invalid document")?;
        object.remove("status");
        let Some(Value::Object(mut metadata)) = object.remove("metadata") else {
            bail!("invalid metadata");
        };
        for key in [
            "uid",
            "resourceVersion",
            "generation",
            "creationTimestamp",
            "managedFields",
            "selfLink",
            "deletionTimestamp",
            "deletionGracePeriodSeconds",
        ] {
            metadata.remove(key);
        }
        if metadata.get("namespace") == Some(&Value::Null) {
            metadata.remove("namespace");
        }
        Ok(metadata)
    }
    let mut local_meta = take_metadata(&mut local)?;
    let mut remote_meta = take_metadata(&mut remote)?;
    // Only constant category names can cross the reporting boundary.
    let mut categories = Vec::new();
    for field in ["labels", "annotations"] {
        if local_meta.remove(field) != remote_meta.remove(field) {
            categories.push(field);
        }
    }
    if local_meta != remote_meta {
        categories.push("metadata");
    }
    if local != remote {
        categories.push("content");
    }
    Ok(if categories.is_empty() {
        "equal".into()
    } else {
        format!("changed ({})", categories.join(", "))
    })
}
