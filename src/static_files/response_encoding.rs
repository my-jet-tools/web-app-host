/// Encoding we are sending the content with
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseEncoding {
    Identity,
    Zstd,
    Deflate,
}

impl ResponseEncoding {
    pub fn header_value(&self) -> Option<&'static str> {
        match self {
            ResponseEncoding::Identity => None,
            ResponseEncoding::Zstd => Some("zstd"),
            ResponseEncoding::Deflate => Some("deflate"),
        }
    }
}
