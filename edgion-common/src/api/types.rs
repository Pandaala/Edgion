use serde::Serialize;

/// Standard API response format used by Controller and Center Admin APIs.
#[derive(Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn ok_body(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn err_body(message: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message),
        }
    }
}

/// List response format with pagination support.
#[derive(Serialize)]
pub struct ListResponse<T> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Vec<T>>,
    pub count: usize,
    /// Token for fetching the next page. Absent when this is the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continue_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl<T> ListResponse<T> {
    pub fn success(data: Vec<T>) -> Self {
        Self::success_with_token(data, None)
    }

    pub fn success_with_token(data: Vec<T>, continue_token: Option<String>) -> Self {
        let count = data.len();
        Self {
            success: true,
            data: Some(data),
            count,
            continue_token,
            error: None,
        }
    }

    pub fn error(message: String) -> Self {
        Self {
            success: false,
            data: None,
            count: 0,
            continue_token: None,
            error: Some(message),
        }
    }
}
