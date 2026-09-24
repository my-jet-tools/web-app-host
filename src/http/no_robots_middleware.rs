use my_http_server::{
    HttpContext, HttpFailResult, HttpOkResult, HttpOutput, HttpServerMiddleware, WebContentType,
};

use crate::models::ROBOTS_FILE;

/// Forbids all the crawlers to crawl the whole site
const NO_ROBOTS_CONTENT: &str = "User-agent: *\nDisallow: /\n";

/// Serves robots.txt which forbids the crawling - instead of the one from the root folder
pub struct NoRobotsMiddleware;

#[async_trait::async_trait]
impl HttpServerMiddleware for NoRobotsMiddleware {
    async fn handle_request(
        &self,
        ctx: &mut HttpContext,
    ) -> Option<Result<HttpOkResult, HttpFailResult>> {
        if !rust_extensions::str_utils::compare_strings_case_insensitive(
            ctx.request.http_path.as_str(),
            ROBOTS_FILE,
        ) {
            return None;
        }

        let result = HttpOutput::from_builder()
            .set_content_type(WebContentType::Text)
            .set_content(NO_ROBOTS_CONTENT.as_bytes().to_vec())
            .into_ok_result(false);

        Some(result)
    }
}
