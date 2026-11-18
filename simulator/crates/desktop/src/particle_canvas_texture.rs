//! SHA659d texture upload: 49fbd0 reduction, 4b52c0 uncompressed sampling.
//! This transforms the existing manager's private texture, never an image
//! wrapper or its HG metadata. UVs remain normalized to the whole texture.
use cs2_renderer::RgbaFrame;

pub(super) fn reduce(frame: RgbaFrame, policy: [u32; 2]) -> anyhow::Result<RgbaFrame> {
    let [size, compression] = policy;
    // 49fc23 selects a distinct compressed-surface/D3DX conversion path.
    // Do not substitute this uncompressed sampler for that conversion.
    anyhow::ensure!(compression == 0, "MDL compressed texture upload lacks its actual surface conversion");
    let shift = match size {
        1 => 1,
        2 => 2,
        _ => return Ok(frame), // The fixed PE also leaves other DWORDs full-size.
    };
    let source_width = frame.width();
    let source_height = frame.height();
    let width = source_width >> shift;
    let height = source_height >> shift;
    anyhow::ensure!(width != 0 && height != 0, "MDL texture reduction produces a zero-sized upload");
    // 4b5309/4b532c use signed dimension<<15 before division. Larger
    // dimensions have no recovered well-defined sampling contract here.
    anyhow::ensure!(source_width <= (i32::MAX as u32 >> 15)
        && source_height <= (i32::MAX as u32 >> 15), "MDL texture exceeds its fixed-point sampler range");
    let length = (width as usize).checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4)).filter(|&n| n <= 128 * 1024 * 1024)
        .ok_or_else(|| anyhow::anyhow!("MDL reduced texture exceeds128MiB"))?;
    let mut pixels = vec![0; length];
    let step_x = (u64::from(source_width) << 15) / u64::from(width);
    let step_y = (u64::from(source_height) << 15) / u64::from(height);
    let source = frame.pixels();
    for y in 0..height {
        let fixed_y = u64::from(y) * step_y;
        let y0 = (fixed_y >> 15) as u32;
        let y1 = (y0 + 1).min(source_height - 1);
        let fy = (fixed_y & 0x7fff) as u32;
        for x in 0..width {
            let fixed_x = u64::from(x) * step_x;
            let x0 = (fixed_x >> 15) as u32;
            let x1 = (x0 + 1).min(source_width - 1);
            let fx = (fixed_x & 0x7fff) as u32;
            let at = |x: u32, y: u32| (y as usize * source_width as usize + x as usize) * 4;
            let offsets = [at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1)];
            // Original 4b53cf skips a wholly transparent neighborhood.
            // The existing software upload allocation is transparent zero.
            if offsets.iter().all(|&at| source[at + 3] == 0) { continue; }
            let weights = [((0x8000 - fx) * (0x8000 - fy)) >> 15,
                (fx * (0x8000 - fy)) >> 15, ((0x8000 - fx) * fy) >> 15,
                (fx * fy) >> 15];
            let destination = (y as usize * width as usize + x as usize) * 4;
            for channel in 0..4 {
                let sum: u32 = offsets.iter().zip(weights).map(|(&at, weight)|
                    u32::from(source[at + channel]) * weight).sum();
                pixels[destination + channel] = (sum >> 15) as u8;
            }
        }
    }
    Ok(RgbaFrame::from_rgba8(width, height, pixels)?)
}
