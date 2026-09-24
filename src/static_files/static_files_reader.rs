use my_http_server::{
    HttpContext, HttpFailResult, HttpOkResult, HttpOutput, HttpPath, HttpRequestHeaders,
    HttpResultBuilder, WebContentType,
};

use crate::models::INDEX_FILE;

use super::{
    compile_folder_index_path, is_safe_uri_path, AcceptedEncodings, FilesAccess, FoundFile,
    NoCache, ServeParams,
};

/// Full set of headers which forbids any caching of the content
const NO_CACHE_CACHE_CONTROL: &str = "no-store, no-cache, must-revalidate, max-age=0";
const NO_CACHE_PRAGMA: &str = "no-cache";
const NO_CACHE_EXPIRES: &str = "0";

/// Serves the content of the root folder. The root folder is not a setting of the reader -
/// it comes with each request, since in WITH_MOBILE mode it is either the desktop
/// or the mobile folder
pub struct StaticFilesReader {
    files_access: FilesAccess,
    no_cache: NoCache,
}

impl StaticFilesReader {
    pub fn new(enable_files_caching: bool, no_cache_paths: Vec<String>) -> Self {
        Self {
            files_access: FilesAccess::new(enable_files_caching),
            no_cache: NoCache::new(no_cache_paths),
        }
    }

    /// Looks the file up in the folders - in the given order. If the path points to the folder -
    /// its index file is served. If it is nowhere - the index file of the first folder
    /// renders the SPA route
    pub async fn find_file(&self, uri_path: &str, folders: &[&str]) -> Option<FoundFile> {
        // path which leads out of the root folder is never looked up - it is the SPA route
        if is_safe_uri_path(uri_path) {
            for folder in folders {
                if let Some(found_file) = self.find_in_folder(folder, uri_path).await {
                    return Some(found_file);
                }
            }
        }

        let content = self.files_access.get(folders.first()?, INDEX_FILE).await?;

        Some(FoundFile::as_index_file(content))
    }

    /// Path which ends with '/' points to the folder. Path without it points to the file -
    /// or to the folder, if there is no such file
    async fn find_in_folder(&self, folder: &str, uri_path: &str) -> Option<FoundFile> {
        if !uri_path.ends_with('/') {
            if let Some(content) = self.files_access.get(folder, uri_path).await {
                return Some(FoundFile::as_requested(content, uri_path.to_string()));
            }
        }

        // the index file exists only if the path is the folder on the disk
        let index_path = compile_folder_index_path(uri_path);
        let content = self.files_access.get(folder, index_path.as_str()).await?;

        Some(FoundFile::as_requested(content, index_path))
    }

    pub fn compile_response(
        &self,
        ctx: &HttpContext,
        found_file: &FoundFile,
        params: &ServeParams<'_>,
    ) -> Option<Result<HttpOkResult, HttpFailResult>> {
        let disable_caching = self.has_to_disable_caching(&ctx.request.http_path, found_file);

        // etag makes no sense for the content the client is not allowed to store
        let etag = if disable_caching {
            None
        } else {
            match found_file.content.get_etag() {
                Ok(etag) => Some(etag),
                Err(err) => return Some(Err(compile_error(found_file.uri_path.as_str(), err))),
            }
        };

        if let Some(etag) = etag.as_ref() {
            if has_the_same_etag(ctx, etag.as_str()) {
                return Some(HttpOutput::as_not_modified().into_ok_result(false));
            }
        }

        let (body, encoding) = match found_file
            .content
            .compile_body(AcceptedEncodings::from_request(ctx))
        {
            Ok(result) => result,
            Err(err) => return Some(Err(compile_error(found_file.uri_path.as_str(), err))),
        };

        let mut builder = HttpOutput::from_builder()
            .set_content_type_opt(WebContentType::detect_by_extension(
                found_file.uri_path.as_str(),
            ))
            .add_header("Vary", "Accept-Encoding");

        builder = apply_cache_headers(builder, etag, params.vary_by_user_agent);

        if let Some(encoding) = encoding.header_value() {
            builder = builder.add_header("Content-Encoding", encoding);
        }

        Some(builder.set_content(body).into_ok_result(false))
    }

    /// Index file of the folder is served as the file itself - so the no-cache list
    /// is checked by both paths: the requested one and the one of the file
    fn has_to_disable_caching(&self, request_path: &HttpPath, found_file: &FoundFile) -> bool {
        if found_file.is_index_file || self.no_cache.marked_as_no_cache(request_path) {
            return true;
        }

        if found_file.uri_path == request_path.as_str() {
            return false;
        }

        self.no_cache
            .marked_as_no_cache(&HttpPath::from_str(found_file.uri_path.as_str()))
    }
}

/// Puts the caching headers on the response. No etag means - the content must not be cached
pub fn apply_cache_headers(
    builder: HttpResultBuilder,
    etag: Option<String>,
    vary_by_user_agent: bool,
) -> HttpResultBuilder {
    let builder = if vary_by_user_agent {
        builder.add_header("Vary", "User-Agent")
    } else {
        builder
    };

    match etag {
        Some(etag) => builder
            .add_header("ETag", etag)
            .add_header("Cache-Control", "no-cache"),
        None => builder
            .add_header("Cache-Control", NO_CACHE_CACHE_CONTROL)
            .add_header("Pragma", NO_CACHE_PRAGMA)
            .add_header("Expires", NO_CACHE_EXPIRES),
    }
}

pub fn compile_error(uri_path: &str, err: std::io::Error) -> HttpFailResult {
    HttpFailResult::as_fatal_error(format!(
        "Can not prepare the content of '{}'. Err: {}",
        uri_path, err
    ))
}

fn has_the_same_etag(ctx: &HttpContext, etag: &str) -> bool {
    let Some(header) = ctx
        .request
        .get_headers()
        .try_get_case_insensitive("if-none-match")
    else {
        return false;
    };

    let Ok(header) = header.as_str() else {
        return false;
    };

    header == etag
}

#[cfg(test)]
mod tests {
    use super::super::CachedContent;
    use super::*;

    /// Folder with the files of the test. It is removed when the test is over
    struct TestFolder {
        path: std::path::PathBuf,
    }

    impl TestFolder {
        fn new(test_name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "web-app-host-{}-{}",
                test_name,
                std::process::id()
            ));

            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();

            Self { path }
        }

        fn add_file(&self, file_name: &str, content: &str) -> &Self {
            let file_name = self.path.join(file_name);
            std::fs::create_dir_all(file_name.parent().unwrap()).unwrap();
            std::fs::write(file_name, content).unwrap();
            self
        }

        fn get_folder(&self, name: &str) -> String {
            self.path.join(name).to_str().unwrap().to_string()
        }
    }

    impl Drop for TestFolder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    async fn find_file(folders: &[&str], uri_path: &str) -> FoundFile {
        StaticFilesReader::new(true, vec![])
            .find_file(uri_path, folders)
            .await
            .unwrap()
    }

    fn get_content(found_file: &FoundFile) -> String {
        String::from_utf8(found_file.content.get_raw().unwrap()).unwrap()
    }

    fn create_test_site(test_name: &str) -> TestFolder {
        let result = TestFolder::new(test_name);

        result
            .add_file("wwwroot/index.html", "root")
            .add_file("wwwroot/es/index.html", "es")
            .add_file("wwwroot/es/app.js", "es-app")
            .add_file("wwwroot/es/legal/index.html", "es-legal")
            .add_file("wwwroot/assets/app.js", "app");

        result
    }

    #[tokio::test]
    async fn serves_the_index_file_of_the_folder() {
        let site = create_test_site("folder-index");
        let www_root = site.get_folder("wwwroot");

        for uri_path in ["/es/", "/es"] {
            let found_file = find_file(&[www_root.as_str()], uri_path).await;

            assert_eq!(found_file.uri_path, "/es/index.html", "{}", uri_path);
            assert_eq!(get_content(&found_file), "es", "{}", uri_path);
            assert!(!found_file.is_index_file, "{}", uri_path);
        }

        for uri_path in ["/es/legal/", "/es/legal"] {
            let found_file = find_file(&[www_root.as_str()], uri_path).await;

            assert_eq!(found_file.uri_path, "/es/legal/index.html", "{}", uri_path);
            assert_eq!(get_content(&found_file), "es-legal", "{}", uri_path);
        }
    }

    #[tokio::test]
    async fn serves_the_files_as_before() {
        let site = create_test_site("files");
        let www_root = site.get_folder("wwwroot");

        let found_file = find_file(&[www_root.as_str()], "/es/app.js").await;
        assert_eq!(found_file.uri_path, "/es/app.js");
        assert_eq!(get_content(&found_file), "es-app");

        let found_file = find_file(&[www_root.as_str()], "/es/index.html").await;
        assert_eq!(found_file.uri_path, "/es/index.html");
        assert_eq!(get_content(&found_file), "es");
    }

    #[tokio::test]
    async fn serves_the_root_index_file() {
        let site = create_test_site("root-index");
        let www_root = site.get_folder("wwwroot");

        for uri_path in ["/", "/index.html"] {
            let found_file = find_file(&[www_root.as_str()], uri_path).await;

            assert_eq!(found_file.uri_path, INDEX_FILE, "{}", uri_path);
            assert_eq!(get_content(&found_file), "root", "{}", uri_path);
            assert!(found_file.is_index_file, "{}", uri_path);
        }
    }

    #[tokio::test]
    async fn renders_the_spa_route_if_there_is_no_folder_index_file() {
        let site = create_test_site("spa-route");
        let www_root = site.get_folder("wwwroot");

        for uri_path in ["/nope/", "/nope", "/es/nope/", "/assets/", "/assets"] {
            let found_file = find_file(&[www_root.as_str()], uri_path).await;

            assert_eq!(found_file.uri_path, INDEX_FILE, "{}", uri_path);
            assert_eq!(get_content(&found_file), "root", "{}", uri_path);
            assert!(found_file.is_index_file, "{}", uri_path);
        }
    }

    #[tokio::test]
    async fn looks_the_folder_up_in_the_device_folder_first() {
        let site = TestFolder::new("with-mobile");

        site.add_file("wwwroot/mobile/index.html", "mobile")
            .add_file("wwwroot/mobile/es/index.html", "mobile-es")
            .add_file("wwwroot/mobile/fr/app.js", "mobile-fr-app")
            .add_file("wwwroot/desktop/index.html", "desktop")
            .add_file("wwwroot/desktop/es/index.html", "desktop-es")
            .add_file("wwwroot/desktop/de/index.html", "desktop-de")
            .add_file("wwwroot/desktop/fr/index.html", "desktop-fr");

        let mobile = site.get_folder("wwwroot/mobile");
        let desktop = site.get_folder("wwwroot/desktop");
        let folders = [mobile.as_str(), desktop.as_str()];

        for uri_path in ["/es/", "/es"] {
            let found_file = find_file(&folders, uri_path).await;
            assert_eq!(found_file.uri_path, "/es/index.html", "{}", uri_path);
            assert_eq!(get_content(&found_file), "mobile-es", "{}", uri_path);
        }

        // there is no such folder in the device folder
        for uri_path in ["/de/", "/de"] {
            let found_file = find_file(&folders, uri_path).await;
            assert_eq!(found_file.uri_path, "/de/index.html", "{}", uri_path);
            assert_eq!(get_content(&found_file), "desktop-de", "{}", uri_path);
        }

        // folder of the device has no index file
        let found_file = find_file(&folders, "/fr/").await;
        assert_eq!(found_file.uri_path, "/fr/index.html");
        assert_eq!(get_content(&found_file), "desktop-fr");

        let found_file = find_file(&folders, "/nope/").await;
        assert_eq!(found_file.uri_path, INDEX_FILE);
        assert_eq!(get_content(&found_file), "mobile");
    }

    #[tokio::test]
    async fn does_not_lead_out_of_the_root_folder() {
        let site = create_test_site("out-of-root");
        site.add_file("secret.txt", "secret")
            .add_file("private/index.html", "private");

        let www_root = site.get_folder("wwwroot");

        for uri_path in [
            "/../secret.txt",
            "/es/../../secret.txt",
            "/../private/",
            "/../private",
            "/es/../../private/",
        ] {
            let found_file = find_file(&[www_root.as_str()], uri_path).await;

            assert_eq!(found_file.uri_path, INDEX_FILE, "{}", uri_path);
            assert_eq!(get_content(&found_file), "root", "{}", uri_path);
        }
    }

    fn create_found_file(uri_path: &str) -> FoundFile {
        let content = CachedContent {
            data: vec![],
            is_zstd: false,
            etag: None,
        };

        FoundFile::as_requested(content, uri_path.to_string())
    }

    fn has_to_disable_caching(no_cache: &[&str], request_path: &str, uri_path: &str) -> bool {
        let no_cache = no_cache.iter().map(|itm| itm.to_string()).collect();

        StaticFilesReader::new(true, no_cache).has_to_disable_caching(
            &HttpPath::from_str(request_path),
            &create_found_file(uri_path),
        )
    }

    #[test]
    fn folder_index_file_is_cached_as_the_file() {
        assert!(!has_to_disable_caching(&[], "/es/", "/es/index.html"));
        assert!(!has_to_disable_caching(&[], "/es", "/es/index.html"));

        assert!(has_to_disable_caching(&[], "/", INDEX_FILE));
        assert!(has_to_disable_caching(&[], INDEX_FILE, INDEX_FILE));
    }

    #[test]
    fn folder_index_file_is_checked_by_both_paths() {
        let no_cache = ["/es/index.html"];
        assert!(has_to_disable_caching(&no_cache, "/es/", "/es/index.html"));
        assert!(has_to_disable_caching(&no_cache, "/es", "/es/index.html"));

        let no_cache = ["/es/"];
        assert!(has_to_disable_caching(&no_cache, "/es", "/es/index.html"));

        let no_cache = ["/de/index.html"];
        assert!(!has_to_disable_caching(&no_cache, "/es/", "/es/index.html"));
    }
}
