use my_http_server::{HttpContext, HttpRequestHeaders};

/// Process the rejected requests are written to the log with
const LOG_PROCESS: &str = "RejectedRequest";

/// Writes the request the service has rejected to the log as a warning: the wrong password,
/// the path which leads out of the root folder. The credentials are never written.
///
/// 'ip' is the whole 'X-Forwarded-For' chain, if the request has it: its first address is set
/// by the client and can be forged - the real one is the address the proxy in front of
/// the service has added. 'socket' is the address the connection came from
pub fn write_rejected_request_log(ctx: &HttpContext, reason: &'static str) {
    let request = &ctx.request;

    let mut log_ctx = my_logger::LogEventCtx::new()
        .add("method", request.method.to_string())
        .add("path", request.http_path.as_str())
        .add("ip", request.get_ip().to_string())
        .add("socket", request.addr.to_string());

    if let Some(user_agent) = request.get_headers().try_get_case_insensitive("user-agent") {
        if let Ok(user_agent) = user_agent.as_str() {
            log_ctx = log_ctx.add("user-agent", user_agent);
        }
    }

    my_logger::LOGGER.write_warning(LOG_PROCESS, reason, log_ctx);
}
