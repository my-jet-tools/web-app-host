use std::collections::HashMap;

use tokio::sync::RwLock;

use super::{calc_etag, try_compress, CachedContent, CompressionSettings};

/// Reads the files from the disk - with the optional in-memory caching.
/// Caching is about the server side only - it has nothing to do with the caching headers
pub struct FilesAccess {
    cache: RwLock<HashMap<String, CachedContent>>,
    enable_caching: bool,
    compression: CompressionSettings,
}

impl FilesAccess {
    pub fn new(enable_caching: bool, compression: CompressionSettings) -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
            enable_caching,
            compression,
        }
    }

    pub async fn get(&self, folder: &str, uri_path: &str) -> Option<CachedContent> {
        let file_name = compile_file_name(folder, uri_path);

        if self.enable_caching {
            let cache = self.cache.read().await;
            if let Some(content) = cache.get(file_name.as_str()) {
                return Some(content.clone());
            }
        }

        let raw = tokio::fs::read(file_name.as_str()).await.ok()?;

        if !self.enable_caching {
            return Some(CachedContent {
                data: raw,
                compression: None,
                etag: None,
            });
        }

        let compression = self.compression;

        // the maximum compression level takes seconds for the big file -
        // it must not block the threads of the async runtime
        let entry = tokio::task::spawn_blocking(move || compile_cached_content(raw, compression))
            .await
            .ok()?;

        let mut cache = self.cache.write().await;
        cache.insert(file_name, entry.clone());

        Some(entry)
    }
}

fn compile_cached_content(raw: Vec<u8>, compression: CompressionSettings) -> CachedContent {
    let etag = Some(calc_etag(&raw));

    match try_compress(&raw, compression) {
        Some(compressed) => CachedContent {
            data: compressed,
            compression: Some(compression.algorithm),
            etag,
        },
        None => CachedContent {
            data: raw,
            compression: None,
            etag,
        },
    }
}

pub fn compile_file_name(folder: &str, uri_path: &str) -> String {
    if uri_path.starts_with('/') {
        format!("{}{}", folder, uri_path)
    } else {
        format!("{}/{}", folder, uri_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_the_file_name() {
        assert_eq!(
            compile_file_name("./wwwroot", "/index.html"),
            "./wwwroot/index.html"
        );
        assert_eq!(
            compile_file_name("./wwwroot", "index.html"),
            "./wwwroot/index.html"
        );
    }
}
