//! History name IMAGE frame-table writes over the existing media bank.
//! No name-image state, pixel cache, resource loader or input owner lives here.
use super::*;

/// log_A SETNAME writes slot zero and then issues the ordinary setid command.
/// Keep those separate: settbl changes the existing mapping, while setid owns
/// resource selection/decode and the platform's BUTTON source refresh receipt.
pub(crate) fn apply_name_image_table(
    runtime: &mut FesRuntime, driver: &mut FesScriptDriver,
    command: &FesDriverCommand, shared: &mut ExpressionState,
    encoding: ScriptTextEncoding,
) -> Result<bool> {
    if !command.parameters.get(1).is_some_and(|verb| verb.eq_ignore_ascii_case(b"settbl")) {
        return Ok(false);
    }
    let Some(target) = command.parameters.first() else { return Ok(false); };
    let target = if target == b"@" {
        command.object_event.as_ref().context("IMAGE settbl @ has no event object")?.parameter()
    } else { target.clone() };
    let (name, bank) = super::super::fes_media::split_target(&target, encoding)?;
    let Some(definition) = driver.adapter.object(name).map(|object| object.definition.clone()) else {
        return Ok(false);
    };
    if !matches!(definition.kind_name.as_slice(), b"IMAGE" | b"BUTTON" | b"CIMAGE" | b"PLANE" | b"CPLANE") {
        return Ok(false); // SLIDER retains its original four-slot receiver.
    }
    let bank = bank.map(|source| driver.evaluate_parameter(source, command.context.current, shared)).transpose()?;
    let banks: Vec<_> = match bank {
        None | Some(-1) => (0..definition.bank_count as i32).collect(),
        Some(value) => vec![value],
    };
    if banks.first().is_none_or(|index| usize::try_from(*index).ok()
        .and_then(|index| driver.adapter.object_bank(name, index)).is_none()) {
        driver.fallback_command(command, shared)?;
        return Ok(true);
    }
    // The bank expression precedes these two evaluations, even for a slot
    // outside the table. Ignore extra arguments without evaluating them.
    let slot = command.parameters.get(2)
        .map(|source| driver.evaluate_parameter(source, 0, shared)).transpose()?.unwrap_or(0);
    let frame = command.parameters.get(3)
        .map(|source| driver.evaluate_parameter(source, 0, shared)).transpose()?.unwrap_or(0) as u32;
    if !(0..32).contains(&slot) { return Ok(true); }

    let decoded = encoding.decode(name);
    let mut targets = Vec::new();
    for index in banks {
        let Some(definition) = usize::try_from(index).ok()
            .and_then(|index| driver.adapter.object_bank_definition(name, index)) else { break; };
        if !matches!(definition.kind_name.as_slice(), b"IMAGE" | b"BUTTON") { continue; }
        let key = if definition.bank_count == 1 { decoded.clone() } else { format!("{decoded}[{index}]") };
        let layer = runtime.media.get(&key).context("IMAGE settbl bank has no live media source")?;
        if layer.kind.as_bytes() != definition.kind_name.as_slice() {
            bail!("IMAGE settbl media bank differs from its original declaration");
        }
        targets.push(key);
    }
    // All fallible lookups finish before mutation. The original table is also
    // captured/released with the media bank; aliases and checkpoints see it.
    for key in targets {
        let layer = runtime.media.get_mut(&key).unwrap();
        layer.frame_ids.resize(layer.frame_ids.len().max(32), 0);
        layer.frame_ids[slot as usize] = frame;
        // The existing select_id always reselects, including the same ID;
        // leaving image/selected_id untouched here preserves deferred loading.
    }
    Ok(true)
}
