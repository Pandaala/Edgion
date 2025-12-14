use async_trait::async_trait;
use tokio::fs;
use std::path::PathBuf;

use crate::types::{EdgionResourceMgr, ResourceMgrError};
use crate::core::utils::extract_resource_metadata;
use super::loader::LocalPathLoader;

impl LocalPathLoader {
    /// 构建资源文件路径: Kind_namespace_name.yaml
    fn build_resource_path(&self, kind: &str, namespace: Option<&str>, name: &str) -> PathBuf {
        let filename = if let Some(ns) = namespace {
            format!("{}_{}_{ }.yaml", kind, ns, name)
        } else {
            format!("{}__{}.yaml", kind, name)
        };
        self.root().join(filename)
    }
}

#[async_trait]
impl EdgionResourceMgr for LocalPathLoader {
    async fn get(&self, kind: String, namespace: String, name: String) -> Result<(), ResourceMgrError> {
        let path = self.build_resource_path(&kind, Some(&namespace), &name);
        
        if !path.exists() {
            return Err(ResourceMgrError::NotFound(format!(
                "{}/{}/{}",
                kind, namespace, name
            )));
        }
        
        tracing::debug!(
            component = "file_system_loader",
            event = "resource_get",
            kind = %kind,
            namespace = %namespace,
            name = %name,
            path = ?path,
            "Resource found"
        );
        
        Ok(())
    }
    
    async fn create(&self, resource_yaml: String) -> Result<(), ResourceMgrError> {
        // 1. 解析 YAML 提取元数据
        let metadata = extract_resource_metadata(&resource_yaml)
            .ok_or_else(|| ResourceMgrError::ParseError("Failed to extract metadata".to_string()))?;
        
        let kind = metadata.kind.as_ref()
            .ok_or_else(|| ResourceMgrError::ParseError("Missing kind".to_string()))?;
        let name = metadata.name.as_ref()
            .ok_or_else(|| ResourceMgrError::ParseError("Missing name".to_string()))?;
        
        // 2. 构建文件路径
        let path = self.build_resource_path(
            kind,
            metadata.namespace.as_deref(),
            name
        );
        
        // 3. 检查文件是否已存在
        if path.exists() {
            return Err(ResourceMgrError::AlreadyExists(format!(
                "{}/{}/{}",
                kind,
                metadata.namespace.as_deref().unwrap_or("_"),
                name
            )));
        }
        
        // 4. 写入文件
        fs::write(&path, resource_yaml)
            .await
            .map_err(|e| ResourceMgrError::InternalError(format!("Failed to write file: {}", e)))?;
        
        tracing::info!(
            component = "file_system_loader",
            event = "resource_created",
            kind = %kind,
            namespace = ?metadata.namespace,
            name = %name,
            path = ?path,
            "Resource created successfully"
        );
        
        Ok(())
    }
    
    async fn update(&self, resource_yaml: String) -> Result<(), ResourceMgrError> {
        // 1. 解析 YAML 提取元数据
        let metadata = extract_resource_metadata(&resource_yaml)
            .ok_or_else(|| ResourceMgrError::ParseError("Failed to extract metadata".to_string()))?;
        
        let kind = metadata.kind.as_ref()
            .ok_or_else(|| ResourceMgrError::ParseError("Missing kind".to_string()))?;
        let name = metadata.name.as_ref()
            .ok_or_else(|| ResourceMgrError::ParseError("Missing name".to_string()))?;
        
        // 2. 构建文件路径
        let path = self.build_resource_path(
            kind,
            metadata.namespace.as_deref(),
            name
        );
        
        // 3. 检查文件是否存在
        if !path.exists() {
            return Err(ResourceMgrError::NotFound(format!(
                "{}/{}/{}",
                kind,
                metadata.namespace.as_deref().unwrap_or("_"),
                name
            )));
        }
        
        // 4. 覆盖文件
        fs::write(&path, resource_yaml)
            .await
            .map_err(|e| ResourceMgrError::InternalError(format!("Failed to write file: {}", e)))?;
        
        tracing::info!(
            component = "file_system_loader",
            event = "resource_updated",
            kind = %kind,
            namespace = ?metadata.namespace,
            name = %name,
            path = ?path,
            "Resource updated successfully"
        );
        
        Ok(())
    }
    
    async fn patch(&self, kind: String, namespace: String, name: String, patch_data: String) -> Result<(), ResourceMgrError> {
        // 1. 构建文件路径
        let path = self.build_resource_path(&kind, Some(&namespace), &name);
        
        // 2. 检查文件是否存在
        if !path.exists() {
            return Err(ResourceMgrError::NotFound(format!(
                "{}/{}/{}",
                kind, namespace, name
            )));
        }
        
        // 3. 读取现有文件
        let existing_content = fs::read_to_string(&path)
            .await
            .map_err(|e| ResourceMgrError::InternalError(format!("Failed to read file: {}", e)))?;
        
        // 4. 解析现有内容和 patch 数据
        let mut existing: serde_yaml::Value = serde_yaml::from_str(&existing_content)
            .map_err(|e| ResourceMgrError::ParseError(format!("Failed to parse existing YAML: {}", e)))?;
        
        let patch: serde_yaml::Value = serde_yaml::from_str(&patch_data)
            .map_err(|e| ResourceMgrError::ParseError(format!("Failed to parse patch YAML: {}", e)))?;
        
        // 5. 合并 (简单的深度合并)
        merge_yaml(&mut existing, patch);
        
        // 6. 写回文件
        let merged_yaml = serde_yaml::to_string(&existing)
            .map_err(|e| ResourceMgrError::InternalError(format!("Failed to serialize YAML: {}", e)))?;
        
        fs::write(&path, merged_yaml)
            .await
            .map_err(|e| ResourceMgrError::InternalError(format!("Failed to write file: {}", e)))?;
        
        tracing::info!(
            component = "file_system_loader",
            event = "resource_patched",
            kind = %kind,
            namespace = %namespace,
            name = %name,
            path = ?path,
            "Resource patched successfully"
        );
        
        Ok(())
    }
    
    async fn delete(&self, kind: String, namespace: String, name: String) -> Result<(), ResourceMgrError> {
        // 1. 构建文件路径
        let path = self.build_resource_path(&kind, Some(&namespace), &name);
        
        // 2. 检查文件是否存在
        if !path.exists() {
            return Err(ResourceMgrError::NotFound(format!(
                "{}/{}/{}",
                kind, namespace, name
            )));
        }
        
        // 3. 删除文件
        fs::remove_file(&path)
            .await
            .map_err(|e| ResourceMgrError::InternalError(format!("Failed to delete file: {}", e)))?;
        
        tracing::info!(
            component = "file_system_loader",
            event = "resource_deleted",
            kind = %kind,
            namespace = %namespace,
            name = %name,
            path = ?path,
            "Resource deleted successfully"
        );
        
        Ok(())
    }
}

/// 简单的 YAML 深度合并
fn merge_yaml(base: &mut serde_yaml::Value, patch: serde_yaml::Value) {
    use serde_yaml::Value;
    
    if let (Value::Mapping(base_map), Value::Mapping(patch_map)) = (base, patch) {
        for (key, value) in patch_map {
            base_map.insert(key, value);
        }
    }
}

