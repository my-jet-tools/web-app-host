use my_http_server::{HttpContext, HttpFailResult, HttpOkResult};

use crate::static_files::{ServeParams, StaticFilesReader};

/// WITH_MOBILE is disabled - the root folder of the content is the wwwroot one
pub async fn handle_no_mobile(
    static_files: &StaticFilesReader,
    ctx: &HttpContext,
) -> Option<Result<HttpOkResult, HttpFailResult>> {
    let params = ServeParams {
        folders: &[crate::app::APP_CTX.www_root.as_str()],
        vary_by_user_agent: false,
    };

    let found_file = static_files
        .find_file(ctx.request.http_path.as_str(), params.folders)
        .await?;

    if crate::app::APP_CTX.has_to_inject_version(found_file.uri_path.as_str()) {
        return crate::scripts::inject_version(&found_file, params.vary_by_user_agent);
    }

    static_files.compile_response(ctx, &found_file, &params)
}
