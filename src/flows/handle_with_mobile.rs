use my_http_server::{HttpContext, HttpFailResult, HttpOkResult};

use crate::static_files::{ServeParams, StaticFilesReader};

/// WITH_MOBILE is enabled - the root folder of the content is the desktop or the mobile one,
/// detected by the UserAgent. If the file is not found in the device folder -
/// we are looking at the other one
pub async fn handle_with_mobile(
    static_files: &StaticFilesReader,
    ctx: &HttpContext,
) -> Option<Result<HttpOkResult, HttpFailResult>> {
    let device_folder = crate::scripts::detect_device_folder(ctx);

    let params = ServeParams {
        folders: &[
            device_folder.www_root(),
            device_folder.fallback().www_root(),
        ],
        vary_by_user_agent: true,
    };

    let found_file = static_files
        .find_file(ctx.request.http_path.as_str(), params.folders)
        .await?;

    if crate::app::APP_CTX.has_to_inject_version(found_file.uri_path.as_str()) {
        return crate::scripts::inject_version(&found_file, params.vary_by_user_agent);
    }

    static_files.compile_response(ctx, &found_file, &params)
}
