use my_http_server::{HttpContext, HttpRequestHeaders};

/// Encodings the client is ready to accept
#[derive(Debug, Clone, Copy, Default)]
pub struct AcceptedEncodings {
    pub zstd: bool,
    pub deflate: bool,
}

impl AcceptedEncodings {
    pub fn from_request(ctx: &HttpContext) -> Self {
        let Some(header) = ctx
            .request
            .get_headers()
            .try_get_case_insensitive("accept-encoding")
        else {
            return Self::default();
        };

        let Ok(header) = header.as_str() else {
            return Self::default();
        };

        Self::parse(header)
    }

    fn parse(header_value: &str) -> Self {
        let mut result = Self::default();

        for token in header_value.split(',') {
            let token = token.trim();

            let name = match token.split(';').next() {
                Some(name) => name.trim(),
                None => token,
            };

            if name.eq_ignore_ascii_case("zstd") {
                result.zstd = true;
            } else if name.eq_ignore_ascii_case("deflate") {
                result.deflate = true;
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_zstd() {
        let result = AcceptedEncodings::parse("zstd");
        assert!(result.zstd);
        assert!(!result.deflate);
    }

    #[test]
    fn parses_both() {
        let result = AcceptedEncodings::parse("zstd, deflate, br");
        assert!(result.zstd);
        assert!(result.deflate);
    }

    #[test]
    fn respects_case_and_qvalues() {
        let result = AcceptedEncodings::parse("Zstd;q=1.0, DEFLATE;q=0.5");
        assert!(result.zstd);
        assert!(result.deflate);
    }

    #[test]
    fn is_not_confused_by_other_encodings() {
        let result = AcceptedEncodings::parse("gzip, br");
        assert!(!result.zstd);
        assert!(!result.deflate);
    }
}
