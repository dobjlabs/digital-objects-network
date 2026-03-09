use axum::http::HeaderMap;

pub fn is_authorized(headers: &HeaderMap, expected_api_key: &str) -> bool {
    let Some(value) = headers.get(axum::http::header::AUTHORIZATION) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return false;
    };
    token == expected_api_key
}

#[cfg(test)]
mod tests {
    use super::is_authorized;
    use axum::http::{header::AUTHORIZATION, HeaderMap};

    #[test]
    fn auth_missing_header() {
        let headers = HeaderMap::new();
        assert!(!is_authorized(&headers, "k"));
    }

    #[test]
    fn auth_invalid_token() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, "Bearer wrong".parse().unwrap());
        assert!(!is_authorized(&headers, "expected"));
    }

    #[test]
    fn auth_valid_token() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, "Bearer expected".parse().unwrap());
        assert!(is_authorized(&headers, "expected"));
    }
}
