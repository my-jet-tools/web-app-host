use std::{
    collections::HashMap,
    fs::File,
    io::{BufReader, BufWriter, ErrorKind, Read, Write},
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use super::{
    is_compressed_well, AcceptedEncodings, CompressionAlgorithm, EtagHasher, COMPRESSION_MIN_SIZE,
    STREAM_CHUNK_SIZE,
};

/// The copy is written to the temp file and gets its name when it is complete - so the file
/// with the name of the copy is always the whole copy, even if the service stops while writing
const TEMP_EXTENSION: &str = ".temp";

/// Formats which are compressed already: images, video, audio, fonts and archives.
/// Compressing them once more takes the time and makes them no smaller
const COMPRESSED_FORMATS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "avif", "heic", "heif", "jxl", "mp4", "m4v", "webm",
    "mov", "mkv", "avi", "ogv", "mp3", "m4a", "aac", "ogg", "oga", "opus", "flac", "weba", "woff",
    "woff2", "zip", "gz", "tgz", "br", "zst", "bz2", "xz", "7z", "rar",
];

/// The only hidden folder which is served: the standards keep their files in it -
/// ACME challenges, security.txt, the links of the mobile applications
const WELL_KNOWN_FOLDER: &str = ".well-known";

/// Folder of the copies of the content compressed with the algorithm.
/// It has the same structure as the root folder of the content
#[derive(Clone)]
pub struct CopiesFolder {
    pub algorithm: CompressionAlgorithm,
    pub path: String,
}

/// Folders of the compressed copies - in the order of preference of the algorithms:
/// '{home}/wwwroot-br' and '{home}/wwwroot-gzip'
pub fn compile_copies_folders(home: &str) -> Vec<CopiesFolder> {
    let home = home.trim_end_matches('/');

    CompressionAlgorithm::ALL
        .into_iter()
        .map(|algorithm| CopiesFolder {
            algorithm,
            path: format!("{}/wwwroot-{}", home, algorithm.as_str()),
        })
        .collect()
}

/// The copies of the previous start are removed - they are the copies of the previous content.
/// The folder we can not create is skipped: the files are served without its copies
pub fn prepare_copies_folders(copies_folders: Vec<CopiesFolder>) -> Vec<CopiesFolder> {
    copies_folders
        .into_iter()
        .filter(|folder| match recreate_folder(folder.path.as_str()) {
            Ok(()) => true,
            Err(err) => {
                println!(
                    "Can not create the folder '{}' of the compressed copies. Err: {}",
                    folder.path, err
                );
                false
            }
        })
        .collect()
}

/// File on the disk we send to the client
pub struct FileVariant {
    pub path: String,
    /// Etag of the content of this very file - so the raw file and each of its
    /// compressed copies have their own etag
    pub etag: String,
    /// It decides whether the file is sent in one piece or by the chunks
    pub size: u64,
}

/// Variant of the file the request can send at the moment
pub enum VariantChoice<'f> {
    Ready(&'f FileVariant, Option<CompressionAlgorithm>),
    /// None of the variants the client accepts is prepared yet
    NotReady,
    /// The file can not be read
    Failed(&'f str),
}

/// The order the copies of the file are created in: the gzip copy takes a moment, so the client
/// which accepts only it does not wait for the brotli one, which takes the most of the time
const PREPARATION_ORDER: [CompressionAlgorithm; 2] =
    [CompressionAlgorithm::Gzip, CompressionAlgorithm::Brotli];

/// File of the content. The scan at the start only finds it: it is read, given the etag
/// and compressed in the background - see `CompressionQueue`. The request for the file
/// which is not prepared yet waits for it
pub struct ContentFile {
    /// Path of the file within the root folder: '/es/app.js' - or '/mobile/es/app.js'.
    /// The compressed copies are created by it
    uri_path: String,
    /// Real location of the file - with the symlinks resolved. It is checked to be within
    /// the root folder, and the file is read by it: the symlink re-pointed after the scan
    /// does not lead the requests out of the root folder
    pub raw_path: String,
    size: u64,
    /// Etag of the raw file. The failed read is kept as well: the request which waits
    /// for the file gets the error instead of waiting
    raw: OnceLock<Result<FileVariant, String>>,
    /// Set when the copy is created. None - there is no such copy: the file is small,
    /// of the compressed format, is not compressed well, or the copy can not be written
    br_copy: OnceLock<Option<FileVariant>>,
    gzip_copy: OnceLock<Option<FileVariant>>,
}

impl ContentFile {
    pub fn new(uri_path: String, raw_path: String, size: u64) -> Self {
        let result = Self {
            uri_path,
            raw_path,
            size,
            raw: OnceLock::new(),
            br_copy: OnceLock::new(),
            gzip_copy: OnceLock::new(),
        };

        // the small file and the file of the compressed format are not compressed - it is known
        // without reading them, so the request does not wait for the compression of such a file:
        // only the etag of the raw file is calculated for them
        if size <= COMPRESSION_MIN_SIZE as u64 || is_compressed_format(result.uri_path.as_str()) {
            for algorithm in CompressionAlgorithm::ALL {
                let _ = result.copy_slot(algorithm).set(None);
            }
        }

        result
    }

    pub fn get_raw(&self) -> Option<&Result<FileVariant, String>> {
        self.raw.get()
    }

    /// None - the copy is not decided yet
    pub fn get_copy(&self, algorithm: CompressionAlgorithm) -> Option<&Option<FileVariant>> {
        self.copy_slot(algorithm).get()
    }

    /// The etag and all the copies are decided - there is nothing to do with the file any more
    pub fn is_prepared(&self) -> bool {
        self.raw.get().is_some()
            && CompressionAlgorithm::ALL
                .into_iter()
                .all(|algorithm| self.copy_slot(algorithm).get().is_some())
    }

    /// The best variant the client accepts. If its copy is not created yet, the request waits
    /// for it - even when the worse copy the client accepts is ready: the client and the caches
    /// in the middle keep the very variant they get from now on, with its etag
    pub fn choose_variant(&self, accepted: AcceptedEncodings) -> VariantChoice<'_> {
        for algorithm in CompressionAlgorithm::ALL {
            if !accepted.accepts(algorithm) {
                continue;
            }

            match self.copy_slot(algorithm).get() {
                Some(Some(copy)) => return VariantChoice::Ready(copy, Some(algorithm)),
                // there is no such copy - the next algorithm the client accepts
                Some(None) => {}
                None => return VariantChoice::NotReady,
            }
        }

        match self.raw.get() {
            Some(Ok(raw)) => VariantChoice::Ready(raw, None),
            Some(Err(err)) => VariantChoice::Failed(err.as_str()),
            None => VariantChoice::NotReady,
        }
    }

    /// Reads the file, calculates its etag and creates its compressed copies - with the maximum
    /// level, the gzip one first. Each of them is set as soon as it is ready - so the request
    /// which waits for the gzip copy does not wait for the brotli one. Whatever happens,
    /// all of them are decided after it: a request may wait for them
    pub fn prepare(&self, copies_folders: &[CopiesFolder]) {
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| self.compile(copies_folders)));

        if result.is_err() {
            println!(
                "Can not prepare the file '{}' - the preparation has panicked",
                self.raw_path
            );

            let _ = self
                .raw
                .set(Err("the preparation has panicked".to_string()));
        }

        for algorithm in CompressionAlgorithm::ALL {
            let _ = self.copy_slot(algorithm).set(None);
        }
    }

    /// The file is read by the chunks - for its etag and for each copy: it is never kept
    /// in the memory whole, and neither is any of its copies
    fn compile(&self, copies_folders: &[CopiesFolder]) {
        let (etag, raw_size) = match calc_file_etag(self.raw_path.as_str()) {
            Ok(result) => result,
            Err(err) => {
                println!("Can not read the file '{}'. Err: {}", self.raw_path, err);
                let _ = self.raw.set(Err(err.to_string()));
                return;
            }
        };

        let _ = self.raw.set(Ok(FileVariant {
            etag,
            path: self.raw_path.clone(),
            size: raw_size,
        }));

        for algorithm in PREPARATION_ORDER {
            let slot = self.copy_slot(algorithm);

            if slot.get().is_some() {
                continue;
            }

            let copy = copies_folders
                .iter()
                .find(|folder| folder.algorithm == algorithm)
                .and_then(|folder| {
                    create_copy(
                        self.raw_path.as_str(),
                        raw_size,
                        self.uri_path.as_str(),
                        folder,
                    )
                });

            let _ = slot.set(copy);
        }
    }

    fn copy_slot(&self, algorithm: CompressionAlgorithm) -> &OnceLock<Option<FileVariant>> {
        match algorithm {
            CompressionAlgorithm::Brotli => &self.br_copy,
            CompressionAlgorithm::Gzip => &self.gzip_copy,
        }
    }
}

/// Files of the content. The content is scanned once - at the start of the service:
/// the file which appears later is not served, the file which is changed later is served
/// with the etag and the compressed copies of the content it had when it was prepared
#[derive(Default)]
pub struct ContentFiles {
    /// The key is the path the request is mapped to: the site folder and the uri path of the file
    files: HashMap<String, Arc<ContentFile>>,
}

impl ContentFiles {
    /// Scans the folders of the sites within the root folder. The site is the root folder itself
    /// ("") - or its desktop and mobile folders, when WITH_MOBILE is enabled. The files are only
    /// found here - so the scan is quick: they are read and compressed in the background
    pub fn scan(www_root: &str, site_folders: &[&str]) -> Self {
        let mut files = Vec::new();

        // the content is served only from within the root folder: the symlink which leads
        // out of it is not followed
        match std::fs::canonicalize(www_root) {
            Ok(root) => {
                for site_folder in site_folders {
                    let folder = format!("{}{}", www_root, site_folder);
                    collect_files(
                        Path::new(folder.as_str()),
                        site_folder,
                        &root,
                        &mut Vec::new(),
                        &mut files,
                    );
                }
            }
            Err(err) => println!("Can not read the folder '{}'. Err: {}", www_root, err),
        }

        Self {
            files: files
                .into_iter()
                .map(|file| (format!("{}{}", www_root, file.uri_path), Arc::new(file)))
                .collect(),
        }
    }

    /// File of the site folder - by the path of the request. It is the only way the request
    /// reaches the disk: the file is read by the path resolved at the scan, never by the path
    /// of the request. So even the path which gets through `is_safe_uri_path` (with the double
    /// encoded dots, for example) resolves to the scanned file of the root folder - or to nothing
    pub fn get(&self, folder: &str, uri_path: &str) -> Option<&Arc<ContentFile>> {
        self.files.get(compile_file_name(folder, uri_path).as_str())
    }

    pub fn files_count(&self) -> usize {
        self.files.len()
    }

    /// All the files - the smallest first: it is the order they are prepared in the background.
    /// Most of the files get ready in a moment, and the big ones, which take the most of the time,
    /// go last. So right after the start a request rarely waits for a big file in progress
    pub fn files_by_size(&self) -> Vec<Arc<ContentFile>> {
        let mut result: Vec<_> = self.files.values().cloned().collect();
        result.sort_by_key(|file| file.size);
        result
    }
}

fn compile_file_name(folder: &str, uri_path: &str) -> String {
    if uri_path.starts_with('/') {
        format!("{}{}", folder, uri_path)
    } else {
        format!("{}/{}", folder, uri_path)
    }
}

fn recreate_folder(path: &str) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }

    std::fs::create_dir_all(path)
}

/// Collects the files of the folder and of its subfolders. Symlinks are followed only within
/// the root folder - the content outside of it is never served. The symlink to the folder above
/// is not followed as well: such a loop would never end
fn collect_files(
    folder: &Path,
    uri_path: &str,
    root: &Path,
    ancestors: &mut Vec<PathBuf>,
    result: &mut Vec<ContentFile>,
) {
    let Some(real_folder) = resolve_within_root(folder, root) else {
        return;
    };

    if ancestors.contains(&real_folder) {
        return;
    }

    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(err) => {
            println!(
                "Can not read the folder '{}'. Err: {}",
                folder.display(),
                err
            );
            return;
        }
    };

    ancestors.push(real_folder);

    for entry in entries.flatten() {
        // the name which is not utf-8 can not be requested
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };

        let path = entry.path();

        // '.env', '.git', '.DS_Store' are the files of the system and of the tools -
        // they are not the content of the site
        if is_hidden(name.as_str()) {
            println!(
                "Skipping '{}' - hidden files are not served",
                path.display()
            );
            continue;
        }

        let file_uri_path = format!("{}/{}", uri_path, name);

        // metadata of the target of the symlink
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };

        if metadata.is_dir() {
            collect_files(&path, file_uri_path.as_str(), root, ancestors, result);
        } else if metadata.is_file() {
            let Some(real_path) = resolve_within_root(&path, root) else {
                continue;
            };

            // the path which is not utf-8 can not be read by the string path
            let Ok(real_path) = real_path.into_os_string().into_string() else {
                continue;
            };

            result.push(ContentFile::new(file_uri_path, real_path, metadata.len()));
        }
    }

    ancestors.pop();
}

/// Real location of the file or of the folder - if it is within the root folder.
/// The symlink which leads out of it is skipped
fn resolve_within_root(path: &Path, root: &Path) -> Option<PathBuf> {
    let real_path = match std::fs::canonicalize(path) {
        Ok(real_path) => real_path,
        Err(err) => {
            println!("Can not read '{}'. Err: {}", path.display(), err);
            return None;
        }
    };

    if real_path.starts_with(root) {
        return Some(real_path);
    }

    println!(
        "Skipping '{}' - it leads out of the content root to '{}'",
        path.display(),
        real_path.display()
    );

    None
}

/// Etag and size of the file - it is read by the chunks, so it is not kept in the memory whole
fn calc_file_etag(path: &str) -> std::io::Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut chunk = vec![0u8; STREAM_CHUNK_SIZE];
    let mut hasher = EtagHasher::default();
    let mut size = 0;

    loop {
        let read = match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };

        hasher.update(&chunk[..read]);
        size += read as u64;
    }

    Ok((hasher.finish(), size))
}

/// Creates the copy of the file compressed with the algorithm of the folder - if it makes sense.
/// The copy is written to the temp file and gets its name when it is complete
fn create_copy(
    raw_path: &str,
    raw_size: u64,
    uri_path: &str,
    folder: &CopiesFolder,
) -> Option<FileVariant> {
    let path = format!("{}{}", folder.path, uri_path);
    let temp_path = compile_temp_path(path.as_str());

    match write_copy(raw_path, raw_size, temp_path.as_str(), folder.algorithm) {
        Ok(Some((etag, size))) => match std::fs::rename(temp_path.as_str(), path.as_str()) {
            Ok(()) => return Some(FileVariant { path, etag, size }),
            Err(err) => println!(
                "Can not create the compressed copy '{}'. Err: {}",
                path, err
            ),
        },
        // the copy is not much smaller than the file - it is not worth it
        Ok(None) => {}
        Err(err) => println!(
            "Can not create the compressed copy '{}'. Err: {}",
            path, err
        ),
    }

    let _ = std::fs::remove_file(temp_path.as_str());
    None
}

/// Writes the compressed copy - the file is read and the copy is written by the chunks.
/// None - the copy is not much smaller than the file
fn write_copy(
    raw_path: &str,
    raw_size: u64,
    path: &str,
    algorithm: CompressionAlgorithm,
) -> std::io::Result<Option<(String, u64)>> {
    if let Some(folder) = Path::new(path).parent() {
        std::fs::create_dir_all(folder)?;
    }

    let mut reader = BufReader::with_capacity(STREAM_CHUNK_SIZE, File::open(raw_path)?);
    let mut writer = CopyWriter::new(File::create(path)?);

    algorithm.compress(&mut reader, &mut writer, raw_size)?;

    let (etag, size) = writer.finish()?;

    if !is_compressed_well(raw_size, size) {
        return Ok(None);
    }

    Ok(Some((etag, size)))
}

/// The temp file is hidden: no file of the content is hidden - they are skipped by the scan,
/// so the temp file never takes the name of the copy of another file
fn compile_temp_path(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((folder, name)) => format!("{}/.{}{}", folder, name, TEMP_EXTENSION),
        None => format!(".{}{}", path, TEMP_EXTENSION),
    }
}

/// Writes the copy to the file by the chunks - calculating its etag and its size on the way
struct CopyWriter {
    file: BufWriter<File>,
    hasher: EtagHasher,
    size: u64,
}

impl CopyWriter {
    fn new(file: File) -> Self {
        Self {
            file: BufWriter::with_capacity(STREAM_CHUNK_SIZE, file),
            hasher: EtagHasher::default(),
            size: 0,
        }
    }

    /// The etag and the size of the copy - when all of it is written to the file
    fn finish(mut self) -> std::io::Result<(String, u64)> {
        self.file.flush()?;
        Ok((self.hasher.finish(), self.size))
    }
}

impl Write for CopyWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.file.write(buf)?;
        self.hasher.update(&buf[..written]);
        self.size += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

fn is_compressed_format(uri_path: &str) -> bool {
    let file_name = uri_path.rsplit('/').next().unwrap_or(uri_path);

    let Some((_, extension)) = file_name.rsplit_once('.') else {
        return false;
    };

    COMPRESSED_FORMATS
        .iter()
        .any(|format| extension.eq_ignore_ascii_case(format))
}

/// Hidden file or folder - its name starts with the dot. '.well-known' is the exception:
/// it is hidden by the name, but it is the content of the site
fn is_hidden(name: &str) -> bool {
    name.starts_with('.') && name != WELL_KNOWN_FOLDER
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::static_files::{calc_etag, compile_text, decompress, CompressionQueue, TestFolder};

    /// All the files of the folder and of its subfolders
    fn list_files(folder: &std::path::Path) -> Vec<String> {
        let mut result = Vec::new();

        for entry in std::fs::read_dir(folder).unwrap().flatten() {
            let path = entry.path();

            if path.is_dir() {
                result.extend(list_files(&path));
            } else {
                result.push(path.to_str().unwrap().to_string());
            }
        }

        result
    }

    const BOTH: AcceptedEncodings = AcceptedEncodings {
        brotli: true,
        gzip: true,
    };

    const BR_ONLY: AcceptedEncodings = AcceptedEncodings {
        brotli: true,
        gzip: false,
    };

    fn create_copies_folders(site: &TestFolder) -> Vec<CopiesFolder> {
        compile_copies_folders(site.path.to_str().unwrap())
    }

    /// Real location of the file - the temp folder itself is a symlink on macOS
    fn real_path(path: &str) -> String {
        std::fs::canonicalize(path)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    }

    /// Scans the content and prepares all its files - the way the background thread does
    fn create(
        www_root: &str,
        site_folders: &[&str],
        copies_folders: Vec<CopiesFolder>,
    ) -> ContentFiles {
        let content_files = ContentFiles::scan(www_root, site_folders);

        let queue = CompressionQueue::default();
        queue.enqueue(content_files.files_by_size());
        queue.prepare_all(&prepare_copies_folders(copies_folders));

        content_files
    }

    fn get_file<'s>(
        content_files: &'s ContentFiles,
        folder: &str,
        uri_path: &str,
    ) -> &'s ContentFile {
        content_files.get(folder, uri_path).unwrap()
    }

    fn raw_of(file: &ContentFile) -> &FileVariant {
        file.get_raw().unwrap().as_ref().unwrap()
    }

    /// The copies the file has - in the order of preference of the algorithms
    fn copies_of(file: &ContentFile) -> Vec<(CompressionAlgorithm, &FileVariant)> {
        CompressionAlgorithm::ALL
            .into_iter()
            .filter_map(|algorithm| {
                let copy = file.get_copy(algorithm).unwrap().as_ref()?;
                Some((algorithm, copy))
            })
            .collect()
    }

    fn chosen(
        file: &ContentFile,
        accepted: AcceptedEncodings,
    ) -> (&FileVariant, Option<CompressionAlgorithm>) {
        match file.choose_variant(accepted) {
            VariantChoice::Ready(variant, encoding) => (variant, encoding),
            _ => panic!("no variant is ready"),
        }
    }

    fn create_variant(path: &str) -> FileVariant {
        FileVariant {
            path: path.to_string(),
            etag: calc_etag(path.as_bytes()),
            size: 0,
        }
    }

    #[test]
    fn creates_the_compressed_copies() {
        let site = TestFolder::new("compressed-copies");
        let text = compile_text(50 * 1024);

        site.add_file("wwwroot/es/app.js", &text)
            .add_file("wwwroot/small.js", "small")
            .add_file("wwwroot/image.PNG", &text);

        let www_root = site.get_folder("wwwroot");
        let content_files = create(www_root.as_str(), &[""], create_copies_folders(&site));

        assert_eq!(content_files.files_count(), 3);

        let file = get_file(&content_files, www_root.as_str(), "/es/app.js");
        assert_eq!(
            raw_of(file).path,
            real_path(&format!("{}/es/app.js", www_root))
        );

        let copies = copies_of(file);
        let algorithms: Vec<_> = copies.iter().map(|(algorithm, _)| *algorithm).collect();
        assert_eq!(algorithms, CompressionAlgorithm::ALL);

        for (algorithm, copy) in copies {
            let copies_folder = site.get_folder(&format!("wwwroot-{}", algorithm.as_str()));
            assert_eq!(copy.path, format!("{}/es/app.js", copies_folder));

            let compressed = std::fs::read(copy.path.as_str()).unwrap();
            assert_eq!(decompress(algorithm, &compressed), text);

            // the etag and the size are calculated while the copy is written
            assert_eq!(copy.etag, calc_etag(&compressed), "{:?}", algorithm);
            assert_eq!(copy.size, compressed.len() as u64, "{:?}", algorithm);

            // the temp file has got the name of the copy - nothing is left behind
            assert_eq!(
                list_files(std::path::Path::new(copies_folder.as_str())),
                [copy.path.as_str()],
                "{:?}",
                algorithm
            );
        }

        // the small file and the file of the compressed format have no copies
        for uri_path in ["/small.js", "/image.PNG"] {
            let file = get_file(&content_files, www_root.as_str(), uri_path);
            assert!(copies_of(file).is_empty(), "{}", uri_path);
            assert!(file.get_raw().is_some(), "{}", uri_path);
        }
    }

    #[test]
    fn calculates_the_etag_of_the_big_file_by_the_chunks() {
        let site = TestFolder::new("big-file-etag");
        let text = compile_text(STREAM_CHUNK_SIZE * 3 + 7);
        site.add_file("wwwroot/app.js", &text);

        let www_root = site.get_folder("wwwroot");
        let content_files = create(www_root.as_str(), &[""], create_copies_folders(&site));
        let file = get_file(&content_files, www_root.as_str(), "/app.js");

        assert_eq!(raw_of(file).etag, calc_etag(&text));
        assert_eq!(raw_of(file).size, text.len() as u64);

        for (algorithm, copy) in copies_of(file) {
            let compressed = std::fs::read(copy.path.as_str()).unwrap();
            assert_eq!(decompress(algorithm, &compressed), text, "{:?}", algorithm);
        }
    }

    #[test]
    fn copy_which_is_not_compressed_well_leaves_nothing() {
        let site = TestFolder::new("not-compressed-well");

        // random bytes - but of the format which is not known to be compressed
        let mut content = Vec::with_capacity(50 * 1024);
        let mut seed: u32 = 0x9E37_79B1;
        for _ in 0..50 * 1024 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            content.push((seed >> 24) as u8);
        }

        site.add_file("wwwroot/data.bin", &content);

        let www_root = site.get_folder("wwwroot");
        let content_files = create(www_root.as_str(), &[""], create_copies_folders(&site));
        let file = get_file(&content_files, www_root.as_str(), "/data.bin");

        assert!(copies_of(file).is_empty());

        for folder in ["wwwroot-br", "wwwroot-gzip"] {
            assert!(list_files(&site.path.join(folder)).is_empty(), "{}", folder);
        }
    }

    #[test]
    fn temp_file_is_hidden() {
        assert_eq!(
            compile_temp_path("/root/wwwroot-br/assets/app.js"),
            "/root/wwwroot-br/assets/.app.js.temp"
        );
        assert_eq!(compile_temp_path("app.js"), ".app.js.temp");

        // the copy of the file 'app.js.temp' is not the temp file of 'app.js'
        assert_ne!(
            compile_temp_path("/root/wwwroot-br/app.js"),
            "/root/wwwroot-br/app.js.temp"
        );
    }

    #[test]
    fn each_variant_has_its_own_etag() {
        let site = TestFolder::new("variants-etags");
        let text = compile_text(50 * 1024);
        site.add_file("wwwroot/app.js", &text);

        let www_root = site.get_folder("wwwroot");
        let content_files = create(www_root.as_str(), &[""], create_copies_folders(&site));

        let file = get_file(&content_files, www_root.as_str(), "/app.js");
        assert_eq!(raw_of(file).etag, calc_etag(&text));

        let mut etags = vec![raw_of(file).etag.as_str()];
        etags.extend(copies_of(file).iter().map(|(_, copy)| copy.etag.as_str()));
        etags.sort();
        etags.dedup();

        assert_eq!(etags.len(), 3);
    }

    #[test]
    fn each_device_has_its_own_copies() {
        let site = TestFolder::new("device-copies");
        let desktop = compile_text(50 * 1024);
        let mobile = compile_text(40 * 1024);

        site.add_file("wwwroot/desktop/app.js", &desktop)
            .add_file("wwwroot/mobile/app.js", &mobile)
            .add_file("wwwroot/not-served.js", &desktop);

        let www_root = site.get_folder("wwwroot");
        let content_files = create(
            www_root.as_str(),
            &["/desktop", "/mobile"],
            create_copies_folders(&site),
        );

        assert_eq!(content_files.files_count(), 2);

        for (device, text) in [("desktop", &desktop), ("mobile", &mobile)] {
            let device_folder = format!("{}/{}", www_root, device);
            let file = get_file(&content_files, device_folder.as_str(), "/app.js");
            let copies = copies_of(file);

            assert_eq!(copies.len(), 2, "{}", device);

            for (algorithm, copy) in copies {
                let copies_folder = site.get_folder(&format!("wwwroot-{}", algorithm.as_str()));
                assert_eq!(copy.path, format!("{}/{}/app.js", copies_folder, device));

                let compressed = std::fs::read(copy.path.as_str()).unwrap();
                assert_eq!(&decompress(algorithm, &compressed), text);
            }
        }

        // the files outside of the device folders are not served in WITH_MOBILE mode
        assert!(content_files
            .get(www_root.as_str(), "/not-served.js")
            .is_none());
    }

    #[test]
    fn removes_the_copies_of_the_previous_start() {
        let site = TestFolder::new("previous-copies");

        site.add_file("wwwroot/app.js", compile_text(50 * 1024))
            .add_file("wwwroot-br/removed.js", "previous")
            .add_file("wwwroot-gzip/removed.js", "previous");

        create(
            site.get_folder("wwwroot").as_str(),
            &[""],
            create_copies_folders(&site),
        );

        for folder in ["wwwroot-br", "wwwroot-gzip"] {
            assert!(
                !site.path.join(folder).join("removed.js").exists(),
                "{}",
                folder
            );
            assert!(site.path.join(folder).join("app.js").exists(), "{}", folder);
        }
    }

    #[test]
    fn serves_the_raw_files_without_the_copies_folders() {
        let site = TestFolder::new("no-copies-folders");
        site.add_file("wwwroot/app.js", compile_text(50 * 1024));

        let www_root = site.get_folder("wwwroot");
        let content_files = create(www_root.as_str(), &[""], vec![]);

        let file = get_file(&content_files, www_root.as_str(), "/app.js");
        assert!(copies_of(file).is_empty());

        let (variant, encoding) = chosen(file, BOTH);
        assert_eq!(variant.path, raw_of(file).path);
        assert_eq!(encoding, None);
    }

    #[test]
    fn chooses_the_variant_by_the_accepted_encodings() {
        let site = TestFolder::new("choose-variant");

        site.add_file("wwwroot/app.js", compile_text(50 * 1024))
            .add_file("wwwroot/small.js", "small");

        let www_root = site.get_folder("wwwroot");
        let content_files = create(www_root.as_str(), &[""], create_copies_folders(&site));

        let file = get_file(&content_files, www_root.as_str(), "/app.js");
        let copies = copies_of(file);

        let (variant, encoding) = chosen(file, BOTH);
        assert_eq!(encoding, Some(CompressionAlgorithm::Brotli));
        assert_eq!(variant.path, copies[0].1.path);

        let gzip_only = AcceptedEncodings {
            brotli: false,
            gzip: true,
        };

        let (variant, encoding) = chosen(file, gzip_only);
        assert_eq!(encoding, Some(CompressionAlgorithm::Gzip));
        assert_eq!(variant.path, copies[1].1.path);

        let (variant, encoding) = chosen(file, AcceptedEncodings::default());
        assert_eq!(encoding, None);
        assert_eq!(variant.path, raw_of(file).path);

        // the file without the copies is sent raw - whatever the client accepts
        let file = get_file(&content_files, www_root.as_str(), "/small.js");
        let (variant, encoding) = chosen(file, BOTH);
        assert_eq!(encoding, None);
        assert_eq!(variant.path, raw_of(file).path);
    }

    #[test]
    fn waits_for_the_best_copy_the_client_accepts() {
        let file = ContentFile::new("/app.js".to_string(), "/app.js".to_string(), 50 * 1024);

        let gzip_only = AcceptedEncodings {
            brotli: false,
            gzip: true,
        };

        // nothing is prepared yet
        assert!(matches!(file.choose_variant(BOTH), VariantChoice::NotReady));

        // the file in the middle of its preparation: the etag and the gzip copy, which goes
        // first, are ready - the brotli copy is not yet
        let _ = file.raw.set(Ok(create_variant("/app.js")));
        let _ = file.gzip_copy.set(Some(create_variant("/app.js.gz")));

        // the client which accepts the brotli waits for it - it does not get the gzip copy,
        // which it would keep with its etag
        assert!(matches!(file.choose_variant(BOTH), VariantChoice::NotReady));
        assert!(matches!(
            file.choose_variant(BR_ONLY),
            VariantChoice::NotReady
        ));

        // the clients which do not accept the brotli do not wait for it
        assert_eq!(chosen(&file, gzip_only).1, Some(CompressionAlgorithm::Gzip));
        assert_eq!(chosen(&file, AcceptedEncodings::default()).1, None);

        let _ = file.br_copy.set(Some(create_variant("/app.js.br")));

        assert_eq!(chosen(&file, BOTH).1, Some(CompressionAlgorithm::Brotli));
        assert_eq!(chosen(&file, BR_ONLY).1, Some(CompressionAlgorithm::Brotli));
    }

    #[test]
    fn sends_the_next_copy_if_there_is_no_better_one() {
        let file = ContentFile::new("/app.js".to_string(), "/app.js".to_string(), 50 * 1024);

        // the brotli copy is not created: it is not compressed well enough
        let _ = file.raw.set(Ok(create_variant("/app.js")));
        let _ = file.br_copy.set(None);
        let _ = file.gzip_copy.set(Some(create_variant("/app.js.gz")));

        assert_eq!(chosen(&file, BOTH).1, Some(CompressionAlgorithm::Gzip));
        assert_eq!(chosen(&file, BR_ONLY).1, None);
    }

    #[test]
    fn small_file_waits_for_the_etag_only() {
        let site = TestFolder::new("small-file");
        site.add_file("wwwroot/small.js", "small")
            .add_file("wwwroot/photo.jpg", compile_text(50 * 1024));

        let www_root = site.get_folder("wwwroot");
        let content_files = ContentFiles::scan(www_root.as_str(), &[""]);

        for uri_path in ["/small.js", "/photo.jpg"] {
            let file = get_file(&content_files, www_root.as_str(), uri_path);

            // it is known at the scan: there are no copies to wait for
            for algorithm in CompressionAlgorithm::ALL {
                assert!(
                    matches!(file.get_copy(algorithm), Some(None)),
                    "{}",
                    uri_path
                );
            }

            assert!(matches!(file.choose_variant(BOTH), VariantChoice::NotReady));

            file.prepare(&[]);

            assert_eq!(chosen(file, BOTH).1, None, "{}", uri_path);
            assert!(file.is_prepared(), "{}", uri_path);
        }
    }

    #[test]
    fn scan_only_finds_the_files() {
        let site = TestFolder::new("scan-only");

        site.add_file("wwwroot/big.js", compile_text(50 * 1024))
            .add_file("wwwroot/medium.js", compile_text(20 * 1024))
            .add_file("wwwroot/small.js", "small");

        let www_root = site.get_folder("wwwroot");
        let content_files = ContentFiles::scan(www_root.as_str(), &[""]);

        // nothing is read or compressed by the scan
        let files = content_files.files_by_size();
        assert!(files.iter().all(|file| file.get_raw().is_none()));

        // the smallest are prepared first
        let names: Vec<_> = files
            .iter()
            .map(|file| file.raw_path.rsplit('/').next().unwrap())
            .collect();

        assert_eq!(names, ["small.js", "medium.js", "big.js"]);
    }

    #[test]
    fn keeps_the_failed_preparation() {
        let file = ContentFile::new(
            "/removed.js".to_string(),
            "/there/is/no/such/file.js".to_string(),
            50 * 1024,
        );

        file.prepare(&[]);

        // the request which waits for the file gets the error - it does not wait forever
        assert!(matches!(file.get_raw(), Some(Err(_))));
        assert!(matches!(
            file.choose_variant(BOTH),
            VariantChoice::Failed(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn follows_the_symlinks_within_the_root() {
        let site = TestFolder::new("symlinks-within-root");

        site.add_file("wwwroot/es/app.js", "app")
            .add_file("wwwroot/shared/lib.js", "lib");

        let www_root = site.get_folder("wwwroot");
        let symlink = |target: &str, link: &str| {
            std::os::unix::fs::symlink(site.path.join(target), site.path.join(link)).unwrap()
        };

        symlink("wwwroot/shared", "wwwroot/es/lib");
        symlink("wwwroot/shared/lib.js", "wwwroot/lib.js");
        symlink("wwwroot", "wwwroot/es/loop");

        let content_files = ContentFiles::scan(www_root.as_str(), &[""]);

        assert!(content_files.get(www_root.as_str(), "/es/app.js").is_some());

        // the file is read by its real location - whatever path it is requested by
        let lib_path = real_path(&format!("{}/shared/lib.js", www_root));

        for uri_path in ["/shared/lib.js", "/es/lib/lib.js", "/lib.js"] {
            let file = content_files.get(www_root.as_str(), uri_path).unwrap();
            assert_eq!(file.raw_path, lib_path, "{}", uri_path);
        }

        assert!(content_files
            .get(www_root.as_str(), "/es/loop/es/app.js")
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_the_symlinks_out_of_the_root() {
        let site = TestFolder::new("symlinks-out-of-root");

        site.add_file("wwwroot/app.js", "app")
            .add_file("outside/secret.txt", "secret")
            .add_file("wwwroot-evil/secret.txt", "secret");

        let www_root = site.get_folder("wwwroot");
        let symlink = |target: &str, link: &str| {
            std::os::unix::fs::symlink(site.path.join(target), site.path.join(link)).unwrap()
        };

        symlink("outside/secret.txt", "wwwroot/leak.txt");
        symlink("outside", "wwwroot/leak-folder");
        // the folder whose name starts with the name of the root folder is not within it
        symlink("wwwroot-evil", "wwwroot/evil");

        let content_files = ContentFiles::scan(www_root.as_str(), &[""]);

        assert_eq!(content_files.files_count(), 1);
        assert!(content_files.get(www_root.as_str(), "/app.js").is_some());

        for uri_path in ["/leak.txt", "/leak-folder/secret.txt", "/evil/secret.txt"] {
            assert!(
                content_files.get(www_root.as_str(), uri_path).is_none(),
                "{}",
                uri_path
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_re_pointed_after_the_scan_does_not_lead_out_of_the_root() {
        let site = TestFolder::new("symlinks-re-pointed");

        site.add_file("wwwroot/shared/lib.js", "lib")
            .add_file("outside/secret.txt", "secret");

        let www_root = site.get_folder("wwwroot");
        let link = site.path.join("wwwroot/lib.js");

        std::os::unix::fs::symlink(site.path.join("wwwroot/shared/lib.js"), &link).unwrap();

        let content_files = ContentFiles::scan(www_root.as_str(), &[""]);

        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(site.path.join("outside/secret.txt"), &link).unwrap();

        let file = content_files.get(www_root.as_str(), "/lib.js").unwrap();
        assert_eq!(
            std::fs::read_to_string(file.raw_path.as_str()).unwrap(),
            "lib"
        );

        // the file is prepared in the background - after the symlink is re-pointed
        file.prepare(&[]);
        assert_eq!(raw_of(file).etag, calc_etag(b"lib"));
    }

    #[test]
    fn hidden_files_are_not_served() {
        let site = TestFolder::new("hidden-files");

        site.add_file("wwwroot/app.js", "app")
            .add_file("wwwroot/.env", "secret")
            .add_file("wwwroot/.git/config", "secret")
            .add_file("wwwroot/assets/.hidden.js", "secret")
            .add_file("wwwroot/.well-known/acme-challenge/token", "token")
            .add_file("wwwroot/.well-known/.hidden", "secret");

        let www_root = site.get_folder("wwwroot");
        let content_files = ContentFiles::scan(www_root.as_str(), &[""]);

        assert_eq!(content_files.files_count(), 2);
        assert!(content_files.get(www_root.as_str(), "/app.js").is_some());
        assert!(content_files
            .get(www_root.as_str(), "/.well-known/acme-challenge/token")
            .is_some());

        for uri_path in [
            "/.env",
            "/.git/config",
            "/assets/.hidden.js",
            "/.well-known/.hidden",
        ] {
            assert!(
                content_files.get(www_root.as_str(), uri_path).is_none(),
                "{}",
                uri_path
            );
        }
    }

    #[test]
    fn detects_the_hidden_names() {
        assert!(is_hidden(".env"));
        assert!(is_hidden(".git"));
        assert!(is_hidden(".DS_Store"));
        assert!(!is_hidden(".well-known"));
        assert!(!is_hidden("app.js"));
        assert!(!is_hidden("well-known"));
    }

    #[test]
    fn detects_the_compressed_formats() {
        assert!(is_compressed_format("/img/photo.JPG"));
        assert!(is_compressed_format("/fonts/inter.woff2"));
        assert!(!is_compressed_format("/app.js"));
        assert!(!is_compressed_format("/icons/logo.svg"));
        assert!(!is_compressed_format("/folder.png/app.js"));
        assert!(!is_compressed_format("/LICENSE"));
    }

    #[test]
    fn compiles_the_file_name() {
        assert_eq!(
            compile_file_name("./wwwroot", "/index.html"),
            "./wwwroot/index.html"
        );
        assert_eq!(
            compile_file_name("./wwwroot", "index.html"),
            "./wwwroot/index.html"
        );
    }

    #[test]
    fn compiles_the_copies_folders() {
        let folders = compile_copies_folders("/root/");

        let folders: Vec<_> = folders
            .iter()
            .map(|folder| (folder.algorithm, folder.path.as_str()))
            .collect();

        assert_eq!(
            folders,
            [
                (CompressionAlgorithm::Brotli, "/root/wwwroot-br"),
                (CompressionAlgorithm::Gzip, "/root/wwwroot-gzip"),
            ]
        );
    }
}
