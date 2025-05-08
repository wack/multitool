use std::io::Write;
use std::path::Path;

use base64::{engine::general_purpose::STANDARD, write::EncoderWriter};
use futures_util::{StreamExt as _, TryFutureExt as _};
use miette::{IntoDiagnostic, Result};
use tokio::{fs::File, io::BufReader};
use tokio_util::io::ReaderStream;

pub(crate) use manifest::Manifest as CloudFlareManifest;

mod manifest;

pub async fn read_file_as_b64<P: AsRef<Path>>(filepath: P, sink: &mut impl Write) -> Result<()> {
    let path = filepath.as_ref();
    let mut encoder = EncoderWriter::new(sink, &STANDARD);

    let mut stream = File::open(path)
        .await
        .map(BufReader::new)
        .map(ReaderStream::new)
        .into_diagnostic()?;

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.into_diagnostic()?;
        encoder.write_all(bytes.as_ref()).unwrap();
    }

    encoder.finish().into_diagnostic().map(|_| ())
}
