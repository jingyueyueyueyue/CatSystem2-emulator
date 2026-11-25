//! FES background and character gallery tables.
use super::*;
use cs2_runtime::resource_router::ResourceMode;

const NEUTRAL_TONE: [i32; 3] = [100; 3];

#[derive(
    Clone, Default, cs2_runtime::graph_serde::Serialize, cs2_runtime::graph_serde::Deserialize,
)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct BankState {
    tone_table: Option<String>,
    tone: Option<[i32; 3]>,
    image_plan: Option<String>,
    dirty: bool,
    base_option: Vec<u8>,
}

#[derive(Default)]
pub(super) struct TableCache {
    backgrounds: BTreeMap<String, Option<BackgroundTable>>,
    tones: BTreeMap<String, Option<ToneTable>>,
    characters: BTreeMap<String, Option<CharacterTable>>,
}

pub(super) fn is_gallery_kind(kind: &[u8]) -> bool {
    matches!(kind, b"BGTABLE2" | b"CHTABLE2" | b"CHTABLE3")
}

fn is_character_kind(kind: &[u8]) -> bool {
    matches!(kind, b"CHTABLE2" | b"CHTABLE3")
}

struct BackgroundTable {
    rows: Vec<Vec<Vec<u8>>>,
}

impl BackgroundTable {
    fn parse(bytes: &[u8]) -> Self {
        let mut rows = Vec::new();
        let mut row = Vec::new();
        let mut cell = Vec::new();
        let mut quoted = false;
        let mut closed = false;
        let mut bytes = crate::trim_nul(bytes)
            .iter()
            .copied()
            .chain([b'\n'])
            .peekable();
        while let Some(byte) = bytes.next() {
            match byte {
                b'"' if quoted => {
                    if bytes.next_if_eq(&b'"').is_some() {
                        cell.push(b'"');
                    } else {
                        quoted = false;
                        closed = true;
                    }
                }
                b'"' if cell.is_empty() && !closed => quoted = true,
                b'"' => closed = true,
                b',' | b'\r' | b'\n' if !quoted => {
                    row.push(std::mem::take(&mut cell));
                    closed = false;
                    if byte != b',' {
                        // The original reader compares the first field to a CP932 circle.
                        if row[0] == b"\x81\x9b" {
                            rows.push(std::mem::take(&mut row));
                        } else {
                            row.clear();
                        }
                    }
                }
                _ if !closed => cell.push(byte),
                _ => {}
            }
        }
        Self { rows }
    }

    fn image(&self, row: i32, variant: i32) -> Option<&[u8]> {
        self.rows
            .get(row as usize)?
            .get(variant.wrapping_add(2) as usize)
            .filter(|cell| !cell.is_empty())
            .map(Vec::as_slice)
    }
}

type ToneTable = BTreeMap<Vec<u8>, [i32; 3]>;

fn read_tones(bytes: &[u8]) -> ToneTable {
    let mut table = BTreeMap::new();
    let mut tone = NEUTRAL_TONE;
    for line in crate::trim_nul(bytes).split(|&byte| byte == b'\n') {
        let line = line
            .splitn(2, |&byte| byte == b'\t')
            .next()
            .unwrap_or_default();
        let line = line
            .windows(2)
            .position(|chars| chars == b"//")
            .map_or(line, |at| &line[..at])
            .trim_ascii();
        if line.is_empty() {
            continue;
        }
        if line.starts_with(b"#") {
            if let Some(open) = line.iter().position(|&byte| byte == b'(')
                && let Some(close) = line.iter().rposition(|&byte| byte == b')')
                && close > open
            {
                let mut parts = line[open + 1..close].split(|&byte| byte == b',');
                tone = std::array::from_fn(|component| {
                    parts
                        .next()
                        .and_then(|part| std::str::from_utf8(part.trim_ascii()).ok())
                        .and_then(|part| part.parse().ok())
                        .unwrap_or(tone[component])
                });
            }
        } else {
            table.insert(line.to_ascii_lowercase(), tone);
        }
    }
    table
}

fn cached<'a, T>(
    cache: &'a mut BTreeMap<String, Option<T>>,
    resources: &ResourceIndex,
    path: &str,
    mode: ResourceMode,
    parse: impl FnOnce(&[u8]) -> Result<T>,
) -> Result<Option<&'a T>> {
    if path.is_empty() {
        return Ok(None);
    }
    let value = match cache.entry(path.to_owned()) {
        std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
        std::collections::btree_map::Entry::Vacant(entry) => {
            let bytes = resources.read_mode(path, mode)?;
            entry.insert(bytes.as_deref().map(parse).transpose()?)
        }
    };
    Ok(value.as_ref())
}

fn background<'a>(
    cache: &'a mut TableCache,
    resources: &ResourceIndex,
    path: &str,
) -> Result<Option<&'a BackgroundTable>> {
    cached(
        &mut cache.backgrounds,
        resources,
        path,
        ResourceMode::LooseFirst,
        |bytes| Ok(BackgroundTable::parse(bytes)),
    )
}

fn tones<'a>(
    cache: &'a mut TableCache,
    resources: &ResourceIndex,
    path: &str,
) -> Result<Option<&'a ToneTable>> {
    cached(
        &mut cache.tones,
        resources,
        path,
        ResourceMode::ArchiveFirst,
        |bytes| Ok(read_tones(bytes)),
    )
}

fn characters<'a>(
    cache: &'a mut TableCache,
    resources: &ResourceIndex,
    path: &str,
) -> Result<Option<&'a CharacterTable>> {
    cached(
        &mut cache.characters,
        resources,
        path,
        ResourceMode::LooseFirst,
        |bytes| {
            if path.to_ascii_lowercase().ends_with(".cl") {
                Ok(CharacterTable::Text(TextTable::parse(bytes)))
            } else {
                Ok(CharacterTable::Binary(BinaryTable::parse(bytes)?))
            }
        },
    )
}

pub(super) fn initialize_table(
    host: &mut LoggingHost<'_>,
    kind: &[u8],
    value: &[u8],
) -> Result<()> {
    let path = host.text_encoding.decode(crate::trim_nul(value));
    let cache = &mut host.data_services.tasks.fes.platform.gallery_tables;
    if is_character_kind(kind) {
        characters(cache, host.resources, &path)?;
    } else {
        background(cache, host.resources, &path)?;
    }
    Ok(())
}

enum CharacterTable {
    Binary(BinaryTable),
    Text(TextTable),
}

impl CharacterTable {
    fn count(&self, path: &[i32]) -> i32 {
        match self {
            Self::Binary(table) => table.count(path),
            Self::Text(table) => path
                .iter()
                .copied()
                .map(u32::try_from)
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_or(0, |path| table.count(&path)),
        }
    }

    fn image(&self, path: &[i32], encoding: crate::ScriptTextEncoding) -> Option<String> {
        let sprite = match self {
            Self::Binary(table) => table.image(path),
            Self::Text(table) => {
                let path = path
                    .iter()
                    .copied()
                    .map(u32::try_from)
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .ok()?;
                table.image(&path)
            }
        }?;
        Some(sprite.plan(encoding))
    }
}

type Choices = BTreeMap<i32, Sprite>;
type Variants = BTreeMap<i32, Choices>;
type Poses = BTreeMap<i32, Variants>;

#[derive(Default)]
struct BinaryTable(BTreeMap<i32, Poses>);

#[derive(Default)]
struct Sprite {
    stem: Vec<u8>,
    selectors: Vec<u8>,
}

impl Sprite {
    fn merge(&mut self, stem: &[u8], selectors: &[u8]) {
        if !stem.is_empty() {
            self.stem = stem.to_vec();
        }
        self.selectors
            .resize(self.selectors.len().max(selectors.len()), b'0');
        for (value, &selector) in self.selectors.iter_mut().zip(selectors) {
            if selector != b'@' {
                *value = selector;
            }
        }
    }

    fn plan(self, encoding: crate::ScriptTextEncoding) -> String {
        let mut name = self.stem;
        for selector in self.selectors {
            name.extend_from_slice(&[b',', selector]);
        }
        encoding.decode(&name)
    }
}

struct SetNode<'a> {
    id: i32,
    fields: &'a [u8],
    children: &'a [u8],
}

fn set_nodes(mut bytes: &[u8]) -> Result<Vec<SetNode<'_>>> {
    let mut nodes = Vec::new();
    loop {
        let next = read_u32(bytes, 0)? as usize;
        let children = read_u32(bytes, 4)? as usize;
        let fields = read_u32(bytes, 8)? as usize;
        let id = read_u32(bytes, 12)? as i32;
        let end = if next == 0 { bytes.len() } else { next };
        let node = bytes
            .get(..end)
            .context("FES character table sibling crosses its parent")?;
        let fields = node
            .get(16..12usize.saturating_add(fields))
            .context("FES character table has invalid fields")?;
        let descendants = if children == 0 {
            &[][..]
        } else {
            node.get(children..)
                .filter(|_| children >= 16 + fields.len())
                .context("FES character table has invalid child offset")?
        };
        nodes.push(SetNode {
            id,
            fields,
            children: descendants,
        });
        if next == 0 {
            return Ok(nodes);
        }
        bytes = &bytes[next..];
    }
}

impl BinaryTable {
    fn parse(bytes: &[u8]) -> Result<Self> {
        let mut table = Self::default();
        if bytes.is_empty() {
            return Ok(table);
        }
        for character in set_nodes(bytes)? {
            let mut poses = Poses::new();
            for pose in set_nodes(character.children)? {
                let mut variants = Variants::new();
                for variant in set_nodes(pose.children)? {
                    let mut choices = Choices::new();
                    if !variant.children.is_empty() {
                        for choice in set_nodes(variant.children)? {
                            // The long SET records use a 32-byte stem. Older ones reserve eight.
                            let width = match choice.fields.len() {
                                72 => 8,
                                96 => 32,
                                _ => bail!(
                                    "FES character table sprite has invalid field size {}",
                                    choice.fields.len()
                                ),
                            };
                            let stem = crate::trim_nul(&choice.fields[32..32 + width]).to_vec();
                            let selectors = crate::trim_nul(&choice.fields[32 + width..]).to_vec();
                            choices.insert(choice.id, Sprite { stem, selectors });
                        }
                    }
                    variants.insert(variant.id, choices);
                }
                poses.insert(pose.id, variants);
            }
            table.0.insert(character.id, poses);
        }
        Ok(table)
    }

    fn count(&self, path: &[i32]) -> i32 {
        let count = match path {
            [character, pose, variant] => self
                .0
                .get(character)
                .and_then(|poses| poses.get(pose))
                .and_then(|variants| variants.get(variant))
                .map_or(0, BTreeMap::len),
            [character, pose] => self
                .0
                .get(character)
                .and_then(|poses| poses.get(pose))
                .map_or(0, BTreeMap::len),
            [character] => self.0.get(character).map_or(0, BTreeMap::len),
            _ => self.0.len(),
        };
        count as i32
    }

    fn image(&self, path: &[i32]) -> Option<Sprite> {
        let [character, pose, selections @ ..] = path else {
            return None;
        };
        let variants = self.0.get(character)?.get(pose)?;
        let mut sprite = Sprite::default();
        for (variant, &index) in selections.iter().enumerate() {
            let Some(choice) = variants
                .get(&(variant as i32))
                .and_then(|choices| choices.get(&index))
            else {
                continue;
            };
            let stem = if sprite.stem.is_empty() {
                choice.stem.as_slice()
            } else {
                b""
            };
            sprite.merge(stem, &choice.selectors);
        }
        (!sprite.stem.is_empty()).then_some(sprite)
    }
}

struct TextChoice {
    value: Vec<u8>,
    next: Vec<u8>,
}

#[derive(Default)]
struct TextTable(BTreeMap<Vec<u8>, Vec<TextChoice>>);

impl TextTable {
    fn parse(bytes: &[u8]) -> Self {
        let mut table = Self::default();
        let mut current = None;
        for line in crate::trim_nul(bytes).split(|&byte| byte == b'\n') {
            let line = line.trim_ascii();
            if let Some(key) = line.strip_prefix(b"#") {
                let key = key.trim_ascii().to_vec();
                table.0.entry(key.clone()).or_default();
                current = Some(key);
            } else if !line.is_empty()
                && !line.starts_with(b"//")
                && let Some(key) = &current
            {
                let mut columns = line.splitn(2, |b| b.is_ascii_whitespace());
                table.0.get_mut(key).unwrap().push(TextChoice {
                    value: columns.next().unwrap_or_default().to_vec(),
                    next: columns.next().unwrap_or_default().trim_ascii().to_vec(),
                });
            }
        }
        table
    }

    fn traverse(&self, path: &[u32], mut apply: impl FnMut(usize, &[u8])) -> Option<&[u8]> {
        let mut key = b"_TOP_".as_slice();
        for (depth, &index) in path.iter().enumerate() {
            let choice = self.0.get(key)?.get(usize::try_from(index).ok()?)?;
            apply(depth, &choice.value);
            key = &choice.next;
        }
        Some(key)
    }

    fn count(&self, path: &[u32]) -> i32 {
        self.traverse(path, |_, _| {})
            .and_then(|key| self.0.get(key))
            .map_or(0, |choices| choices.len() as i32)
    }

    fn image(&self, path: &[u32]) -> Option<Sprite> {
        let mut sprite = Sprite::default();
        self.traverse(path, |depth, value| {
            if depth == 1 {
                if value != b"@" {
                    sprite.merge(value, b"");
                }
            } else if depth > 1 {
                let selectors: Vec<_> = value
                    .split(|&byte| byte == b',')
                    .map(|part| part.first().copied().unwrap_or(b'@'))
                    .collect();
                sprite.merge(b"", &selectors);
            }
        })?;
        (!sprite.stem.is_empty()).then_some(sprite)
    }
}

#[cfg(test)]
mod text_table_tests {
    use super::{CharacterTable, TextTable};

    #[test]
    fn text_table_uses_unsigned_indices_and_composes_image_selectors() {
        let table = TextTable::parse(
            b"#_TOP_\nentry CHAR\n#CHAR\nbg01 POSE\n#POSE\n1,2 VARIANT\n#VARIANT\nlast\n",
        );
        assert_eq!(table.count(&[0, 0, 0]), 1);
        let image = table.image(&[0, 0, 0]).unwrap();
        assert_eq!(image.plan(crate::ScriptTextEncoding::Utf8), "bg01,1,2");
    }

    #[test]
    fn text_table_rejects_indices_outside_the_u32_and_table_ranges() {
        let table = CharacterTable::Text(TextTable::parse(b"#_TOP_\nentry #NEXT\n#NEXT\nitem\n"));
        assert_eq!(table.count(&[-1]), 0);
        assert!(
            table
                .image(&[-1], crate::ScriptTextEncoding::Utf8)
                .is_none()
        );
        assert_eq!(table.count(&[u32::MAX as i32]), 0);
        assert!(
            table
                .image(&[u32::MAX as i32], crate::ScriptTextEncoding::Utf8)
                .is_none()
        );
    }
}

pub(super) fn update_characters(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &mut FesRuntime,
) -> Result<()> {
    let objects = &host.data_services.tasks.fes.platform.objects;
    let pending: Vec<_> = objects
        .iter()
        .filter(|((owner, _, _), binding)| *owner == view.id && binding.gallery.dirty)
        .map(|(identity, binding)| {
            (
                identity.clone(),
                binding.key.clone(),
                binding.gallery.image_plan.clone(),
                binding.gallery.tone.unwrap_or(NEUTRAL_TONE),
                binding.gallery.base_option.clone(),
            )
        })
        .collect();
    for (identity, key, plan, tone, base_option) in pending {
        let Some(plan) = plan else {
            continue;
        };
        let mut image = None;
        for name in host.resources.category_names(b"image", &plan)? {
            if let Some(frame) = host.load_managed_image_plan(&name, false)? {
                image = Some(apply_tone(frame, tone)?);
                break;
            }
        }
        let layer = runtime
            .media
            .get_mut(&key)
            .context("FES character image has no media binding")?;
        layer.file = Some(plan);
        layer.image = None;
        if let Some(mut image) = image {
            let mut metadata = image.metadata();
            match base_option.first() {
                Some(b's') => metadata.base = [metadata.canvas_size[0] as i32 / 2, 0],
                Some(b'c') => metadata.base = metadata.canvas_size.map(|extent| extent as i32 / 2),
                _ => {}
            }
            image = image.with_raw_metadata(metadata);
            crate::fes_media::install_ordinary_frame(layer, image);
        }
        host.data_services
            .tasks
            .fes
            .platform
            .objects
            .get_mut(&identity)
            .context("FES character bank disappeared during loading")?
            .gallery
            .dirty = false;
    }
    Ok(())
}

pub(in crate) fn apply_tone(image: RgbaFrame, tone: [i32; 3]) -> Result<RgbaFrame> {
    if tone == NEUTRAL_TONE {
        return Ok(image);
    }
    let mut pixels = image.pixels().to_vec();
    for pixel in pixels.chunks_exact_mut(4) {
        for (channel, percent) in pixel[..3].iter_mut().zip(tone) {
            *channel = (i64::from(*channel) * i64::from(percent) / 100).clamp(0, 255) as u8;
        }
    }
    Ok(
        RgbaFrame::from_rgba8(image.width(), image.height(), pixels)?
            .with_raw_metadata(image.metadata()),
    )
}

pub(super) fn apply_command(
    host: &mut LoggingHost<'_>,
    view: &FesTaskView,
    runtime: &mut FesRuntime,
    driver: &mut FesScriptDriver,
    command: &FesDriverCommand,
    shared: &mut ExpressionState,
) -> Result<bool> {
    let Some(verb) = command
        .parameters
        .get(1)
        .map(|verb| verb.to_ascii_lowercase())
    else {
        return Ok(false);
    };
    if !matches!(
        verb.as_slice(),
        b"load"
            | b"max"
            | b"exist"
            | b"tonetbl"
            | b"gettonetbl"
            | b"tone"
            | b"baseopt"
            | b"coordmode"
            | b"pri"
            | b"getpos"
    ) {
        return Ok(false);
    }
    let target = object_command_target(command)?;
    let (name, bank) = crate::fes_media::split_target(&target, host.text_encoding)?;
    let Some(object) = driver.adapter.object(name) else {
        return Ok(false);
    };
    let kind = object.definition.kind_name.clone();
    if !is_gallery_kind(&kind) {
        return Ok(false);
    }
    let banks = object.definition.bank_count;
    let current = command.context.current;
    let bank = bank
        .map(|bank| driver.evaluate_parameter(bank, current, shared))
        .transpose()?
        .unwrap_or(-1);
    let range = if bank == -1 {
        0..i32::try_from(banks)?
    } else {
        bank..bank.saturating_add(1)
    };
    let operands = &command.parameters[2..];
    let character = is_character_kind(&kind);
    let count = match (character, verb.as_slice()) {
        (false, b"max") => 1,
        (false, b"exist" | b"gettonetbl") => 3,
        (false, b"load") => 2,
        (false, b"tonetbl") | (true, b"baseopt") => 0,
        (true, b"max") => 4,
        (true, b"load") => 5,
        (true, b"tone") => 3,
        (true, b"coordmode" | b"pri") => 1,
        (true, b"getpos") => 2,
        _ => return Ok(false),
    };
    let mut values = [0; 5];
    for (value, operand) in values[..count].iter_mut().zip(operands) {
        *value = driver.evaluate_parameter(operand, current, shared)?;
    }
    let mut write = |operand: usize, value| {
        if operands.get(operand).is_some()
            && let Some(slot) = shared.shared.get_mut(values[operand] as usize)
        {
            *slot = value;
        }
    };
    for bank in range {
        let Some(object) = driver.adapter.object_bank(name, bank as usize) else {
            continue;
        };
        let identity = (view.id, name.to_vec(), bank);
        let platform = &mut host.data_services.tasks.fes.platform;
        let Some(binding) = platform.objects.get_mut(&identity) else {
            continue;
        };
        if binding.kind != kind {
            continue;
        }
        let layer = runtime
            .media
            .get_mut(&binding.key)
            .context("FES gallery bank has no media binding")?;
        let cache = &mut platform.gallery_tables;
        let paths = object
            .initializers
            .iter()
            .rev()
            .filter(|init| init.column == b"TABLE")
            .map(|init| host.text_encoding.decode(crate::trim_nul(&init.value)));
        if character {
            match verb.as_slice() {
                b"tone" => {
                    let tone = std::array::from_fn(|component| {
                        if operands.get(component).is_some() {
                            values[component]
                        } else {
                            100
                        }
                    });
                    if binding.gallery.tone.unwrap_or(NEUTRAL_TONE) != tone {
                        binding.gallery.tone = Some(tone);
                        binding.gallery.dirty = binding.gallery.image_plan.is_some();
                    }
                }
                b"baseopt" => {
                    let option = operands.first().cloned().unwrap_or_default();
                    if binding.gallery.base_option != option {
                        binding.gallery.base_option = option;
                        binding.gallery.dirty = binding.gallery.image_plan.is_some();
                    }
                }
                b"coordmode" => layer.source.coordmode = values[0],
                b"pri" => layer.priority = values[0],
                b"getpos" => {
                    write(0, layer.position.x);
                    write(1, layer.position.y);
                }
                b"max" | b"load" => {
                    let mut count = 0;
                    for path in paths {
                        if let Some(table) = characters(cache, host.resources, &path)? {
                            if verb == b"max" {
                                count = table.count(&values[1..operands.len().clamp(1, 4)]);
                            } else if let Some(plan) =
                                table.image(&values[..operands.len().min(5)], host.text_encoding)
                            {
                                binding.gallery.dirty |=
                                    binding.gallery.image_plan.as_ref() != Some(&plan);
                                binding.gallery.image_plan = Some(plan);
                            }
                            break;
                        }
                    }
                    if verb == b"max" {
                        write(0, count);
                    }
                }
                _ => unreachable!(),
            }
        } else {
            match verb.as_slice() {
                b"tonetbl" => {
                    if let Some(value) = operands.first() {
                        let path = host.text_encoding.decode(crate::trim_nul(value));
                        binding.gallery.tone_table =
                            tones(cache, host.resources, &path)?.map(|_| path);
                    }
                }
                b"gettonetbl" => {
                    let tone = match (&binding.gallery.tone_table, &layer.file) {
                        (Some(path), Some(file)) => {
                            let key = file.as_bytes().to_ascii_lowercase();
                            tones(cache, host.resources, path)?
                                .and_then(|table| table.get(&key))
                                .copied()
                        }
                        _ => None,
                    }
                    .unwrap_or(NEUTRAL_TONE);
                    for (index, value) in tone.into_iter().enumerate() {
                        write(index, value);
                    }
                }
                b"load" | b"max" | b"exist" => {
                    let mut result = 0;
                    let mut selected = None;
                    for path in paths {
                        if let Some(table) = background(cache, host.resources, &path)? {
                            if verb == b"max" {
                                result = table.rows.len() as i32;
                            } else if let Some(cell) = table.image(values[0], values[1]) {
                                result = 1;
                                selected = Some(cell.to_vec());
                            }
                            break;
                        }
                    }
                    match verb.as_slice() {
                        b"max" => write(0, result),
                        b"exist" => write(2, result),
                        b"load" => {
                            if let Some(file) = selected {
                                crate::fes_media::request_image_file(host, layer, &file)?;
                            }
                        }
                        _ => unreachable!(),
                    }
                }
                _ => unreachable!(),
            }
        }
    }
    Ok(true)
}
