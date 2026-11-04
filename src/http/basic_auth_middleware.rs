use std::time::Duration;

use my_http_server::{HttpContext, HttpFailResult, HttpOkResult, HttpOutput, HttpServerMiddleware};

use crate::scripts::BasicAuthResult;

/// The answer to the wrong password is delayed: whoever guesses the password gets the answer
/// to each guess no faster than this. Only the request with the wrong password waits -
/// the server serves the others meanwhile
const WRONG_PASSWORD_DELAY: Duration = Duration::from_secs(1);

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
        match crate::scripts::check_basic_auth(ctx, self.password.as_str()) {
            BasicAuthResult::Passed => return None,
            // the browser asks the user for the password after this 401 - it is not an attempt yet
            BasicAuthResult::NoCredentials => {}
            BasicAuthResult::Failed => {
                crate::scripts::write_rejected_request_log(
                    ctx,
                    "Wrong password of the Basic authentication",
                );

                tokio::time::sleep(WRONG_PASSWORD_DELAY).await;
            }
        }

        let result = HttpOutput::as_unauthorized(None)
            .add_header("WWW-Authenticate", self.www_authenticate.as_str())
            .into_err(false, false);

        Some(result)
    }
}
