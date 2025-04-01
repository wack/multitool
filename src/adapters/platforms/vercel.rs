use std::{fs::metadata, path::{Path, PathBuf}};

use aws_sdk_lambda::config::IntoShared;
use tokio_util::io::ReaderStream;
use futures_util::TryFutureExt;
use sha1::{digest::{DynDigest, Update}, Digest, Sha1};
use async_trait::async_trait;
use miette::{IntoDiagnostic, Result};
use ignore::{gitignore::GitignoreBuilder, WalkBuilder, WalkState};
use tokio::{io::{AsyncReadExt, BufStream}, task::JoinSet};
use tokio::io::{self, BufReader, AsyncBufReadExt};
use tokio::fs::File;

use crate::{subsystems::ShutdownResult, Shutdownable};
use super::Platform;

pub struct Vercel {
    /// the vercel API client
    client: (),
    /// the path to the root of the project
    project_root: PathBuf,
}

/// This is the expected file name for any files ignored for upload.
/// This is spiritially similiar to a gitignore or a Dockerignore file.
const VERCEL_IGNORE: &str = ".vercelignore";

async fn hash_and_upload(file_path: &Path, client: ()) {
    let mut hasher = Sha1::new();
    let file = File::open(file_path).await.unwrap();
    let mut stream = ReaderStream::new(BufReader::new(file));
    while let Some(chunk) = stream.next().await {
        Digest::update(&mut hasher, chunk);
    }
    // Calculate its size in bytes.
    let byte_count = hasher.output_size();
    // Calculate the sha1 of this file.
    let digest = hasher.finalize();
    // Upload to Vercel.
    todo!()
}

#[async_trait]
impl Platform for Vercel {
    
    async fn deploy(&mut self) -> Result<String> {
        // Vercel's deployment API has us upload the files individually,
        // passing in their SHA1 and length as checksums.
        //
        // For parity with Vercel's CLI tools, we ignore files specified
        // in `.vercelignore`
        let mut requests = JoinSet::new();
        let walker = WalkBuilder::new(self.project_root.clone())
            .standard_filters(false)
            .add_custom_ignore_filename(VERCEL_IGNORE)
            .build();

        for entry in walker {
            let file = entry.into_diagnostic()?.path();
            requests.spawn(hash_and_upload(file, ()));
            // Finally, we can make the request to Vercel.
            // let reader = BufReader::new(file);
            // let stream = BufStream::new(file);
            // async move {
            //     while let Some(next) = stream.next().await {
            //         hasher.update(next);
            //     }
            // }
            // reader.read_to_end(&mut hasher);

            
            // requests.spawn(async move {
            //     let mut reader = BufReader::new(f);
            // });
        }
        requests.join_all().await;
        /*
        let mut vercelignore_builder = GitignoreBuilder::new(self.project_root);        
        // • Check if the `.vercelignore` file exists, and add it to
        //   the ignore list if so.
        let ignorefile = self.project_root.join(VERCEL_IGNORE);
        if  metadata(&ignorefile).is_ok() {
            vercel_ignore.add(ignorefile);            
        }
        let vercelignore = vercelignore_builder.build()?;
        // Now, walk the filesystem. We checksum each file and upload its contents
        // to vercel.
        let walker = WalkBuilder::new(self.project_root).standard_filters(false).filter_entry(|file| {
            vercelignore.
        })
        for result in WalkBuilder::new(self.project_root).build() {

             println!("{:?}", result);
        }
        */
        Ok(todo!())
    }

    async fn yank_canary(&mut self) -> Result<()> {
        todo!()
    }

    async fn delete_canary(&mut self) ->  Result<()> {
        todo!()
    }

    async fn promote_deployment(&mut self) -> Result<()> {
        todo!()
    }
}

impl Vercel {
    pub fn new<T: AsRef<Path>>(root: T) -> Self {
        Self {
            project_root: root.as_ref().into(),
        }
    }
}

impl Shutdownable for Vercel {
    async fn shutdown(&mut self) -> ShutdownResult {
        todo!()
    }
}
