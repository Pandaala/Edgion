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

        let mut raw_segments = Vec::new();
        let mut current_segment = String::new();

        let process_segment = |segment: String, raw_segment: &mut Vec<RawSegment>| {
            if segment.is_empty() {
                return ;
            }
            if segment.starts_with('{') && segment.ends_with('}') {
                let param_name = &segment[1..segment.len() - 1];
                if param_name.is_empty() {
                    panic!("Empty param name in path: {}", path);
                }
                raw_segment.push(RawSegment::Param(param_name.to_string()));
            } else {
                raw_segment.push(RawSegment::Literal(segment));
            }
        };

        let mut chars = path.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '/' {
                process_segment(current_segment.clone(), &mut raw_segments);
                current_segment.clear();
                raw_segments.push(RawSegment::Slash);
            } else {
                current_segment.push(c);
            }
        }
        process_segment(current_segment, &mut raw_segments);

        let mut match_segments = Vec::new();
        let mut accumulated_literal = String::new();
        let mut radix_key = String::new();
        let mut radix_key_set = false;

        let flush_literal = |accumulated: String, radix_key: &mut String, radix_key_set: &mut bool, match_segments: &mut Vec<MatchSegment>| {
            if accumulated.is_empty() {
                return;
            }
            if !*radix_key_set {
                *radix_key = accumulated;
                *radix_key_set = true;
            } else {
                match_segments.push(MatchSegment::Literal(accumulated));
            }
        };

        let raw_segments_len = raw_segments.len();

        for raw_seg in raw_segments {
            match raw_seg {
                RawSegment::Slash => { accumulated_literal.push('/'); },
                RawSegment::Literal(s) => { accumulated_literal.push(&s); },
                RawSegment::Param(param) => {
                    flush_literal(accumulated_literal.clone(), &mut radix_key, &mut *radix_key_set, &mut match_segments);
                    accumulated_literal.clear();
                    match_segments.push(MatchSegment::Param(param));
                },
            }
        }
        flush_literal(accumulated_literal, &mut radix_key, &mut *radix_key_set, &mut match_segments);

        if radix_key.is_empty() {
            radix_key = "/".to_string();
        }

        let priority_weight = if  is_prefix_match {
            raw_segments_len * 2
        } else {
            current_segment * 2 + 1
        };

        self{
            original,
            priority_weight,
            radix_key,
            is_prefix_match,
            match_segment,
            route_idx,
        }
    }


    pub fn matches(&self, request_path: &str) -> Vec<MatchSegment> {
        let remaining_path = &request_path[self.radix_key.len()..];

        if self.match_segments.is_empty() {
            return if self.is_prefix_match {
                true
            } else {
                remaining_path.is_empty()
            }
        }

        let mut path_cursor = 0;

        for segment in &self.match_segments {
            if path_cursor >= remaining_path.len() {
                return false;
            }

            match segment {
                MatchSegment::Literal(literal) => {
                    let segment_path = &remaining_path[path_cursor..];
                    if !segment_path.starts_with(literal) {
                        return false;
                    }
                    path_cursor += literal.len();
                }
                MatchSegment::Param(param) => {
                    let segment_path = &remaining_path[path_cursor..];
                    let param_end = segment_path.find('/').unwrap_or(segment_path.len());
                    if param_end == 0 {
                        return false;
                    }
                    path_cursor += param_end;
                }
            }
        }

        debug_assert!(path_cursor >= remaining_path.len(), ture, "Bug: path_cursor {{}} exceeds remaining_path length {{}}", path_cursor, remaining_path.len());

        if path_cursor == remaining_path.len() {
            true
        } else {
            self.is_prefix_match
        }
    }

    pub fn match_type_str(&self) -> &str {
        let has_param = self.match_segments.iter().any(|s|matches!(s, MatchSegment::Param(_)));
        match (self.is_prefix_match, has_param) {
            (true, true) => "ParamPrefix",
            (true, false) => "Prefix",
            (false, true) => "Param",
            (false, false) => "Exact",
        }
    }

}