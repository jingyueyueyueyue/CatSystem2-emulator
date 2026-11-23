//! Opportunistic decoding of script-declared HG-3 preloads.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Weak, mpsc};

use cs2_formats::{HgFile, hg::native_image_slices};
use cs2_renderer::{AnimationFrameSelection, RgbaFrame};

const MAX_ENTRIES: usize = 4;
const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
const MAX_FRAME_BYTES: u64 = 8 * 1024 * 1024;
const MAX_INFLATED_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub(super) struct ImagePredecode {
    worker: Option<mpsc::SyncSender<Job>>,
    entries: VecDeque<Entry>,
}

type Decoded = Mutex<Option<RgbaFrame>>;

#[derive(Clone, Debug)]
struct Entry {
    source: Arc<[u8]>,
    decoded: Arc<Decoded>,
}

#[derive(Debug)]
struct Job {
    source: Weak<[u8]>,
    decoded: Weak<Decoded>,
}

impl ImagePredecode {
    pub(super) fn snapshot(&self) -> Vec<(Arc<[u8]>, RgbaFrame)> {
        self.entries.iter().filter_map(|entry| {
            Some((entry.source.clone(), entry.decoded.try_lock().ok()?.as_ref()?.clone()))
        }).collect()
    }
    pub(super) fn submit(&mut self, bytes: &[u8]) {
        if bytes.len() > MAX_SOURCE_BYTES
            || !bytes.starts_with(b"HG-3")
            || self.entries.iter().any(|entry| &*entry.source == bytes)
        {
            return;
        }
        if self.worker.is_none() {
            self.worker = start_worker();
        }
        let Some(worker) = self.worker.as_ref() else {
            return;
        };
        let entry = Entry {
            source: bytes.into(),
            decoded: Default::default(),
        };
        let job = Job {
            source: Arc::downgrade(&entry.source),
            decoded: Arc::downgrade(&entry.decoded),
        };
        match worker.try_send(job) {
            Ok(()) => {
                if self.entries.len() == MAX_ENTRIES {
                    self.entries.pop_front();
                }
                self.entries.push_back(entry);
            }
            Err(mpsc::TrySendError::Full(_)) => {}
            Err(mpsc::TrySendError::Disconnected(_)) => self.worker = None,
        }
    }

    pub(super) fn take(&mut self, bytes: &[u8], id: u32) -> Option<RgbaFrame> {
        if id != 0 || !bytes.starts_with(b"HG-3") {
            return None;
        }
        // The demand read uses normal resource routing. Full byte equality
        // also covers loose-file replacement and same-name archive aliases.
        let index = self
            .entries
            .iter()
            .position(|entry| &*entry.source == bytes)?;
        let entry = self.entries.remove(index)?;
        entry.decoded.try_lock().ok()?.take()
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }
}

fn start_worker() -> Option<mpsc::SyncSender<Job>> {
    let (sender, receiver) = mpsc::sync_channel::<Job>(MAX_ENTRIES);
    std::thread::Builder::new()
        .name("cs2-image-predecode".into())
        .spawn(move || {
            while let Ok(job) = receiver.recv() {
                let Some(source) = job.source.upgrade() else {
                    continue;
                };
                if job.decoded.strong_count() == 0 {
                    continue;
                }
                let Some(frame) = decode(&source) else {
                    continue;
                };
                if let Some(decoded) = job.decoded.upgrade()
                    && let Ok(mut slot) = decoded.lock()
                {
                    *slot = Some(frame);
                }
            }
        })
        .ok()?;
    Some(sender)
}

fn decode(bytes: &[u8]) -> Option<RgbaFrame> {
    let file = HgFile::parse(bytes).ok()?;
    let (index, frame) = file
        .frames
        .iter()
        .enumerate()
        .find(|(_, frame)| frame.id == 0)?;
    let pixel_bytes = u64::from(frame.width)
        .checked_mul(u64::from(frame.height))?
        .checked_mul(4)?;
    if pixel_bytes > MAX_FRAME_BYTES {
        return None;
    }
    let inflated = match frame.encoded_image.as_ref() {
        Some(image) => u64::from(image.alpha_decompressed_length.unwrap_or(0)),
        None => native_image_slices(frame, bytes)
            .ok()?
            .iter()
            .map(|slice| {
                u64::from(slice.image.decompressed_data_length)
                    + u64::from(slice.image.decompressed_command_length)
            })
            .sum(),
    };
    if inflated > MAX_INFLATED_BYTES {
        return None;
    }
    RgbaFrame::decode_parsed_hg(bytes, &file, AnimationFrameSelection::Index(index)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn hg_frame(compressed: [u8; 9]) -> Vec<u8> {
        let mut bytes = b"HG-3".to_vec();
        bytes.extend([12u32, 0x300, 0, 0].into_iter().flat_map(u32::to_le_bytes));
        bytes.extend_from_slice(b"stdinfo\0");
        bytes.extend(
            [56u32, 40, 1, 1, 32, 0, 0, 1, 1, 1, 0, 0]
                .into_iter()
                .flat_map(u32::to_le_bytes),
        );
        bytes.extend_from_slice(b"img_al\0\0");
        bytes.extend([0u32, 17, 9, 1].into_iter().flat_map(u32::to_le_bytes));
        bytes.extend(compressed);
        bytes
    }

    #[test]
    fn worker_result_matches_the_normal_decoder_and_current_bytes() {
        let asset = hg_frame([120, 156, 83, 5, 0, 0, 38, 0, 38]);
        let changed = hg_frame([120, 156, 83, 3, 0, 0, 39, 0, 39]);
        let expected = crate::image_manager::decode_image_frame(&asset, 0)
            .unwrap()
            .unwrap();
        let mut predecode = ImagePredecode::default();
        predecode.submit(&asset);
        predecode.submit(&asset);
        assert_eq!(predecode.entries.len(), 1);
        let deadline = Instant::now() + Duration::from_secs(3);
        while predecode.entries[0].decoded.lock().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "image predecode worker did not complete"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(predecode.take(&changed, 0).is_none());
        assert!(predecode.take(&asset, 1).is_none());
        let actual = predecode.take(&asset, 0).unwrap();
        assert_eq!(actual.pixels(), expected.pixels());
        assert_eq!(actual.metadata(), expected.metadata());
        assert_eq!(actual.hg_attributes(), expected.hg_attributes());
        assert!(predecode.take(&asset, 0).is_none());
    }

    #[test]
    fn pending_predecode_can_fall_back_and_reset() {
        let asset = hg_frame([120, 156, 83, 5, 0, 0, 38, 0, 38]);
        let mut predecode = ImagePredecode::default();
        let pending = Entry {
            source: Arc::from(asset.as_slice()),
            decoded: Default::default(),
        };
        predecode.entries.push_back(pending.clone());
        assert!(predecode.take(&asset, 0).is_none());
        assert!(predecode.entries.is_empty());
        predecode.entries.push_back(pending);
        predecode.clear();
        assert!(predecode.entries.is_empty());
    }
}
