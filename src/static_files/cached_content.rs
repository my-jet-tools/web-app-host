use super::{
    calc_etag, deflate_compress, AcceptedEncodings, CompressionAlgorithm, CompressionLevel,
    ResponseEncoding,
};

/// Content of the file - as we keep it in memory. It is kept compressed,
/// so the client which accepts the algorithm gets it as it is - without any work on our side
#[derive(Clone)]
pub struct CachedContent {
    pub data: Vec<u8>,
    /// Algorithm the data is compressed with. None - the data is kept as it is
    pub compression: Option<CompressionAlgorithm>,
    pub etag: Option<String>,
}

impl CachedContent {
    pub fn get_raw(&self) -> std::io::Result<Vec<u8>> {
        if let Some(algorithm) = self.compression {
            return algorithm.decompress(&self.data);
        }

        Ok(self.data.clone())
    }

    pub fn get_etag(&self) -> std::io::Result<String> {
        if let Some(etag) = self.etag.as_ref() {
            return Ok(etag.clone());
        }

        Ok(calc_etag(self.get_raw()?.as_slice()))
    }

    /// Compiles the body we are sending - according to the encodings the client accepts.
    /// The client which does not accept the algorithm of the cached content gets it deflated
    /// on each request - so it is done with the default level, the maximum one costs much
    /// more CPU and gives the same size
    pub fn compile_body(
        &self,
        accepted: AcceptedEncodings,
    ) -> std::io::Result<(Vec<u8>, ResponseEncoding)> {
        let Some(algorithm) = self.compression else {
            return Ok((self.data.clone(), ResponseEncoding::Identity));
        };

        if accepted.accepts(algorithm) {
            return Ok((self.data.clone(), ResponseEncoding::Compressed(algorithm)));
        }

        let raw = algorithm.decompress(&self.data)?;

        if accepted.deflate {
            return Ok((
                deflate_compress(raw.as_slice(), CompressionLevel::Default)?,
                ResponseEncoding::Compressed(CompressionAlgorithm::Deflate),
            ));
        }

        Ok((raw, ResponseEncoding::Identity))
    }
}
