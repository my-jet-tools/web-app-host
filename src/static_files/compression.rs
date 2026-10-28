use std::io::{Read, Write};

use flate2::{
    read::{DeflateDecoder, GzDecoder},
    write::{DeflateEncoder, GzEncoder},
    Compression as FlateCompression,
};

/// Content below this size is not worth compressing
pub const COMPRESSION_MIN_SIZE: usize = 10 * 1024;
const EFFICIENCY_NUM: usize = 80;
const EFFICIENCY_DEN: usize = 100;

/// Compression happens once - when the file gets into the in-memory cache
const ZSTD_LEVEL: i32 = 11;
const ZSTD_MAX_LEVEL: i32 = 22;
/// Browsers decode zstd with the window of 8MB at most (RFC 9659). Levels above 19
/// use the bigger one - unless it is limited
const ZSTD_MAX_WINDOW_LOG: u32 = 23;

const BROTLI_QUALITY: i32 = 9;
const BROTLI_MAX_QUALITY: i32 = 11;
const BROTLI_WINDOW_LOG: i32 = 22;
/// The biggest window of the standard brotli. The large window one is not decoded by browsers
const BROTLI_MAX_WINDOW_LOG: i32 = 24;

/// Algorithm the content is kept compressed with in memory.
/// Its name is the value of the Content-Encoding header as well
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionAlgorithm {
    Zstd,
    Brotli,
    Gzip,
    Deflate,
}

impl CompressionAlgorithm {
    pub const ALL: [Self; 4] = [Self::Zstd, Self::Brotli, Self::Gzip, Self::Deflate];

    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();

        if value.eq_ignore_ascii_case("brotli") {
            return Some(Self::Brotli);
        }

        Self::ALL
            .into_iter()
            .find(|algorithm| value.eq_ignore_ascii_case(algorithm.as_str()))
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Zstd => "zstd",
            Self::Brotli => "br",
            Self::Gzip => "gzip",
            Self::Deflate => "deflate",
        }
    }

    pub fn compress(&self, raw: &[u8], level: CompressionLevel) -> std::io::Result<Vec<u8>> {
        match self {
            Self::Zstd => zstd_compress(raw, level),
            Self::Brotli => brotli_compress(raw, level),
            Self::Gzip => {
                let mut encoder = GzEncoder::new(Vec::with_capacity(raw.len()), level.flate());
                encoder.write_all(raw)?;
                encoder.finish()
            }
            Self::Deflate => deflate_compress(raw, level),
        }
    }

    pub fn decompress(&self, data: &[u8]) -> std::io::Result<Vec<u8>> {
        match self {
            Self::Zstd => zstd::decode_all(data),
            Self::Brotli => {
                let mut result = Vec::new();
                brotli::BrotliDecompress(&mut &data[..], &mut result)?;
                Ok(result)
            }
            Self::Gzip => read_to_end(GzDecoder::new(data)),
            Self::Deflate => read_to_end(DeflateDecoder::new(data)),
        }
    }
}

/// Level the content is compressed with
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionLevel {
    /// Balance between the size of the content and the time of the compression
    Default,
    /// The smallest content - the compression takes much more time
    Max,
}

impl CompressionLevel {
    fn flate(&self) -> FlateCompression {
        match self {
            Self::Default => FlateCompression::default(),
            Self::Max => FlateCompression::best(),
        }
    }
}

/// How the content is compressed - it is set by the environment variables
#[derive(Debug, Clone, Copy)]
pub struct CompressionSettings {
    pub algorithm: CompressionAlgorithm,
    pub level: CompressionLevel,
}

impl Default for CompressionSettings {
    fn default() -> Self {
        Self {
            algorithm: CompressionAlgorithm::Zstd,
            level: CompressionLevel::Default,
        }
    }
}

/// Compresses the content we are going to keep in memory - if it makes sense
pub fn try_compress(raw: &[u8], settings: CompressionSettings) -> Option<Vec<u8>> {
    if raw.len() <= COMPRESSION_MIN_SIZE {
        return None;
    }

    let compressed = settings.algorithm.compress(raw, settings.level).ok()?;

    if compressed.len() * EFFICIENCY_DEN <= raw.len() * EFFICIENCY_NUM {
        Some(compressed)
    } else {
        None
    }
}

fn zstd_compress(raw: &[u8], level: CompressionLevel) -> std::io::Result<Vec<u8>> {
    match level {
        CompressionLevel::Default => zstd::encode_all(raw, ZSTD_LEVEL),
        CompressionLevel::Max => {
            let mut compressor = zstd::bulk::Compressor::new(ZSTD_MAX_LEVEL)?;
            compressor.set_parameter(zstd::zstd_safe::CParameter::WindowLog(
                ZSTD_MAX_WINDOW_LOG,
            ))?;
            compressor.compress(raw)
        }
    }
}

fn brotli_compress(raw: &[u8], level: CompressionLevel) -> std::io::Result<Vec<u8>> {
    let (quality, lgwin) = match level {
        CompressionLevel::Default => (BROTLI_QUALITY, BROTLI_WINDOW_LOG),
        CompressionLevel::Max => (BROTLI_MAX_QUALITY, BROTLI_MAX_WINDOW_LOG),
    };

    let params = brotli::enc::BrotliEncoderParams {
        quality,
        lgwin,
        size_hint: raw.len(),
        ..Default::default()
    };

    let mut result = Vec::with_capacity(raw.len());
    brotli::BrotliCompress(&mut &raw[..], &mut result, &params)?;
    Ok(result)
}

pub fn deflate_compress(raw: &[u8], level: CompressionLevel) -> std::io::Result<Vec<u8>> {
    let mut encoder = DeflateEncoder::new(Vec::with_capacity(raw.len()), level.flate());
    encoder.write_all(raw)?;
    encoder.finish()
}

fn read_to_end(mut reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut result = Vec::new();
    reader.read_to_end(&mut result)?;
    Ok(result)
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

    const LEVELS: [CompressionLevel; 2] = [CompressionLevel::Default, CompressionLevel::Max];

    fn compile_text(size: usize) -> Vec<u8> {
        let mut result = Vec::with_capacity(size);
        let mut i: u64 = 0;

        while result.len() < size {
            result.extend_from_slice(format!("const value_{} = {};\n", i, i * 7919).as_bytes());
            i += 1;
        }

        result.truncate(size);
        result
    }

    #[test]
    fn small_file_is_not_compressed() {
        let raw = vec![b'a'; 5 * 1024];
        assert!(try_compress(&raw, CompressionSettings::default()).is_none());
    }

    #[test]
    fn highly_compressible_is_compressed() {
        let raw = vec![b'a'; 50 * 1024];
        let compressed =
            try_compress(&raw, CompressionSettings::default()).expect("should compress");
        assert!(compressed.len() * 100 <= raw.len() * 80);
        assert_eq!(CompressionAlgorithm::Zstd.decompress(&compressed).unwrap(), raw);
    }

    #[test]
    fn incompressible_is_rejected() {
        let mut raw = Vec::with_capacity(50 * 1024);
        let mut seed: u32 = 0x9E37_79B1;
        for _ in 0..50 * 1024 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            raw.push((seed >> 24) as u8);
        }
        assert!(try_compress(&raw, CompressionSettings::default()).is_none());
    }

    #[test]
    fn exactly_threshold_is_not_compressed() {
        let raw = vec![b'a'; COMPRESSION_MIN_SIZE];
        assert!(try_compress(&raw, CompressionSettings::default()).is_none());
    }

    #[test]
    fn every_algorithm_restores_the_content() {
        let raw = compile_text(100 * 1024);

        for algorithm in CompressionAlgorithm::ALL {
            for level in LEVELS {
                let compressed = try_compress(&raw, CompressionSettings { algorithm, level })
                    .expect("should compress");

                assert_eq!(
                    algorithm.decompress(&compressed).unwrap(),
                    raw,
                    "{:?} {:?}",
                    algorithm,
                    level
                );
            }
        }
    }

    /// Deflate with the level 9 is not always smaller than with the default one - so it is not checked
    #[test]
    fn max_level_gives_the_smaller_content() {
        let raw = compile_text(100 * 1024);

        for algorithm in [CompressionAlgorithm::Zstd, CompressionAlgorithm::Brotli] {
            let default = algorithm.compress(&raw, CompressionLevel::Default).unwrap();
            let max = algorithm.compress(&raw, CompressionLevel::Max).unwrap();

            assert!(
                max.len() < default.len(),
                "{:?}: max {} default {}",
                algorithm,
                max.len(),
                default.len()
            );
        }
    }

    /// Browsers refuse to decode the zstd content which requires the window above 8MB
    #[test]
    fn zstd_max_level_is_decoded_by_browsers() {
        // the content of the size above 8MB - repeated, so it takes no time to compress
        let raw = compile_text(64 * 1024).repeat(144);
        let compressed = CompressionAlgorithm::Zstd
            .compress(&raw, CompressionLevel::Max)
            .unwrap();

        let mut decoder = zstd::stream::read::Decoder::new(compressed.as_slice()).unwrap();
        decoder.window_log_max(ZSTD_MAX_WINDOW_LOG).unwrap();

        assert_eq!(read_to_end(decoder).unwrap(), raw);
    }

    #[test]
    fn parses_the_algorithm() {
        assert_eq!(
            CompressionAlgorithm::parse("zstd"),
            Some(CompressionAlgorithm::Zstd)
        );
        assert_eq!(
            CompressionAlgorithm::parse("br"),
            Some(CompressionAlgorithm::Brotli)
        );
        assert_eq!(
            CompressionAlgorithm::parse(" Brotli "),
            Some(CompressionAlgorithm::Brotli)
        );
        assert_eq!(
            CompressionAlgorithm::parse("GZIP"),
            Some(CompressionAlgorithm::Gzip)
        );
        assert_eq!(
            CompressionAlgorithm::parse("deflate"),
            Some(CompressionAlgorithm::Deflate)
        );
        assert_eq!(CompressionAlgorithm::parse("lz4"), None);
    }
}
