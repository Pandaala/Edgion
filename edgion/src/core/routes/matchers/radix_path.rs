use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
enum RawSegment {
    Slash,
    Literal(String),
    Param(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchSegment {
    Literal(String),
    Param(String),
}

#[derive(Debug, Clone)]
pub struct RadixPath {
    pub original: String,
    pub priority_weight: usize,
    pub radix_key: String,
    pub is_prefix_match: bool,
    pub match_segments: Vec<MatchSegment>,
    pub route_idx: usize,
}

impl RadixPath {
    pub fn new(path: &str, route_idx: usize, is_prefix: bool) -> Self {
        let original = path.to_string();
        let is_prefix_match = is_prefix;

        // Tokenize the path into raw segments (Slash, Literal, Param)
        let mut raw_segments: Vec<RawSegment> = Vec::new();
        let mut current_segment = String::new();

        let mut push_current = |seg: &mut String, list: &mut Vec<RawSegment>| {
            if seg.is_empty() {
                return;
            }
            if seg.starts_with('{') && seg.ends_with('}') {
                let name = &seg[1..seg.len() - 1];
                assert!(!name.is_empty(), "Empty param name in path: {}", path);
                list.push(RawSegment::Param(name.to_string()));
            } else {
                list.push(RawSegment::Literal(std::mem::take(seg)));
            }
        };

        let mut chars = path.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '/' {
                push_current(&mut current_segment, &mut raw_segments);
                raw_segments.push(RawSegment::Slash);
            } else {
                current_segment.push(c);
            }
        }
        push_current(&mut current_segment, &mut raw_segments);

        // Build radix_key and match_segments
        let mut match_segments: Vec<MatchSegment> = Vec::new();
        let mut accumulated_literal = String::new();
        let mut radix_key = String::new();
        let mut radix_key_set = false;

        let mut flush_literal = |accumulated: &mut String| {
            if accumulated.is_empty() {
                return;
            }
            if !radix_key_set {
                radix_key = std::mem::take(accumulated);
                radix_key_set = true;
            } else {
                match_segments.push(MatchSegment::Literal(std::mem::take(accumulated)));
            }
        };

        let raw_segments_len = raw_segments.len();
        for seg in raw_segments {
            match seg {
                RawSegment::Slash => accumulated_literal.push('/'),
                RawSegment::Literal(s) => accumulated_literal.push_str(&s),
                RawSegment::Param(p) => {
                    flush_literal(&mut accumulated_literal);
                    match_segments.push(MatchSegment::Param(p));
                }
            }
        }
        flush_literal(&mut accumulated_literal);

        if radix_key.is_empty() {
            // Default radix key to "/" when path starts with slash
            radix_key = if path.starts_with('/') { "/".to_string() } else { String::new() };
        }

        let priority_weight = if is_prefix_match {
            raw_segments_len.saturating_mul(2)
        } else {
            raw_segments_len.saturating_mul(2).saturating_add(1)
        };

        Self {
            original,
            priority_weight,
            radix_key,
            is_prefix_match,
            match_segments,
            route_idx,
        }
    }

    pub fn matches(&self, request_path: &str, out_params: &mut HashMap<String, String>) -> bool {
        if !request_path.starts_with(&self.radix_key) {
            return false;
        }
        let remaining_path = &request_path[self.radix_key.len()..];

        if self.match_segments.is_empty() {
            return if self.is_prefix_match { true } else { remaining_path.is_empty() };
        }

        let mut path_cursor = 0usize;
        for segment in &self.match_segments {
            if path_cursor > remaining_path.len() {
                return false;
            }
            match segment {
                MatchSegment::Literal(lit) => {
                    let slice = &remaining_path[path_cursor..];
                    if !slice.starts_with(lit) {
                        return false;
                    }
                    path_cursor += lit.len();
                }
                MatchSegment::Param(name) => {
                    let slice = &remaining_path[path_cursor..];
                    let end = slice.find('/').unwrap_or(slice.len());
                    if end == 0 {
                        return false;
                    }
                    let value = &slice[..end];
                    out_params.insert(name.clone(), value.to_string());
                    path_cursor += end;
                }
            }
        }

        if path_cursor == remaining_path.len() {
            true
        } else {
            self.is_prefix_match
        }
    }

    pub fn match_type_str(&self) -> &str {
        let has_param = self
            .match_segments
            .iter()
            .any(|s| matches!(s, MatchSegment::Param(_)));
        match (self.is_prefix_match, has_param) {
            (true, true) => "ParamPrefix",
            (true, false) => "Prefix",
            (false, true) => "Param",
            (false, false) => "Exact",
        }
    }
}