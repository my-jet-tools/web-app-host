use my_http_server::WebContentType;

/// Content types of the extensions the library does not know. The table of the library covers
/// what a site is mostly made of - html, css, js, images, fonts; these are the rest of it.
/// The file sent without 'Content-Type' is sniffed by the browser - which 'nosniff' forbids
/// for the scripts and the styles
const EXTRA_CONTENT_TYPES: &[(&str, &str)] = &[
    ("cjs", "text/javascript; charset=utf-8"),
    ("md", "text/markdown; charset=utf-8"),
    ("bmp", "image/bmp"),
    ("eot", "application/vnd.ms-fontobject"),
    ("ogv", "video/ogg"),
    ("m4a", "audio/mp4"),
    ("oga", "audio/ogg"),
    ("weba", "audio/webm"),
    ("zip", "application/zip"),
    ("gz", "application/gzip"),
];

/// Content type of the file - by its extension. The table of the library goes first, so the
/// service sends the same types as the other services on the library. The file of the unknown
/// extension is sent without it
pub fn detect_content_type(uri_path: &str) -> Option<WebContentType> {
    if let Some(content_type) = WebContentType::detect_by_extension(uri_path) {
        return Some(content_type);
    }

    let file_name = uri_path.rsplit('/').next().unwrap_or(uri_path);
    let (_, extension) = file_name.rsplit_once('.')?;
    let extension = extension.to_ascii_lowercase();

    EXTRA_CONTENT_TYPES
        .iter()
        .find(|(known_extension, _)| *known_extension == extension.as_str())
        .map(|(_, content_type)| WebContentType::Raw(content_type.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content_type(uri_path: &str) -> Option<String> {
        detect_content_type(uri_path).map(|content_type| content_type.as_str().to_string())
    }

    #[test]
    fn detects_the_content_type_by_the_extension() {
        assert_eq!(
            content_type("/index.html").as_deref(),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(
            content_type("/assets/app.min.JS").as_deref(),
            Some("text/javascript; charset=utf-8")
        );
        assert_eq!(
            content_type("/robots.txt").as_deref(),
            Some("text/plain; charset=utf-8")
        );
        assert_eq!(
            content_type("/favicon.ico").as_deref(),
            Some("image/x-icon")
        );
        assert_eq!(
            content_type("/fonts/inter.woff2").as_deref(),
            Some("font/woff2")
        );
        assert_eq!(
            content_type("/site.webmanifest").as_deref(),
            Some("application/manifest+json")
        );
        assert_eq!(
            content_type("/assets/app.js.map").as_deref(),
            Some("application/json")
        );
    }

    #[test]
    fn detects_the_extensions_the_library_does_not_know() {
        assert_eq!(
            content_type("/assets/app.cjs").as_deref(),
            Some("text/javascript; charset=utf-8")
        );
        assert_eq!(
            content_type("/README.md").as_deref(),
            Some("text/markdown; charset=utf-8")
        );
        assert_eq!(
            content_type("/files/archive.ZIP").as_deref(),
            Some("application/zip")
        );
    }

    #[test]
    fn the_table_of_the_library_goes_first() {
        for uri_path in [
            "/index.html",
            "/site.css",
            "/app.js",
            "/data.json",
            "/logo.svg",
            "/image.png",
            "/fonts/inter.woff2",
        ] {
            assert_eq!(
                content_type(uri_path),
                WebContentType::detect_by_extension(uri_path)
                    .map(|content_type| content_type.as_str().to_string()),
                "{}",
                uri_path
            );
        }
    }

    /// The extension the library has learned is removed from the table here -
    /// so the types do not drift apart
    #[test]
    fn extra_extensions_are_unknown_to_the_library() {
        for (extension, _) in EXTRA_CONTENT_TYPES {
            let uri_path = format!("/file.{}", extension);

            assert!(
                WebContentType::detect_by_extension(uri_path.as_str()).is_none(),
                "the library knows '{}' now - remove it from EXTRA_CONTENT_TYPES",
                extension
            );
        }
    }

    #[test]
    fn unknown_extension_has_no_content_type() {
        assert_eq!(content_type("/LICENSE"), None);
        assert_eq!(content_type("/folder.png/file"), None);
        assert_eq!(content_type("/file.unknown"), None);
        assert_eq!(content_type("/"), None);
    }

    #[test]
    fn every_extra_extension_is_lowercase_and_unique() {
        let mut extensions: Vec<&str> = EXTRA_CONTENT_TYPES.iter().map(|(ext, _)| *ext).collect();

        assert!(extensions.iter().all(|ext| ext
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())));

        let total = extensions.len();
        extensions.sort();
        extensions.dedup();
        assert_eq!(extensions.len(), total);
    }
}
