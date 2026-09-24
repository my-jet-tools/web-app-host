use my_http_server::{HttpFailResult, HttpOkResult, HttpOutput, WebContentType};

use crate::static_files::{apply_cache_headers, compile_error, FoundFile};

/// Replaces the version placeholders in the content of the file we have found.
/// The result is compiled per request - so it is never cached
pub fn inject_version(
    found_file: &FoundFile,
    vary_by_user_agent: bool,
) -> Option<Result<HttpOkResult, HttpFailResult>> {
    let content = match found_file.content.get_raw() {
        Ok(content) => content,
        Err(err) => return Some(Err(compile_error(found_file.uri_path.as_str(), err))),
    };

    let content = String::from_utf8_lossy(content.as_slice());

    let content = content
        .replace("${APP_VERSION}", &crate::app::APP_CTX.app_version)
        .replace("${APP_COMPILE_TIME}", &crate::app::APP_CTX.compile_time);

    let builder = HttpOutput::from_builder().set_content_type_opt(
        WebContentType::detect_by_extension(found_file.uri_path.as_str()),
    );

    let result = apply_cache_headers(builder, None, vary_by_user_agent)
        .set_content(content.into_bytes())
        .into_ok_result(false);

    Some(result)
}
