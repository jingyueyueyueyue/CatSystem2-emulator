//! Frame-safe resource prediction from actual interpreter cursors.
//! This module only reads syntax and router state; it never advances a VM,
//! evaluates expressions, sends a task command, or starts device playback.
use super::*;
use crate::resource_preload::AheadRequest;

impl HeadlessPlayer {
    pub(super) fn update_script_prefetch(&mut self) {
        if self.data_services.fast_forward {
            self.data_services.ordinary.update_prefetch(&self.scan.resources, &[], &[]);
            return;
        }
        let mut requests = Vec::new();
        let mut boundaries = Vec::new();
        if let Some(driver) = self.fes.as_ref().and_then(|runtime| runtime.driver.as_ref()) {
            self.collect_fes_prefetch(driver, &mut requests, &mut boundaries);
        }
        for (_, runtime) in self.data_services.tasks.fes.runtimes().take(32) {
            if let Some(driver) = runtime.driver.as_ref() {
                self.collect_fes_prefetch(driver, &mut requests, &mut boundaries);
            }
            if requests.len() >= 20 { break; }
        }
        for vm in self.vm.iter().chain(self.task_vms.values()).take(32) {
            let Some(call) = vm.static_native_lookahead() else {
                if boundaries.len() < 16 { boundaries.push("KCS-control-or-nonliteral"); }
                continue;
            };
            let literal = |offset| -> Option<Vec<u8>> {
                let address = read_u32(&call.arguments, offset).ok()?;
                let name = vm.read_string(address).ok()?;
                (name.len() < 260 && !name.iter().any(|b| matches!(b,b'%'|b'$'|b'@'))).then_some(name)
            };
            match call.index {
                435 => {
                    if let (Some(name), Some(category)) = (literal(0), literal(4)) {
                        self.collect_category_prefetch(&category, &name, &mut requests);
                    }
                }
                906 => if let Some(spec) = literal(4) { requests.push(AheadRequest::M2Spec(spec)); },
                935 | 936 => if let Some(name) = literal(8) { self.collect_sound_prefetch(&name, &mut requests); },
                _ => if boundaries.len() < 16 { boundaries.push("KCS-native-not-supported"); },
            }
            if requests.len() >= 20 { break; }
        }
        self.data_services.ordinary.update_prefetch(&self.scan.resources, &requests, &boundaries);
        for note in self.data_services.ordinary.drain_prefetch_notes() {
            perf_detail::resource_warmup_note(&note);
            self.log.push(note);
        }
    }

    fn collect_fes_prefetch(&self, driver: &fes_driver::FesScriptDriver,
        requests: &mut Vec<AheadRequest>, boundaries: &mut Vec<&'static str>) {
        let scan = driver.interpreter.static_lookahead(20);
        if boundaries.len() < 16 { boundaries.push(scan.stopped); }
        for command in scan.commands {
            let words = command.parameters;
            if words.first().is_none_or(|word| word == b"wait") { continue; }
            let target = &words[0];
            let end = target.iter().position(|byte| *byte == b'[').unwrap_or(target.len());
            let Some(object) = driver.adapter.object(&target[..end]) else {
                if boundaries.len() < 16 { boundaries.push("FES-target-unresolved"); }
                break;
            };
            match object.definition.kind_name.as_slice() {
                b"IMAGE" | b"BUTTON" | b"DIFIMAGE" => {
                    self.collect_category_prefetch(b"image", &words[2], requests);
                }
                b"SOUND" => self.collect_sound_prefetch(&words[2], requests),
                _ => break,
            }
            if requests.len() >= 20 { break; }
        }
    }

    fn collect_category_prefetch(&self, category: &[u8], name: &[u8], requests: &mut Vec<AheadRequest>) {
        let name = String::from_utf8(cp932_to_utf8(name)).expect("CP932 produces UTF-8");
        if let Ok(names) = self.scan.resources.category_names(category, &name) {
            // Preserve the same first-existing winner as demand routing.
            for name in names {
                if self.scan.resources.resolve_mode_candidate(&name,
                    cs2_runtime::resource_router::ResourceMode::LooseFirst).ok().flatten().is_some() {
                    requests.push(AheadRequest::Named(name));
                    break;
                }
            }
        }
    }

    fn collect_sound_prefetch(&self, name: &[u8], requests: &mut Vec<AheadRequest>) {
        let decoded = self.script_encoding.decode(name);
        if decoded.rfind(['/', '\\']).is_some_and(|offset| offset > 0) {
            requests.push(AheadRequest::Named(decoded));
        } else if self.sound_config_enabled != 0 {
            if let Some(part) = self.sound_config.file_part(name) {
                self.collect_category_prefetch(&part.archive, name, requests);
            }
        }
        // Legacy dynamic sound selection is intentionally a prediction miss.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires local FES archive fixtures"]
    fn real_fes_safe_window_coverage() -> anyhow::Result<()> {
        let root = std::env::var_os("CS2_EMOTE_FIXTURE_ROOT").expect("fixture directory");
        let index = cs2_runtime::ResourceIndex::scan_with_name_seed(root,Some(0x0F6BB0AE))?;
        let mut parsed = 0usize;
        let mut windows = 0usize;
        let mut nonempty = 0usize;
        let mut resource_windows = 0usize;
        let mut maximum = 0usize;
        let mut stops = BTreeMap::<&str,usize>::new();
        for (archive_index,archive) in index.archives().iter().enumerate() {
            if !archive.path.file_name().is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("fes.int")) { continue; }
            for (entry_index,entry) in archive.entries.iter().enumerate() {
                if entry.size > 4*1024*1024 { continue; }
                let bytes=index.read_archive_entry_at(archive_index,entry_index)?;
                let mut program=match cs2_script::FesProgram::parse(&bytes) {
                    Ok(program)=>program, Err(error)=>{ eprintln!("FES skip {}: {error}",entry.name); continue; }
                };
                parsed+=1;
                // Inspect every actual source cursor. Remove declarations only
                // to bypass unexecuted startup events; no evaluator is invoked.
                program.declarations.clear();
                let positions=program.blocks.iter().flat_map(|block|
                    (1..=block.lines.len()).map(|position|(block.name.clone(),position as u32)).collect::<Vec<_>>()).collect::<Vec<_>>();
                let mut interpreter=cs2_script::FesInterpreter::new(program)?;
                for (block,position) in positions {
                    assert!(interpreter.enter_statement(&block,position));
                    let scan=interpreter.static_lookahead(20);
                    windows+=1;
                    nonempty+=usize::from(!scan.commands.is_empty());
                    resource_windows+=usize::from(scan.commands.iter().any(|command|
                        command.parameters.get(1).is_some_and(|verb|verb.eq_ignore_ascii_case(b"load"))));
                    maximum=maximum.max(scan.commands.len());
                    *stops.entry(scan.stopped).or_default()+=1;
                }
            }
        }
        eprintln!("FES static coverage parsed={parsed} source_cursors={windows} nonempty_windows={nonempty} load_windows={resource_windows} max_safe_commands={maximum} stops={stops:?}");
        assert!(parsed>0,"fixture contained parsed FES documents");
        Ok(())
    }
}
