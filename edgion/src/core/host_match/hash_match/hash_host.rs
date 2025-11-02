use crate::types::schema::is_valid_domain;
use std::collections::HashMap;

pub struct HashHost<T> {
    map: HashMap<String, T>,
}

impl<T> HashHost<T> {
    pub fn new() -> HashHost<T> {
        HashHost {
            map: HashMap::new(),
        }
    }

    pub fn insert(&mut self, k: &str, v: T) -> bool {
        let key = if k.starts_with("*.") {
            if is_valid_domain(&k[2..]) {
                k[1..].to_string() // "*.aaa.com" -> ".aaa.com" to distinguish from "aaa.com"
            } else {
                return false;
            }
        } else {
            if is_valid_domain(k) {
                k.to_string()
            } else {
                return false;
            }
        };

        self.map.insert(key, v);
        true
    }

    pub fn get(&self, k: &str) -> Option<&T> {
        // Validate the input domain first
        if !is_valid_domain(k) {
            return None;
        }

        // Step 1: Try exact match
        if let Some(value) = self.map.get(k) {
            return Some(value);
        }

        // Step 2: Try wildcard match
        if let Some(first_dot_pos) = k.find('.') {
            let wildcard_key = &k[first_dot_pos..];
            // Validate the wildcard key part (without the first label)
            // e.g., for "api.example.com", validate "example.com"
            if is_valid_domain(&wildcard_key[1..]) {
                return self.map.get(wildcard_key);
            }
        }

        None
    }

    pub fn remove(&mut self, k: &str) -> Option<T> {
        self.map.remove(k)
    }
}
