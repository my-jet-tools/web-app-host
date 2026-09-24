use my_http_server::{HttpContext, HttpRequestHeaders};
use rust_common::user_agent::{DeviceType, UserAgentString};

use crate::models::DeviceFolder;

/// Detects the folder we are serving the content from - by the UserAgent header.
/// If we can not detect it - Desktop is used
pub fn detect_device_folder(ctx: &HttpContext) -> DeviceFolder {
    let Some(user_agent) = ctx
        .request
        .get_headers()
        .try_get_case_insensitive("user-agent")
    else {
        return DeviceFolder::Desktop;
    };

    let Ok(user_agent) = user_agent.as_str() else {
        return DeviceFolder::Desktop;
    };

    match UserAgentString::new(user_agent).get_device_type() {
        DeviceType::Mobile => DeviceFolder::Mobile,
        DeviceType::Tablet => DeviceFolder::Desktop,
        DeviceType::Desktop => DeviceFolder::Desktop,
    }
}
