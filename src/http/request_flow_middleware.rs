use my_http_server::{HttpContext, HttpFailResult, HttpOkResult, HttpServerMiddleware};

use crate::static_files::{is_safe_uri_path, StaticFilesReader};

pub struct RequestFlowMiddleware {
    static_files: StaticFilesReader,
}

impl RequestFlowMiddleware {
    pub fn new(static_files: StaticFilesReader) -> Self {
        Self { static_files }
    }
}

#[async_trait::async_trait]
impl HttpServerMiddleware for RequestFlowMiddleware {
    async fn handle_request(
        &self,
        ctx: &mut HttpContext,
    ) -> Option<Result<HttpOkResult, HttpFailResult>> {
        // path which leads out of the root folder is not served at all - so nobody can read
        // the files of the system
        if !is_safe_uri_path(ctx.request.http_path.as_str()) {
            return Some(Err(HttpFailResult::as_forbidden(Some("Forbidden"))));
        }

        if crate::app::APP_CTX.with_mobile {
            return crate::flows::handle_with_mobile(&self.static_files, ctx).await;
        }

        crate::flows::handle_no_mobile(&self.static_files, ctx).await
    }
}
