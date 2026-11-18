//! Host-backed IMAGE/BUTTON source requests over the existing media layer.
//! Names retain script bytes until language expansion; pixels stay in image.
use anyhow::{Context, Result, bail};
use cs2_renderer::RgbaFrame;
use cs2_script::ExpressionState;

use super::{FesDriverCommand, FesScriptDriver, decode, split_target};
use crate::{FesMediaLayer, FesRuntime, LoggingHost};
use crate::fes_tasks::FesTaskView;

#[derive(Clone, Debug, Default, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct ImageSourceState {
    requested: Option<Vec<u8>>,
    expanded: Option<Vec<u8>>,
    selected_frame: Option<u32>,
    dirty: bool,
    // The encoded source and the canonical list frame directory have separate
    // lifetimes from the optional ordinary pixels on FesMediaLayer.
    raw_bytes: Option<Vec<u8>>,
    raw_entry: Option<u32>,
    raw_pending: bool,
    #[serde(skip)]
    difimage_resource: Option<u32>,
}

impl ImageSourceState {
    pub(crate) fn validate_checkpoint(&self) -> Result<()> {
        if self.difimage_resource.is_some() {
            bail!("DIFIMAGE checkpoint requires a retained image resource link");
        }
        if self.requested.as_ref().is_some_and(|name| name.is_empty() || name.len() > 255 || name.contains(&0))
            || self.expanded.as_ref().is_some_and(|name| name.len() > 2047 || name.contains(&0))
            || (self.expanded.is_some() && self.requested.is_none())
            || (self.selected_frame.is_some() && self.expanded.is_none())
            || self.raw_bytes.as_ref().is_some_and(|bytes| bytes.len() > 64 * 1024 * 1024)
            || self.raw_entry.is_some_and(|handle| handle == 0 || self.raw_bytes.is_none())
            || (self.raw_pending && (!self.dirty || self.raw_entry.is_some() || self.raw_bytes.is_none()))
        {
            bail!("FES IMAGE checkpoint has an invalid source request");
        }
        Ok(())
    }

    pub(crate) fn request_rebuild(&mut self) { self.dirty = true; self.raw_pending = false; }

    pub(crate) fn has_request(&self) -> bool { self.requested.is_some() }

    pub(crate) fn raw_image_bytes(&self) -> Option<&[u8]> { self.raw_bytes.as_deref() }

    pub(crate) fn raw_entry_handle(&self) -> Option<u32> { self.raw_entry }

    /// The legacy media updater must leave raw frame selection to the
    /// retained directory instead of reopening FILE on every ANM frame.
    pub(crate) fn has_raw_source(&self) -> bool { self.raw_bytes.is_some() }
}

pub(crate) fn apply_difimage_reference(
    host: &mut LoggingHost<'_>, runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver, command: &FesDriverCommand,
    shared: &mut ExpressionState,
) -> Result<bool> {
    if !command.parameters.get(1).is_some_and(|verb| verb.eq_ignore_ascii_case(b"refex")) {
        return Ok(false);
    }
    let Some(target) = command.parameters.first() else { return Ok(false); };
    let target = if target == b"@" {
        command.object_event.as_ref().context("DIFIMAGE @ has no event object")?.parameter()
    } else { target.clone() };
    let (name, bank) = split_target(&target, host.text_encoding)?;
    let Some(object) = driver.adapter.object(name)
        .filter(|object| object.definition.kind_name == b"DIFIMAGE") else { return Ok(false); };
    let count = i32::try_from(object.definition.bank_count).context("DIFIMAGE bank count exceeds i32")?;
    let bank = bank.map(|value| driver.evaluate_parameter(value, command.context.current, shared))
        .transpose()?.unwrap_or(-1);
    let source = command.parameters.get(2)
        .map(|value| driver.evaluate_parameter(value, command.context.current, shared))
        .transpose()?.unwrap_or(0) as u32;
    let banks = match (count, bank) {
        (1, _) => 0..1,
        (_, -1) => 0..count,
        (_, bank) if (0..count).contains(&bank) => bank..bank + 1,
        _ => return Ok(true),
    };
    let name_string = decode(name, host.text_encoding)?;
    for bank in banks {
        if driver.adapter.object_bank(name, bank as usize).is_none() { continue; }
        let key = if count == 1 { name_string.clone() } else { format!("{name_string}[{bank}]") };
        let layer = runtime.media.get_mut(&key).context("DIFIMAGE bank has no media binding")?;
        replace_difimage_source(host, layer, (source != 0).then_some(source))?;
    }
    Ok(true)
}

fn replace_difimage_source(
    host: &mut LoggingHost<'_>, layer: &mut FesMediaLayer, next: Option<u32>,
) -> Result<()> {
    let previous = layer.source.difimage_resource;
    let base = next.map(|handle| {
        host.image_resources.get(&handle).map(|resource| resource.geometry.base)
            .with_context(|| format!("DIFIMAGE refex has invalid image resource {handle}"))
    }).transpose()?;
    if previous != next {
        let remaining = previous.map(|handle| {
            host.image_resources.get(&handle).and_then(|resource| resource.references.checked_sub(1))
                .context("DIFIMAGE lost its previous resource reference")
        }).transpose()?;
        if let Some(handle) = next {
            let resource = host.image_resources.get_mut(&handle).context("DIFIMAGE resource disappeared")?;
            resource.references = resource.references.checked_add(1)
                .context("DIFIMAGE resource reference count overflow")?;
        }
        if let Some((handle, remaining)) = previous.zip(remaining) {
            if remaining == 0 {
                host.image_resources.remove(&handle);
                host.release_image_resource_layers(handle);
            } else {
                host.image_resources.get_mut(&handle).unwrap().references = remaining;
            }
        }
        layer.source.difimage_resource = next;
    }
    if let Some(base) = base {
        layer.image_base = base;
        if let Some(words) = &mut layer.motion {
            words[3] = (base[0] as f32).to_bits();
            words[4] = (base[1] as f32).to_bits();
        }
    }
    Ok(())
}

/// IDN is applied before this row's ID.n mapping, as in the IMAGE initializer.
/// An enabled map only accepts its 32 logical slots; -1 is a raw source only
/// when selected directly or supplied by a valid mapping slot.
pub(crate) fn initialize_image_idn(layer: &mut FesMediaLayer, id: i32) {
    if !layer.frame_ids.is_empty() && !(0..32).contains(&id) { return; }
    layer.selected_id = id as u32 as usize;
    layer.source.request_rebuild();
}

fn selected_frame(layer: &FesMediaLayer) -> u32 {
    if layer.frame_ids.is_empty() { layer.selected_id as u32 }
    else { layer.frame_ids.get(layer.selected_id).copied().unwrap_or(0) }
}

/// An empty FILE preserves the previous request and current resource name.
/// Conversion happens before the load command evaluates its numeric argument.
pub(crate) fn request_image_file(
    host: &LoggingHost<'_>, layer: &mut FesMediaLayer, filename: &[u8],
) -> Result<()> {
    let filename = &filename[..filename.iter().position(|&byte| byte == 0).unwrap_or(filename.len())];
    if filename.is_empty() { return Ok(()); }
    if filename.len() > 255 { bail!("FES IMAGE request exceeds 255 original bytes"); }
    decode(filename, host.text_encoding)?;
    layer.source.requested = Some(filename.to_vec());
    refresh_name(host, layer)
}

fn refresh_name(host: &LoggingHost<'_>, layer: &mut FesMediaLayer) -> Result<()> {
    let Some(requested) = &layer.source.requested else { return Ok(()); };
    let expanded = host.data_services.language.languages.expand_image(requested)?;
    let name = decode(&expanded, host.text_encoding)?;
    if layer.source.expanded.as_ref() != Some(&expanded) {
        layer.file = Some(name);
        layer.source.expanded = Some(expanded);
        layer.source.request_rebuild();
    }
    Ok(())
}

/// Deferred IO is performed by finish_task_media, so a resource error cannot
/// replay the bank/frame expressions or replace the interpreter continuation.
pub(crate) fn apply_image_source_command(
    host: &mut LoggingHost<'_>, runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver, command: &FesDriverCommand,
    shared: &mut ExpressionState,
) -> Result<bool> {
    let Some(verb) = command.parameters.get(1) else { return Ok(false); };
    let load = verb.eq_ignore_ascii_case(b"load");
    if !load && !verb.eq_ignore_ascii_case(b"setid") { return Ok(false); }
    let Some(target) = command.parameters.first() else { return Ok(false); };
    let target = if target == b"@" {
        command.object_event.as_ref().context("IMAGE source @ has no event object")?.parameter()
    } else { target.clone() };
    let (raw_name, bank) = split_target(&target, host.text_encoding)?;
    let Some(definition) = driver.adapter.object(raw_name).map(|object| object.definition.clone()) else {
        return Ok(false);
    };
    if !matches!(definition.kind_name.as_slice(), b"IMAGE" | b"BUTTON") { return Ok(false); }
    let name = decode(raw_name, host.text_encoding)?;
    let bank = bank.map(|source| driver.evaluate_parameter(source, command.context.current, shared)).transpose()?;
    let banks: Vec<_> = match bank {
        None | Some(-1) => (0..i32::try_from(definition.bank_count).context("IMAGE bank count exceeds i32")?).collect(),
        Some(index) => vec![index],
    };
    if banks.first().is_none_or(|index| usize::try_from(*index).ok()
        .and_then(|index| driver.adapter.object_bank(raw_name, index)).is_none()) {
        driver.fallback_command(command, shared)?;
        return Ok(true);
    }
    let mut common_id = None;
    for bank in banks {
        let Some(definition) = usize::try_from(bank).ok()
            .and_then(|bank| driver.adapter.object_bank_definition(raw_name, bank)) else { break; };
        if !matches!(definition.kind_name.as_slice(), b"IMAGE" | b"BUTTON") { continue; }
        let key = if bank == 0 && runtime.media.contains_key(&name) {
            name.clone()
        } else { format!("{name}[{bank}]") };
        let layer = runtime.media.get_mut(&key).context("IMAGE source bank has no live media layer")?;
        if layer.kind.as_bytes() != definition.kind_name.as_slice() {
            bail!("IMAGE source bank differs from its declaration");
        }
        if load {
            request_image_file(host, layer, command.parameters.get(2).map(Vec::as_slice).unwrap_or_default())?;
        }
        let current = layer.selected_id as u32 as i32;
        let id = if let Some(id) = common_id { id } else {
            let id = command.parameters.get(if load { 3 } else { 2 })
                .map(|source| driver.evaluate_parameter(source, current, shared)).transpose()?.unwrap_or(0);
            if !load { common_id = Some(id); }
            id
        };
        if !layer.frame_ids.is_empty() && !(0..32).contains(&id) { continue; }
        layer.selected_id = id as u32 as usize;
        // Preserve explicit re-selection, including settbl followed by setid
        // of the same logical slot in the existing log_A consumer.
        layer.source.request_rebuild();
    }
    Ok(true)
}

pub(crate) fn update_image_sources(
    host: &mut LoggingHost<'_>, view: &FesTaskView, runtime: &mut FesRuntime,
) -> Result<()> {
    let buttons = crate::fes_task_platform::navigation_bindings(host, view.id);
    for (key, layer) in &mut runtime.media {
        if !matches!(layer.kind.as_str(), "IMAGE" | "BUTTON") { continue; }
        refresh_name(host, layer)?;
        if !layer.source.dirty { continue; }
        let Some(name) = layer.file.clone() else {
            // No FILE has ever been supplied; keep the original empty source.
            layer.source.dirty = false;
            continue;
        };
        let frame = selected_frame(layer);
        if frame == u32::MAX {
            rebuild_raw_source(host, layer, &name)?;
            if layer.kind == "BUTTON" {
                let (target, _, _) = buttons.iter().find(|(_, binding, _)| binding == key)
                    .context("BUTTON raw source has no original live input binding")?;
                crate::fes_task_platform::mark_button_source_changed(host, view.id, &target.name, target.bank)?;
            }
            layer.source.selected_frame = Some(frame);
            layer.source.dirty = false;
            continue;
        }
        let parts: Vec<_> = name.split(' ').filter(|name| !name.is_empty()).collect();
        if parts.len() > 1 {
            bail!("FES IMAGE file list requires the zero-alpha color-composition receiver");
        }
        // Missing files/frames consume the request with an empty source, not
        // an old image or an invented transparent image. Geometry survives.
        layer.image = None;
        let mut image = None;
        if let Some(name) = parts.first() {
            for name in host.resources.category_names(b"image", name)? {
                if let Some(loaded) = host.load_managed_image(&name, frame, false)? {
                    image = Some(loaded);
                    break;
                }
            }
        }
        if let Some(image) = image {
            let size = layer.clip.map(|clip| [clip.width as i32, clip.height as i32])
                .unwrap_or([image.width() as i32, image.height() as i32]);
            layer.navigation.set_size(size);
            install_ordinary_frame(layer, image);
        }
        // Publish each completed bank before trying the next resource, so a
        // later IO error cannot lose an earlier BUTTON's source receipt.
        if layer.kind == "BUTTON" {
            let (target, _, _) = buttons.iter().find(|(_, binding, _)| binding == key)
                .context("BUTTON source has no original live input binding")?;
            crate::fes_task_platform::mark_button_source_changed(host, view.id, &target.name, target.bank)?;
        }
        layer.source.selected_frame = Some(frame);
        layer.source.dirty = false;
    }
    Ok(())
}

pub(super) fn install_ordinary_frame(layer: &mut FesMediaLayer, image: RgbaFrame) {
    let mut metadata = image.metadata();
    layer.image_offset = metadata.base;
    layer.image_base = std::array::from_fn(|axis| {
        metadata.base[axis].wrapping_sub(metadata.offset[axis])
    });
    if let Some(words) = &mut layer.motion {
        words[3..5].copy_from_slice(&layer.image_base.map(|base| (base as f32).to_bits()));
    }
    metadata.offset = [0; 2];
    metadata.canvas_size = [image.width(), image.height()];
    layer.image = Some(image.with_raw_metadata(metadata));
}

fn rebuild_raw_source(host: &mut LoggingHost<'_>, layer: &mut FesMediaLayer, name: &str) -> Result<()> {
    if name.is_empty() { return Ok(()); }
    // FILE remains whole (including spaces); PATHMODE is deliberately absent
    // from this raw-source receiver. Read errors leave the previous raw source.
    if !layer.source.raw_pending {
        let mut bytes = None;
        for candidate in host.resources.category_names(b"image", name)? {
            if let Some(found) = host.read_managed_raw_image_bytes(&candidate)? {
                bytes = Some(found);
                break;
            }
        }
        if let Some(old) = layer.source.raw_entry {
            host.release_list_entry_source(old)?;
            layer.source.raw_entry = None;
        }
        layer.source.raw_bytes = bytes;
        layer.source.raw_pending = layer.source.raw_bytes.is_some();
    }
    let replacement = match layer.source.raw_bytes.as_deref() {
        Some(bytes) => {
            if !bytes.starts_with(b"HG-3") { bail!("FES raw IMAGE source requires an HG-3 frame directory"); }
            Some(host.create_unowned_list_entry_source(bytes)?)
        }
        None => None,
    };
    layer.source.raw_entry = replacement;
    layer.source.raw_pending = false;
    // Existing ordinary pixel binding and object geometry survive. A later
    // failed ordinary request can expose this raw source again.
    Ok(())
}

/// The normal layer pixels win. Otherwise select from the canonical retained
/// frame directory; failed IDs preserve its current unsigned-minimum selection.
pub(crate) fn source_image(host: &LoggingHost<'_>, layer: &FesMediaLayer)
    -> Result<Option<cs2_renderer::RgbaFrame>> {
    if let Some(handle) = layer.source.difimage_resource {
        return host.image_resources.get(&handle).map(|resource| Some(resource.image.clone()))
            .with_context(|| format!("DIFIMAGE lost its retained image resource {handle}"));
    }
    if let Some(image) = &layer.image { return Ok(Some(image.clone())); }
    let Some(entry) = layer.source.raw_entry else { return Ok(None); };
    let select = layer.animation_display.map(|display| display.frame_id as u32);
    host.retained_image_source_frame(entry, select)
}

/// Raw list frames retain their own crop and selected-frame base. Ordinary
/// IMAGE pixels already received their BTYPE geometry on the media layer.
pub(crate) fn source_draw_options(layer: &FesMediaLayer, image: &cs2_renderer::RgbaFrame,
    options: &mut cs2_renderer::ImageDrawOptions) {
    if layer.image.is_none() && layer.source.raw_entry.is_some() {
        let metadata = image.metadata();
        options.position.x = options.position.x.wrapping_add(metadata.offset[0])
            .wrapping_sub(metadata.base[0]);
        options.position.y = options.position.y.wrapping_add(metadata.offset[1])
            .wrapping_sub(metadata.base[1]);
    }
}

/// userfontobj may consume the source in the same body as its FILE/IDN
/// initialization, before finish_task_media. Complete that pending raw read
/// once and borrow the retained bytes instead of reopening the current FILE.
pub(crate) fn prepare_raw_image_source<'a>(host: &mut LoggingHost<'_>, layer: &'a mut FesMediaLayer)
    -> Result<Option<&'a [u8]>> {
    refresh_name(host, layer)?;
    let frame = selected_frame(layer);
    if frame == u32::MAX && layer.source.dirty {
        if let Some(name) = layer.file.clone() {
            rebuild_raw_source(host, layer, &name)?;
            layer.source.selected_frame = Some(frame);
            layer.source.dirty = false;
        }
    }
    Ok(layer.source.raw_image_bytes())
}

pub(crate) fn release_image_source(host: &mut LoggingHost<'_>, layer: &mut FesMediaLayer) -> Result<()> {
    if layer.source.difimage_resource.is_some() {
        replace_difimage_source(host, layer, None)?;
    }
    if let Some(entry) = layer.source.raw_entry {
        host.release_list_entry_source(entry)?;
        layer.source.raw_entry = None;
    }
    layer.source.raw_bytes = None;
    layer.source.raw_pending = false;
    Ok(())
}

pub(crate) fn release_image_sources(host: &mut LoggingHost<'_>, runtime: &mut FesRuntime) -> Result<()> {
    for layer in runtime.media.values_mut() { release_image_source(host, layer)?; }
    Ok(())
}

pub(crate) fn validate_image_source_bindings(host: &LoggingHost<'_>, runtime: &FesRuntime) -> Result<()> {
    for layer in runtime.media.values() {
        layer.source.validate_checkpoint()?;
        if layer.source.raw_entry.is_some_and(|handle| !host.has_retained_image_source(handle)) {
            bail!("FES raw IMAGE checkpoint has no matching retained list source");
        }
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn cropped_button_source_refreshes_existing_motion_base() -> Result<()> {
    let mut layer = super::internal_layer("BUTTON", "btn");
    let mut words = [0; 36];
    words[6..8].copy_from_slice(&[12.5_f32.to_bits(), 20.0_f32.to_bits()]);
    words[28] = 0x8000_0000;
    layer.motion = Some(words);
    let image = RgbaFrame::from_rgba8(2, 1, vec![255; 8])?;
    let mut metadata = image.metadata();
    metadata.canvas_size = [1024, 576];
    for (crop, base) in [([247, 471], [0, 0]), ([439, 471], [439, 471])] {
        metadata.offset = crop;
        metadata.base = base;
        install_ordinary_frame(&mut layer, image.clone().with_raw_metadata(metadata));

        let motion = layer.motion.unwrap();
        let motion_base = [3, 4].map(|index| f32::from_bits(motion[index]) as i32);
        assert_eq!(layer.image_base, motion_base);
        assert_eq!(std::array::from_fn(|axis| layer.image_offset[axis] - motion_base[axis]), crop);
        assert_eq!(&motion[5..], &words[5..]);
        let installed = layer.image.as_ref().unwrap();
        assert_eq!(installed.metadata().offset, [0, 0]);
        assert_eq!(installed.metadata().canvas_size, [2, 1]);
        assert_eq!(installed.pixels(), image.pixels());
    }
    Ok(())
}
