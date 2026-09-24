use crate::models::INDEX_FILE;

use super::CachedContent;

/// File we are going to serve
pub struct FoundFile {
    pub content: CachedContent,
    /// Uri path of the file within the root folder. It differs from the requested one
    /// when the folder is requested and its index file is served - or when the file
    /// is not found and the index file renders the SPA route
    pub uri_path: String,
    /// Index file is the entry point of the application - it is never cached
    pub is_index_file: bool,
}

impl FoundFile {
    pub fn as_requested(content: CachedContent, uri_path: String) -> Self {
        let is_index_file = rust_extensions::str_utils::compare_strings_case_insensitive(
            uri_path.as_str(),
            INDEX_FILE,
        );

        Self {
            content,
            uri_path,
            is_index_file,
        }
    }

    pub fn as_index_file(content: CachedContent) -> Self {
        Self {
            content,
            uri_path: INDEX_FILE.to_string(),
            is_index_file: true,
        }
    }
}
