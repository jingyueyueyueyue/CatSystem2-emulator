//! One completed callback's writes to the existing auxiliary targets.
//! No task owns these textures: 859 may replace either slot between callbacks.
//! Keep pending pixels local until both scene ranges have completed, without
//! recreating a pool that an independent retained capture has already released.
use super::check_work;
use anyhow::{Context, Result, bail};
use cs2_renderer::RgbaFrame;

pub(super) struct SurfacePass<'a> {
    sources: [Option<&'a RgbaFrame>; 2],
    writes: [Option<RgbaFrame>; 2],
    count: u32,
}

impl<'a> SurfacePass<'a> {
    pub(super) fn new(sources: [Option<&'a RgbaFrame>; 2], count: u32) -> Self {
        Self {
            sources,
            writes: [None, None],
            count,
        }
    }

    pub(super) fn source(&self, slot: u32, size: [u32; 2]) -> Result<&RgbaFrame> {
        if slot >= self.count {
            bail!("render callback auxiliary target has been released");
        }
        let frame = self.writes.get(slot as usize).and_then(Option::as_ref)
            .or_else(|| self.sources.get(slot as usize).copied().flatten())
            .context("render callback auxiliary pixels are absent")?;
        if [frame.width(), frame.height()] != size {
            bail!("render callback auxiliary target changed size without registration");
        }
        Ok(frame)
    }

    pub(super) fn replace(&mut self, slot: u32, frame: RgbaFrame) -> Result<()> {
        let size = [frame.width(), frame.height()];
        check_work(size)?;
        self.source(slot, size)?;
        self.writes[slot as usize] = Some(frame);
        Ok(())
    }

    pub(super) fn finish(self) -> [Option<RgbaFrame>; 2] {
        self.writes
    }
}
