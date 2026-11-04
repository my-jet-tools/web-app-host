use std::io::{Read, Write};

use flate2::{write::GzEncoder, Compression};
use sha2::{Digest, Sha256};

/// Content below this size is not worth compressing
pub const COMPRESSION_MIN_SIZE: usize = 10 * 1024;
/// Compressed copy is kept only if it is at most 80% of the raw content
const EFFICIENCY_NUM: u64 = 80;
const EFFICIENCY_DEN: u64 = 100;

/// Copies are compressed once - at the start of the service, so they are compressed
/// with the maximum level
const BROTLI_QUALITY: i32 = 11;
const BROTLI_MIN_WINDOW_LOG: i32 = 10;
/// The biggest window of the standard brotli. The large window one is not decoded by browsers
const BROTLI_MAX_WINDOW_LOG: i32 = 24;

/// Algorithm the copy of the content is compressed with.
/// Its name is the value of the Content-Encoding header as well
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionAlgorithm {
    Brotli,
    Gzip,
}

impl CompressionAlgorithm {
    /// In the order of preference: the client which accepts several algorithms
    /// gets the content compressed with the first one of them
    pub const ALL: [Self; 2] = [Self::Brotli, Self::Gzip];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Brotli => "br",
            Self::Gzip => "gzip",
        }
    }

    /// Compresses the content of the reader into the writer - with the maximum level, by the
    /// chunks: neither the content nor its copy is kept in the memory whole. The size of the
    /// content chooses the window of the brotli
    pub fn compress(
        &self,
        reader: &mut impl Read,
        writer: &mut impl Write,
        size: u64,
    ) -> std::io::Result<()> {
        match self {
            Self::Brotli => {
                let params = brotli::enc::BrotliEncoderParams {
                    quality: BROTLI_QUALITY,
                    lgwin: brotli_window_log(size),
                    size_hint: size as usize,
                    ..Default::default()
                };

                brotli::BrotliCompress(reader, writer, &params)?;
            }
            Self::Gzip => {
                let mut encoder = GzEncoder::new(writer, Compression::best());
                std::io::copy(reader, &mut encoder)?;
                encoder.finish()?;
            }
        }

        Ok(())
    }
}

/// The copy is kept only if it is at most 80% of the content - otherwise it is not worth it
pub fn is_compressed_well(raw_size: u64, compressed_size: u64) -> bool {
    compressed_size * EFFICIENCY_DEN <= raw_size * EFFICIENCY_NUM
}

/// The smallest window which covers the whole file - the brotli tool chooses it the same way.
/// The bigger one compresses no better, but takes more memory: the encoder of the quality 11
/// allocates the memory by the size of the window - and so does the decoder of the browser
fn brotli_window_log(size: u64) -> i32 {
    let mut result = BROTLI_MIN_WINDOW_LOG;

    // the window of the brotli is 16 bytes less than the power of two
    while result < BROTLI_MAX_WINDOW_LOG && (1u64 << result) - 16 < size {
        result += 1;
    }

    result
}

/// Calculates the etag of the content which comes by the chunks - so the content
/// is not kept in the memory whole
#[derive(Default)]
pub struct EtagHasher {
    sha_256: Sha256,
}

impl EtagHasher {
    pub fn update(&mut self, chunk: &[u8]) {
        self.sha_256.update(chunk);
    }

    /// SHA-256 of the content as a strong etag - quoted, the way RFC 9110 requires. Each variant
    /// of the file - the raw one and every compressed copy - is the bytes of its own, so each has
    /// its own strong etag. The hash is the same as the one the previous versions sent without
    /// the quotes: the library still matches those in 'If-None-Match'
    pub fn finish(self) -> String {
        use base64::Engine;

        let result = self.sha_256.finalize();
        let mut result = base64::engine::general_purpose::STANDARD.encode(result);

        if result.ends_with('=') {
            result.pop();
        }

        my_http_server::strong_etag(result.as_str())
    }
}

/// Etag of the content which is in the memory whole
#[cfg(test)]
pub fn calc_etag(content: &[u8]) -> String {
    let mut hasher = EtagHasher::default();
    hasher.update(content);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::static_files::{compile_text, decompress};

    fn compress(algorithm: CompressionAlgorithm, raw: &[u8]) -> Vec<u8> {
        let mut result = Vec::new();

        algorithm
            .compress(&mut &raw[..], &mut result, raw.len() as u64)
            .unwrap();

        result
    }

    #[test]
    fn every_algorithm_restores_the_content() {
        let raw = compile_text(100 * 1024);

        for algorithm in CompressionAlgorithm::ALL {
            let compressed = compress(algorithm, &raw);

            assert!(
                is_compressed_well(raw.len() as u64, compressed.len() as u64),
                "{:?}",
                algorithm
            );
            assert_eq!(decompress(algorithm, &compressed), raw, "{:?}", algorithm);
        }
    }

    #[test]
    fn incompressible_is_not_compressed_well() {
        let mut raw = Vec::with_capacity(50 * 1024);
        let mut seed: u32 = 0x9E37_79B1;
        for _ in 0..50 * 1024 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            raw.push((seed >> 24) as u8);
        }

        for algorithm in CompressionAlgorithm::ALL {
            let compressed = compress(algorithm, &raw);

            assert!(
                !is_compressed_well(raw.len() as u64, compressed.len() as u64),
                "{:?}",
                algorithm
            );
        }
    }

    #[test]
    fn copy_is_kept_when_it_is_at_most_80_percent() {
        assert!(is_compressed_well(100, 80));
        assert!(is_compressed_well(100, 10));
        assert!(!is_compressed_well(100, 81));
        assert!(!is_compressed_well(100, 120));
    }

    #[test]
    fn brotli_window_covers_the_file() {
        assert_eq!(brotli_window_log(0), BROTLI_MIN_WINDOW_LOG);
        assert_eq!(brotli_window_log(1008), 10);
        assert_eq!(brotli_window_log(1009), 11);
        assert_eq!(brotli_window_log(100 * 1024), 17);
        assert_eq!(brotli_window_log(8 * 1024 * 1024), 24);
        // the window is not above the standard maximum - whatever the size of the file is
        assert_eq!(brotli_window_log(100 * 1024 * 1024), BROTLI_MAX_WINDOW_LOG);
    }

    #[test]
    fn etag_is_quoted_and_depends_on_the_content() {
        let etag = calc_etag(b"content");

        assert!(etag.starts_with('"') && etag.ends_with('"'), "{}", etag);
        assert!(etag.len() > 2);
        assert!(!etag.contains('='), "{}", etag);

        assert_eq!(etag, calc_etag(b"content"));
        assert_ne!(etag, calc_etag(b"other content"));
    }

    #[test]
    fn etag_of_the_chunks_is_the_etag_of_the_content() {
        let content = compile_text(200 * 1024);

        let mut hasher = EtagHasher::default();

        for chunk in content.chunks(64 * 1024) {
            hasher.update(chunk);
        }

        assert_eq!(hasher.finish(), calc_etag(&content));
    }

    #[test]
    fn has_the_names_of_the_content_encoding() {
        assert_eq!(CompressionAlgorithm::Brotli.as_str(), "br");
        assert_eq!(CompressionAlgorithm::Gzip.as_str(), "gzip");
    }
}
