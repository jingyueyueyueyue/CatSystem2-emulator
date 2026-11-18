//! Ordinary text planes use the existing common motion controller. The text
//! bank remains the only MessageObject/device owner; no second surface/store.
use super::*;
use cs2_runtime::fes_motion::{FesMotionContext, x86_integer};

fn bank<'a>(store: &'a FesTextStore, owner: u32, name: &[u8], index: i32) -> Result<&'a TextBank> {
    let bank = store.banks.get(&(owner, name.to_vec(), index))
        .context("STRING motion target has no live text bank")?;
    if bank.log.is_some() { bail!("LOGSTRING motion needs its history-specific plane consumer"); }
    Ok(bank)
}

fn color(bank: &TextBank) -> u32 {
    let alpha = (u32::from(bank.alpha) * (bank.tint >> 24)) / 255;
    (alpha << 24) | (bank.tint & 0x00ff_ffff)
}

pub(crate) fn motion_properties(
    store: &FesTextStore, owner: u32, name: &[u8], index: i32,
    mut words: [u32; 36],
) -> Result<[u32; 36]> {
    let bank = bank(store, owner, name, index)?;
    words[5] = (bank.priority as f32).to_bits();
    words[28] = color(bank);
    words[35] = u32::from(bank.visible);
    Ok(words)
}

pub(crate) fn motion_context(
    store: &FesTextStore, owner: u32, name: &[u8], index: i32,
    words: [u32; 36],
) -> Result<FesMotionContext> {
    let bank = bank(store, owner, name, index)?;
    // fes-text.md: Unicode has a default SUBCOMMAND context, but its
    // capture/writeback still uses the full real plane properties.
    if bank.message.encoding() == MessageEncoding::Unicode {
        return Ok(FesMotionContext { argb: 0, ..Default::default() });
    }
    Ok(FesMotionContext {
        base: std::array::from_fn(|axis| f32::from_bits(words[3 + axis])),
        position: std::array::from_fn(|axis| f32::from_bits(words[6 + axis])),
        velocity: [bank.vram[0] as f32, bank.vram[1] as f32, 0.],
        rotation: std::array::from_fn(|axis| f32::from_bits(words[29 + axis])),
        scale: std::array::from_fn(|axis| f32::from_bits(words[25 + axis])),
        argb: color(bank),
    })
}

pub(crate) fn apply_motion_properties(
    store: &mut FesTextStore, owner: u32, name: &[u8], index: i32,
    words: [u32; 36],
) -> Result<()> {
    bank(store, owner, name, index)?;
    let bank = store.banks.get_mut(&(owner, name.to_vec(), index)).unwrap();
    bank.origin = [3, 4].map(|i| x86_integer(f32::from_bits(words[i])));
    bank.position = [6, 7].map(|i| x86_integer(f32::from_bits(words[i])));
    bank.scale = [25, 26].map(|i| f32::from_bits(words[i]));
    bank.rotation = f32::from_bits(words[31]);
    bank.alpha = (words[28] >> 24) as u8;
    // The submitted glyph pixels receive RGB once, the list consumes Alpha
    // once. Keeping tint alpha here would apply the same motion alpha twice.
    bank.tint = 0xff00_0000 | (words[28] & 0x00ff_ffff);
    bank.visible = words[35] != 0;
    // Word5 is base Z on writeback; PRI remains independent.
    Ok(())
}

/// Called only for actual writes by the ordinary text receiver. Other axes,
/// fractional pose and the controller's captured baseline are retained.
pub(crate) fn synchronize_motion_properties(
    store: &FesTextStore, owner: u32, name: &[u8], index: i32,
    field: &[u8], words: &mut [u32; 36],
) -> Result<()> {
    let bank = bank(store, owner, name, index)?;
    let all = field == b"ALL";
    if all || matches!(field, b"POS" | b"POS2" | b"pos2") {
        for axis in 0..2 { words[6 + axis] = (bank.position[axis] as f32).to_bits(); }
    }
    if all || matches!(field, b"ORG" | b"org") {
        for axis in 0..2 { words[3 + axis] = (bank.origin[axis] as f32).to_bits(); }
    }
    if all || matches!(field, b"SCALE" | b"scale") {
        for axis in 0..2 { words[25 + axis] = bank.scale[axis].to_bits(); }
    }
    if all || matches!(field, b"ARC" | b"arc") {
        words[31] = bank.rotation.to_bits();
        let rotation = [29, 30, 31].map(|i| f32::from_bits(words[i]));
        words[9..25].copy_from_slice(
            &crate::plane_native::fes_rotation_matrix(rotation).map(f32::to_bits),
        );
    }
    if all || matches!(field, b"COLOR" | b"BLEND" | b"ALPHA" | b"color" | b"alpha") {
        words[28] = color(bank);
    }
    if all || matches!(field, b"DISP" | b"disp") { words[35] = u32::from(bank.visible); }
    Ok(())
}

/// Project actual submitted glyph pixels through the full saved matrix.
/// pRot/motRot update both rotation words and the 16 matrix words in mot KCS;
/// never substitute an identity matrix or a 2D angle for those matrix words.
pub(crate) fn prepare_motion_projection(
    host: &LoggingHost<'_>, view: &FesTaskView, runtime: &FesRuntime,
    mut projection: FesTextProjection,
) -> Result<FesTextProjection> {
    let Some(mut words) = host.data_services.fes_motion.text_properties(
        &host.data_services.fes_text, view.id, &projection.name, projection.bank,
    )? else { return Ok(projection); };
    // An unreachable/hidden message still updates playback, but has no
    // submitted pixels to transform until it is bound and shown.
    if view.list.is_none() || !projection.options.visible { return Ok(projection); }
    let bank = bank(&host.data_services.fes_text, view.id, &projection.name, projection.bank)?;
    if let Some(parent) = bank.parent_plane.as_ref().and_then(|name| runtime.planes.get(name)) {
        let (position, _) = parent_projection(Some(parent), is_canvas_parent(bank, runtime));
        words[6] = (f32::from_bits(words[6]) + position.x as f32).to_bits();
        words[7] = (f32::from_bits(words[7]) + position.y as f32).to_bits();
    }
    // Preserve the cheap integer path for the common untransformed page.
    let plain = words[9..25] == crate::fes_motion::IDENTITY.map(f32::to_bits)
        && words[25..28] == [1f32.to_bits(); 3]
        && f32::from_bits(words[8]) == 0.;
    if plain {
        let xy = [0, 1].map(|axis| f32::from_bits(words[6 + axis]) - f32::from_bits(words[3 + axis]));
        if xy.iter().all(|&v| v.is_finite() && v == x86_integer(v) as f32) {
            projection.options.position = Point::new(x86_integer(xy[0]), x86_integer(xy[1]));
            projection.options.rotation_radians = 0.;
            return Ok(projection);
        }
    }
    let binding = view.list.as_ref().context("STRING motion projection needs its real bound list")?;
    let list = host.data_services.tasks.fes.platform.lists.get(&binding.task)
        .context("STRING motion projection lost its real image list")?;
    if u64::from(list.size[0]) * u64::from(list.size[1]) * 4 > MAX_TEXT_BYTES as u64 {
        bail!("STRING motion projection exceeds 64 MiB");
    }
    let (size, _) = bank.message.registered_geometry()
        .context("STRING motion projection has no submitted message geometry")?;
    let mut options = ImageDrawOptions::new(Point::new(0, 0));
    options.visible = projection.options.visible;
    options.scale_x = size[0] as f32 / projection.frame.width().max(1) as f32;
    options.scale_y = size[1] as f32 / projection.frame.height().max(1) as f32;
    // RGB is already in render_bank's real submitted glyphs. Alpha stays on
    // the final list entry, including its documented integer mix lookup.
    words[28] = u32::MAX;
    // The lower text plane, like sync_fes_plane, submits independent PRI
    // as drawing depth. Motion word5 remains abstract base Z in the owner;
    // using that value here would clip half an X-rotated selection at Z=0.
    words[5] = (bank.priority as f32).to_bits();
    let mut canvas = cs2_renderer::Canvas::new(list.size[0], list.size[1])?;
    crate::plane_native::draw_fes_motion_frame(
        &mut canvas, &projection.frame, &options, None, Some(&words), Some([0.; 2]),
    )?;
    projection.frame = RgbaFrame::from_rgba8(list.size[0], list.size[1], canvas.pixels().to_vec())?;
    let mut projected = ImageDrawOptions::new(Point::new(0, 0));
    projected.visible = projection.options.visible;
    projected.opacity = projection.options.opacity;
    projected.clip = projection.options.clip;
    projection.options = projected;
    Ok(projection)
}
