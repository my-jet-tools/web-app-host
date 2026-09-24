use my_http_server::{
    HttpContext, HttpFailResult, HttpOkResult, HttpOutput, HttpRequestHeaders, HttpResultBuilder,
    WebContentType,
};

use crate::models::INDEX_FILE;

use super::{AcceptedEncodings, FilesAccess, FoundFile, NoCache, ServeParams};

/// Full set of headers which forbids any caching of the content
const NO_CACHE_CACHE_CONTROL: &str = "no-store, no-cache, must-revalidate, max-age=0";
const NO_CACHE_PRAGMA: &str = "no-cache";
const NO_CACHE_EXPIRES: &str = "0";

/// Serves the content of the root folder. The root folder is not a setting of the reader -
/// it comes with each request, since in WITH_MOBILE mode it is either the desktop
/// or the mobile folder
pub struct StaticFilesReader {
    files_access: FilesAccess,
    no_cache: NoCache,
}

impl StaticFilesReader {
    pub fn new(enable_files_caching: bool, no_cache_paths: Vec<String>) -> Self {
        Self {
            files_access: FilesAccess::new(enable_files_caching),
            no_cache: NoCache::new(no_cache_paths),
        }
    }

    /// Looks the file up in the folders - in the given order.
    /// If it is nowhere - the index file of the first folder renders the SPA route
    pub async fn find_file(&self, ctx: &HttpContext, folders: &[&str]) -> Option<FoundFile> {
        let uri_path = ctx.request.http_path.as_str();

        for folder in folders {
            if let Some(content) = self.files_access.get(folder, uri_path).await {
                return Some(FoundFile::as_requested(content, uri_path.to_string()));
            }
        }

        let content = self.files_access.get(folders.first()?, INDEX_FILE).await?;

        Some(FoundFile::as_index_file(content))
    }

    pub fn compile_response(
        &self,
        ctx: &HttpContext,
        found_file: &FoundFile,
        params: &ServeParams<'_>,
    ) -> Option<Result<HttpOkResult, HttpFailResult>> {
        let disable_caching =
            found_file.is_index_file || self.no_cache.marked_as_no_cache(&ctx.request.http_path);

        // etag makes no sense for the content the client is not allowed to store
        let etag = if disable_caching {
            None
        } else {
            match found_file.content.get_etag() {
                Ok(etag) => Some(etag),
                Err(err) => return Some(Err(compile_error(found_file.uri_path.as_str(), err))),
            }
        };

        if let Some(etag) = etag.as_ref() {
            if has_the_same_etag(ctx, etag.as_str()) {
                return Some(HttpOutput::as_not_modified().into_ok_result(false));
            }
        }

        let (body, encoding) = match found_file
            .content
            .compile_body(AcceptedEncodings::from_request(ctx))
        {
            Ok(result) => result,
            Err(err) => return Some(Err(compile_error(found_file.uri_path.as_str(), err))),
        };

        let mut builder = HttpOutput::from_builder()
            .set_content_type_opt(WebContentType::detect_by_extension(
                found_file.uri_path.as_str(),
            ))
            .add_header("Vary", "Accept-Encoding");

        builder = apply_cache_headers(builder, etag, params.vary_by_user_agent);

        if let Some(encoding) = encoding.header_value() {
            builder = builder.add_header("Content-Encoding", encoding);
        }

        Some(builder.set_content(body).into_ok_result(false))
    }
}

/// Puts the caching headers on the response. No etag means - the content must not be cached
pub fn apply_cache_headers(
    builder: HttpResultBuilder,
    etag: Option<String>,
    vary_by_user_agent: bool,
) -> HttpResultBuilder {
    let builder = if vary_by_user_agent {
        builder.add_header("Vary", "User-Agent")
    } else {
        builder
    };

    match etag {
        Some(etag) => builder
            .add_header("ETag", etag)
            .add_header("Cache-Control", "no-cache"),
        None => builder
            .add_header("Cache-Control", NO_CACHE_CACHE_CONTROL)
            .add_header("Pragma", NO_CACHE_PRAGMA)
            .add_header("Expires", NO_CACHE_EXPIRES),
    }
}

pub fn compile_error(uri_path: &str, err: std::io::Error) -> HttpFailResult {
    HttpFailResult::as_fatal_error(format!(
        "Can not prepare the content of '{}'. Err: {}",
        uri_path, err
    ))
}

fn has_the_same_etag(ctx: &HttpContext, etag: &str) -> bool {
    let Some(header) = ctx
        .request
        .get_headers()
        .try_get_case_insensitive("if-none-match")
    else {
        return false;
    };

    let Ok(header) = header.as_str() else {
        return false;
    };

    header == etag
}
