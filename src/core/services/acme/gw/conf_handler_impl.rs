//! ConfHandler implementation for EdgionAcme (Gateway side)
//!
//! Receives EdgionAcme resources synced from the Controller and extracts
//! active challenge tokens into the AcmeChallengeStore.

use super::challenge_store::get_global_challenge_store;
use crate::core::conf_sync::traits::ConfHandler;
use crate::types::resources::edgion_acme::EdgionAcme;
use std::collections::{HashMap, HashSet};

/// ConfHandler that extracts active challenges from EdgionAcme resources
/// and populates the global AcmeChallengeStore.
struct AcmeConfHandler;

impl ConfHandler<EdgionAcme> for AcmeConfHandler {
    fn full_set(&self, data: &HashMap<String, EdgionAcme>) {
        get_global_challenge_store().full_set(data);
    }

    fn partial_update(
        &self,
        add: HashMap<String, EdgionAcme>,
        update: HashMap<String, EdgionAcme>,
        remove: HashSet<String>,
    ) {
        if add.is_empty() && update.is_empty() && remove.is_empty() {
            return;
        }

        tracing::debug!(
            add = add.len(),
            update = update.len(),
            remove = remove.len(),
            "EdgionAcme partial_update: updating challenge store"
        );

        // Merge add + update into a single upsert map
        let mut upsert: HashMap<String, EdgionAcme> = add;
        upsert.extend(update);

        get_global_challenge_store().partial_update(&upsert, &remove);
    }
}

/// Create the ConfHandler for EdgionAcme resources
pub fn create_acme_handler() -> Box<dyn ConfHandler<EdgionAcme> + Send + Sync> {
    Box::new(AcmeConfHandler)
}
