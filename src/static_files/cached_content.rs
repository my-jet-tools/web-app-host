use super::{calc_etag, deflate_compress, zstd_decompress, AcceptedEncodings, ResponseEncoding};

/// Content of the file - as we keep it in memory. It is kept zstd compressed,
/// so the client which accepts zstd gets it as it is - without any work on our side
#[derive(Clone)]
pub struct CachedContent {
    pub data: Vec<u8>,
    pub is_zstd: bool,
    pub etag: Option<String>,
}

impl CachedContent {
    pub fn get_raw(&self) -> std::io::Result<Vec<u8>> {
        if self.is_zstd {
            return zstd_decompress(&self.data);
        }

        Ok(self.data.clone())
    }

    pub fn get_etag(&self) -> std::io::Result<String> {
        if let Some(etag) = self.etag.as_ref() {
            return Ok(etag.clone());
        }

        Ok(calc_etag(self.get_raw()?.as_slice()))
    }

    /// Compiles the body we are sending - according to the encodings the client accepts
    pub fn compile_body(
        &self,
        accepted: AcceptedEncodings,
    ) -> std::io::Result<(Vec<u8>, ResponseEncoding)> {
        if !self.is_zstd {
            return Ok((self.data.clone(), ResponseEncoding::Identity));
        }

        if accepted.zstd {
            return Ok((self.data.clone(), ResponseEncoding::Zstd));
        }

        let raw = zstd_decompress(&self.data)?;

        if accepted.deflate {
            return Ok((deflate_compress(raw.as_slice())?, ResponseEncoding::Deflate));
        }

        Ok((raw, ResponseEncoding::Identity))
    }
}
