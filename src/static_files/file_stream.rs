use std::io::{ErrorKind, Read};

use my_http_server::HttpOutputProducer;

/// The file up to this size is read whole and sent in one piece
pub const WHOLE_FILE_MAX_SIZE: u64 = 128 * 1024;

/// The bigger file is read and sent by the chunks of this size - so the request takes a few chunks
/// of the memory, whatever the size of the file is. It is the default window of the HTTP/2 stream:
/// the bigger chunk would wait for the client to confirm the window anyway
pub const STREAM_CHUNK_SIZE: usize = 64 * 1024;

/// The chunks which are read, but are not taken by the connection yet: one chunk waits in the
/// queue, one is read meanwhile and one is being sent - so the request takes 3 chunks at most
pub const STREAM_QUEUE_SIZE: usize = 0;

/// Reads the file by the chunks. Each chunk is read in the blocking pool right into the buffer
/// which is sent: `tokio::fs::File` reads into its own buffer first - one more chunk per request
pub struct FileChunks {
    path: String,
    /// Moves to the blocking pool for each read - and comes back with the chunk
    file: Option<std::fs::File>,
    content_length: u64,
    /// The bytes left to read. The file is sent with the length it had when it was opened -
    /// the bytes appended to it later are not sent: they are not counted in Content-Length
    remaining: u64,
}

impl FileChunks {
    /// The file is opened before the response is started: if it can not be opened,
    /// the client gets the error instead of the broken response
    pub async fn open(path: &str) -> std::io::Result<Self> {
        let file_path = path.to_string();

        let (file, content_length) = tokio::task::spawn_blocking(move || {
            let file = std::fs::File::open(file_path.as_str())?;
            let content_length = file.metadata()?.len();
            Ok::<_, std::io::Error>((file, content_length))
        })
        .await
        .map_err(std::io::Error::other)??;

        Ok(Self {
            path: path.to_string(),
            file: Some(file),
            content_length,
            remaining: content_length,
        })
    }

    pub fn content_length(&self) -> u64 {
        self.content_length
    }

    /// The next chunk of the file - None, when the whole length is read
    pub async fn next_chunk(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        if self.remaining == 0 {
            return Ok(None);
        }

        let Some(file) = self.file.take() else {
            return Err(std::io::Error::other(
                "the previous read of the file has failed",
            ));
        };

        let chunk_size = self.remaining.min(STREAM_CHUNK_SIZE as u64) as usize;

        let (file, chunk) = tokio::task::spawn_blocking(move || {
            let mut file = file;
            let chunk = read_chunk(&mut file, chunk_size)?;
            Ok::<_, std::io::Error>((file, chunk))
        })
        .await
        .map_err(std::io::Error::other)??;

        self.file = Some(file);

        if chunk.is_empty() {
            return Err(std::io::Error::new(
                ErrorKind::UnexpectedEof,
                "the file is shorter than when it was opened",
            ));
        }

        self.remaining -= chunk.len() as u64;

        Ok(Some(chunk))
    }
}

/// Sends the file by the chunks - till its end, or till the client goes away.
/// The end of the producer is the end of the response
pub async fn send_chunks(mut chunks: FileChunks, mut producer: HttpOutputProducer) {
    loop {
        let chunk = match chunks.next_chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => return,
            Err(err) => {
                // the headers are sent already: the response ends before its Content-Length,
                // so the client sees it is broken and does not keep it
                println!(
                    "Can not read the file '{}' while it is sent. Err: {}",
                    chunks.path, err
                );
                return;
            }
        };

        if producer.send(chunk).await.is_err() {
            // the client has gone away
            return;
        }
    }
}

/// Reads the chunk of the size - or less, if the file ends before it
fn read_chunk(file: &mut std::fs::File, size: usize) -> std::io::Result<Vec<u8>> {
    let mut chunk = vec![0u8; size];
    let mut filled = 0;

    while filled < size {
        match file.read(&mut chunk[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }

    chunk.truncate(filled);
    Ok(chunk)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::static_files::{compile_text, TestFolder};

    async fn read_all(chunks: &mut FileChunks) -> (Vec<u8>, Vec<usize>) {
        let mut content = Vec::new();
        let mut sizes = Vec::new();

        while let Some(chunk) = chunks.next_chunk().await.unwrap() {
            sizes.push(chunk.len());
            content.extend(chunk);
        }

        (content, sizes)
    }

    #[tokio::test]
    async fn reads_the_file_by_the_chunks() {
        let site = TestFolder::new("file-chunks");
        let content = compile_text(STREAM_CHUNK_SIZE * 2 + 100);
        site.add_file("app.js", &content);

        let mut chunks = FileChunks::open(site.get_folder("app.js").as_str())
            .await
            .unwrap();

        assert_eq!(chunks.content_length(), content.len() as u64);

        let (read, sizes) = read_all(&mut chunks).await;

        assert_eq!(sizes, [STREAM_CHUNK_SIZE, STREAM_CHUNK_SIZE, 100]);
        assert_eq!(read, content);
    }

    #[tokio::test]
    async fn sends_the_length_the_file_had_when_it_was_opened() {
        let site = TestFolder::new("file-chunks-appended");
        let content = compile_text(STREAM_CHUNK_SIZE + 10);
        site.add_file("app.js", &content);

        let path = site.get_folder("app.js");
        let mut chunks = FileChunks::open(path.as_str()).await.unwrap();

        // the bytes appended after the file is opened are not counted in Content-Length
        std::fs::OpenOptions::new()
            .append(true)
            .open(path.as_str())
            .unwrap()
            .write_all(b"appended")
            .unwrap();

        let (read, _) = read_all(&mut chunks).await;
        assert_eq!(read, content);
    }

    #[tokio::test]
    async fn fails_when_the_file_gets_shorter() {
        let site = TestFolder::new("file-chunks-truncated");
        site.add_file("app.js", compile_text(STREAM_CHUNK_SIZE + 10));

        let path = site.get_folder("app.js");
        let mut chunks = FileChunks::open(path.as_str()).await.unwrap();

        std::fs::OpenOptions::new()
            .write(true)
            .open(path.as_str())
            .unwrap()
            .set_len(100)
            .unwrap();

        assert_eq!(chunks.next_chunk().await.unwrap().unwrap().len(), 100);
        assert!(chunks.next_chunk().await.is_err());
    }

    #[tokio::test]
    async fn the_missing_file_is_not_opened() {
        assert!(FileChunks::open("/there/is/no/such/file.js").await.is_err());
    }
}
