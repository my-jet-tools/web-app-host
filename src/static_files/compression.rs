use std::io::Write;

use flate2::{write::DeflateEncoder, Compression as DeflateCompression};

/// Content below this size is not worth compressing
pub const COMPRESSION_MIN_SIZE: usize = 10 * 1024;
const EFFICIENCY_NUM: usize = 80;
const EFFICIENCY_DEN: usize = 100;
/// Compression happens once - when the file gets into the in-memory cache
const ZSTD_LEVEL: i32 = 11;

/// Compresses the content we are going to keep in memory - if it makes sense
pub fn try_zstd(raw: &[u8]) -> Option<Vec<u8>> {
    if raw.len() <= COMPRESSION_MIN_SIZE {
        return None;
    }

    let compressed = zstd::encode_all(raw, ZSTD_LEVEL).ok()?;

    if compressed.len() * EFFICIENCY_DEN <= raw.len() * EFFICIENCY_NUM {
        Some(compressed)
    } else {
        None
    }
}

pub fn zstd_decompress(data: &[u8]) -> std::io::Result<Vec<u8>> {
    zstd::decode_all(data)
}

pub fn deflate_compress(raw: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut encoder =
        DeflateEncoder::new(Vec::with_capacity(raw.len()), DeflateCompression::default());
    encoder.write_all(raw)?;
    encoder.finish()
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

    #[test]
    fn small_file_is_not_compressed() {
        let raw = vec![b'a'; 5 * 1024];
        assert!(try_zstd(&raw).is_none());
    }

    #[test]
    fn highly_compressible_is_compressed() {
        let raw = vec![b'a'; 50 * 1024];
        let compressed = try_zstd(&raw).expect("should compress");
        assert!(compressed.len() * 100 <= raw.len() * 80);
        assert_eq!(zstd_decompress(&compressed).unwrap(), raw);
    }

    #[test]
    fn incompressible_is_rejected() {
        let mut raw = Vec::with_capacity(50 * 1024);
        let mut seed: u32 = 0x9E37_79B1;
        for _ in 0..50 * 1024 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            raw.push((seed >> 24) as u8);
        }
        assert!(try_zstd(&raw).is_none());
    }

    #[test]
    fn exactly_threshold_is_not_compressed() {
        let raw = vec![b'a'; COMPRESSION_MIN_SIZE];
        assert!(try_zstd(&raw).is_none());
    }
}
