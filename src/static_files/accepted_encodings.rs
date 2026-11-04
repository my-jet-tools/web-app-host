use my_http_server::{HttpContext, HttpRequestHeaders};

use super::CompressionAlgorithm;

/// Encodings the client is ready to accept
#[derive(Debug, Clone, Copy, Default)]
pub struct AcceptedEncodings {
    pub brotli: bool,
    pub gzip: bool,
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

    pub fn accepts(&self, algorithm: CompressionAlgorithm) -> bool {
        match algorithm {
            CompressionAlgorithm::Brotli => self.brotli,
            CompressionAlgorithm::Gzip => self.gzip,
        }
    }

    fn parse(header_value: &str) -> Self {
        let mut result = Self::default();

        for token in header_value.split(',') {
            let mut parts = token.split(';');
            let name = parts.next().unwrap_or_default().trim();

            // 'q=0' means the client refuses the encoding
            if parts.any(is_zero_quality) {
                continue;
            }

            if name.eq_ignore_ascii_case("br") {
                result.brotli = true;
            } else if name.eq_ignore_ascii_case("gzip") || name.eq_ignore_ascii_case("x-gzip") {
                result.gzip = true;
            }
        }

        result
    }
}

fn is_zero_quality(param: &str) -> bool {
    let Some((name, value)) = param.split_once('=') else {
        return false;
    };

    if !name.trim().eq_ignore_ascii_case("q") {
        return false;
    }

    match value.trim().parse::<f64>() {
        Ok(quality) => quality <= 0.0,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_brotli_and_gzip() {
        let result = AcceptedEncodings::parse("gzip, deflate, br, zstd");
        assert!(result.accepts(CompressionAlgorithm::Brotli));
        assert!(result.accepts(CompressionAlgorithm::Gzip));
    }

    #[test]
    fn respects_the_case_and_the_quality() {
        let result = AcceptedEncodings::parse("BR;q=1.0, GZip;q=0.5");
        assert!(result.brotli);
        assert!(result.gzip);

        let result = AcceptedEncodings::parse("x-gzip");
        assert!(!result.brotli);
        assert!(result.gzip);
    }

    #[test]
    fn encoding_with_zero_quality_is_refused() {
        let result = AcceptedEncodings::parse("br;q=0, gzip");
        assert!(!result.brotli);
        assert!(result.gzip);

        let result = AcceptedEncodings::parse("br, gzip; Q = 0.000");
        assert!(result.brotli);
        assert!(!result.gzip);

        let result = AcceptedEncodings::parse("br;q=0.001");
        assert!(result.brotli);
    }

    #[test]
    fn is_not_confused_by_other_encodings() {
        let result = AcceptedEncodings::parse("deflate, zstd, identity");
        assert!(!result.brotli);
        assert!(!result.gzip);

        let result = AcceptedEncodings::parse("");
        assert!(!result.brotli);
        assert!(!result.gzip);
    }
}
