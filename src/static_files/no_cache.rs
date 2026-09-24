use my_http_server::HttpPath;

/// Paths whose content the client is not allowed to cache.
///
/// Path is compared segment by segment: the case and the trailing slash do not matter,
/// the query string is not a part of the [HttpPath] at all - so '/' matches '/?ver=123'
#[derive(Default)]
pub struct NoCache {
    paths: Vec<HttpPath>,
}

impl NoCache {
    pub fn new(paths: Vec<String>) -> Self {
        Self {
            paths: paths
                .into_iter()
                .map(|path| HttpPath::from_string(compile_path(path)))
                .collect(),
        }
    }

    pub fn marked_as_no_cache(&self, path: &HttpPath) -> bool {
        self.paths.iter().any(|itm| itm.is_the_same_to(path))
    }
}

fn compile_path(mut path: String) -> String {
    if let Some(index) = path.find(['?', '#']) {
        path.truncate(index);
    }

    if path.starts_with('/') {
        return path;
    }

    format!("/{}", path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create(paths: &[&str]) -> NoCache {
        NoCache::new(paths.iter().map(|itm| itm.to_string()).collect())
    }

    #[test]
    fn matches_the_root_path_only() {
        let no_cache = create(&["/"]);

        assert!(no_cache.marked_as_no_cache(&HttpPath::from_str("/")));
        assert!(!no_cache.marked_as_no_cache(&HttpPath::from_str("/index.html")));
    }

    #[test]
    fn ignores_case_and_trailing_slash() {
        let no_cache = create(&["/Config.json"]);
        assert!(no_cache.marked_as_no_cache(&HttpPath::from_str("/config.JSON")));

        let no_cache = create(&["/my/path"]);
        assert!(no_cache.marked_as_no_cache(&HttpPath::from_str("/my/path/")));
        assert!(!no_cache.marked_as_no_cache(&HttpPath::from_str("/my/path/sub")));
    }

    #[test]
    fn adds_the_leading_slash() {
        let no_cache = create(&["config.json"]);
        assert!(no_cache.marked_as_no_cache(&HttpPath::from_str("/config.json")));
    }

    #[test]
    fn nothing_is_registered() {
        let no_cache = NoCache::default();
        assert!(!no_cache.marked_as_no_cache(&HttpPath::from_str("/")));
    }
}
