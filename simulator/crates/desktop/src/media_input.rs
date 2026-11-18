//! A bounded, seekable copy of one encoded asset. Never writes the game tree.
use crate::media_process::error;
use cs2_runtime::RuntimeResult;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const MAX_MOVIE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
static NEXT_INPUT: AtomicU64 = AtomicU64::new(1);

pub(crate) struct MediaInput {
    path: PathBuf,
}

impl MediaInput {
    pub(crate) fn from_reader(mut reader: impl Read) -> RuntimeResult<Arc<Self>> {
        let directory = std::env::var_os("CS2_MEDIA_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("cs2-media"));
        std::fs::create_dir_all(&directory).map_err(|cause| {
            error(format!(
                "create media cache {}: {cause}",
                directory.display()
            ))
        })?;
        let (input, mut output) = loop {
            let sequence = NEXT_INPUT.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(format!("cs2-movie-{}-{sequence}.media", std::process::id()));
            match OpenOptions::new().create_new(true).write(true).open(&path) {
                Ok(file) => break (Self { path }, file),
                Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(cause) => return Err(error(format!("create seekable movie input: {cause}"))),
            }
        };
        let mut total = 0_u64;
        let mut block = [0_u8; 64 * 1024];
        loop {
            let read = match reader.read(&mut block) {
                Ok(read) => read,
                Err(cause) if cause.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(cause) => {
                    drop(output);
                    return Err(error(format!("read encoded movie: {cause}")));
                }
            };
            if read == 0 {
                break;
            }
            total += read as u64;
            if total > MAX_MOVIE_BYTES {
                drop(output);
                return Err(error("encoded movie exceeds the 4 GiB media limit"));
            }
            if let Err(cause) = output.write_all(&block[..read]) {
                drop(output);
                return Err(error(format!("write seekable movie input: {cause}")));
            }
        }
        let flushed = output.flush();
        drop(output);
        flushed.map_err(|cause| error(format!("flush movie input: {cause}")))?;
        if total == 0 {
            return Err(error("movie input is empty"));
        }
        Ok(Arc::new(input))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn prefix(&self) -> RuntimeResult<Vec<u8>> {
        let mut prefix = vec![0; 64];
        let read = File::open(&self.path)
            .and_then(|mut file| file.read(&mut prefix))
            .map_err(|cause| error(format!("read movie signature: {cause}")))?;
        prefix.truncate(read);
        Ok(prefix)
    }
}

impl Drop for MediaInput {
    fn drop(&mut self) {
        // DecoderProcess retains an Arc until its child and pipe workers stop.
        let _ = std::fs::remove_file(&self.path);
    }
}
