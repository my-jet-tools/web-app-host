use crate::models::INDEX_FILE;

/// Path does not lead out of the root folder: '.' and '..' segments are not allowed,
/// backslash is not allowed as well - since it separates the segments on Windows.
/// The path comes as it is in the request line - so the percent-encoded dots
/// and separators are checked as the plain ones
pub fn is_safe_uri_path(uri_path: &str) -> bool {
    if uri_path.contains('%') {
        return has_no_dot_segments(decode_dots_and_separators(uri_path).as_str());
    }

    has_no_dot_segments(uri_path)
}

fn has_no_dot_segments(uri_path: &str) -> bool {
    if uri_path.contains('\\') {
        return false;
    }

    uri_path
        .split('/')
        .all(|segment| segment != "." && segment != "..")
}

/// Decodes only the characters the segments are made of: '%2e', '%2f' and '%5c'
fn decode_dots_and_separators(uri_path: &str) -> String {
    let mut result = String::with_capacity(uri_path.len());
    let mut rest = uri_path;

    while let Some(pos) = rest.find('%') {
        result.push_str(&rest[..pos]);

        match rest.get(pos + 1..pos + 3).and_then(decode_path_char) {
            Some(decoded) => {
                result.push(decoded);
                rest = &rest[pos + 3..];
            }
            None => {
                result.push('%');
                rest = &rest[pos + 1..];
            }
        }
    }

    result.push_str(rest);
    result
}

fn decode_path_char(hex: &str) -> Option<char> {
    if hex.eq_ignore_ascii_case("2e") {
        return Some('.');
    }

    if hex.eq_ignore_ascii_case("2f") {
        return Some('/');
    }

    if hex.eq_ignore_ascii_case("5c") {
        return Some('\\');
    }

    None
}

/// Index file of the folder the path points to - '/es/' and '/es' are both served by '/es/index.html'
pub fn compile_folder_index_path(uri_path: &str) -> String {
    format!("{}{}", uri_path.trim_end_matches('/'), INDEX_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_paths_inside_the_root_folder() {
        assert!(is_safe_uri_path("/"));
        assert!(is_safe_uri_path("/index.html"));
        assert!(is_safe_uri_path("/es/legal/"));
        assert!(is_safe_uri_path("/assets/app.min.js"));
        assert!(is_safe_uri_path("/..hidden/file..txt"));
        assert!(is_safe_uri_path("/my%20file.txt"));
        assert!(is_safe_uri_path("/%2e%2ehidden/100%"));
    }

    #[test]
    fn rejects_the_paths_leading_out_of_the_root_folder() {
        assert!(!is_safe_uri_path("/.."));
        assert!(!is_safe_uri_path("/../secret.txt"));
        assert!(!is_safe_uri_path("/es/../../secret.txt"));
        assert!(!is_safe_uri_path("/es/./index.html"));
        assert!(!is_safe_uri_path("/..\\secret.txt"));
        assert!(!is_safe_uri_path("/../../etc/hosts"));
        assert!(!is_safe_uri_path("/./etc/hosts"));
    }

    #[test]
    fn rejects_the_percent_encoded_paths_leading_out_of_the_root_folder() {
        assert!(!is_safe_uri_path("/%2e%2e/%2E%2E/etc/hosts"));
        assert!(!is_safe_uri_path("/.%2e/etc/hosts"));
        assert!(!is_safe_uri_path("/%2e/index.html"));
        assert!(!is_safe_uri_path("/es/..%2f..%2Fetc/hosts"));
        assert!(!is_safe_uri_path("/es/..%5c..%5Cetc/hosts"));
    }

    #[test]
    fn compiles_the_folder_index_path() {
        assert_eq!(compile_folder_index_path("/"), "/index.html");
        assert_eq!(compile_folder_index_path("/es/"), "/es/index.html");
        assert_eq!(compile_folder_index_path("/es"), "/es/index.html");
        assert_eq!(
            compile_folder_index_path("/es/legal/"),
            "/es/legal/index.html"
        );
    }
}
