use super::*;

impl FileSystemCenter {
    #[cfg(test)]
    pub(super) fn conf_dir_abs(&self) -> &std::path::Path {
        &self.conf_dir_abs
    }

    #[cfg(test)]
    pub(super) fn storage_root(&self) -> &std::path::Path {
        self.writer.root().as_path()
    }
}
