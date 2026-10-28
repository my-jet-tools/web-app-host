use super::CompressionAlgorithm;

/// Encoding we are sending the content with
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseEncoding {
    Identity,
    Compressed(CompressionAlgorithm),
}

impl ResponseEncoding {
    pub fn header_value(&self) -> Option<&'static str> {
        match self {
            ResponseEncoding::Identity => None,
            ResponseEncoding::Compressed(algorithm) => Some(algorithm.as_str()),
        }
    }
}
