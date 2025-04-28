use std::path::Path;

use async_stream::stream;
use miette::{IntoDiagnostic as _, Result};
use tokio::{fs::File, io::BufReader};
use tokio_stream::{Stream, StreamExt as _};
use tokio_util::io::ReaderStream;

/// This function stops reading the file if an Error is encountered,
/// returning that error as the final element in the stream.
pub(crate) async fn stream_file<P: AsRef<Path>>(
    filepath: P,
) -> impl Stream<Item = Result<Box<[u8]>>> {
    let path = filepath.as_ref();
    let file_stream = File::open(path)
        .await
        .map(BufReader::new) // buffer the file
        .map(ReaderStream::new) // convert into a stream
        .into_diagnostic();

    stream! {
        // Return a stream with one element if we can't read the file.
        if let Err(file_err) = file_stream {
            yield Err(file_err);
            return;
        }
        let mut chunk_stream = file_stream.unwrap();

        while let Some(chunk) = chunk_stream.next().await {
            let bytes = chunk.map(|bytes| {
                Box::from(bytes.as_ref())
            }).into_diagnostic();
            yield bytes;
        }
    }
}
