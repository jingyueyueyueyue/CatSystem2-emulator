//! Shared wrapper+180 action owner:1a9d70 parsing and191910 scheduling.
//! Direct driver targets use the existing scalar/selector receivers.

use std::{collections::{BTreeMap, BTreeSet}, sync::Arc};

use super::{CpuEmoteControllerTarget, EmoteError, Result};

#[derive(Debug, Clone, Default)]
struct Step {
    duration: f32,
    easing: f32,
    wait: u32,
    restore: bool,
    variables: Vec<(String, f32)>,
}

#[derive(Debug, Clone, Default)]
struct Action {
    steps: Vec<Step>,
    names: BTreeSet<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CpuM2ActionConfig { actions: BTreeMap<Vec<u8>, Action> }

fn invalid(why: &'static str) -> EmoteError { EmoteError::InvalidResource(why) }

///The original atof result is narrowed to f32 at each directive.
fn scalar(bytes: &[u8]) -> Result<f32> {
    let mut end = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let start = end;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) { end += 1; }
    let mut digits = end - start;
    if bytes.get(end) == Some(&b'.') {
        end += 1;
        let begin = end;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) { end += 1; }
        digits += end - begin;
    }
    if digits == 0 { return Ok(0.0); }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let exponent = end;
        end += 1;
        if matches!(bytes.get(end), Some(b'+' | b'-')) { end += 1; }
        let begin = end;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) { end += 1; }
        if end == begin { end = exponent; }
    }
    let value = std::str::from_utf8(&bytes[..end]).ok()
        .and_then(|s| s.parse::<f64>().ok()).map(|v| v as f32)
        .ok_or_else(|| invalid("M2 action numeric directive"))?;
    if value.is_finite() { Ok(value) } else { Err(invalid("M2 action nonfinite scalar")) }
}

fn flush(action: &mut Action, step: &mut Step, dirty: &mut bool) -> Result<()> {
    if *dirty {
        if action.steps.len() >= 100_000 { return Err(EmoteError::Capacity); }
        action.steps.push(std::mem::take(step));
        *dirty = false;
    }
    Ok(())
}

impl CpuM2ActionConfig {
    ///A new frame/wait flushes and resets every step field, including easing.
    ///Repeated variable writes remain ordered; saved-name membership is unique.
    pub fn append_cp932(&mut self, bytes: &[u8]) -> Result<()> {
        let cleaned = super::m2config::clean_cp932(bytes)?;
        let mut next = self.clone();
        let mut enabled = false;
        let mut name = None::<Vec<u8>>;
        let mut action = Action::default();
        let mut step = Step::default();
        let mut dirty = false;
        for line in cleaned.split(|&b| b == b'\n').chain(std::iter::once(b"#".as_slice())) {
            let words: Vec<_> = line.split(u8::is_ascii_whitespace)
                .filter(|word| !word.is_empty()).collect();
            if words.is_empty() { continue; }
            if words.iter().any(|word| word.len() > 4095 || word.contains(&0)) {
                return Err(EmoteError::Capacity);
            }
            if words[0].starts_with(b"#") || words[0].starts_with(b"$") {
                if let Some(previous) = name.take() {
                    flush(&mut action, &mut step, &mut dirty)?;
                    if !action.steps.is_empty() {
                        if next.actions.len() >= 100_000 { return Err(EmoteError::Capacity); }
                        next.actions.entry(previous).or_insert(std::mem::take(&mut action));
                    }
                }
                action = Action::default();
                step = Step::default();
                dirty = false;
                if words[0].starts_with(b"#") { enabled = words[0] == b"#action"; }
                else if enabled { name = Some(words[0][1..].to_vec()); }
                continue;
            }
            if name.is_none() { continue; }
            match words[0] {
                b"frame" | b"wait" => {
                    flush(&mut action, &mut step, &mut dirty)?;
                    step = Step::default();
                    if let Some(value) = words.get(1) {
                        if words[0] == b"frame" { step.duration = scalar(value)?; }
                        else { step.wait = cs2_formats::config::parse_integer_prefix(value) as u32; }
                        dirty = true;
                    }
                }
                b"easing" if words.len() >= 2 => { step.easing = scalar(words[1])?; dirty = true; }
                b"var" if words.len() >= 3 => {
                    let (variable, _, errors) = encoding_rs::SHIFT_JIS.decode(words[1]);
                    if errors || variable.is_empty() { return Err(invalid("M2 action variable CP932")); }
                    if step.variables.len() >= 4096 { return Err(EmoteError::Capacity); }
                    let variable = variable.into_owned();
                    action.names.insert(variable.clone());
                    step.variables.push((variable, scalar(words[2])?));
                    dirty = true;
                }
                b"restore" => { step.restore = true; dirty = true; }
                _ => {}
            }
        }
        *self = next;
        Ok(())
    }
}

///Saved values survive action replacement and are captured once per name.
///175830 clears this table without cancelling the selected action/cursor.
#[derive(Debug, Clone, Default)]
pub(super) struct CpuM2ActionOwner {
    config: Arc<CpuM2ActionConfig>,
    selected: Vec<u8>,
    cursor: u32,
    current: Option<(Vec<u8>, usize)>,
    wait_until: Option<u32>,
    capture: bool,
    saved: BTreeMap<String, Option<f32>>,
}

impl CpuM2ActionOwner {
    pub fn new(config: Arc<CpuM2ActionConfig>) -> Self { Self { config, ..Self::default() } }

    pub fn start(&mut self, name: &[u8]) {
        self.selected = name.to_vec();
        self.cursor = 0;
        if let Some(action) = self.config.actions.get(name) {
            for variable in &action.names { self.saved.entry(variable.clone()).or_insert(None); }
        }
        self.wait_until = None;
        self.capture = true;
        //1915f0 does not clear the last actual step pointer at+8.
    }

    pub fn clear_saved(&mut self) { self.saved.clear(); }

    pub fn busy(&self) -> bool { self.current.is_some() }

    ///1917f0 skips ordinary steps until a restore step, then clears wait+28.
    ///The cursor is left on that restore; its targets are sent by the next tick.
    pub fn skip_to_restore(&mut self) {
        while let Some((name, index)) = &self.current {
            if self.config.actions.get(name).and_then(|a| a.steps.get(*index))
                .is_some_and(|step| step.restore) { break; }
            self.cursor = self.cursor.wrapping_add(1);
            self.current = self.config.actions.get(&self.selected)
                .and_then(|a| a.steps.get(self.cursor as usize))
                .map(|_| (self.selected.clone(), self.cursor as usize));
        }
        self.wait_until = None;
    }

    ///Called before the real driver update, after existing owner/force banks.
    ///The caller supplies chain-count and driver-f0 gates; one step per tick.
    pub fn advance(&mut self, tick: u32, current: &BTreeMap<String, f64>,
        chain_busy: bool, driver_busy: bool) -> Result<Vec<CpuEmoteControllerTarget>> {
        if chain_busy || driver_busy { return Ok(Vec::new()); }
        if self.capture {
            for (name, saved) in &mut self.saved {
                if saved.is_none() {
                    let value = *current.get(name).unwrap_or(&0.0) as f32;
                    if !value.is_finite() { return Err(invalid("M2 action saved driver value")); }
                    *saved = Some(value);
                }
            }
            self.capture = false;
        }
        if self.wait_until.is_some_and(|end| end > tick) { return Ok(Vec::new()); }
        self.wait_until = None;
        let index = self.cursor as usize;
        self.cursor = self.cursor.wrapping_add(1);
        let Some(step) = self.config.actions.get(&self.selected).and_then(|a| a.steps.get(index)) else {
            self.current = None;
            self.selected.clear();
            return Ok(Vec::new());
        };
        self.current = Some((self.selected.clone(), index));
        if step.wait != 0 { self.wait_until = Some(tick.wrapping_add(step.wait)); return Ok(Vec::new()); }
        let variables = if step.restore {
            self.saved.iter().map(|(name, value)| (name.clone(), value.unwrap_or(0.0))).collect()
        } else { step.variables.clone() };
        Ok(variables.into_iter().map(|(name, value)| CpuEmoteControllerTarget {
            name, value, duration: step.duration, easing: step.easing,
        }).collect())
    }
}
