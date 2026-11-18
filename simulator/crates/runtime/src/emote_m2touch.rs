//! Primary difference #touch descriptors:18b490 ->187a40 ->17fff0/17ff90.
//! Color keys select an ordered vector of $blocks. Each block retains its
//! original rows and byte fields, including additional columns. Queries do
//! not choose a current pattern, hit-test pixels, or start the named action.

use std::collections::BTreeMap;

use super::{CpuM2ControllerConfig, CpuM2LiveOwner};
use super::super::{CpuM2Config, EmoteError, Result};

type Rows = Vec<Vec<Vec<u8>>>;

#[derive(Debug, Clone, Default)]
pub(in crate::emote) struct CpuM2TouchRegistry {
    // config+1a0: exact unsigned color key -> vector<descriptor table>.
    colors: BTreeMap<u32, Vec<Rows>>,
}

impl CpuM2TouchRegistry {
    pub(in crate::emote) fn from_expanded<'a>(
        configs: impl IntoIterator<Item = &'a CpuM2Config>,
    ) -> Result<Self> {
        let mut result = Self::default();
        let mut cells = 0usize;
        for source in configs {
            //18b4f4..18b527 stops a #touch key list at either # or $.
            let declarations = source.records(b"touch").iter()
                .take_while(|row| row.first().is_none_or(|word|
                    !matches!(word.first(), Some(b'#' | b'$'))));
            for declaration in declarations {
                if declaration.len() < 2 { continue; }
                let color = cs2_formats::config::parse_integer_prefix(&declaration[0]) as u32;
                //18b5c6 keeps the first declaration of an exact key.
                if result.colors.contains_key(&color) { continue; }
                let mut tables = Vec::new();
                for reference in &declaration[1..] {
                    let Some(name) = reference.strip_prefix(b"$") else { continue; };
                    //17fd20 collects every matching $block in this source,
                    //in order, stopping each block at # or $. No fallback to
                    //a selected pattern or to another included file exists.
                    let mut rows = Vec::new();
                    let mut defining_section = None;
                    for (section, records) in &source.sections {
                        let mut active = false;
                        for row in records {
                            let Some(first) = row.first() else { continue; };
                            if matches!(first.first(), Some(b'#' | b'$')) {
                                active = first.strip_prefix(b"$") == Some(name);
                                if active {
                                    //The shared section reader retains row
                                    //order, but not order between distinct
                                    //#sections. Refuse only that ambiguity.
                                    if row.len() != 1 || defining_section
                                        .is_some_and(|previous| previous != section)
                                    {
                                        return Err(EmoteError::InvalidResource(
                                            "M2 touch block header/order outside recovered resource family",
                                        ));
                                    }
                                    defining_section = Some(section);
                                }
                                continue;
                            }
                            if !active { continue; }
                            cells = cells.checked_add(row.len()).filter(|&count| count <= 100_000)
                                .ok_or(EmoteError::Capacity)?;
                            if row.iter().any(|word| word.len() > 4095 || word.contains(&0)) {
                                return Err(EmoteError::InvalidResource("M2 touch descriptor byte field"));
                            }
                            rows.push(row.clone());
                        }
                    }
                    tables.push(rows);
                }
                result.colors.insert(color, tables);
            }
        }
        Ok(result)
    }

    fn table(&self, color: u32, table: u32) -> Option<&Rows> {
        self.colors.get(&color)?.get(table as usize)
    }

    fn count(&self, color: u32, table: u32) -> u32 {
        self.table(color, table).map_or(0, |rows| rows.len() as u32)
    }

    fn item(&self, color: u32, table: u32, row: u32) -> [Option<Vec<u8>>; 2] {
        let row = self.table(color, table).and_then(|rows| rows.get(row as usize));
        [row.and_then(|fields| fields.first()).cloned(),
         row.and_then(|fields| fields.get(1)).cloned()]
    }
}

impl CpuM2ControllerConfig {
    ///Native1029 uses1758e0 to read config+64;17fff0 counts table rows.
    ///Owned554 itself queries descriptor identity, not this row count.
    ///Missing unsigned color/table keys have no descriptor and return zero.
    pub fn touch_descriptor_count(&self, color: u32, table: u32) -> u32 {
        self.touch_descriptors.count(color, table)
    }

    ///Native1030 reads columns0/1 independently through17ff90. None means
    ///leave that VM destination untouched; Some(empty) still writes NUL.
    ///Return success is determined only by column0 being Some. Bytes exclude
    ///the terminator, which belongs to the host's bounded VM writeback.
    pub fn touch_descriptor_item_cp932(
        &self, color: u32, table: u32, row: u32,
    ) -> [Option<Vec<u8>>; 2] {
        self.touch_descriptors.item(color, table, row)
    }
}

impl CpuM2LiveOwner {
    ///The current primary config is rebound by906/profile reuse; the query
    ///does not retain pointers into old configs or require a second owner.
    pub fn touch_descriptor_count(&self, color: u32, table: u32) -> u32 {
        self.config.touch_descriptor_count(color, table)
    }

    pub fn touch_descriptor_item_cp932(
        &self, color: u32, table: u32, row: u32,
    ) -> [Option<Vec<u8>>; 2] {
        self.config.touch_descriptor_item_cp932(color, table, row)
    }
}
