use super::tls_store::{get_global_tls_store, TlsStore};
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::EdgionTls;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Implement ConfHandler directly for TlsStore
impl ConfHandler<EdgionTls> for TlsStore {
    fn full_set(&self, data: &HashMap<String, Arc<EdgionTls>>) {
        self.replace_all_tls(data.clone())
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<EdgionTls>>,
        update: HashMap<String, Arc<EdgionTls>>,
        remove: HashSet<String>,
    ) {
        self.apply_partial_update(add, update, &remove)
    }
}

/// Create a TlsStore handler for registration with ConfigClient
pub fn create_tls_handler() -> Box<dyn ConfHandler<EdgionTls> + Send + Sync> {
    Box::new(get_global_tls_store())
}
