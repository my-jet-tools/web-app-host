use my_http_server::{HttpContext, HttpFailResult, HttpOkResult, HttpOutput, HttpServerMiddleware};

/// Lets through only the requests authenticated with the Basic authentication.
/// The rest get 401 - so the browser asks for the password
pub struct BasicAuthMiddleware {
    password: String,
    www_authenticate: String,
}

impl BasicAuthMiddleware {
    pub fn new(password: String) -> Self {
        Self {
            password,
            www_authenticate: format!(
                "Basic realm=\"{}\", charset=\"UTF-8\"",
                crate::app::APP_CTX.app_name
            ),
        }
    }
}

#[async_trait::async_trait]
impl HttpServerMiddleware for BasicAuthMiddleware {
    async fn handle_request(
        &self,
        ctx: &mut HttpContext,
    ) -> Option<Result<HttpOkResult, HttpFailResult>> {
        if crate::scripts::is_basic_auth_passed(ctx, self.password.as_str()) {
            return None;
        }

        let result = HttpOutput::as_unauthorized(None)
            .add_header("WWW-Authenticate", self.www_authenticate.as_str())
            .into_err(false, false);

        Some(result)
    }
}
