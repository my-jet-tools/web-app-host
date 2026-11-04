use my_http_server::{HttpFailResult, HttpOkResult, HttpOutput};

use crate::static_files::{
    apply_cache_headers, apply_security_headers, compile_error, compile_vary, detect_content_type,
    FoundFile, ServeParams,
};

/// Replaces the version placeholders in the content of the file we have found.
/// The placeholders are in the raw file - so it is sent uncompressed, whatever the client
/// accepts. The result is compiled per request - so it is never cached. It does not wait
/// for the file to be prepared in the background: neither the etag nor the copies are used
pub async fn inject_version(
    found_file: &FoundFile<'_>,
    params: &ServeParams<'_>,
) -> Option<Result<HttpOkResult, HttpFailResult>> {
    let content = match tokio::fs::read(found_file.file.raw_path.as_str()).await {
        Ok(content) => content,
        Err(err) => return Some(Err(compile_error(found_file.uri_path.as_str(), err))),
    };

    let content = String::from_utf8_lossy(content.as_slice());

    let content = content
        .replace("${APP_VERSION}", &crate::app::APP_CTX.app_version)
        .replace("${APP_COMPILE_TIME}", &crate::app::APP_CTX.compile_time);

    let builder = HttpOutput::from_builder()
        .set_content_type_opt(detect_content_type(found_file.uri_path.as_str()));

    let builder = apply_security_headers(builder, params.allow_framing);

    // the injected file is always sent uncompressed - it does not depend on 'Accept-Encoding'
    let result = apply_cache_headers(
        builder,
        None,
        compile_vary(false, params.vary_by_user_agent),
    )
    .set_content(content.into_bytes())
    .into_ok_result(false);

    Some(result)
}
