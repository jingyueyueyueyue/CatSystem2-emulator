//! SNDTABLE is a SOUND bank with an owned entry/order array (PE kind 10).
//! Selection requests the existing SOUND loader; it never invents NEXT/REV
//! notifications or another set of physical channels.
use super::*;

#[derive(Clone, Default, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Entry {
    value: i32,
    file: Vec<u8>,
}

#[derive(Clone, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct Playlist {
    entries: Vec<Entry>,
    order: Vec<usize>,
    cursor: i32,
    repeat: i32,
    shuffled: bool,
}

impl Default for Playlist {
    fn default() -> Self {
        Self { entries: Vec::new(), order: Vec::new(), cursor: -1,
            repeat: 0, shuffled: false }
    }
}

impl Playlist {
    pub(super) fn validate_checkpoint(&self) -> Result<()> {
        if self.entries.len() > 1_048_576 || self.entries.len() != self.order.len()
            || self.cursor < -1 || self.cursor >= self.entries.len() as i32
            || self.order.iter().any(|id| *id >= self.entries.len())
            || self.order.iter().copied().collect::<BTreeSet<_>>().len() != self.order.len()
            || self.entries.iter().any(|entry| entry.file.len() > 31 || entry.file.contains(&0))
        { bail!("SNDTABLE checkpoint entry/order state is invalid"); }
        Ok(())
    }

    pub(super) fn resize(&mut self, count: i32) -> Result<()> {
        // Allocation limits are host limits, never a successful empty table.
        let count = usize::try_from(count).context("SNDTABLE SIZE is negative")?;
        if count > 1_048_576 { bail!("SNDTABLE SIZE exceeds host allocation limit"); }
        self.entries = (0..count).map(|_| Entry::default()).collect();
        self.order = (0..count).collect();
        self.cursor = -1;
        self.shuffled = false;
        Ok(())
    }

    fn id(&self) -> i32 {
        self.order.get(self.cursor as usize).copied().unwrap_or(0) as i32
    }

    fn reset_order(&mut self) {
        let selected = self.order.get(self.cursor as usize).copied();
        self.order.iter_mut().enumerate().for_each(|(i, slot)| *slot = i);
        if let Some(id) = selected { self.cursor = id as i32; }
        self.shuffled = false;
    }

    fn selectable(&self, position: i32) -> bool {
        self.order.get(position as usize)
            .is_some_and(|id| self.entries[*id].value != 0)
    }

    /// PE 2bf9d0/2bfce0: forward scan and end fallback are shared by setid,
    /// next and reverse. Exhaustion retains a cursor but requests real stop.
    fn select(&mut self, mut position: i32) -> bool {
        let count = self.order.len() as i32;
        if count == 0 { self.cursor = -1; return false; }
        if position < 0 { position = if self.repeat == 0 { 0 } else { count - 1 }; }
        else if position >= count { position = if self.repeat != 0 { 0 } else { count - 1 }; }
        while position < count && !self.selectable(position) { position += 1; }
        if position == count && self.repeat != 0 && self.cursor > 0 {
            position = 0;
            while position < self.cursor && !self.selectable(position) { position += 1; }
        }
        if position >= count {
            self.cursor = (0..count).rev().find(|position| self.selectable(*position)).unwrap_or(-1);
            return false;
        }
        self.cursor = position;
        true
    }

    fn reverse(&mut self) -> bool {
        let mut position = self.cursor.wrapping_sub(1);
        while position >= 0 && !self.selectable(position) { position -= 1; }
        self.select(position)
    }

    fn selected(&self) -> Option<&Entry> {
        self.order.get(self.cursor as usize).map(|id| &self.entries[*id])
    }
}

pub(super) fn is_command(verb: &[u8]) -> bool {
    matches!(verb, b"sndtbl" | b"setid" | b"getid" | b"next" | b"rev" | b"repeat" | b"shuffle")
}

pub(super) fn single_bank(verb: &[u8]) -> bool {
    matches!(verb, b"getid" | b"next" | b"rev" | b"repeat" | b"shuffle")
}

pub(super) fn prepare(
    progress: &mut CommandProgress, verb: &[u8], args: &[Vec<u8>],
    driver: &mut FesScriptDriver, shared: &mut ExpressionState,
) -> Result<()> {
    let count = match verb { b"sndtbl" | b"shuffle" => 2, b"next" | b"rev" => 0, _ => 1 };
    while progress.values.len() < count {
        let slot = progress.values.len();
        let argument = if verb == b"sndtbl" && slot == 1 { 2 } else { slot };
        if verb == b"sndtbl" && slot == 1 && progress.raw.is_none() {
            progress.raw = Some(bounded_name(args.get(1).map(Vec::as_slice).unwrap_or_default(), 32)?);
        }
        progress.values.push(number(args, argument, 0, driver, shared)?);
    }
    Ok(())
}

pub(super) fn command(
    host: &mut LoggingHost<'_>, record: &mut SoundBank, progress: &CommandProgress,
    verb: &[u8], shared: &mut ExpressionState,
) -> Result<()> {
    let table = record.playlist.as_mut().context("SNDTABLE command requires a playlist bank")?;
    let mut selected = false;
    let mut stop = false;
    match verb {
        b"sndtbl" => {
            if let Some(entry) = table.entries.get_mut(progress.values[0] as usize) {
                entry.file = progress.raw.as_ref().unwrap().clone();
                entry.value = i32::from(progress.values[1] != 0);
            }
        }
        b"setid" | b"next" | b"rev" => {
            selected = if verb == b"rev" { table.reverse() } else {
                table.select(if verb == b"next" { table.cursor.wrapping_add(1) } else { progress.values[0] })
            };
            stop = !selected && record.requested_play;
        }
        b"getid" => {
            if let Some(slot) = shared.shared.get_mut(progress.values[0] as u32 as usize) { *slot = table.id(); }
        }
        b"repeat" => table.repeat = progress.values[0],
        b"shuffle" if progress.values[0] == 0 => table.reset_order(),
        b"shuffle" => {
            if table.order.is_empty() { bail!("SNDTABLE shuffle requires nonempty SIZE"); }
            let reload = record.load_stage.is_some() || (record.requested_play && table.shuffled);
            table.reset_order();
            // PE 2bf890: 1000 swaps, two draws from the shared script RNG each.
            for _ in 0..1000 {
                let a = host.script_random.next_integer() as usize % table.order.len();
                let b = host.script_random.next_integer() as usize % table.order.len();
                table.order.swap(a, b);
            }
            let keep = progress.values[1];
            if keep >= 0 {
                if let Some(position) = table.order.iter().position(|id| *id == keep as usize) {
                    table.order.swap(0, position);
                }
            }
            if let Some(position) = (0..table.order.len() as i32).find(|position| table.selectable(*position)) {
                table.cursor = position;
            }
            table.shuffled = true;
            selected = reload;
        }
        _ => unreachable!(),
    }
    if selected {
        if let Some(file) = table.selected().map(|entry| entry.file.clone()) {
            record.file = file;
            record.load_stage = Some(0);
        }
    }
    if stop { stop_record(record); }
    Ok(())
}
