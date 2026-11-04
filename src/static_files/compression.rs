use std::io::Write;

use flate2::{write::GzEncoder, Compression};

/// Content below this size is not worth compressing
pub const COMPRESSION_MIN_SIZE: usize = 10 * 1024;
/// Compressed copy is kept only if it is at most 80% of the raw content
const EFFICIENCY_NUM: usize = 80;
const EFFICIENCY_DEN: usize = 100;

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

    /// Compresses with the maximum level
    pub fn compress(&self, raw: &[u8]) -> std::io::Result<Vec<u8>> {
        match self {
            Self::Brotli => brotli_compress(raw),
            Self::Gzip => {
                let mut encoder =
                    GzEncoder::new(Vec::with_capacity(raw.len()), Compression::best());
                encoder.write_all(raw)?;
                encoder.finish()
            }
        }
    }
}

/// Compresses the content - if it makes sense
pub fn try_compress(raw: &[u8], algorithm: CompressionAlgorithm) -> Option<Vec<u8>> {
    if raw.len() <= COMPRESSION_MIN_SIZE {
        return None;
    }

    let compressed = algorithm.compress(raw).ok()?;

    if compressed.len() * EFFICIENCY_DEN <= raw.len() * EFFICIENCY_NUM {
        Some(compressed)
    } else {
        None
    }
}

fn brotli_compress(raw: &[u8]) -> std::io::Result<Vec<u8>> {
    let params = brotli::enc::BrotliEncoderParams {
        quality: BROTLI_QUALITY,
        lgwin: brotli_window_log(raw.len()),
        size_hint: raw.len(),
        ..Default::default()
    };

    let mut result = Vec::with_capacity(raw.len());
    brotli::BrotliCompress(&mut &raw[..], &mut result, &params)?;
    Ok(result)
}

/// The smallest window which covers the whole file - the brotli tool chooses it the same way.
/// The bigger one compresses no better, but takes more memory: the encoder of the quality 11
/// allocates the memory by the size of the window - and so does the decoder of the browser
fn brotli_window_log(size: usize) -> i32 {
    let mut result = BROTLI_MIN_WINDOW_LOG;

    // the window of the brotli is 16 bytes less than the power of two
    while result < BROTLI_MAX_WINDOW_LOG && (1usize << result) - 16 < size {
        result += 1;
    }

    result
}

pub fn calc_etag(content: &[u8]) -> String {
    use base64::Engine;
    use sha2::Digest;
    use sha2::Sha256;

    let mut sha_256 = Sha256::new();
    sha_256.update(content);

    let result = sha_256.finalize();
    let mut result = base64::engine::general_purpose::STANDARD.encode(result);

    if result.ends_with('=') {
        result.pop();
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::static_files::{compile_text, decompress};

    #[test]
    fn small_file_is_not_compressed() {
        let raw = vec![b'a'; 5 * 1024];

        for algorithm in CompressionAlgorithm::ALL {
            assert!(try_compress(&raw, algorithm).is_none(), "{:?}", algorithm);
        }
    }

    #[test]
    fn exactly_threshold_is_not_compressed() {
        let raw = vec![b'a'; COMPRESSION_MIN_SIZE];

        for algorithm in CompressionAlgorithm::ALL {
            assert!(try_compress(&raw, algorithm).is_none(), "{:?}", algorithm);
        }
    }

    #[test]
    fn incompressible_is_rejected() {
        let mut raw = Vec::with_capacity(50 * 1024);
        let mut seed: u32 = 0x9E37_79B1;
        for _ in 0..50 * 1024 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            raw.push((seed >> 24) as u8);
        }

        for algorithm in CompressionAlgorithm::ALL {
            assert!(try_compress(&raw, algorithm).is_none(), "{:?}", algorithm);
        }
    }

    #[test]
    fn every_algorithm_restores_the_content() {
        let raw = compile_text(100 * 1024);

        for algorithm in CompressionAlgorithm::ALL {
            let compressed = try_compress(&raw, algorithm).expect("should compress");

            assert!(compressed.len() * EFFICIENCY_DEN <= raw.len() * EFFICIENCY_NUM);
            assert_eq!(decompress(algorithm, &compressed), raw, "{:?}", algorithm);
        }
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
    fn has_the_names_of_the_content_encoding() {
        assert_eq!(CompressionAlgorithm::Brotli.as_str(), "br");
        assert_eq!(CompressionAlgorithm::Gzip.as_str(), "gzip");
    }
}
