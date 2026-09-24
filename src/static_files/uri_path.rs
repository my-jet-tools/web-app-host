use crate::models::INDEX_FILE;

/// Path does not lead out of the root folder: '.' and '..' segments are not allowed,
/// backslash is not allowed as well - since it separates the segments on Windows
pub fn is_safe_uri_path(uri_path: &str) -> bool {
    if uri_path.contains('\\') {
        return false;
    }

    uri_path
        .split('/')
        .all(|segment| segment != "." && segment != "..")
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
    }

    #[test]
    fn rejects_the_paths_leading_out_of_the_root_folder() {
        assert!(!is_safe_uri_path("/.."));
        assert!(!is_safe_uri_path("/../secret.txt"));
        assert!(!is_safe_uri_path("/es/../../secret.txt"));
        assert!(!is_safe_uri_path("/es/./index.html"));
        assert!(!is_safe_uri_path("/..\\secret.txt"));
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
