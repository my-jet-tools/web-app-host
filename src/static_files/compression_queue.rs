use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
    time::Instant,
};

use super::{CompressionAlgorithm, ContentFile, CopiesFolder};

/// Prepares the files of the content in the background - one by one, in one thread:
/// reads the file, calculates its etag and creates its compressed copies. So the compression
/// takes one core at most, and the service starts without waiting for it. The thread ends
/// when all the files are prepared.
///
/// The request waits for the best copy the client accepts - the file it waits for is taken next,
/// right after the file in progress
#[derive(Default)]
pub struct CompressionQueue {
    queues: Mutex<Queues>,
}

#[derive(Default)]
struct Queues {
    /// All the files of the content - in the order they are prepared
    regular: VecDeque<Arc<ContentFile>>,
    /// The files the requests wait for - they go before the regular ones
    priority: VecDeque<Arc<ContentFile>>,
}

/// What the preparation has done
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PreparationSummary {
    pub files: usize,
    pub br_copies: usize,
    pub gzip_copies: usize,
    /// The files which can not be read
    pub failed: usize,
}

impl PreparationSummary {
    fn add(&mut self, file: &ContentFile) {
        if let Some(Err(_)) = file.get_raw() {
            self.failed += 1;
            return;
        }

        self.files += 1;

        for algorithm in CompressionAlgorithm::ALL {
            if let Some(Some(_)) = file.get_copy(algorithm) {
                match algorithm {
                    CompressionAlgorithm::Brotli => self.br_copies += 1,
                    CompressionAlgorithm::Gzip => self.gzip_copies += 1,
                }
            }
        }
    }
}

impl std::fmt::Display for PreparationSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} files, {} br copies, {} gzip copies",
            self.files, self.br_copies, self.gzip_copies
        )?;

        if self.failed > 0 {
            write!(f, ", {} files can not be read", self.failed)?;
        }

        Ok(())
    }
}

impl CompressionQueue {
    pub fn enqueue(&self, files: Vec<Arc<ContentFile>>) {
        self.lock().regular.extend(files);
    }

    /// A request waits for the file - it is taken right after the file in progress
    pub fn prioritize(&self, file: &Arc<ContentFile>) {
        let mut queues = self.lock();

        if !queues.priority.iter().any(|itm| Arc::ptr_eq(itm, file)) {
            queues.priority.push_back(file.clone());
        }
    }

    /// Prepares the files until the queue is empty. The file which is prepared already
    /// is skipped: the request puts the file into the priority queue while it is
    /// in the regular one as well
    pub fn prepare_all(&self, copies_folders: &[CopiesFolder]) -> PreparationSummary {
        let mut summary = PreparationSummary::default();

        while let Some(file) = self.take_next() {
            if file.is_prepared() {
                continue;
            }

            file.prepare(copies_folders);
            summary.add(&file);
        }

        summary
    }

    /// Starts the background thread. All the files of the content must be in the queue:
    /// the request waits for the file until it is prepared
    pub fn start(self: &Arc<Self>, copies_folders: Vec<CopiesFolder>) {
        let queue = self.clone();

        std::thread::Builder::new()
            .name("content-preparation".to_string())
            .spawn(move || {
                let started = Instant::now();
                let summary = queue.prepare_all(copies_folders.as_slice());

                println!(
                    "Content is prepared in the background in {:.1?}: {}",
                    started.elapsed(),
                    summary
                );
            })
            .expect("Can not start the thread which prepares the content");
    }

    fn take_next(&self) -> Option<Arc<ContentFile>> {
        let mut queues = self.lock();
        queues
            .priority
            .pop_front()
            .or_else(|| queues.regular.pop_front())
    }

    /// The lock is never held while a file is prepared - so a panic can not poison it.
    /// If it is poisoned anyway, the queues are still consistent: they are changed by one call
    fn lock(&self) -> MutexGuard<'_, Queues> {
        self.queues.lock().unwrap_or_else(|err| err.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::static_files::{
        compile_copies_folders, compile_text, prepare_copies_folders, TestFolder,
    };

    fn create_file(site: &TestFolder, name: &str, content: &[u8]) -> Arc<ContentFile> {
        let file_name = format!("wwwroot/{}", name);
        site.add_file(file_name.as_str(), content);

        let raw_path = site.get_folder(file_name.as_str());
        let size = std::fs::metadata(raw_path.as_str()).unwrap().len();

        Arc::new(ContentFile::new(format!("/{}", name), raw_path, size))
    }

    #[test]
    fn the_file_a_request_waits_for_goes_first() {
        let site = TestFolder::new("queue-priority");

        let files: Vec<_> = ["a.js", "b.js", "c.js"]
            .iter()
            .map(|name| create_file(&site, name, b"content"))
            .collect();

        let queue = CompressionQueue::default();
        queue.enqueue(files.clone());

        queue.prioritize(&files[2]);
        // the same file is put into the priority queue once
        queue.prioritize(&files[2]);
        queue.prioritize(&files[1]);

        let order: Vec<_> = std::iter::from_fn(|| queue.take_next()).collect();
        let expected = [&files[2], &files[1], &files[0], &files[1], &files[2]];

        assert_eq!(order.len(), expected.len());

        for (index, (taken, expected)) in order.iter().zip(expected).enumerate() {
            assert!(Arc::ptr_eq(taken, expected), "{}", index);
        }
    }

    #[test]
    fn prepares_each_file_once() {
        let site = TestFolder::new("queue-once");

        let big = create_file(&site, "big.js", compile_text(50 * 1024).as_slice());
        let small = create_file(&site, "small.js", b"small");

        let queue = CompressionQueue::default();
        queue.enqueue(vec![small.clone(), big.clone()]);

        // the requests wait for both of them - while they are in the regular queue as well
        queue.prioritize(&big);
        queue.prioritize(&small);

        let copies_folders =
            prepare_copies_folders(compile_copies_folders(site.path.to_str().unwrap()));

        let summary = queue.prepare_all(&copies_folders);

        // the small file has no copies
        assert_eq!(
            summary,
            PreparationSummary {
                files: 2,
                br_copies: 1,
                gzip_copies: 1,
                failed: 0,
            }
        );

        for algorithm in CompressionAlgorithm::ALL {
            assert!(matches!(big.get_copy(algorithm), Some(Some(_))));
            assert!(matches!(small.get_copy(algorithm), Some(None)));
        }

        assert!(big.is_prepared());
        assert!(small.is_prepared());
    }

    #[test]
    fn failed_file_does_not_stop_the_queue() {
        let site = TestFolder::new("queue-failed");

        let removed = create_file(&site, "removed.js", b"removed");
        let file = create_file(&site, "app.js", b"app");
        std::fs::remove_file(removed.raw_path.as_str()).unwrap();

        let queue = CompressionQueue::default();
        queue.enqueue(vec![removed.clone(), file.clone()]);

        let summary = queue.prepare_all(&[]);

        assert_eq!(summary.files, 1);
        assert_eq!(summary.failed, 1);

        // the failed file is prepared as well - the request does not wait for it forever
        assert!(removed.is_prepared());
        assert!(matches!(removed.get_raw(), Some(Err(_))));
        assert!(matches!(file.get_raw(), Some(Ok(_))));
    }
}
