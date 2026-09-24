use std::sync::Arc;

use rust_extensions::AppStates;

pub const DEFAULT_WWW_ROOT: &str = "./wwwroot";

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const APP_NAME: &str = env!("CARGO_PKG_NAME");

lazy_static::lazy_static! {
    pub static ref APP_CTX: Arc<AppContext> = {
        Arc::new(AppContext::new())
    };
}

pub struct AppContext {
    pub app_states: Arc<AppStates>,
    pub app_name: String,
    pub app_version: String,
    pub compile_time: String,

    pub file_to_inject: Option<String>,

    pub with_mobile: bool,

    /// Root folder of the content - when WITH_MOBILE is disabled
    pub www_root: String,
    /// Root folder of the content of the desktop device - when WITH_MOBILE is enabled
    pub desktop_www_root: String,
    /// Root folder of the content of the mobile device - when WITH_MOBILE is enabled
    pub mobile_www_root: String,

    /// Password of the Basic authentication. If it is set - only the authenticated requests are served
    pub basic_auth_password: Option<String>,
    /// robots.txt which forbids the crawlers to crawl the site is served
    pub no_robots: bool,
}

impl AppContext {
    pub fn new() -> Self {
        let www_root = get_www_root();

        Self {
            app_states: Arc::new(AppStates::create_initialized()),
            app_name: get_app_name(),
            app_version: get_app_version(),
            compile_time: get_compile_time(),
            file_to_inject: get_file_to_version_injection(),
            with_mobile: get_with_mobile(),
            desktop_www_root: format!("{}/desktop", www_root),
            mobile_www_root: format!("{}/mobile", www_root),
            www_root,
            basic_auth_password: get_basic_auth_password(),
            no_robots: get_no_robots(),
        }
    }

    /// Version placeholders are injected into the configured file only
    pub fn has_to_inject_version(&self, uri_path: &str) -> bool {
        let Some(file_to_inject) = self.file_to_inject.as_ref() else {
            return false;
        };

        rust_extensions::str_utils::compare_strings_case_insensitive(
            file_to_inject.as_str(),
            uri_path,
        )
    }
}

fn get_app_name() -> String {
    if let Ok(app_name) = std::env::var("BUILD_NAME") {
        app_name
    } else {
        APP_NAME.to_string()
    }
}

fn get_app_version() -> String {
    if let Ok(app_version) = std::env::var("BUILD_VERSION") {
        app_version
    } else {
        APP_VERSION.to_string()
    }
}

fn get_compile_time() -> String {
    if let Ok(app_version) = std::env::var("COMPILE_TIME") {
        app_version
    } else {
        "".to_string()
    }
}

fn get_file_to_version_injection() -> Option<String> {
    if let Ok(file) = std::env::var("FILE_TO_VERSION_INJECTION") {
        if file.starts_with('/') {
            Some(file)
        } else {
            Some(format!("/{}", file))
        }
    } else {
        None
    }
}

/// Root folder of the content. With WITH_MOBILE enabled the content is served
/// from the '{www_root}/desktop' and '{www_root}/mobile' folders of it
fn get_www_root() -> String {
    let result = match std::env::var("WWW_ROOT") {
        Ok(value) => value.trim().to_string(),
        Err(_) => DEFAULT_WWW_ROOT.to_string(),
    };

    if result.is_empty() {
        return DEFAULT_WWW_ROOT.to_string();
    }

    match result.strip_suffix('/') {
        Some(result) => result.to_string(),
        None => result,
    }
}

fn get_with_mobile() -> bool {
    match std::env::var("WITH_MOBILE") {
        Ok(value) => value.trim() == "1",
        Err(_) => false,
    }
}

/// Variable with the dash in the name can not be exported from the shell -
/// so the BASIC_AUTH spelling is accepted as well
fn get_basic_auth_password() -> Option<String> {
    let password = std::env::var("BASIC-AUTH")
        .or_else(|_| std::env::var("BASIC_AUTH"))
        .ok()?;

    if password.is_empty() {
        return None;
    }

    Some(password)
}

fn get_no_robots() -> bool {
    match std::env::var("NO_ROBOTS") {
        Ok(value) => {
            let value = value.trim();
            value == "1" || value.eq_ignore_ascii_case("true")
        }
        Err(_) => false,
    }
}
