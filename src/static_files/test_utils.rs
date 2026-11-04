use std::io::Read;

use super::CompressionAlgorithm;

/// Folder with the files of the test. It is removed when the test is over
pub struct TestFolder {
    pub path: std::path::PathBuf,
}

impl TestFolder {
    pub fn new(test_name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("web-app-host-{}-{}", test_name, std::process::id()));

        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();

        Self { path }
    }

    pub fn add_file(&self, file_name: &str, content: impl AsRef<[u8]>) -> &Self {
        let file_name = self.path.join(file_name);
        std::fs::create_dir_all(file_name.parent().unwrap()).unwrap();
        std::fs::write(file_name, content).unwrap();
        self
    }

    pub fn get_folder(&self, name: &str) -> String {
        self.path.join(name).to_str().unwrap().to_string()
    }
}

impl Drop for TestFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Text which is compressed the way the real js and css files are
pub fn compile_text(size: usize) -> Vec<u8> {
    let mut result = Vec::with_capacity(size);
    let mut i: u64 = 0;

    while result.len() < size {
        result.extend_from_slice(format!("const value_{} = {};\n", i, i * 7919).as_bytes());
        i += 1;
    }

    result.truncate(size);
    result
}

/// Restores the compressed content - the way the browser does
pub fn decompress(algorithm: CompressionAlgorithm, data: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();

    match algorithm {
        CompressionAlgorithm::Brotli => {
            brotli::BrotliDecompress(&mut &data[..], &mut result).unwrap();
        }
        CompressionAlgorithm::Gzip => {
            flate2::read::GzDecoder::new(data)
                .read_to_end(&mut result)
                .unwrap();
        }
    }

    result
}
