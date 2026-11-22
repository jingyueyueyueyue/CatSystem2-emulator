//! One panel bank and its real runtime.media IMAGE layer. The store is passed
//! explicitly by the shared dispatcher, including while that owner is taken.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use anyhow::{bail, ensure};
use cs2_renderer::{Color, Point};
use cs2_script::{FesEvent, FesObjectDefinition, FesObjectInitialization};
use cs2_runtime::graph_serde::{Serialize, Deserialize};
use crate::{FesMediaLayer, FesRuntime, fes_driver::FesDriverEffect, fes_tasks::FesTaskView};

pub(super) type Key = (u32, Vec<u8>, i32);
pub(super) const MAX_BANKS: usize = 65_536;
pub(super) const MAX_CELLS: usize = 65_536;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct UserFont {
    pub(super) file: Vec<u8>, pub(super) character: u16, pub(super) frame: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct Bank {
    pub(super) layer: String,
    pub(super) data: SavePanelData,
    pub(super) file: Vec<u8>,
    pub(super) expanded_file: Option<Vec<u8>>,
    pub(super) reference_sources: Vec<(Option<String>, Vec<u32>)>,
    pub(super) cells: BTreeMap<i32, cell::Cell>,
    pub(super) contents: BTreeMap<i32, cell::Content>,
    pub(super) enabled: BTreeSet<i32>,
    pub(super) user_fonts: BTreeMap<i32, Vec<UserFont>>,
    pub(super) dirty: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct Pending {
    pub(super) command: FesDriverCommand, pub(super) key: Key,
    pub(super) request: PanelDataRequest,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct FesSavePanelStore {
    pub(super) banks: BTreeMap<Key, Bank>,
    pub(super) declarations: BTreeSet<(u32, usize)>,
    pub(super) initializers: BTreeMap<(u32, usize), usize>,
    pub(super) pending: BTreeMap<u32, Pending>,
    // Immutable encoded resources, never a second retained pixel or scene map.
    pub(super) sources: BTreeMap<Vec<u8>, Vec<u8>>,
}

pub(super) fn split_target(text: &[u8]) -> Result<(&[u8], Option<&[u8]>)> {
    let Some(open) = text.iter().position(|&b| b == b'[') else { return Ok((text, None)) };
    ensure!(text.last() == Some(&b']'), "SAVEPANEL bank is not closed");
    Ok((&text[..open], Some(&text[open + 1..text.len() - 1])))
}

fn blank_layer(name: String) -> FesMediaLayer {
    FesMediaLayer { navigation: Default::default(), metadata: Default::default(), motion: None,
        name, kind: "SAVEPANEL".into(), file: None, source: Default::default(),
        frame_ids: Vec::new(), selected_id: 0, plane: None, position: Point::default(),
        priority: -1, clip: None, color: Color::WHITE, visible: false, enabled: true,
        opacity: 255, image: None, image_offset: [0; 2], image_base: [0; 2], animation: None,
        animation_request: Default::default(), animation_display: None, fade: None }
}

impl FesSavePanelStore {
    pub(crate) fn is_empty(&self) -> bool {
        self.banks.is_empty() && self.declarations.is_empty() && self.initializers.is_empty()
            && self.pending.is_empty() && self.sources.is_empty()
    }
    pub(crate) fn has_bank(&self, owner: u32, name: &[u8], bank: i32) -> bool {
        self.banks.contains_key(&(owner, name.to_vec(), bank))
    }
    /// Also used by the main platform to register exactly these IMAGE keys.
    pub(crate) fn bank_layer(&self, owner: u32, name: &[u8], bank: i32) -> Option<&str> {
        self.banks.get(&(owner, name.to_vec(), bank)).map(|bank| bank.layer.as_str())
    }
    pub(crate) fn owner_bindings(&self, owner: u32) -> impl Iterator<Item = (&[u8], i32, &str)> {
        self.banks.iter().filter(move |((id, _, _), _)| *id == owner)
            .map(|((_, name, index), bank)| (name.as_slice(), *index, bank.layer.as_str()))
    }
    fn keys(&self, owner: u32, name: &[u8], bank: Option<i32>) -> Vec<Key> {
        if let Some(bank) = bank.filter(|&bank| bank != -1) {
            return self.banks.contains_key(&(owner, name.to_vec(), bank))
                .then(|| (owner, name.to_vec(), bank)).into_iter().collect();
        }
        let mut keys = Vec::new();
        for bank in 0..MAX_BANKS as i32 {
            let key = (owner, name.to_vec(), bank);
            if !self.banks.contains_key(&key) { break; }
            keys.push(key);
        }
        keys
    }
    fn create(&mut self, owner: u32, definition: &FesObjectDefinition,
        driver: &FesScriptDriver, runtime: &mut FesRuntime, host: &LoggingHost<'_>) -> Result<()> {
        if self.declarations.contains(&(owner, definition.offset)) { return Ok(()); }
        let count = driver.adapter.object(&definition.name)
            .context("SAVEPANEL declaration has no adapter")?.definition.bank_count;
        ensure!(count <= MAX_BANKS && self.banks.len().saturating_add(count) <= MAX_BANKS,
            "SAVEPANEL bank capacity exceeded");
        let name = host.text_encoding.decode(&definition.name);
        for index in 0..count {
            let key = (owner, definition.name.clone(), index as i32);
            let layer = if count == 1 { name.clone() } else { format!("{name}[{index}]") };
            if self.banks.contains_key(&key) { self.release_bank(owner, &key.1, key.2, runtime)?; }
            ensure!(!runtime.media.contains_key(&layer), "SAVEPANEL IMAGE key is already owned by another object");
            runtime.media.insert(layer.clone(), blank_layer(layer.clone()));
            self.banks.insert(key, Bank { layer, dirty: true, ..Default::default() });
        }
        self.declarations.insert((owner, definition.offset));
        Ok(())
    }

    pub(crate) fn apply_effect(&mut self, host: &mut LoggingHost<'_>, view: &FesTaskView,
        runtime: &mut FesRuntime, driver: &mut FesScriptDriver,
        effect: &FesDriverEffect, shared: &mut ExpressionState) -> Result<Option<bool>> {
        match effect {
            FesDriverEffect::Initialize { event: FesEvent::CreateObject(definition), .. }
                if definition.kind == b"SAVEPANEL" => {
                self.create(view.id, definition, driver, runtime, host)?;
                Ok(Some(true))
            }
            FesDriverEffect::Initialize { event: FesEvent::InitializeObject(init), .. } =>
                Ok(self.initialize(host, view, runtime, driver, init, shared)?.then_some(true)),
            FesDriverEffect::Command(command) =>
                Ok(self.command(host, view, runtime, driver, command, shared)?.then_some(true)),
            _ => Ok(None),
        }
    }

    fn initialize(&mut self, host: &mut LoggingHost<'_>, view: &FesTaskView,
        runtime: &mut FesRuntime, driver: &mut FesScriptDriver,
        init: &FesObjectInitialization, shared: &mut ExpressionState) -> Result<bool> {
        let (name, bank) = split_target(&init.target)?;
        if !driver.adapter.object(name).is_some_and(|v| v.definition.kind_name == b"SAVEPANEL") {
            return Ok(false);
        }
        // Interpreter already expanded the initializer's bank to decimal.
        let bank = bank.map(|text| -> Result<i32> { Ok(std::str::from_utf8(text)?.parse()?) }).transpose()?;
        let keys = self.keys(view.id, name, bank);
        let fields = init.columns.len().min(init.values.len());
        let total = keys.len().checked_mul(fields).context("SAVEPANEL initializer overflow")?;
        let identity = (view.id, init.offset);
        let mut cursor = *self.initializers.get(&identity).unwrap_or(&0);
        while cursor < total {
            let key = &keys[cursor / fields];
            let field = cursor % fields;
            let column = init.columns[field].as_slice();
            let value = init.values[field].as_slice();
            ensure!(column != b"SIZE2", "SAVEPANEL SIZE2 needs its inherited image resize consumer");
            let layer = self.banks[key].layer.clone();
            if !crate::fes_driver::main_flow::initialize_field(runtime, driver, shared,
                host.text_encoding, &layer, b"SAVEPANEL", column, value)?
                && !crate::fes_motion::initialize_field(host, view, runtime, driver, shared,
                    name, key.2, init.offset, field, column, value)? {
                match column {
                    b"FILE" => {
                        let filename = value.split(|&b| b == 0).next().unwrap();
                        if !filename.is_empty() {
                            ensure!(filename.len() <= 255, "SAVEPANEL FILE exceeds original capacity");
                            self.banks.get_mut(key).unwrap().file = filename.to_vec();
                            self.banks.get_mut(key).unwrap().dirty = true;
                        }
                    }
                    b"PL" => runtime.media.get_mut(&layer).context("SAVEPANEL IMAGE is absent")?
                        .plane = (!value.is_empty()).then(|| host.text_encoding.decode(value)),
                    b"DEFAULT" => {},
                    b"PARENT" => {
                        crate::fes_motion::initialize_field(host, view, runtime, driver, shared,
                            name, key.2, init.offset, field, column, value)?;
                    }
                    b"IDN" => { driver.evaluate_parameter(value, 0, shared)?; }
                    _ => {
                        let values = value.split(|&b| b == b',').map(|text|
                            driver.evaluate_parameter(text, 0, shared)).collect::<Result<Vec<_>>>()?;
                        Self::property(runtime.media.get_mut(&layer).context("SAVEPANEL IMAGE is absent")?,
                            column, &values)?;
                    }
                }
            }
            cursor += 1;
            self.initializers.insert(identity, cursor);
        }
        Ok(true)
    }

    fn property(layer: &mut FesMediaLayer, verb: &[u8], values: &[i32]) -> Result<()> {
        let first = values.first().copied().unwrap_or(0);
        match verb {
            b"POS" | b"pos" => layer.position = Point::new(first, values.get(1).copied().unwrap_or(0)),
            b"PRI" | b"pri" => layer.priority = first,
            b"DISP" | b"disp" => layer.visible = first != 0,
            b"ENABLE" | b"enable" => layer.enabled = first != 0,
            b"ALPHA" | b"BLEND" | b"alpha" => layer.opacity = first.clamp(0, 255) as u8,
            b"COLOR" | b"color" => { let color = first as u32;
                layer.color = Color::rgba((color >> 16) as u8, (color >> 8) as u8, color as u8, (color >> 24) as u8); }
            _ => bail!("SAVEPANEL IMAGE property {:?} has no confirmed consumer", String::from_utf8_lossy(verb)),
        }
        Ok(())
    }

    fn command(&mut self, host: &mut LoggingHost<'_>, view: &FesTaskView,
        runtime: &mut FesRuntime, driver: &mut FesScriptDriver,
        command: &FesDriverCommand, shared: &mut ExpressionState) -> Result<bool> {
        if let Some(pending) = self.pending.get(&view.id) {
            ensure!(pending.command.offset == command.offset && pending.command.parameters == command.parameters
                && pending.command.context == command.context && pending.command.object_event == command.object_event,
                "SAVEPANEL continuation changed command identity");
            return self.finish_source_request(view.id, host, shared).map(|()| true);
        }
        let Some(target) = command.parameters.first() else { return Ok(false) };
        let target = if target == b"@" {
            let Some(event) = &command.object_event else { return Ok(false) };
            event.parameter()
        } else { target.clone() };
        let (name, raw_bank) = split_target(&target)?;
        if !driver.adapter.object(name).is_some_and(|v| v.definition.kind_name == b"SAVEPANEL") {
            return Ok(false);
        }
        let Some(verb) = command.parameters.get(1).map(Vec::as_slice) else { return Ok(true) };
        if verb.eq_ignore_ascii_case(b"delete") {
            return Ok(false); // The shared graphics release also detaches list and motion state.
        }
        let mut bank = raw_bank.map(|text| driver.evaluate_parameter(text, command.context.current, shared)).transpose()?;
        if bank.is_none_or(|index| index == -1)
            && driver.adapter.object(name).unwrap().definition.bank_count <= 1 { bank = Some(0); }
        if verb == b"getsavestr" {
            let Some(bank) = bank.filter(|&bank| bank != -1) else { return Ok(true) };
            let Some(destination) = number(command, 2, driver, shared)? else { return Ok(true) };
            let Some(record) = number(command, 3, driver, shared)? else { return Ok(true) };
            let key = (view.id, name.to_vec(), bank);
            if let Some(panel) = self.banks.get(&key) {
                let record = record as u32;
                let source = panel.data.record(record)?;
                let bytes = match source {
                    None | Some([]) => &[][..],
                    Some(source) => {
                        let end = source.iter().position(|&byte| byte == 0)
                            .context("SAVEPANEL getsavestr CSS record lacks a terminator")?;
                        &source[..end]
                    }
                };
                let codec = panel.data.text_encoding(host, record, bytes)?;
                // The shared string remains a byte string for KCS and the comment editor.
                // Record its codec for FES text expansion without attaching a CST page.
                host.data_services.cst_text_origins.set_shared_encoding(destination as u32, bytes, codec);
                if host.data_services.story_trace {
                    host.events.push(crate::LogicEvent {
                        frame: host.frame,
                        kind: "fes-savepanel-getsavestr".into(),
                        detail: format!("owner={} bank={bank} destination={destination} record={record:#010x} present={} bytes={} sha256={}",
                            view.id, source.is_some(), bytes.len(), crate::particle_plugin_digest::sha256(bytes)),
                    });
                }
                shared.strings.insert(destination as u32, bytes.to_vec());
            }
            return Ok(true);
        }
        if verb == b"getsavedate" {
            let Some(bank) = bank.filter(|&bank| bank != -1) else { return Ok(true) };
            let mut destinations = [0; 6];
            for (index, destination) in destinations.iter_mut().enumerate() {
                let Some(slot) = number(command, index + 2, driver, shared)? else { return Ok(true) };
                *destination = slot as usize;
            }
            if let Some(panel) = self.banks.get(&(view.id, name.to_vec(), bank)) {
                let date = panel.data.date();
                for (destination, value) in destinations.into_iter().zip(date) {
                    if let Some(slot) = shared.shared.get_mut(destination) {
                        *slot = value as i32;
                    }
                }
                if host.data_services.story_trace {
                    let [year, month, day, hour, minute, second] = date;
                    host.events.push(crate::LogicEvent {
                        frame: host.frame,
                        kind: "fes-savepanel-getsavedate".into(),
                        detail: format!("owner={} bank={bank} date={year:04}/{month:02}/{day:02} {hour:02}:{minute:02}:{second:02}", view.id),
                    });
                }
            }
            return Ok(true);
        }
        if let Some(request) = PanelDataRequest::prepare(command, bank, driver, shared)? {
            if let Some(key) = bank.filter(|&bank| bank != -1)
                .map(|bank| (view.id, name.to_vec(), bank)).filter(|key| self.banks.contains_key(key)) {
                self.pending.insert(view.id, Pending { command: command.clone(), key, request });
                self.finish_source_request(view.id, host, shared)?;
            }
            return Ok(true);
        }
        if verb == b"ptype" {
            let definition = cell::parse(command, driver, shared)?;
            if let Some((id, definition)) = definition {
                for key in self.keys(view.id, name, bank) {
                    let panel = self.banks.get_mut(&key).unwrap();
                    ensure!(panel.cells.contains_key(&id) || panel.cells.len() < MAX_CELLS, "SAVEPANEL cell capacity exceeded");
                    panel.cells.insert(id, definition.clone());
                }
            }
            return Ok(true);
        }
        if verb == b"setnum" {
            let Some(id) = number(command, 2, driver, shared)? else { return Ok(true) };
            let Some(value) = number(command, 3, driver, shared)? else { return Ok(true) };
            for key in self.keys(view.id, name, bank) {
                let panel = self.banks.get_mut(&key).unwrap();
                if let Some(definition) = panel.cells.get_mut(&id) {
                    definition.put(0x60, value as u32);
                    if let Some(content) = panel.contents.get_mut(&id) { content.record.put(0x40, value as u32); }
                    panel.dirty = true;
                }
            }
            return Ok(true);
        }
        let keys = self.keys(view.id, name, bank);
        if verb == b"dispid" {
            let enabled = number(command, 2, driver, shared)?.unwrap_or(1) != 0;
            for key in &keys {
                for text in command.parameters.iter().skip(3) {
                    let (first, last) = interval(text, command.context.current, driver, shared)?;
                    if first > last { continue; }
                    ensure!(last != i32::MAX && i64::from(last) - i64::from(first) < 1_000_000,
                        "SAVEPANEL dispid range exceeds original finite scan");
                    let panel = self.banks.get_mut(key).unwrap();
                    for id in first..=last {
                        if enabled {
                            ensure!(panel.enabled.contains(&id) || panel.enabled.len() < MAX_CELLS, "SAVEPANEL enabled-cell capacity exceeded");
                            panel.enabled.insert(id);
                        } else { panel.enabled.remove(&id); }
                    }
                    panel.dirty = true;
                }
            }
            return Ok(true);
        }
        if keys.is_empty() { return Ok(true); }
        if verb == b"userfont" {
            let Some(id) = number(command, 2, driver, shared)? else { return Ok(true) };
            let Some(file) = command.parameters.get(3) else { return Ok(true) };
            ensure!(file.len() < 64, "SAVEPANEL userfont filename exceeds original record");
            let Some(character) = number(command, 4, driver, shared)? else { return Ok(true) };
            let Some(frame) = number(command, 5, driver, shared)? else { return Ok(true) };
            ensure!(command.parameters.len() <= 6, "SAVEPANEL cropped userfont rectangle needs its confirmed pixel consumer");
            for key in keys {
                let panel = self.banks.get_mut(&key).unwrap();
                ensure!(panel.cells.contains_key(&(id as u16 as i32)) || panel.cells.len() < MAX_CELLS,
                    "SAVEPANEL userfont cell capacity exceeded");
                panel.cells.entry(id as u16 as i32).or_default();
                let fonts = panel.user_fonts.entry(id as u16 as i32).or_default();
                ensure!(fonts.len() < MAX_CELLS, "SAVEPANEL userfont mapping capacity exceeded");
                fonts.push(UserFont { file: file.clone(), character: character as u16, frame: frame as u32 });
                panel.dirty = true;
            }
            return Ok(true);
        }
        if verb == b"load" {
            let file = command.parameters.get(2).cloned().unwrap_or_default();
            ensure!(file.len() <= 255, "SAVEPANEL FILE exceeds original capacity");
            for key in keys {
                let panel = self.banks.get_mut(&key).unwrap();
                if !file.is_empty() { panel.file.clone_from(&file); panel.dirty = true; }
                // IMAGE load evaluates its optional frame per bank. Panel
                // composition reads its cell ids, not this inherited selector.
                number(command, 3, driver, shared)?;
            }
            return Ok(true);
        }
        if matches!(verb, b"pos2" | b"size2") {
            ensure!(verb != b"size2", "SAVEPANEL size2 needs its inherited image resize consumer");
            if let Some(source) = command.parameters.get(2) {
                for key in keys {
                    crate::fes_driver::main_flow::initialize_field(runtime, driver, shared,
                        host.text_encoding, &self.banks[&key].layer, b"SAVEPANEL",
                        if verb == b"pos2" { b"POS2" } else { b"SIZE2" }, source)?;
                }
            }
            return Ok(true);
        }
        let values = command.parameters.iter().skip(2).map(|text|
            driver.evaluate_parameter(text, command.context.current, shared)).collect::<Result<Vec<_>>>()?;
        let values = if values.is_empty() && verb == b"disp" { vec![1] } else { values };
        for key in keys {
            Self::property(runtime.media.get_mut(&self.banks[&key].layer)
                .context("SAVEPANEL property IMAGE is absent")?, verb, &values)?;
        }
        Ok(true)
    }

    fn finish_source_request(&mut self, owner: u32, host: &mut LoggingHost<'_>, shared: &mut ExpressionState) -> Result<()> {
        let pending = self.pending.get_mut(&owner).context("SAVEPANEL source request is absent")?;
        let panel = self.banks.get_mut(&pending.key).context("SAVEPANEL pending source bank was released")?;
        if panel.data.apply(&mut pending.request, host, shared)? == PanelDataOutcome::ApplyCellSources {
            cell::apply_sources(panel, host)?;
        }
        self.pending.remove(&owner);
        Ok(())
    }

    /// Invoke after ordinary IMAGE source updates and before the existing CPU
    /// media/list projection. A failure preserves the old submitted pixels.
    pub(crate) fn update_owner(&mut self, host: &mut LoggingHost<'_>, view: &FesTaskView,
        runtime: &mut FesRuntime) -> Result<()> {
        let keys: Vec<_> = self.banks.keys().filter(|(owner, _, _)| *owner == view.id).cloned().collect();
        for key in keys {
            let panel = self.banks.get_mut(&key).unwrap();
            pixels::update(panel, &mut self.sources, host, runtime)?;
        }
        Ok(())
    }

    /// The common postorder plan removes this exact layer from all lists
    /// before calling release_bank. No pixel-wrapper or audio handle is made.
    pub(crate) fn bank_release_layers(&self, owner: u32, name: &[u8], index: i32) -> Result<Vec<String>> {
        Ok(vec![self.bank_layer(owner, name, index).context("SAVEPANEL release bank is absent")?.to_owned()])
    }
    pub(crate) fn release_bank(&mut self, owner: u32, name: &[u8], index: i32, runtime: &mut FesRuntime) -> Result<()> {
        let key = (owner, name.to_vec(), index);
        if let Some(panel) = self.banks.get(&key) {
            ensure!(runtime.media.get(&panel.layer).is_some_and(|layer| layer.kind == "SAVEPANEL"),
                "SAVEPANEL release layer belongs to another object");
        }
        if let Some(panel) = self.banks.remove(&key) { runtime.media.remove(&panel.layer); }
        if self.pending.get(&owner).is_some_and(|pending| pending.key == key) { self.pending.remove(&owner); }
        self.prune_sources();
        Ok(())
    }
    pub(crate) fn release_owner(&mut self, owner: u32, runtime: &mut FesRuntime) -> Result<()> {
        let keys: Vec<_> = self.banks.keys().filter(|(id, _, _)| *id == owner).cloned().collect();
        for key in keys { self.release_bank(owner, &key.1, key.2, runtime)?; }
        self.forget_owner(owner);
        Ok(())
    }
    /// Reload's old runtime is borrowed by its caller and dropped after the
    /// shared list/child teardown. Release owned CSS/cells at that boundary.
    pub(crate) fn forget_owner(&mut self, owner: u32) {
        self.banks.retain(|(id, _, _), _| *id != owner);
        self.declarations.retain(|(id, _)| *id != owner);
        self.initializers.retain(|(id, _), _| *id != owner);
        self.pending.remove(&owner);
        self.prune_sources();
    }
    fn prune_sources(&mut self) {
        // Source bytes may also belong to a digit IMAGE or userfont filename;
        // the final panel release is the unambiguous shared-cache boundary.
        if self.banks.is_empty() { self.sources.clear(); }
    }
}

fn number(command: &FesDriverCommand, index: usize, driver: &mut FesScriptDriver,
    shared: &mut ExpressionState) -> Result<Option<i32>> {
    command.parameters.get(index).map(|text| if text.is_empty() { Ok(0) } else {
        driver.evaluate_parameter(text, command.context.current, shared)
    }).transpose()
}

fn interval(text: &[u8], current: i32, driver: &mut FesScriptDriver,
    shared: &mut ExpressionState) -> Result<(i32, i32)> {
    let mut eval = |text: &[u8]| driver.evaluate_parameter(text, current, shared);
    if text.first() != Some(&b'[') { let value = eval(text)?; return Ok((value, value)); }
    let body = &text[1..];
    let split = body.iter().position(|&b| b == b'-');
    let first = split.map_or(body, |at| &body[..at]);
    ensure!(first.len() < 128, "SAVEPANEL range expression exceeds original buffer");
    let first = eval(first)?;
    let last = if let Some(at) = split {
        let tail = &body[at + 1..];
        let end = tail.iter().position(|&b| b == b']').unwrap_or(tail.len());
        ensure!(end < 128, "SAVEPANEL range expression exceeds original buffer");
        eval(&tail[..end])?
    } else { first };
    Ok((first, last))
}
