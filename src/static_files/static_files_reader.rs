use std::{sync::Arc, time::Duration};

use my_http_server::{
    HttpContext, HttpFailResult, HttpOkResult, HttpOutput, HttpOutputAsStream, HttpPath,
    HttpResultBuilder,
};

use crate::models::INDEX_FILE;

use super::{
    compile_folder_index_path, detect_content_type, is_safe_uri_path, send_chunks,
    AcceptedEncodings, CompressionAlgorithm, CompressionQueue, ContentFile, ContentFiles,
    FileChunks, FileVariant, FoundFile, NoCache, ServeParams, VariantChoice, STREAM_QUEUE_SIZE,
    WHOLE_FILE_MAX_SIZE,
};

/// How often the request checks whether the file it waits for is prepared
const PREPARED_FILE_CHECK_INTERVAL: Duration = Duration::from_millis(100);

/// Full set of headers which forbids any caching of the content
const NO_CACHE_CACHE_CONTROL: &str = "no-store, no-cache, must-revalidate, max-age=0";
const NO_CACHE_PRAGMA: &str = "no-cache";
const NO_CACHE_EXPIRES: &str = "0";

/// The browser must not guess the type of the file by its content - it takes the one we send
const NOSNIFF_HEADER: (&str, &str) = ("X-Content-Type-Options", "nosniff");
/// The other sites get the origin of the page only - not its full url
const REFERRER_POLICY_HEADER: (&str, &str) = ("Referrer-Policy", "strict-origin-when-cross-origin");
/// Only the pages of the same site may show the page in a frame
const FRAME_OPTIONS_HEADER: (&str, &str) = ("X-Frame-Options", "SAMEORIGIN");

/// Serves the content of the root folder. The root folder is not a setting of the reader -
/// it comes with each request, since in WITH_MOBILE mode it is either the desktop
/// or the mobile folder
pub struct StaticFilesReader {
    content_files: ContentFiles,
    /// Prepares the files in the background - the request puts the file it waits for into it
    compression_queue: Arc<CompressionQueue>,
    no_cache: NoCache,
}

impl StaticFilesReader {
    pub fn new(
        content_files: ContentFiles,
        compression_queue: Arc<CompressionQueue>,
        no_cache_paths: Vec<String>,
    ) -> Self {
        Self {
            content_files,
            compression_queue,
            no_cache: NoCache::new(no_cache_paths),
        }
    }

    /// Looks the file up in the folders - in the given order. If the path points to the folder -
    /// its index file is served. If it is nowhere - the index file of the first folder
    /// renders the SPA route
    pub fn find_file(&self, uri_path: &str, folders: &[&str]) -> Option<FoundFile<'_>> {
        // path which leads out of the root folder is rejected by the middleware as forbidden.
        // If it gets here anyway - it is never looked up
        if is_safe_uri_path(uri_path) {
            for folder in folders {
                if let Some(found_file) = self.find_in_folder(folder, uri_path) {
                    return Some(found_file);
                }
            }
        }

        let file = self.content_files.get(folders.first()?, INDEX_FILE)?;

        Some(FoundFile::as_index_file(file))
    }

    /// Path which ends with '/' points to the folder. Path without it points to the file -
    /// or to the folder, if there is no such file
    fn find_in_folder(&self, folder: &str, uri_path: &str) -> Option<FoundFile<'_>> {
        if !uri_path.ends_with('/') {
            if let Some(file) = self.content_files.get(folder, uri_path) {
                return Some(FoundFile::as_requested(file, uri_path.to_string()));
            }
        }

        // the index file exists only if the path is the folder on the disk
        let index_path = compile_folder_index_path(uri_path);
        let file = self.content_files.get(folder, index_path.as_str())?;

        Some(FoundFile::as_requested(file, index_path))
    }

    /// Sends the variant of the file the client accepts: the compressed copy - or the raw file.
    /// Each variant has its own etag
    pub async fn compile_response(
        &self,
        ctx: &HttpContext,
        found_file: &FoundFile<'_>,
        params: &ServeParams<'_>,
    ) -> Option<Result<HttpOkResult, HttpFailResult>> {
        let disable_caching = self.has_to_disable_caching(&ctx.request.http_path, found_file);

        let accepted = AcceptedEncodings::from_request(ctx);

        let (variant, encoding) = match self.wait_for_variant(found_file.file, accepted).await {
            Ok(result) => result,
            Err(err) => return Some(Err(compile_error(found_file.uri_path.as_str(), err))),
        };

        // etag makes no sense for the content the client is not allowed to store
        let etag = if disable_caching {
            None
        } else {
            Some(variant.etag.clone())
        };

        if let Some(etag) = etag.as_ref() {
            // the library reads every 'If-None-Match' line as one list: '*', weak etags,
            // and the etags of the previous versions which were sent without the quotes
            if ctx.request.if_none_match_matches(etag.as_str()) {
                // 304 carries the caching headers the 200 would have - the client and the caches
                // in the middle refresh the response they keep with them
                let builder = apply_cache_headers(
                    HttpOutput::as_not_modified(),
                    Some(etag.clone()),
                    compile_vary(true, params.vary_by_user_agent),
                );

                return Some(builder.into_ok_result(false));
            }
        }

        let content_type = detect_content_type(found_file.uri_path.as_str());

        if variant.size <= WHOLE_FILE_MAX_SIZE {
            let body = match tokio::fs::read(variant.path.as_str()).await {
                Ok(body) => body,
                Err(err) => return Some(Err(compile_error(found_file.uri_path.as_str(), err))),
            };

            let builder = HttpOutput::from_builder().set_content_type_opt(content_type);
            let builder = apply_variant_headers(builder, etag, encoding, params);

            return Some(builder.set_content(body).into_ok_result(false));
        }

        // the bigger file is sent by the chunks - so the request does not take the memory
        // of the whole file
        let chunks = match FileChunks::open(variant.path.as_str()).await {
            Ok(chunks) => chunks,
            Err(err) => return Some(Err(compile_error(found_file.uri_path.as_str(), err))),
        };

        let (stream, producer) = HttpOutput::as_stream(STREAM_QUEUE_SIZE);

        let mut stream = stream.with_header("Content-Length", chunks.content_length().to_string());

        if let Some(content_type) = content_type {
            stream = stream.with_header("Content-Type", content_type.as_str());
        }

        let stream = apply_variant_headers(stream, etag, encoding, params);

        tokio::spawn(send_chunks(chunks, producer));

        Some(stream.get_result())
    }

    /// The best variant of the file the client accepts. The files are prepared in the background:
    /// if the variant is not ready yet, the file is put into the priority queues - it is taken
    /// right after the files in progress, and the request waits for it. It is prepared sooner
    /// or later: all the files are in the queues
    async fn wait_for_variant<'f>(
        &self,
        file: &'f Arc<ContentFile>,
        accepted: AcceptedEncodings,
    ) -> Result<(&'f FileVariant, Option<CompressionAlgorithm>), &'f str> {
        let mut is_prioritized = false;

        loop {
            match file.choose_variant(accepted) {
                VariantChoice::Ready(variant, encoding) => return Ok((variant, encoding)),
                VariantChoice::Failed(err) => return Err(err),
                VariantChoice::NotReady => {}
            }

            if !is_prioritized {
                self.compression_queue.prioritize(file);
                is_prioritized = true;
            }

            tokio::time::sleep(PREPARED_FILE_CHECK_INTERVAL).await;
        }
    }

    /// Index file of the folder is served as the file itself - so the no-cache list
    /// is checked by both paths: the requested one and the one of the file
    fn has_to_disable_caching(&self, request_path: &HttpPath, found_file: &FoundFile<'_>) -> bool {
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

/// Value of the 'Vary' header: the request headers the content depends on - all in one header.
/// The caches in the middle should combine several 'Vary' headers, but not all of them do
pub fn compile_vary(by_accept_encoding: bool, by_user_agent: bool) -> Option<&'static str> {
    match (by_accept_encoding, by_user_agent) {
        (true, true) => Some("Accept-Encoding, User-Agent"),
        (true, false) => Some("Accept-Encoding"),
        (false, true) => Some("User-Agent"),
        (false, false) => None,
    }
}

/// The response the headers are put on: the content which is sent in one piece -
/// or the stream of the chunks. Both of them get the same headers
pub trait ResponseHeaders: Sized {
    fn put_header(self, name: &'static str, value: impl Into<String>) -> Self;
}

impl ResponseHeaders for HttpResultBuilder {
    fn put_header(self, name: &'static str, value: impl Into<String>) -> Self {
        self.add_header(name, value.into())
    }
}

impl ResponseHeaders for HttpOutputAsStream {
    fn put_header(self, name: &'static str, value: impl Into<String>) -> Self {
        self.with_header(name, value.into())
    }
}

/// The headers of the file variant: the protection, the caching and the encoding of the copy
fn apply_variant_headers<T: ResponseHeaders>(
    response: T,
    etag: Option<String>,
    encoding: Option<CompressionAlgorithm>,
    params: &ServeParams<'_>,
) -> T {
    let response = apply_security_headers(response, params.allow_framing);
    let response = apply_cache_headers(
        response,
        etag,
        compile_vary(true, params.vary_by_user_agent),
    );

    match encoding {
        Some(algorithm) => response.put_header("Content-Encoding", algorithm.as_str()),
        None => response,
    }
}

/// Puts the headers which protect the content on the response - whatever the content is.
/// Content-Security-Policy is not here: it depends on the application, so it is set
/// by the application itself or by the ingress in front of the service
pub fn apply_security_headers<T: ResponseHeaders>(response: T, allow_framing: bool) -> T {
    let response = response
        .put_header(NOSNIFF_HEADER.0, NOSNIFF_HEADER.1)
        .put_header(REFERRER_POLICY_HEADER.0, REFERRER_POLICY_HEADER.1);

    if allow_framing {
        return response;
    }

    response.put_header(FRAME_OPTIONS_HEADER.0, FRAME_OPTIONS_HEADER.1)
}

/// Puts the caching headers on the response. No etag means - the content must not be cached
pub fn apply_cache_headers<T: ResponseHeaders>(
    response: T,
    etag: Option<String>,
    vary: Option<&'static str>,
) -> T {
    let response = match vary {
        Some(vary) => response.put_header("Vary", vary),
        None => response,
    };

    match etag {
        Some(etag) => response
            .put_header("ETag", etag)
            .put_header("Cache-Control", "no-cache"),
        None => response
            .put_header("Cache-Control", NO_CACHE_CACHE_CONTROL)
            .put_header("Pragma", NO_CACHE_PRAGMA)
            .put_header("Expires", NO_CACHE_EXPIRES),
    }
}

pub fn compile_error(uri_path: &str, err: impl std::fmt::Display) -> HttpFailResult {
    HttpFailResult::as_fatal_error(format!(
        "Can not prepare the content of '{}'. Err: {}",
        uri_path, err
    ))
}

#[cfg(test)]
mod tests {
    use super::super::{calc_etag, TestFolder};
    use super::*;

    /// File the reader has found - with the content of its raw file
    struct Found {
        uri_path: String,
        content: String,
        is_index_file: bool,
    }

    /// Looks the file up in the content of the test site - its 'wwwroot' folder
    fn find_file(site: &TestFolder, folders: &[&str], uri_path: &str) -> Found {
        let content_files = ContentFiles::scan(site.get_folder("wwwroot").as_str(), &[""]);
        let reader =
            StaticFilesReader::new(content_files, Arc::new(CompressionQueue::default()), vec![]);
        let found_file = reader.find_file(uri_path, folders).unwrap();

        Found {
            content: std::fs::read_to_string(found_file.file.raw_path.as_str()).unwrap(),
            uri_path: found_file.uri_path,
            is_index_file: found_file.is_index_file,
        }
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

    #[test]
    fn serves_the_index_file_of_the_folder() {
        let site = create_test_site("folder-index");
        let www_root = site.get_folder("wwwroot");

        for uri_path in ["/es/", "/es"] {
            let found_file = find_file(&site, &[www_root.as_str()], uri_path);

            assert_eq!(found_file.uri_path, "/es/index.html", "{}", uri_path);
            assert_eq!(found_file.content, "es", "{}", uri_path);
            assert!(!found_file.is_index_file, "{}", uri_path);
        }

        for uri_path in ["/es/legal/", "/es/legal"] {
            let found_file = find_file(&site, &[www_root.as_str()], uri_path);

            assert_eq!(found_file.uri_path, "/es/legal/index.html", "{}", uri_path);
            assert_eq!(found_file.content, "es-legal", "{}", uri_path);
        }
    }

    #[test]
    fn serves_the_files_as_before() {
        let site = create_test_site("files");
        let www_root = site.get_folder("wwwroot");

        let found_file = find_file(&site, &[www_root.as_str()], "/es/app.js");
        assert_eq!(found_file.uri_path, "/es/app.js");
        assert_eq!(found_file.content, "es-app");

        let found_file = find_file(&site, &[www_root.as_str()], "/es/index.html");
        assert_eq!(found_file.uri_path, "/es/index.html");
        assert_eq!(found_file.content, "es");
    }

    #[test]
    fn serves_the_root_index_file() {
        let site = create_test_site("root-index");
        let www_root = site.get_folder("wwwroot");

        for uri_path in ["/", "/index.html"] {
            let found_file = find_file(&site, &[www_root.as_str()], uri_path);

            assert_eq!(found_file.uri_path, INDEX_FILE, "{}", uri_path);
            assert_eq!(found_file.content, "root", "{}", uri_path);
            assert!(found_file.is_index_file, "{}", uri_path);
        }
    }

    #[test]
    fn renders_the_spa_route_if_there_is_no_folder_index_file() {
        let site = create_test_site("spa-route");
        let www_root = site.get_folder("wwwroot");

        for uri_path in ["/nope/", "/nope", "/es/nope/", "/assets/", "/assets"] {
            let found_file = find_file(&site, &[www_root.as_str()], uri_path);

            assert_eq!(found_file.uri_path, INDEX_FILE, "{}", uri_path);
            assert_eq!(found_file.content, "root", "{}", uri_path);
            assert!(found_file.is_index_file, "{}", uri_path);
        }
    }

    #[test]
    fn looks_the_folder_up_in_the_device_folder_first() {
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
            let found_file = find_file(&site, &folders, uri_path);
            assert_eq!(found_file.uri_path, "/es/index.html", "{}", uri_path);
            assert_eq!(found_file.content, "mobile-es", "{}", uri_path);
        }

        // there is no such folder in the device folder
        for uri_path in ["/de/", "/de"] {
            let found_file = find_file(&site, &folders, uri_path);
            assert_eq!(found_file.uri_path, "/de/index.html", "{}", uri_path);
            assert_eq!(found_file.content, "desktop-de", "{}", uri_path);
        }

        // folder of the device has no index file
        let found_file = find_file(&site, &folders, "/fr/");
        assert_eq!(found_file.uri_path, "/fr/index.html");
        assert_eq!(found_file.content, "desktop-fr");

        let found_file = find_file(&site, &folders, "/nope/");
        assert_eq!(found_file.uri_path, INDEX_FILE);
        assert_eq!(found_file.content, "mobile");
    }

    #[test]
    fn does_not_lead_out_of_the_root_folder() {
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
            "/%2e%2e/secret.txt",
            "/es%2f..%2f..%2fsecret.txt",
            // the double encoded dots get through is_safe_uri_path -
            // but resolve to nothing, since only the scanned files are served
            "/%252e%252e/secret.txt",
            "/es/%252e%252e/%252e%252e/secret.txt",
        ] {
            let found_file = find_file(&site, &[www_root.as_str()], uri_path);

            assert_eq!(found_file.uri_path, INDEX_FILE, "{}", uri_path);
            assert_eq!(found_file.content, "root", "{}", uri_path);
        }
    }

    /// 'If-None-Match' is read by the library - this is the contract the service relies on.
    /// The previous versions of the service sent the same hash without the quotes: the clients
    /// which keep such an etag get 304, not the whole content once again
    #[test]
    fn matches_the_etag_in_the_if_none_match_header() {
        let etag = calc_etag(b"content");
        let previous_version_etag = etag.trim_matches('"').to_string();
        let weak_etag = format!("W/{}", etag);
        let etag_in_list = format!("\"xyz\", {}", etag);

        for if_none_match in [
            etag.as_str(),
            previous_version_etag.as_str(),
            weak_etag.as_str(),
            etag_in_list.as_str(),
            "*",
        ] {
            assert!(
                my_http_server::if_none_match_matches([if_none_match], etag.as_str()),
                "{}",
                if_none_match
            );
        }

        // the header sent as several lines is one list
        assert!(my_http_server::if_none_match_matches(
            ["\"xyz\"", etag.as_str()],
            etag.as_str()
        ));

        let other_etag = calc_etag(b"other content");

        for if_none_match in ["\"xyz\"", other_etag.as_str(), ""] {
            assert!(
                !my_http_server::if_none_match_matches([if_none_match], etag.as_str()),
                "{}",
                if_none_match
            );
        }
    }

    #[test]
    fn sends_one_vary_header() {
        assert_eq!(
            compile_vary(true, true),
            Some("Accept-Encoding, User-Agent")
        );
        assert_eq!(compile_vary(true, false), Some("Accept-Encoding"));
        assert_eq!(compile_vary(false, true), Some("User-Agent"));
        assert_eq!(compile_vary(false, false), None);
    }

    fn has_to_disable_caching(no_cache: &[&str], request_path: &str, uri_path: &str) -> bool {
        let no_cache = no_cache.iter().map(|itm| itm.to_string()).collect();

        let file = Arc::new(ContentFile::new(uri_path.to_string(), String::new(), 0));

        StaticFilesReader::new(
            ContentFiles::default(),
            Arc::new(CompressionQueue::default()),
            no_cache,
        )
        .has_to_disable_caching(
            &HttpPath::from_str(request_path),
            &FoundFile::as_requested(&file, uri_path.to_string()),
        )
    }

    #[tokio::test]
    async fn waits_for_the_file_prepared_in_the_background() {
        let site = create_test_site("wait-prepared");
        let www_root = site.get_folder("wwwroot");

        let content_files = ContentFiles::scan(www_root.as_str(), &[""]);
        let queue = Arc::new(CompressionQueue::default());
        queue.enqueue(content_files.files_by_size());

        let reader = StaticFilesReader::new(content_files, queue.clone(), vec![]);
        let found_file = reader
            .find_file("/es/app.js", &[www_root.as_str()])
            .unwrap();

        assert!(found_file.file.get_raw().is_none());

        // the background thread starts preparing later than the request comes
        let background = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            queue.prepare_all(&[]);
        });

        let (variant, encoding) = reader
            .wait_for_variant(found_file.file, AcceptedEncodings::default())
            .await
            .unwrap();

        assert_eq!(encoding, None);
        assert_eq!(
            std::fs::read_to_string(variant.path.as_str()).unwrap(),
            "es-app"
        );

        background.join().unwrap();
    }

    #[tokio::test]
    async fn gets_the_failed_preparation_instead_of_waiting_forever() {
        let site = create_test_site("wait-failed");
        let www_root = site.get_folder("wwwroot");

        let content_files = ContentFiles::scan(www_root.as_str(), &[""]);
        let queue = Arc::new(CompressionQueue::default());
        queue.enqueue(content_files.files_by_size());

        let reader = StaticFilesReader::new(content_files, queue.clone(), vec![]);
        let found_file = reader
            .find_file("/es/app.js", &[www_root.as_str()])
            .unwrap();

        // the file is removed after the scan - it can not be read any more
        std::fs::remove_file(found_file.file.raw_path.as_str()).unwrap();

        let background = std::thread::spawn(move || queue.prepare_all(&[]));

        let accepted = AcceptedEncodings {
            brotli: true,
            gzip: true,
        };

        assert!(reader
            .wait_for_variant(found_file.file, accepted)
            .await
            .is_err());

        background.join().unwrap();
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
