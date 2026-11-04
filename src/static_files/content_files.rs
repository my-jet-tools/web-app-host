use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use super::{calc_etag, try_compress, AcceptedEncodings, CompressionAlgorithm};

/// Formats which are compressed already: images, video, audio, fonts and archives.
/// Compressing them once more takes the time and makes them no smaller
const COMPRESSED_FORMATS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "avif", "heic", "heif", "jxl", "mp4", "m4v", "webm",
    "mov", "mkv", "avi", "ogv", "mp3", "m4a", "aac", "ogg", "oga", "opus", "flac", "weba", "woff",
    "woff2", "zip", "gz", "tgz", "br", "zst", "bz2", "xz", "7z", "rar",
];

/// Folder of the copies of the content compressed with the algorithm.
/// It has the same structure as the root folder of the content
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

/// File on the disk we send to the client
pub struct FileVariant {
    pub path: String,
    /// Etag of the content of this very file - so the raw file and each of its
    /// compressed copies have their own etag
    pub etag: String,
}

/// Copy of the file compressed with the algorithm
pub struct CompressedCopy {
    pub algorithm: CompressionAlgorithm,
    pub file: FileVariant,
}

/// File of the content - with its compressed copies
pub struct ContentFile {
    pub raw: FileVariant,
    /// In the order of preference of the algorithms. The small file, the file of the compressed
    /// format and the file which is not compressed well have no copies
    pub copies: Vec<CompressedCopy>,
}

impl ContentFile {
    /// File we send to the client: the copy of the first algorithm it accepts -
    /// or the raw file, if it accepts none of them
    pub fn choose_variant(
        &self,
        accepted: AcceptedEncodings,
    ) -> (&FileVariant, Option<CompressionAlgorithm>) {
        for copy in self.copies.iter() {
            if accepted.accepts(copy.algorithm) {
                return (&copy.file, Some(copy.algorithm));
            }
        }

        (&self.raw, None)
    }
}

/// Files of the content. The content is scanned once - at the start of the service:
/// the file which appears later is not served, the file which is changed later is served
/// with the etag and the compressed copies of its content at the start
#[derive(Default)]
pub struct ContentFiles {
    /// The key is the path the request is mapped to: the site folder and the uri path of the file
    files: HashMap<String, ContentFile>,
}

impl ContentFiles {
    /// Scans the folders of the sites within the root folder and creates the compressed copies
    /// of their files - with the same structure. The site is the root folder itself ("") -
    /// or its desktop and mobile folders, when WITH_MOBILE is enabled.
    /// The copies are compressed with the maximum level - in all the threads we have
    pub fn create(www_root: &str, site_folders: &[&str], copies_folders: &[CopiesFolder]) -> Self {
        let copies_folders = prepare_copies_folders(copies_folders);

        let mut raw_files = Vec::new();

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
                        &mut raw_files,
                    );
                }
            }
            Err(err) => println!("Can not read the folder '{}'. Err: {}", www_root, err),
        }

        // the biggest files take the most time - they go first, so no thread is left
        // compressing the big one when the others are done
        raw_files.sort_by_key(|raw_file| std::cmp::Reverse(raw_file.size));

        Self {
            files: compile_in_parallel(www_root, &raw_files, &copies_folders),
        }
    }

    /// File of the site folder - by the path of the request. It is the only way the request
    /// reaches the disk: the file is read by the path resolved at the scan, never by the path
    /// of the request. So even the path which gets through `is_safe_uri_path` (with the double
    /// encoded dots, for example) resolves to the scanned file of the root folder - or to nothing
    pub fn get(&self, folder: &str, uri_path: &str) -> Option<&ContentFile> {
        self.files.get(compile_file_name(folder, uri_path).as_str())
    }

    pub fn files_count(&self) -> usize {
        self.files.len()
    }

    /// Amount of the files which have the copy compressed with the algorithm
    pub fn copies_count(&self, algorithm: CompressionAlgorithm) -> usize {
        self.files
            .values()
            .filter(|file| file.copies.iter().any(|copy| copy.algorithm == algorithm))
            .count()
    }
}

fn compile_file_name(folder: &str, uri_path: &str) -> String {
    if uri_path.starts_with('/') {
        format!("{}{}", folder, uri_path)
    } else {
        format!("{}/{}", folder, uri_path)
    }
}

/// The copies of the previous start are removed - they are the copies of the previous content.
/// The folder we can not create is skipped: the files are served without its copies
fn prepare_copies_folders(copies_folders: &[CopiesFolder]) -> Vec<&CopiesFolder> {
    copies_folders
        .iter()
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

fn recreate_folder(path: &str) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }

    std::fs::create_dir_all(path)
}

/// File of the content
struct RawFile {
    /// Path of the file within the root folder: '/es/app.js' - or '/mobile/es/app.js'
    uri_path: String,
    /// Real location of the file - with the symlinks resolved. It is checked to be within
    /// the root folder, and the file is read by it: the symlink re-pointed after the scan
    /// does not lead the requests out of the root folder
    real_path: String,
    size: u64,
}

/// Collects the files of the folder and of its subfolders. Symlinks are followed only within
/// the root folder - the content outside of it is never served. The symlink to the folder above
/// is not followed as well: such a loop would never end
fn collect_files(
    folder: &Path,
    uri_path: &str,
    root: &Path,
    ancestors: &mut Vec<PathBuf>,
    result: &mut Vec<RawFile>,
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

            result.push(RawFile {
                uri_path: file_uri_path,
                real_path,
                size: metadata.len(),
            });
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

/// Compiles the files in all the threads we have - their number respects the CPU limit
/// of the container
fn compile_in_parallel(
    www_root: &str,
    raw_files: &[RawFile],
    copies_folders: &[&CopiesFolder],
) -> HashMap<String, ContentFile> {
    let threads = std::thread::available_parallelism()
        .map(|threads| threads.get())
        .unwrap_or(1)
        .min(raw_files.len())
        .max(1);

    let next_file = &AtomicUsize::new(0);

    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(move || {
                    let mut result = Vec::new();

                    while let Some(raw_file) =
                        raw_files.get(next_file.fetch_add(1, Ordering::Relaxed))
                    {
                        if let Some(content_file) =
                            compile_content_file(www_root, raw_file, copies_folders)
                        {
                            result.push(content_file);
                        }
                    }

                    result
                })
            })
            .collect();

        workers
            .into_iter()
            .flat_map(|worker| {
                worker
                    .join()
                    .expect("Compression of the content has panicked")
            })
            .collect()
    })
}

/// Reads the raw file and creates its compressed copies. The file we can not read is not served.
/// Returns the file with the path the request is mapped to
fn compile_content_file(
    www_root: &str,
    raw_file: &RawFile,
    copies_folders: &[&CopiesFolder],
) -> Option<(String, ContentFile)> {
    let raw = match std::fs::read(raw_file.real_path.as_str()) {
        Ok(raw) => raw,
        Err(err) => {
            println!(
                "Can not read the file '{}'. Err: {}",
                raw_file.real_path, err
            );
            return None;
        }
    };

    let uri_path = raw_file.uri_path.as_str();

    let copies = if is_compressed_format(uri_path) {
        vec![]
    } else {
        copies_folders
            .iter()
            .filter_map(|folder| create_copy(raw.as_slice(), uri_path, folder))
            .collect()
    };

    let content_file = ContentFile {
        raw: FileVariant {
            etag: calc_etag(raw.as_slice()),
            path: raw_file.real_path.clone(),
        },
        copies,
    };

    Some((format!("{}{}", www_root, uri_path), content_file))
}

/// Creates the copy of the file compressed with the algorithm of the folder - if it makes sense
fn create_copy(raw: &[u8], uri_path: &str, folder: &CopiesFolder) -> Option<CompressedCopy> {
    let compressed = try_compress(raw, folder.algorithm)?;
    let path = format!("{}{}", folder.path, uri_path);

    if let Err(err) = write_file(path.as_str(), compressed.as_slice()) {
        println!(
            "Can not create the compressed copy '{}'. Err: {}",
            path, err
        );
        return None;
    }

    Some(CompressedCopy {
        algorithm: folder.algorithm,
        file: FileVariant {
            etag: calc_etag(compressed.as_slice()),
            path,
        },
    })
}

fn write_file(path: &str, content: &[u8]) -> std::io::Result<()> {
    if let Some(folder) = Path::new(path).parent() {
        std::fs::create_dir_all(folder)?;
    }

    std::fs::write(path, content)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::static_files::{compile_text, decompress, TestFolder};

    const BOTH: AcceptedEncodings = AcceptedEncodings {
        brotli: true,
        gzip: true,
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

    #[test]
    fn creates_the_compressed_copies() {
        let site = TestFolder::new("compressed-copies");
        let text = compile_text(50 * 1024);

        site.add_file("wwwroot/es/app.js", &text)
            .add_file("wwwroot/small.js", "small")
            .add_file("wwwroot/image.PNG", &text);

        let www_root = site.get_folder("wwwroot");
        let content_files =
            ContentFiles::create(www_root.as_str(), &[""], &create_copies_folders(&site));

        assert_eq!(content_files.files_count(), 3);

        let file = content_files.get(www_root.as_str(), "/es/app.js").unwrap();
        assert_eq!(file.raw.path, real_path(&format!("{}/es/app.js", www_root)));

        let algorithms: Vec<_> = file.copies.iter().map(|copy| copy.algorithm).collect();
        assert_eq!(algorithms, CompressionAlgorithm::ALL);

        for copy in file.copies.iter() {
            let copies_folder = site.get_folder(&format!("wwwroot-{}", copy.algorithm.as_str()));
            assert_eq!(copy.file.path, format!("{}/es/app.js", copies_folder));

            let compressed = std::fs::read(copy.file.path.as_str()).unwrap();
            assert_eq!(decompress(copy.algorithm, &compressed), text);
        }

        // the small file and the file of the compressed format have no copies
        for uri_path in ["/small.js", "/image.PNG"] {
            let file = content_files.get(www_root.as_str(), uri_path).unwrap();
            assert!(file.copies.is_empty(), "{}", uri_path);
        }

        assert_eq!(content_files.copies_count(CompressionAlgorithm::Brotli), 1);
        assert_eq!(content_files.copies_count(CompressionAlgorithm::Gzip), 1);
    }

    #[test]
    fn each_variant_has_its_own_etag() {
        let site = TestFolder::new("variants-etags");
        let text = compile_text(50 * 1024);
        site.add_file("wwwroot/app.js", &text);

        let www_root = site.get_folder("wwwroot");
        let content_files =
            ContentFiles::create(www_root.as_str(), &[""], &create_copies_folders(&site));

        let file = content_files.get(www_root.as_str(), "/app.js").unwrap();
        assert_eq!(file.raw.etag, calc_etag(&text));

        let mut etags = vec![file.raw.etag.as_str()];
        etags.extend(file.copies.iter().map(|copy| copy.file.etag.as_str()));
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
        let content_files = ContentFiles::create(
            www_root.as_str(),
            &["/desktop", "/mobile"],
            &create_copies_folders(&site),
        );

        assert_eq!(content_files.files_count(), 2);

        for (device, text) in [("desktop", &desktop), ("mobile", &mobile)] {
            let device_folder = format!("{}/{}", www_root, device);
            let file = content_files
                .get(device_folder.as_str(), "/app.js")
                .unwrap();

            assert_eq!(file.copies.len(), 2, "{}", device);

            for copy in file.copies.iter() {
                let copies_folder =
                    site.get_folder(&format!("wwwroot-{}", copy.algorithm.as_str()));
                assert_eq!(
                    copy.file.path,
                    format!("{}/{}/app.js", copies_folder, device)
                );

                let compressed = std::fs::read(copy.file.path.as_str()).unwrap();
                assert_eq!(&decompress(copy.algorithm, &compressed), text);
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

        ContentFiles::create(
            site.get_folder("wwwroot").as_str(),
            &[""],
            &create_copies_folders(&site),
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
        let content_files = ContentFiles::create(www_root.as_str(), &[""], &[]);

        let file = content_files.get(www_root.as_str(), "/app.js").unwrap();
        assert!(file.copies.is_empty());

        let (variant, encoding) = file.choose_variant(BOTH);
        assert_eq!(variant.path, file.raw.path);
        assert_eq!(encoding, None);
    }

    #[test]
    fn chooses_the_variant_by_the_accepted_encodings() {
        let site = TestFolder::new("choose-variant");

        site.add_file("wwwroot/app.js", compile_text(50 * 1024))
            .add_file("wwwroot/small.js", "small");

        let www_root = site.get_folder("wwwroot");
        let content_files =
            ContentFiles::create(www_root.as_str(), &[""], &create_copies_folders(&site));

        let file = content_files.get(www_root.as_str(), "/app.js").unwrap();

        let (variant, encoding) = file.choose_variant(BOTH);
        assert_eq!(encoding, Some(CompressionAlgorithm::Brotli));
        assert_eq!(variant.path, file.copies[0].file.path);

        let gzip_only = AcceptedEncodings {
            brotli: false,
            gzip: true,
        };

        let (variant, encoding) = file.choose_variant(gzip_only);
        assert_eq!(encoding, Some(CompressionAlgorithm::Gzip));
        assert_eq!(variant.path, file.copies[1].file.path);

        let (variant, encoding) = file.choose_variant(AcceptedEncodings::default());
        assert_eq!(encoding, None);
        assert_eq!(variant.path, file.raw.path);

        // the file without the copies is sent raw - whatever the client accepts
        let file = content_files.get(www_root.as_str(), "/small.js").unwrap();
        let (variant, encoding) = file.choose_variant(BOTH);
        assert_eq!(encoding, None);
        assert_eq!(variant.path, file.raw.path);
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

        let content_files = ContentFiles::create(www_root.as_str(), &[""], &[]);

        assert!(content_files.get(www_root.as_str(), "/es/app.js").is_some());

        // the file is read by its real location - whatever path it is requested by
        let lib_path = real_path(&format!("{}/shared/lib.js", www_root));

        for uri_path in ["/shared/lib.js", "/es/lib/lib.js", "/lib.js"] {
            let file = content_files.get(www_root.as_str(), uri_path).unwrap();
            assert_eq!(file.raw.path, lib_path, "{}", uri_path);
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

        let content_files = ContentFiles::create(www_root.as_str(), &[""], &[]);

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

        let content_files = ContentFiles::create(www_root.as_str(), &[""], &[]);

        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(site.path.join("outside/secret.txt"), &link).unwrap();

        let file = content_files.get(www_root.as_str(), "/lib.js").unwrap();
        assert_eq!(
            std::fs::read_to_string(file.raw.path.as_str()).unwrap(),
            "lib"
        );
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
