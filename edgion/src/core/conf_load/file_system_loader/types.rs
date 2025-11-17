use crate::core::utils::ResourceMetadata;

#[derive(Clone)]
pub struct FileInfo {
    pub metadata: ResourceMetadata,
    pub content: String,
}

