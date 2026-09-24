use base64::Engine;
use my_http_server::{HttpContext, HttpRequestHeaders};
use sha2::{Digest, Sha256};

const BASIC_SCHEME: &str = "basic ";

/// Checks the credentials of the Basic authentication. Only the password is checked -
/// any login with the right password is accepted
pub fn is_basic_auth_passed(ctx: &HttpContext, password: &str) -> bool {
    let Some(header) = ctx
        .request
        .get_headers()
        .try_get_case_insensitive("authorization")
    else {
        return false;
    };

    let Ok(header) = header.as_str() else {
        return false;
    };

    has_the_password(header, password)
}

fn has_the_password(authorization_header: &str, password: &str) -> bool {
    let authorization_header = authorization_header.trim();

    // the scheme name is case insensitive
    let Some(scheme) = authorization_header.get(..BASIC_SCHEME.len()) else {
        return false;
    };

    if !scheme.eq_ignore_ascii_case(BASIC_SCHEME) {
        return false;
    }

    let Ok(credentials) = base64::engine::general_purpose::STANDARD
        .decode(authorization_header[BASIC_SCHEME.len()..].trim())
    else {
        return false;
    };

    // credentials are 'login:password' - the login can not contain ':', the password can
    let Some(index) = credentials.iter().position(|itm| *itm == b':') else {
        return false;
    };

    is_the_same_password(&credentials[index + 1..], password.as_bytes())
}

/// Hashes are compared - so the time of the comparison tells nothing about the password
fn is_the_same_password(src: &[u8], password: &[u8]) -> bool {
    let src = Sha256::digest(src);
    let password = Sha256::digest(password);

    src.iter()
        .zip(password.iter())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_header(credentials: &str) -> String {
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(credentials)
        )
    }

    #[test]
    fn accepts_the_right_password_with_any_login() {
        assert!(has_the_password(&create_header("admin:secret"), "secret"));
        assert!(has_the_password(&create_header("other:secret"), "secret"));
        assert!(has_the_password(&create_header(":secret"), "secret"));
    }

    #[test]
    fn rejects_the_wrong_password() {
        assert!(!has_the_password(&create_header("admin:wrong"), "secret"));
        assert!(!has_the_password(&create_header("admin:"), "secret"));
        assert!(!has_the_password(&create_header("admin:secret2"), "secret"));
    }

    #[test]
    fn password_can_contain_colon() {
        assert!(has_the_password(&create_header("admin:a:b"), "a:b"));
    }

    #[test]
    fn scheme_is_case_insensitive() {
        let header = create_header("admin:secret").replace("Basic", "bAsIc");
        assert!(has_the_password(&header, "secret"));
    }

    #[test]
    fn rejects_invalid_headers() {
        assert!(!has_the_password("", "secret"));
        assert!(!has_the_password("Basic", "secret"));
        assert!(!has_the_password("Basic !!!", "secret"));
        assert!(!has_the_password(&create_header("no-colon"), "secret"));
        assert!(!has_the_password(
            &create_header("admin:secret").replace("Basic", "Bearer"),
            "secret"
        ));
    }
}
