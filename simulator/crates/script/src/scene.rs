use crate::{
    CstDocument, FesDocument, FesLine, FesSection, MAX_SCRIPT_ITEMS, Result, ScriptError,
    cstl::CstlLocalization,
    expression::{
        ExpressionState, eval_expression, eval_expression_fallback_with_text,
        eval_expression_parameter,
    },
    inflate_exact, read_range, read_u32, trim_ascii,
};
use std::collections::BTreeMap;

#[path = "fes_snapshot.rs"]
mod fes_snapshot;

const MAX_FES_CALL_DEPTH: usize = 128;
const MAX_FES_BLOCKS: usize = 1_000_000;
const MAX_FES_LINES: usize = 1_000_000;
const MAX_CST_CALL_DEPTH: usize = 128;
const MAX_CST_CHOICES: usize = 4096;

fn scene_error(offset: usize, message: &'static str) -> ScriptError {
    ScriptError::InvalidSceneControl { offset, message }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesObjectDefinition {
    pub offset: usize,
    pub kind: Vec<u8>,
    pub name: Vec<u8>,
    pub bank_expression: Option<Vec<u8>>,
    pub banks: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesObjectInitialization {
    pub offset: usize,
    pub columns: Vec<Vec<u8>>,
    pub target: Vec<u8>,
    pub values: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesKeyBlock {
    pub index: usize,
    pub size: [i32; 2],
    pub neighbors: [i32; 4],
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesKeyBlockTable {
    pub offset: usize,
    pub declared_count: usize,
    pub blocks: Vec<FesKeyBlock>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
struct FesKeyBlockSpec {
    index: usize,
    size: [Option<Vec<u8>>; 2],
    neighbors: [Option<Vec<u8>>; 4],
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
struct FesKeyBlockTableSpec {
    offset: usize,
    declared_count: usize,
    blocks: Vec<FesKeyBlockSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesKeyBlockBinding {
    pub offset: usize,
    pub target: Vec<u8>,
    pub block: i32,
    pub position: [i32; 2],
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
struct FesKeyBlockBindingSpec {
    offset: usize,
    target: Vec<u8>,
    block: Vec<u8>,
    position: [Option<Vec<u8>>; 2],
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesSubcommandParameter {
    pub descriptor: Vec<u8>,
    pub default: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub enum FesSubcommandArgument {
    Parameter(usize),
    Integer(i32),
    Context(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesSubcommand {
    pub offset: usize,
    pub mode: Vec<u8>,
    pub name: Vec<u8>,
    pub target: Vec<u8>,
    pub parameters: Vec<FesSubcommandParameter>,
    pub arguments: Vec<FesSubcommandArgument>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesSubcommandTable {
    pub declared: bool,
    pub bindings: Vec<FesSubcommand>,
    #[serde(skip)]
    pub diagnostics: Vec<ScriptError>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesInitializationPlan {
    keyblocks: Vec<FesKeyBlockTableSpec>,
    pub definitions: Vec<FesObjectDefinition>,
    pub initializers: Vec<FesObjectInitialization>,
    key_bindings: Vec<FesKeyBlockBindingSpec>,
    pub subcommands: FesSubcommandTable,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesBlock {
    pub name: Vec<u8>,
    pub lines: Vec<FesLine>,
    false_jump: Vec<Option<usize>>,
    end_jump: Vec<Option<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesProgram {
    pub blocks: Vec<FesBlock>,
    pub declarations: Vec<FesSection>,
    #[serde(skip)]
    pub diagnostics: Vec<ScriptError>,
    macros: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl FesProgram {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.get(..4) != Some(b"FES\0") {
            return Err(ScriptError::InvalidSignature { kind: "FES" });
        }
        let compressed = read_u32(bytes, 4)? as usize;
        let uncompressed = read_u32(bytes, 8)? as usize;
        if bytes.len() != 16 + compressed {
            return Err(ScriptError::InvalidStructuredText {
                offset: 4,
                message: "FES length mismatch",
            });
        }
        let body = inflate_exact("FES", read_range(bytes, 16, compressed)?, uncompressed)?;
        Self::from_text(&body)
    }

    pub fn from_document(document: &FesDocument) -> Result<Self> {
        // FesDocument retains compiled text, including comments. Re-enter the
        // same text lexer instead of treating its raw lines as pre-lexed code.
        let mut source = Vec::new();
        let mut offsets = BTreeMap::new();
        for section in &document.sections {
            source.push(b'#');
            source.extend_from_slice(&section.header);
            source.push(b'\n');
            for line in &section.lines {
                offsets.insert(source.len(), line.offset);
                source.extend_from_slice(&line.bytes);
                source.push(b'\n');
            }
            if source.len() > super::MAX_SCRIPT_BYTES {
                return Err(scene_error(source.len(), "FES document exceeds size limit"));
            }
        }
        let mut program = Self::from_text(&source)?;
        for line in program.blocks.iter_mut().flat_map(|block| &mut block.lines)
            .chain(program.declarations.iter_mut().flat_map(|section| &mut section.lines))
        {
            if let Some((&start, &original)) = offsets.range(..=line.offset).next_back() {
                line.offset = original + (line.offset - start);
            }
        }
        for diagnostic in &mut program.diagnostics {
            if let ScriptError::InvalidSceneControl { offset, .. } = diagnostic {
                if let Some((&start, &original)) = offsets.range(..=*offset).next_back() {
                    *offset = original + (*offset - start);
                }
            }
        }
        Ok(program)
    }

    pub fn from_text(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > super::MAX_SCRIPT_BYTES {
            return Err(ScriptError::InvalidHeader {
                kind: "FES",
                message: "text exceeds size limit",
            });
        }
        let cleaned = strip_fes_comments(bytes)?;
        let mut sections = Vec::new();
        let mut offset = 0;
        for raw in cleaned.split_inclusive(|&byte| byte == b'\n') {
            let line_offset = offset;
            offset += raw.len();
            let line = raw
                .strip_suffix(b"\n")
                .unwrap_or(raw)
                .strip_suffix(b"\r")
                .unwrap_or_else(|| raw.strip_suffix(b"\n").unwrap_or(raw));
            let trimmed = trim_ascii(line);
            if let Some(header) = trimmed.strip_prefix(b"#") {
                if sections.len() >= MAX_FES_BLOCKS {
                    return Err(scene_error(line_offset, "too many FES sections"));
                }
                sections.push(FesSection {
                    header: header.to_vec(),
                    lines: Vec::new(),
                });
            } else if !trimmed.is_empty() {
                let Some(section) = sections.last_mut() else {
                    continue;
                };
                if section.lines.len() >= MAX_FES_LINES {
                    return Err(scene_error(line_offset, "too many FES lines"));
                }
                let leading = line.len() - line.trim_ascii_start().len();
                section.lines.push(FesLine {
                    offset: line_offset + leading,
                    bytes: trimmed.to_vec(),
                });
            }
        }
        Self::from_sections(sections)
    }

    fn from_sections(sections: Vec<FesSection>) -> Result<Self> {
        let mut program = Self {
            blocks: Vec::new(),
            declarations: Vec::new(),
            diagnostics: Vec::new(),
            macros: BTreeMap::new(),
        };
        let mut macros = BTreeMap::<Vec<u8>, Vec<u8>>::new();
        for section in &sections {
            let kind = section_kind(&section.header);
            if kind == b"MACRO" {
                for line in &section.lines {
                    match parse_macro(&line.bytes) {
                        Ok((name, value)) => {
                            macros.insert(name, value);
                        }
                        Err(error) if is_fes_source_marker(&line.bytes) => {
                            program.diagnostics.push(scene_error(
                                line.offset + error,
                                "FES macro source marker retained",
                            ));
                        }
                        Err(error) => {
                            return Err(scene_error(line.offset + error, "invalid FES macro"));
                        }
                    }
                }
            }
        }
        program.macros = macros.clone();
        for section in sections {
            let kind = section_kind(&section.header);
            if is_declaration(kind) {
                program.declarations.push(FesSection {
                    header: section.header,
                    lines: section.lines,
                });
                continue;
            }
            let name = section
                .header
                .split(u8::is_ascii_whitespace)
                .next()
                .unwrap_or_default()
                .to_vec();
            // Block and event names share the resource's raw byte spelling.
            // Numeric identifiers have their separate ASCII grammar; it does
            // not restrict object/event headers or native 630 block names.
            if name.is_empty() || name.contains(&0) {
                return Err(scene_error(0, "FES block name is empty or contains zero"));
            }
            let lines = section
                .lines
                .into_iter()
                .map(|line| {
                    Ok(FesLine {
                        offset: line.offset,
                        // Keep source bytes intact. Runtime FES expands string
                        // variables before macros, so a macro can introduce a
                        // `$strN` reference that is resolved on execution.
                        bytes: line.bytes,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            if let Some(block) = program.blocks.iter_mut().find(|block| block.name == name) {
                block.lines.extend(lines);
            } else {
                program.blocks.push(FesBlock {
                    name,
                    lines,
                    false_jump: Vec::new(),
                    end_jump: Vec::new(),
                });
            }
        }
        let (blocks, diagnostics) = (&mut program.blocks, &mut program.diagnostics);
        for block in blocks {
            block.false_jump = vec![None; block.lines.len()];
            block.end_jump = vec![None; block.lines.len()];
            let mut stack = Vec::<(usize, Option<usize>)>::new();
            for index in 0..block.lines.len() {
                let line = &block.lines[index];
                let structural = expand_fes_macros(&line.bytes, &macros)?;
                if let Some((_, tail)) = split_fes_condition(&structural)? {
                    if tail.is_empty() {
                        stack.push((index, None));
                    }
                } else if structural == b"else" {
                    if let Some((start, alternative @ None)) = stack.last_mut() {
                        block.false_jump[*start] = Some(index + 1);
                        *alternative = Some(index);
                    } else {
                        diagnostics.push(scene_error(line.offset, "isolated or repeated else"));
                    }
                } else if structural == b"endif" {
                    if let Some((start, alternative)) = stack.pop() {
                        if let Some(alternative) = alternative {
                            block.end_jump[alternative] = Some(index + 1);
                        } else {
                            block.false_jump[start] = Some(index + 1);
                        }
                    }
                }
            }
            for (start, _) in stack {
                block.false_jump[start] = Some(block.lines.len());
            }
        }
        Ok(program)
    }

    pub fn block(&self, name: &[u8]) -> Option<&FesBlock> {
        self.blocks.iter().find(|block| block.name == name)
    }

    pub fn macro_value(&self, name: &[u8]) -> Option<&[u8]> {
        self.macros.get(name).map(Vec::as_slice)
    }

    pub fn preload_names(&self) -> Vec<Vec<u8>> {
        self.declarations
            .iter()
            .filter(|section| section_kind(&section.header) == b"PRELOAD")
            .flat_map(|section| section.lines.iter().map(|line| line.bytes.clone()))
            .collect()
    }

    pub fn declaration_sections(&self, kind: &[u8]) -> Vec<&FesSection> {
        self.declarations
            .iter()
            .filter(move |section| section_kind(&section.header) == kind)
            .collect()
    }

    pub fn expand_macros(&self, input: &[u8]) -> Result<Vec<u8>> {
        expand_fes_macros(input, &self.macros)
    }

    /// Parameter extraction occurs between the two string-expansion stages.
    /// A macro-introduced string containing spaces remains a single parameter.
    pub fn command_parameters(
        &self,
        input: &[u8],
        strings: &BTreeMap<u32, Vec<u8>>,
    ) -> Result<Vec<Vec<u8>>> {
        let line = expand_strings(input, strings);
        let line = expand_fes_macros(&line, &self.macros)?;
        Ok(fes_parameters(&line)?.into_iter()
            .map(|word| expand_strings(&word, strings)).collect())
    }

    pub fn initialization_plan(&self) -> Result<FesInitializationPlan> {
        let mut plan = FesInitializationPlan::default();
        for section in &self.declarations {
            match section_kind(&section.header) {
                b"DEFINE" => {
                    for line in &section.lines {
                        plan.definitions.push(parse_object_definition(line)?);
                    }
                }
                b"KEYBLOCK" => {
                    if plan.keyblocks.is_empty() {
                        plan.keyblocks.push(parse_keyblock_table(section)?);
                    }
                }
                b"OBJECT" => {
                    let columns = declaration_words(&section.header[6..])?;
                    if columns.first().map(Vec::as_slice) == Some(b"KEYBLOCK") {
                        for line in &section.lines {
                            plan.key_bindings.push(parse_keyblock_binding(line)?);
                        }
                    } else {
                        for line in &section.lines {
                            plan.initializers.push(FesObjectInitialization {
                                offset: line.offset,
                                columns: columns.clone(),
                                target: declaration_words(&line.bytes)?
                                    .first()
                                    .cloned()
                                    .ok_or_else(|| {
                                        scene_error(line.offset, "FES OBJECT 行缺少目标")
                                    })?,
                                values: declaration_words(&line.bytes)?
                                    .into_iter()
                                    .skip(1)
                                    .collect(),
                            });
                        }
                    }
                }
                b"SUBCOMMAND" => {
                    plan.subcommands.declared = true;
                    for line in &section.lines {
                        match parse_subcommand(line) {
                            Ok(binding) => {
                                if !plan
                                    .subcommands
                                    .bindings
                                    .iter()
                                    .any(|existing| existing.name == binding.name)
                                {
                                    plan.subcommands.bindings.push(binding);
                                }
                            }
                            Err(error) => plan.subcommands.diagnostics.push(error),
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(plan)
    }
}

fn section_kind(header: &[u8]) -> &[u8] {
    header
        .split(u8::is_ascii_whitespace)
        .next()
        .unwrap_or_default()
}

fn is_declaration(kind: &[u8]) -> bool {
    matches!(
        kind,
        b"INCLUDE" | b"PRELOAD" | b"MACRO" | b"DEFINE" | b"OBJECT" | b"KEYBLOCK" | b"SUBCOMMAND"
    )
}

fn declaration_words(input: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut words = Vec::new();
    let mut cursor = 0;
    while cursor < input.len() {
        while input
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            cursor += 1;
        }
        if cursor >= input.len() {
            break;
        }
        let start = cursor;
        let mut quote = None;
        let mut parenthesis = 0usize;
        let mut bracket = 0usize;
        while cursor < input.len() {
            let byte = input[cursor];
            if let Some(delimiter) = quote {
                cursor += 1;
                if byte == delimiter {
                    quote = None;
                } else if is_cp932_lead(byte) {
                    cursor += usize::from(cursor < input.len() && is_cp932_trail(input[cursor]));
                }
                continue;
            }
            if byte == b'"' {
                quote = Some(byte);
                cursor += 1;
                continue;
            }
            if byte == b'(' {
                parenthesis += 1;
                cursor += 1;
                continue;
            }
            if byte == b')' {
                parenthesis = parenthesis.saturating_sub(1);
                cursor += 1;
                continue;
            }
            if byte == b'[' {
                bracket += 1;
                cursor += 1;
                continue;
            }
            if byte == b']' {
                bracket = bracket.saturating_sub(1);
                cursor += 1;
                continue;
            }
            if is_cp932_pair(input, cursor) {
                cursor += 2;
                continue;
            }
            if parenthesis == 0 && bracket == 0 && byte.is_ascii_whitespace() {
                break;
            }
            cursor += 1;
        }
        if quote.is_some() {
            return Err(scene_error(
                start,
                "FES declaration quote is not terminated",
            ));
        }
        if parenthesis != 0 {
            return Err(scene_error(
                start,
                "FES declaration parenthesis is not terminated",
            ));
        }
        if bracket != 0 {
            return Err(scene_error(
                start,
                "FES declaration bracket is not terminated",
            ));
        }
        words.push(input[start..cursor].to_vec());
    }
    if words.len() > super::MAX_SCRIPT_ITEMS {
        return Err(scene_error(
            input.len(),
            "too many FES declaration parameters",
        ));
    }
    Ok(words)
}

fn parse_object_definition(line: &FesLine) -> Result<FesObjectDefinition> {
    let words = declaration_words(&line.bytes)?;
    let kind = words
        .first()
        .cloned()
        .ok_or_else(|| scene_error(line.offset, "FES DEFINE 行缺少对象类型"))?;
    let target = words
        .get(1)
        .ok_or_else(|| scene_error(line.offset, "FES DEFINE 行缺少对象名称"))?;
    let (name, bank_expression) = split_object_target(target, line.offset)?;
    let banks = match bank_expression.as_deref() {
        None => -1,
        Some(expression) => parse_i32_prefix(expression).unwrap_or(-1),
    };
    Ok(FesObjectDefinition {
        offset: line.offset,
        kind,
        name,
        bank_expression,
        banks,
    })
}

fn split_object_target(input: &[u8], offset: usize) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
    let split = input
        .iter()
        .position(|byte| *byte == b'[')
        .unwrap_or(input.len());
    let name = input[..split].to_vec();
    if name.is_empty() {
        return Err(scene_error(offset, "FES object name is empty"));
    }
    if split == input.len() {
        return Ok((name, None));
    }
    if input.last() != Some(&b']') || split + 1 == input.len() - 1 {
        return Err(scene_error(
            offset + split,
            "FES object bank expression is invalid",
        ));
    }
    Ok((name, Some(input[split + 1..input.len() - 1].to_vec())))
}

fn parse_keyblock_table(section: &FesSection) -> Result<FesKeyBlockTableSpec> {
    let header = declaration_words(&section.header)?;
    let mut first_row = 0;
    let declared_count =
        if let Some(count) = header.get(1).and_then(|value| parse_i32_prefix(value)) {
            count
        } else if let Some(count) = section
            .lines
            .first()
            .and_then(|line| declaration_words(&line.bytes).ok())
            .and_then(|words| words.first().cloned())
            .and_then(|value| parse_i32_prefix(&value))
        {
            first_row = 1;
            count
        } else {
            return Err(scene_error(
                section.header.len(),
                "FES KEYBLOCK count is invalid",
            ));
        };
    if declared_count < 0 || declared_count as usize > MAX_SCRIPT_ITEMS {
        return Err(scene_error(
            section.header.len(),
            "FES KEYBLOCK count exceeds limit",
        ));
    }
    let declared_count = declared_count as usize;
    let mut blocks = (0..declared_count)
        .map(|index| FesKeyBlockSpec {
            index,
            size: [None, None],
            neighbors: [None, None, None, None],
        })
        .collect::<Vec<_>>();
    for line in section.lines.iter().skip(first_row) {
        let words = declaration_words(&line.bytes)?;
        let Some(index) = words.first().and_then(|value| parse_i32_prefix(value)) else {
            return Err(scene_error(
                line.offset,
                "FES KEYBLOCK row index is invalid",
            ));
        };
        let Some(index) = usize::try_from(index)
            .ok()
            .filter(|index| *index < declared_count)
        else {
            continue;
        };
        let size_word = words
            .get(1)
            .ok_or_else(|| scene_error(line.offset, "FES KEYBLOCK row has no size"))?;
        let size = raw_components(size_word, 2, line.offset)?;
        let neighbors = words
            .get(2)
            .map(|word| raw_components_optional(word, 4, line.offset))
            .transpose()?
            .unwrap_or([None, None, None, None]);
        let block = &mut blocks[index];
        for component in 0..2 {
            if size[component].is_some() {
                block.size[component] = size[component].clone();
            }
        }
        for component in 0..4 {
            if neighbors[component].is_some() {
                block.neighbors[component] = neighbors[component].clone();
            }
        }
    }
    Ok(FesKeyBlockTableSpec {
        offset: section.lines.first().map_or(0, |line| line.offset),
        declared_count,
        blocks,
    })
}

fn parse_keyblock_binding(line: &FesLine) -> Result<FesKeyBlockBindingSpec> {
    let words = declaration_words(&line.bytes)?;
    let target = words
        .first()
        .cloned()
        .ok_or_else(|| scene_error(line.offset, "FES KEYBLOCK binding lacks target"))?;
    let block = words
        .get(1)
        .and_then(|value| value.split(|byte| *byte == b',').next())
        .filter(|value| !value.is_empty())
        .map(<[u8]>::to_vec)
        .ok_or_else(|| scene_error(line.offset, "FES KEYBLOCK binding index is invalid"))?;
    let position = words
        .get(1)
        .map(|value| raw_components_optional(value, 3, line.offset))
        .transpose()?
        .unwrap_or([None, None, None, None]);
    Ok(FesKeyBlockBindingSpec {
        offset: line.offset,
        target,
        block,
        position: [position[1].clone(), position[2].clone()],
    })
}

fn raw_components(value: &[u8], count: usize, offset: usize) -> Result<Vec<Option<Vec<u8>>>> {
    let parts = value.split(|byte| *byte == b',').collect::<Vec<_>>();
    if parts.len() < count {
        return Err(scene_error(
            offset,
            "FES declaration has too few components",
        ));
    }
    Ok(parts
        .into_iter()
        .take(count)
        .map(|part| Some(part.to_vec()))
        .collect())
}

fn raw_components_optional(
    value: &[u8],
    count: usize,
    offset: usize,
) -> Result<[Option<Vec<u8>>; 4]> {
    if count > 4 {
        return Err(scene_error(offset, "FES component count exceeds limit"));
    }
    let parts = value.split(|byte| *byte == b',').collect::<Vec<_>>();
    let mut output = [None, None, None, None];
    for (index, part) in parts.into_iter().take(count).enumerate() {
        if part.is_empty() {
            if index + 1 < count {
                output[index] = Some(Vec::new());
            }
        } else {
            output[index] = Some(part.to_vec());
        }
    }
    Ok(output)
}

fn parse_subcommand(line: &FesLine) -> Result<FesSubcommand> {
    let words = declaration_words(&line.bytes)?;
    let mode = words
        .first()
        .cloned()
        .ok_or_else(|| scene_error(line.offset, "FES SUBCOMMAND mode is missing"))?;
    if mode != b"oneshot" && mode != b"continue" {
        return Err(scene_error(line.offset, "FES SUBCOMMAND mode is invalid"));
    }
    let name = words
        .get(1)
        .cloned()
        .ok_or_else(|| scene_error(line.offset, "FES SUBCOMMAND name is missing"))?;
    let target = words
        .get(2)
        .cloned()
        .ok_or_else(|| scene_error(line.offset, "FES SUBCOMMAND target is missing"))?;
    let separator = words
        .iter()
        .position(|word| word == b":")
        .ok_or_else(|| scene_error(line.offset, "FES SUBCOMMAND mapping separator is missing"))?;
    if separator < 3 || separator - 3 > 16 || words.len() - separator - 1 > 16 {
        return Err(scene_error(
            line.offset,
            "FES SUBCOMMAND has too many parameters",
        ));
    }
    let mut parameters = Vec::new();
    for word in &words[3..separator] {
        let (descriptor, default) = split_bracket_value(word, b"-1", line.offset)?;
        if !is_subcommand_descriptor(&descriptor) || default.len() > 15 {
            return Err(scene_error(
                line.offset,
                "FES SUBCOMMAND parameter is too long",
            ));
        }
        parameters.push(FesSubcommandParameter {
            descriptor,
            default,
        });
    }
    let mut arguments = Vec::new();
    for word in &words[separator + 1..] {
        if word.starts_with(b"[") {
            if word.last() != Some(&b']') || word.len() < 3 {
                return Err(scene_error(
                    line.offset,
                    "FES SUBCOMMAND parameter mapping is invalid",
                ));
            }
            let index = parse_i32_prefix(&word[1..word.len() - 1])
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| {
                    scene_error(line.offset, "FES SUBCOMMAND parameter index is invalid")
                })?;
            arguments.push(FesSubcommandArgument::Parameter(index));
        } else if word == b"cxr"
            || word == b"cxg"
            || word == b"cxb"
            || word == b"cxa"
            || word == b"CA"
        {
            arguments.push(FesSubcommandArgument::Context(word.clone()));
        } else {
            let value = parse_i32_prefix(word)
                .ok_or_else(|| scene_error(line.offset, "FES SUBCOMMAND literal is invalid"))?;
            arguments.push(FesSubcommandArgument::Integer(value));
        }
    }
    Ok(FesSubcommand {
        offset: line.offset,
        mode,
        name,
        target,
        parameters,
        arguments,
    })
}

fn is_subcommand_descriptor(descriptor: &[u8]) -> bool {
    matches!(
        descriptor,
        b"p" | b"BX"
            | b"bx"
            | b"BY"
            | b"by"
            | b"BZ"
            | b"bz"
            | b"PX"
            | b"px"
            | b"PY"
            | b"py"
            | b"PZ"
            | b"pz"
            | b"VX"
            | b"vx"
            | b"VY"
            | b"vy"
            | b"VZ"
            | b"vz"
            | b"RX"
            | b"rx"
            | b"RY"
            | b"ry"
            | b"RZ"
            | b"rz"
            | b"SX"
            | b"sx"
            | b"SY"
            | b"sy"
            | b"SZ"
            | b"sz"
            | b"CR"
            | b"cr"
            | b"CG"
            | b"cg"
            | b"CB"
            | b"cb"
            | b"CA"
            | b"ca"
            | b"CX"
            | b"cx"
    )
}

fn split_bracket_value(input: &[u8], default: &[u8], offset: usize) -> Result<(Vec<u8>, Vec<u8>)> {
    let Some(open) = input.iter().position(|byte| *byte == b'[') else {
        return Ok((input.to_vec(), default.to_vec()));
    };
    if input.last() != Some(&b']') || open == 0 {
        return Err(scene_error(offset, "FES bracket value is invalid"));
    }
    Ok((
        input[..open].to_vec(),
        input[open + 1..input.len() - 1].to_vec(),
    ))
}

fn parse_i32_prefix(input: &[u8]) -> Option<i32> {
    let mut cursor = 0;
    let negative = match input.first() {
        Some(b'-') => {
            cursor = 1;
            true
        }
        Some(b'+') => {
            cursor = 1;
            false
        }
        _ => false,
    };
    let radix = if input
        .get(cursor..)
        .is_some_and(|bytes| bytes.starts_with(b"0x") || bytes.starts_with(b"0X"))
    {
        cursor += 2;
        16
    } else {
        10
    };
    let start = cursor;
    let mut value = 0u32;
    while let Some(byte) = input.get(cursor) {
        let Some(digit) = (*byte as char).to_digit(radix) else {
            break;
        };
        value = value.wrapping_mul(radix).wrapping_add(digit);
        cursor += 1;
    }
    if cursor == start || (radix == 10 && input.get(cursor) == Some(&b'.')) {
        return None;
    }
    Some(if negative { value.wrapping_neg() } else { value } as i32)
}

fn is_fes_source_marker(line: &[u8]) -> bool {
    if line.is_empty()
        || !line
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-/\\".contains(byte))
    {
        return false;
    }
    let Some(separator) = line.iter().rposition(|&byte| byte == b'.') else {
        return false;
    };
    let name = &line[..separator];
    let extension = &line[separator + 1..];
    !name.is_empty()
        && (extension.eq_ignore_ascii_case(b"h") || extension.eq_ignore_ascii_case(b"sub"))
}

fn parse_macro(line: &[u8]) -> std::result::Result<(Vec<u8>, Vec<u8>), usize> {
    let split = line
        .iter()
        .position(u8::is_ascii_whitespace)
        .ok_or(0usize)?;
    let name = line[..split].to_vec();
    let tail = line[split..].trim_ascii_start();
    if name.is_empty() || tail.is_empty() {
        return Err(split);
    }
    let value = if tail.first() == Some(&b'"') {
        let end = tail[1..]
            .iter()
            .position(|&byte| byte == b'"')
            .ok_or(split)?
            + 1;
        tail[1..end].to_vec()
    } else if tail.first() == Some(&b'(') {
        let mut depth = 0;
        let mut end = None;
        for (index, byte) in tail.iter().enumerate() {
            if *byte == b'(' {
                depth += 1;
            } else if *byte == b')' {
                depth -= 1;
                if depth == 0 {
                    end = Some(index + 1);
                    break;
                }
            }
        }
        let end = end.ok_or(split)?;
        if tail.get(1) == Some(&b':') {
            tail[2..end - 1].to_vec()
        } else {
            tail[..end].to_vec()
        }
    } else {
        tail.split(u8::is_ascii_whitespace)
            .next()
            .unwrap_or_default()
            .to_vec()
    };
    if value.is_empty() {
        return Err(split);
    }
    Ok((name, value))
}

fn expand_fes_macros(input: &[u8], macros: &BTreeMap<Vec<u8>, Vec<u8>>) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(input.len());
    let mut cursor = 0;
    while cursor < input.len() {
        let start = cursor;
        let byte = input[cursor];
        let replacement =
            if is_cp932_pair(input, cursor) {
                cursor += 2;
                None
            } else if byte == b'"' || (byte == b'\'' && has_matching_quote(input, cursor, byte)) {
                cursor += 1;
                while cursor < input.len() {
                    if input[cursor] == byte {
                        cursor += 1;
                        break;
                    }
                    if is_cp932_pair(input, cursor) {
                        cursor += 2;
                    } else if input[cursor] == b'\\' && input.get(cursor + 1) == Some(&byte) {
                        cursor += 2;
                    } else {
                        cursor += 1;
                    }
                }
                None
            } else if byte.is_ascii_digit() {
                cursor += 1;
                if input
                    .get(start..)
                    .is_some_and(|value| value.starts_with(b"0x") || value.starts_with(b"0X"))
                {
                    cursor = start + 2;
                    while input.get(cursor).is_some_and(u8::is_ascii_hexdigit) {
                        cursor += 1;
                    }
                } else {
                    while input.get(cursor).is_some_and(u8::is_ascii_digit) {
                        cursor += 1;
                    }
                    if input.get(cursor) == Some(&b'.') {
                        cursor += 1;
                        while input.get(cursor).is_some_and(u8::is_ascii_digit) {
                            cursor += 1;
                        }
                    }
                }
                None
            } else if byte.is_ascii_alphabetic() || byte == b'_' {
                cursor += 1;
                while input
                    .get(cursor)
                    .is_some_and(|value| value.is_ascii_alphanumeric() || *value == b'_')
                {
                    cursor += 1;
                }
                macros.get(&input[start..cursor])
            } else {
                cursor += 1;
                None
            };
        let bytes = replacement.map_or(&input[start..cursor], Vec::as_slice);
        if output
            .len()
            .checked_add(bytes.len())
            .is_none_or(|length| length > super::MAX_SCRIPT_BYTES)
        {
            return Err(scene_error(start, "FES macro expansion exceeds size limit"));
        }
        output.extend_from_slice(bytes);
    }
    Ok(output)
}

fn strip_fes_comments(input: &[u8]) -> Result<Vec<u8>> {
    let mut output = input.to_vec();
    let mut cursor = 0;
    let mut block = false;
    let mut quote = None;
    while cursor < input.len() {
        let byte = input[cursor];
        if block {
            if input.get(cursor..cursor + 2) == Some(b"*/") {
                output[cursor..cursor + 2].fill(b' ');
                cursor += 2;
                block = false;
            } else {
                if byte != b'\n' && byte != b'\r' {
                    output[cursor] = b' ';
                }
                cursor += 1;
            }
        } else if let Some(delimiter) = quote {
            if is_cp932_pair(input, cursor) {
                cursor += 2;
            } else if byte == b'\\' && input.get(cursor + 1) == Some(&delimiter) {
                cursor += 2;
            } else {
                if byte == delimiter {
                    quote = None;
                }
                cursor += 1;
            }
        } else if byte == b'"' {
            quote = Some(b'"');
            cursor += 1;
        } else if is_cp932_pair(input, cursor) {
            cursor += 2;
        } else if input.get(cursor..cursor + 2) == Some(b"/*") {
            output[cursor..cursor + 2].fill(b' ');
            cursor += 2;
            block = true;
        } else if input.get(cursor..cursor + 2) == Some(b"//") {
            while cursor < input.len() && input[cursor] != b'\n' {
                output[cursor] = b' ';
                cursor += 1;
            }
        } else {
            cursor += 1;
        }
    }
    if block {
        return Err(scene_error(cursor, "FES block comment is not terminated"));
    }
    if quote.is_some() {
        return Err(scene_error(cursor, "FES quote is not terminated"));
    }
    Ok(output)
}

fn has_matching_quote(input: &[u8], cursor: usize, delimiter: u8) -> bool {
    input
        .get(cursor + 1..)
        .is_some_and(|tail| tail.iter().any(|&byte| byte == delimiter))
}

fn is_cp932_lead(byte: u8) -> bool {
    matches!(byte, 0x81..=0x9f | 0xe0..=0xef)
}

fn is_cp932_trail(byte: u8) -> bool {
    matches!(byte, 0x40..=0x7e | 0x80..=0xfc)
}

fn is_cp932_pair(input: &[u8], cursor: usize) -> bool {
    input
        .get(cursor..cursor + 2)
        .is_some_and(|bytes| is_cp932_lead(bytes[0]) && is_cp932_trail(bytes[1]))
}

fn split_fes_condition(input: &[u8]) -> Result<Option<(&[u8], &[u8])>> {
    if !input.starts_with(b"if")
        || !input
            .get(2)
            .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'(')
    {
        return Ok(None);
    }
    let start = input[2..]
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map_or(input.len(), |position| position + 2);
    if input.get(start) != Some(&b'(') {
        return Err(scene_error(
            start,
            "FES if requires a parenthesized condition",
        ));
    }
    let mut depth = 0;
    for (position, byte) in input.iter().enumerate().skip(start) {
        if *byte == b'(' {
            depth += 1;
        } else if *byte == b')' {
            depth -= 1;
            if depth == 0 {
                return Ok(Some((
                    &input[start + 1..position],
                    input[position + 1..].trim_ascii_start(),
                )));
            }
        }
    }
    Err(scene_error(start, "FES if condition is not terminated"))
}

fn fes_parameters(input: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut words = Vec::new();
    let mut cursor = 0;
    let mut word = Vec::new();
    let mut separated = true;
    let mut ascii_letter_tail = false;
    while cursor < input.len() && input[cursor] != 0 {
        let byte = input[cursor];
        if byte == b' ' {
            if !separated {
                words.push(std::mem::take(&mut word));
            }
            separated = true;
            ascii_letter_tail = false;
            cursor += 1;
        } else if byte == b'"' {
            cursor += 1;
            let start = cursor;
            while input
                .get(cursor)
                .is_some_and(|value| *value != b'"' && *value != 0)
            {
                cursor += 1;
            }
            if input.get(cursor) != Some(&b'"') {
                return Err(scene_error(start, "FES quoted parameter is not terminated"));
            }
            word.extend_from_slice(&input[start..cursor]);
            // The original parameter reader terminates the visible argument
            // stream after a quoted field. Bytes after the closing quote are
            // retained in the source line but are not parameters.
            cursor = input.len();
            separated = false;
        } else if byte == b'(' {
            if ascii_letter_tail {
                words.push(std::mem::take(&mut word));
            }
            let start = cursor;
            let mut depth = 0;
            while let Some(value) = input.get(cursor) {
                if *value == b'(' {
                    depth += 1;
                } else if *value == b')' {
                    depth -= 1;
                    if depth == 0 {
                        cursor += 1;
                        break;
                    }
                }
                cursor += 1;
            }
            if depth != 0 {
                return Err(scene_error(
                    start,
                    "FES parameter parenthesis is not terminated",
                ));
            }
            if input.get(start + 1) == Some(&b':') {
                word.extend_from_slice(&input[start + 2..cursor - 1]);
            } else {
                word.extend_from_slice(&input[start..cursor]);
            }
            ascii_letter_tail = word.last().is_some_and(u8::is_ascii_alphabetic);
            separated = false;
        } else if is_cp932_pair(input, cursor) {
            word.extend_from_slice(&input[cursor..cursor + 2]);
            cursor += 2;
            ascii_letter_tail = false;
            separated = false;
        } else {
            if separated && !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            word.push(byte);
            ascii_letter_tail = byte.is_ascii_alphabetic();
            separated = false;
            cursor += 1;
        }
    }
    if !separated {
        words.push(word);
    }
    if words.len() > super::MAX_SCRIPT_ITEMS {
        return Err(scene_error(cursor, "too many FES parameters"));
    }
    Ok(words)
}

fn expand_strings(input: &[u8], strings: &BTreeMap<u32, Vec<u8>>) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len());
    let mut cursor = 0;
    while cursor < input.len() {
        if input[cursor..].starts_with(b"$str") {
            let start = cursor + 4;
            let mut end = start;
            let mut index = 0u32;
            while let Some(byte) = input.get(end).filter(|byte| byte.is_ascii_digit()) {
                index = index.wrapping_mul(10).wrapping_add(u32::from(*byte - b'0'));
                end += 1;
            }
            if end > start {
                if let Some(value) = strings.get(&index) {
                    output.extend_from_slice(value);
                }
                cursor = end + usize::from(input.get(end) == Some(&b';'));
                continue;
            }
        }
        output.push(input[cursor]);
        cursor += 1;
    }
    output
}

fn string_prefix(input: &[u8]) -> &[u8] {
    input.split(|&byte| byte == 0).next().unwrap_or(input)
}

fn is_numeric_fes_argument(input: &[u8]) -> bool {
    input.first().is_some_and(|byte| {
        byte.is_ascii_digit() || matches!(byte, b'$' | b'\\' | b'(' | b'@' | b'-' | b'+' | b'!' | b'~')
    })
}

const MAX_FES_FORMAT_BYTES: usize = 2560;

fn fes_format_argument(arguments: &[i32], argument: &mut usize, offset: usize) -> Result<u32> {
    let value = arguments
        .get(*argument)
        .ok_or_else(|| scene_error(offset, "strvar format is missing an argument"))?;
    *argument += 1;
    Ok(*value as u32)
}

fn fes_format_number(format: &[u8], cursor: &mut usize) -> Result<usize> {
    let mut value = 0usize;
    while let Some(byte) = format.get(*cursor).filter(|byte| byte.is_ascii_digit()) {
        value = value
            .checked_mul(10)
            .and_then(|value| value.checked_add((*byte - b'0') as usize))
            .filter(|&value| value < MAX_FES_FORMAT_BYTES)
            .ok_or_else(|| scene_error(*cursor, "strvar format field exceeds the size limit"))?;
        *cursor += 1;
    }
    Ok(value)
}

fn fes_format_digits(mut value: u64, radix: u64, uppercase: bool) -> Vec<u8> {
    let alphabet = if uppercase { b"0123456789ABCDEF" } else { b"0123456789abcdef" };
    let mut digits = Vec::new();
    loop {
        digits.push(alphabet[(value % radix) as usize]);
        value /= radix;
        if value == 0 {
            break;
        }
    }
    digits.reverse();
    digits
}

fn format_fes_string(format: &[u8], arguments: &[i32]) -> Result<Vec<u8>> {
    let format = string_prefix(format);
    let mut output = Vec::with_capacity(format.len().min(MAX_FES_FORMAT_BYTES - 1));
    let mut cursor = 0;
    let mut argument = 0;
    while cursor < format.len() {
        if format[cursor] != b'%' {
            output.push(format[cursor]);
            cursor += 1;
            if output.len() >= MAX_FES_FORMAT_BYTES {
                return Err(scene_error(cursor, "strvar result exceeds the size limit"));
            }
            continue;
        }
        cursor += 1;
        if cursor == format.len() {
            return Err(scene_error(
                cursor - 1,
                "strvar format has a trailing percent",
            ));
        }
        if format[cursor] == b'%' {
            output.push(b'%');
            cursor += 1;
            if output.len() >= MAX_FES_FORMAT_BYTES {
                return Err(scene_error(cursor, "strvar result exceeds the size limit"));
            }
            continue;
        }
        let mut left = false;
        let mut zero = false;
        let mut alternate = false;
        let mut plus = false;
        let mut space = false;
        loop {
            match format.get(cursor).copied() {
                Some(b'-') => left = true,
                Some(b'0') => zero = true,
                Some(b'#') => alternate = true,
                Some(b'+') => plus = true,
                Some(b' ') => space = true,
                _ => break,
            }
            cursor += 1;
        }
        let width = if format.get(cursor) == Some(&b'*') {
            cursor += 1;
            let value = fes_format_argument(arguments, &mut argument, cursor)? as i32;
            left |= value < 0;
            value.unsigned_abs() as usize
        } else {
            fes_format_number(format, &mut cursor)?
        };
        if width >= MAX_FES_FORMAT_BYTES {
            return Err(scene_error(cursor, "strvar format width exceeds the size limit"));
        }
        let precision = if format.get(cursor) == Some(&b'.') {
            cursor += 1;
            if format.get(cursor) == Some(&b'*') {
                cursor += 1;
                let value = fes_format_argument(arguments, &mut argument, cursor)? as i32;
                if value >= MAX_FES_FORMAT_BYTES as i32 {
                    return Err(scene_error(cursor, "strvar format precision exceeds the size limit"));
                }
                (value >= 0).then_some(value as usize)
            } else {
                Some(fes_format_number(format, &mut cursor)?)
            }
        } else {
            None
        };
        let remaining = &format[cursor..];
        let (integer_bits, modifier_length) = if remaining.starts_with(b"hh") {
            (16, 2)
        } else if remaining.starts_with(b"h") {
            (16, 1)
        } else if remaining.starts_with(b"ll") {
            (64, 2)
        } else if remaining.starts_with(b"l") {
            (32, 1)
        } else if remaining.starts_with(b"I32") {
            (32, 3)
        } else if remaining.starts_with(b"I64") {
            (64, 3)
        } else {
            (32, 0)
        };
        cursor += modifier_length;
        let conversion = *format
            .get(cursor)
            .ok_or_else(|| scene_error(cursor, "strvar format conversion is missing"))?;
        cursor += 1;
        if !matches!(conversion, b'd' | b'i' | b'u' | b'o' | b'x' | b'X' | b'c')
            || (conversion == b'c' && modifier_length != 0)
        {
            return Err(scene_error(cursor - 1, "strvar format conversion is unsupported"));
        }
        let low = fes_format_argument(arguments, &mut argument, cursor)?;
        let value = match integer_bits {
            16 => u64::from(low as u16),
            64 => u64::from(low)
                | (u64::from(fes_format_argument(arguments, &mut argument, cursor)?) << 32),
            _ => u64::from(low),
        };
        let mut prefix = Vec::new();
        let mut digits;
        if conversion == b'c' {
            digits = vec![low as u8];
        } else {
            let signed = matches!(conversion, b'd' | b'i');
            let integer = match integer_bits {
                16 => value as i16 as i64,
                64 => value as i64,
                _ => value as i32 as i64,
            };
            let magnitude = if signed { integer.unsigned_abs() } else { value };
            let radix = match conversion {
                b'o' => 8,
                b'x' | b'X' => 16,
                _ => 10,
            };
            digits = fes_format_digits(magnitude, radix, conversion == b'X');
            if magnitude == 0 && precision == Some(0) {
                digits.clear();
            }
            if signed {
                if integer < 0 {
                    prefix.push(b'-');
                } else if plus {
                    prefix.push(b'+');
                } else if space {
                    prefix.push(b' ');
                }
            } else if alternate && radix == 16 && value != 0 {
                prefix.extend_from_slice(if conversion == b'X' { b"0X" } else { b"0x" });
            }
        }
        let mut minimum_digits = if conversion == b'c' { 0 } else { precision.unwrap_or(0) };
        // The baseline emits no octal digit for a zero value with precision
        // zero, even with '#'. Otherwise the octal prefix counts as a digit.
        if conversion == b'o' && alternate && digits.first().is_some_and(|&byte| byte != b'0') {
            minimum_digits = minimum_digits.max(digits.len() + 1);
        }
        let digit_padding = minimum_digits.saturating_sub(digits.len());
        let content_length = prefix.len() + digit_padding + digits.len();
        let field_length = width.max(content_length);
        if output.len().checked_add(field_length)
            .is_none_or(|length| length >= MAX_FES_FORMAT_BYTES)
        {
            return Err(scene_error(cursor, "strvar result exceeds the size limit"));
        }
        let padding = width.saturating_sub(content_length);
        let zero_padding = zero && !left && (precision.is_none() || conversion == b'c');
        if !left && !zero_padding {
            output.resize(output.len() + padding, b' ');
        }
        output.extend_from_slice(&prefix);
        output.resize(
            output.len() + digit_padding + if zero_padding { padding } else { 0 }, b'0',
        );
        output.extend_from_slice(&digits);
        if left {
            output.resize(output.len() + padding, b' ');
        }
    }
    Ok(output)
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub enum FesEvent {
    Declarations(Vec<FesSection>),
    KeyBlocks(FesKeyBlockTable),
    CreateObject(FesObjectDefinition),
    InitializeObject(FesObjectInitialization),
    KeyBlockBinding(FesKeyBlockBinding),
    Subcommands(FesSubcommandTable),
    Command(FesLine),
    Wait(i32),
    WaitControl {
        kind: Vec<u8>,
        arguments: Vec<Vec<u8>>,
    },
    WaitFrame,
    Exit(i32),
    Frame,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FesStep {
    Yield,
    Event,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
struct FesCallFrame {
    block: usize,
    pc: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
struct FesInitializationCursor {
    keyblock: usize,
    definition: usize,
    initializer: usize,
    key_binding: usize,
    subcommands_pending: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesInterpreter {
    pub program: FesProgram,
    block: usize,
    pc: usize,
    calls: Vec<FesCallFrame>,
    local: Vec<i32>,
    pub named: BTreeMap<String, BTreeMap<i32, i32>>,
    pending: Option<FesEvent>,
    command_parameters: Option<Vec<Vec<u8>>>,
    declarations_pending: bool,
    initialization: FesInitializationPlan,
    initialization_cursor: FesInitializationCursor,
    pub return_value: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesExecutionContext {
    pub block: Vec<u8>,
    pub position: usize,
    pub local: Vec<i32>,
    pub current: i32,
}

/// Suspended FES control position. Event dispatch shares locals and the call
/// stack with the main interpreter; this saves neither of those mutable areas.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(crate = "serde")]
pub struct FesControlPosition {
    block: usize,
    position: usize,
    pending: Option<FesEvent>,
    command_parameters: Option<Vec<Vec<u8>>>,
}

impl FesInterpreter {
    pub fn new(mut program: FesProgram) -> Result<Self> {
        let initialization = program.initialization_plan()?;
        let declarations_pending = program
            .declarations
            .iter()
            .any(|section| matches!(section_kind(&section.header), b"INCLUDE" | b"PRELOAD"));
        let subcommands_pending = initialization.subcommands.declared;
        let block = if let Some(index) = program
            .blocks
            .iter()
            .position(|block| block.name == b"START")
        {
            index
        } else {
            program.blocks.push(FesBlock {
                name: b"START".to_vec(),
                lines: Vec::new(),
                false_jump: Vec::new(),
                end_jump: Vec::new(),
            });
            program.blocks.len() - 1
        };
        Ok(Self {
            declarations_pending,
            program,
            block,
            pc: 0,
            calls: Vec::new(),
            local: vec![0; 512],
            named: BTreeMap::new(),
            pending: None,
            command_parameters: None,
            return_value: 0,
            initialization,
            initialization_cursor: FesInitializationCursor {
                subcommands_pending,
                ..Default::default()
            },
        })
    }

    pub fn current_block(&self) -> &[u8] {
        self.program
            .blocks
            .get(self.block)
            .map_or(&[], |block| block.name.as_slice())
    }

    pub fn position(&self) -> (usize, usize) {
        (self.block, self.pc)
    }

    pub fn call_depth(&self) -> usize {
        self.calls.len()
    }

    pub fn at_block_end(&self) -> bool {
        self.program.blocks.get(self.block)
            .is_some_and(|block| self.pc >= block.lines.len())
    }

    pub fn return_call_at_block_end(&mut self, depth_floor: usize) -> bool {
        if self.pending.is_some() || self.calls.len() <= depth_floor || !self.at_block_end() {
            return false;
        }
        let frame = self.calls.pop().expect("call depth exceeds the floor");
        self.block = frame.block;
        self.pc = frame.pc;
        true
    }

    pub fn initialization_pending(&self) -> bool {
        self.declarations_pending
            || self.initialization_cursor.keyblock < self.initialization.keyblocks.len()
            || self.initialization_cursor.definition < self.initialization.definitions.len()
            || self.initialization_cursor.initializer < self.initialization.initializers.len()
            || self.initialization_cursor.key_binding < self.initialization.key_bindings.len()
            || self.initialization_cursor.subcommands_pending
    }

    pub fn control_position(&self) -> FesControlPosition {
        FesControlPosition {
            block: self.block,
            position: self.pc,
            pending: self.pending.clone(),
            command_parameters: self.command_parameters.clone(),
        }
    }

    /// Select exactly one source statement for a host dispatcher. The
    /// external cursor is one-based; this does not execute prior statements,
    /// change locals/calls, or treat an absent/empty block as completion.
    pub fn enter_statement(&mut self, name: &[u8], statement: u32) -> bool {
        let Some(position) = statement.checked_sub(1).map(|value| value as usize) else {
            return false;
        };
        let Some(block) = self.program.blocks.iter().position(|block| block.name == name) else {
            return false;
        };
        if position >= self.program.blocks[block].lines.len() { return false; }
        self.block = block;
        self.pc = position;
        self.resume();
        true
    }

    /// Enters a handler without changing the shared locals/call stack. A
    /// missing handler leaves the current position untouched.
    pub fn enter_block(&mut self, name: &[u8]) -> bool {
        let Some(block) = self.program.blocks.iter().position(|block| block.name == name) else {
            return false;
        };
        self.block = block;
        self.pc = 0;
        self.resume();
        true
    }

    /// Restores only control position. Calls consumed/cleared by an event
    /// remain consumed/cleared, matching FES event `next`/default-call behavior.
    pub fn restore_control_position(&mut self, saved: FesControlPosition) -> Result<()> {
        let block = self.program.blocks.get(saved.block)
            .ok_or_else(|| scene_error(saved.position, "FES saved block is invalid"))?;
        if saved.position > block.lines.len() {
            return Err(scene_error(saved.position, "FES saved position is invalid"));
        }
        self.block = saved.block;
        self.pc = saved.position;
        self.pending = saved.pending;
        self.command_parameters = saved.command_parameters;
        Ok(())
    }

    pub fn context(&self, state: &ExpressionState) -> FesExecutionContext {
        FesExecutionContext {
            block: self.current_block().to_vec(),
            position: self.pc,
            local: self.local.clone(),
            current: state.current,
        }
    }

    pub fn pending(&self) -> Option<&FesEvent> {
        self.pending.as_ref()
    }

    /// Structured arguments for the current Command event. Hosts should use
    /// these instead of reparsing the expanded display/debug source line.
    pub fn pending_command_parameters(&self) -> Option<&[Vec<u8>]> {
        self.command_parameters.as_deref()
    }

    pub fn local_values(&self) -> &[i32] {
        &self.local
    }

    pub fn local_values_mut(&mut self) -> &mut [i32] {
        &mut self.local
    }

    /// Evaluates a host-command parameter in this FES's local/named scope.
    /// Empty parameters are zero; this does not enable KCS '%' stripping.
    pub fn evaluate_parameter(&mut self, source: &[u8], state: &mut ExpressionState) -> Result<i32> {
        self.eval(source, state)
    }

    pub fn resume(&mut self) {
        self.pending = None;
        self.command_parameters = None;
    }

    pub fn run(&mut self, state: &mut ExpressionState, budget: usize) -> Result<FesStep> {
        if self.pending.is_some() {
            return Ok(FesStep::Event);
        }
        for _ in 0..budget.max(1) {
            if self.declarations_pending {
                self.declarations_pending = false;
                let event = FesEvent::Declarations(
                    self.program
                        .declarations
                        .iter()
                        .filter(|section| {
                            matches!(section_kind(&section.header), b"INCLUDE" | b"PRELOAD")
                        })
                        .cloned()
                        .collect(),
                );
                self.pending = Some(event);
                return Ok(FesStep::Event);
            }
            if let Some(event) = self.next_initialization_event(state)? {
                self.pending = Some(event);
                return Ok(FesStep::Event);
            }
            let Some(block) = self.program.blocks.get(self.block) else {
                return Err(scene_error(0, "FES current block is invalid"));
            };
            let Some(line) = block.lines.get(self.pc).cloned() else {
                if let Some(frame) = self.calls.pop() {
                    self.block = frame.block;
                    self.pc = frame.pc;
                    continue;
                }
                self.pc = 0;
                let event = FesEvent::Frame;
                self.pending = Some(event);
                return Ok(FesStep::Event);
            };
            let line_index = self.pc;
            self.pc += 1;
            if let Some(error) = self
                .program
                .diagnostics
                .iter()
                .find(|error| matches!(error, ScriptError::InvalidSceneControl { offset, .. } if *offset == line.offset))
            {
                return Err(error.clone());
            }
            let expanded_strings = expand_strings(&line.bytes, &state.strings);
            let macro_expanded = expand_fes_macros(&expanded_strings, &self.program.macros)?;
            let mut bytes = macro_expanded.as_slice();
            if let Some((condition, tail)) = split_fes_condition(bytes)? {
                let condition = expand_strings(condition, &state.strings);
                let yes = self.eval(&condition, state)? != 0;
                if tail.is_empty() {
                    if !yes {
                        self.pc = self.program.blocks[self.block].false_jump[line_index]
                            .ok_or_else(|| {
                                scene_error(line.offset, "FES if structure is incomplete")
                            })?;
                    }
                    continue;
                }
                if !yes {
                    continue;
                }
                bytes = tail;
            }
            if bytes == b"else" {
                self.pc = self.program.blocks[self.block].end_jump[line_index]
                    .ok_or_else(|| scene_error(line.offset, "FES else structure is incomplete"))?;
                continue;
            }
            if bytes == b"endif" {
                continue;
            }
            if (bytes.first() == Some(&b'$') || bytes.first() == Some(&b'\\'))
                && !bytes.starts_with(b"$str")
            {
                self.eval_fallback(&expand_strings(bytes, &state.strings), state)?;
                continue;
            }
            let arguments = fes_parameters(bytes)?.into_iter()
                .map(|word| expand_strings(&word, &state.strings)).collect::<Vec<_>>();
            let Some(command) = arguments.first().map(Vec::as_slice) else {
                continue;
            };
            match command {
                b"next" | b"call" => {
                    if arguments.len() < 2 {
                        return Err(scene_error(line.offset, "FES transfer requires one target"));
                    }
                    let target = &arguments[1];
                    let target_block = self
                        .program
                        .blocks
                        .iter()
                        .position(|block| block.name == *target);
                    if command == b"call" {
                        if self.calls.len() >= MAX_FES_CALL_DEPTH {
                            return Err(scene_error(line.offset, "FES call stack is too deep"));
                        }
                        self.calls.push(FesCallFrame {
                            block: self.block,
                            pc: self.pc,
                        });
                    }
                    let Some(target_block) = target_block else {
                        if command == b"call" {
                            if let Some(frame) = self.calls.pop() {
                                self.block = frame.block;
                                self.pc = frame.pc;
                                continue;
                            }
                            self.pc = self.program.blocks[self.block].lines.len();
                            continue;
                        }
                        return Err(scene_error(line.offset, "FES target block does not exist"));
                    };
                    if command == b"next" {
                        self.calls.clear();
                    }
                    self.block = target_block;
                    self.pc = 0;
                }
                b"break" => {
                    self.return_value = if arguments.len() == 1 {
                        0
                    } else {
                        self.eval(&arguments[1], state)?
                    };
                    self.pc = self.program.blocks[self.block].lines.len();
                }
                b"exit" => {
                    let code = if arguments.len() == 1 {
                        0
                    } else {
                        self.eval(&arguments[1], state)?
                    };
                    let event = FesEvent::Exit(code);
                    self.pending = Some(event);
                    return Ok(FesStep::Event);
                }
                b"wait" => {
                    let event = match arguments.get(1).map(Vec::as_slice) {
                        None => FesEvent::WaitFrame,
                        Some(argument) if is_numeric_fes_argument(argument) => {
                            FesEvent::Wait(self.eval(argument, state)?.max(1))
                        }
                        Some(kind) if kind == b"frame" => {
                            let value = arguments
                                .get(2)
                                .map(|value| self.eval(value, state))
                                .transpose()?
                                .unwrap_or(0);
                            FesEvent::WaitControl {
                                kind: kind.to_vec(),
                                arguments: vec![value.to_string().into_bytes()],
                            }
                        }
                        Some(kind) if kind == b"privateflag" => {
                            let value = arguments
                                .get(2)
                                .map(|value| self.eval(value, state))
                                .transpose()?
                                .unwrap_or(0);
                            FesEvent::WaitControl {
                                kind: kind.to_vec(),
                                arguments: vec![value.to_string().into_bytes()],
                            }
                        }
                        Some(kind) => FesEvent::WaitControl {
                            kind: kind.to_vec(),
                            arguments: arguments[2..].to_vec(),
                        },
                    };
                    self.pending = Some(event);
                    return Ok(FesStep::Event);
                }
                b"strvar" => {
                    self.execute_strvar(&arguments, state)?;
                    continue;
                }
                b"strcmp" => {
                    let Some(destination) = arguments.get(1) else {
                        continue;
                    };
                    let destination = self.eval_declaration_component(
                        &Some(destination.clone()), 0, state,
                    )?;
                    let left = string_prefix(arguments.get(2).map_or(&[], Vec::as_slice));
                    let right = string_prefix(arguments.get(3).map_or(&[], Vec::as_slice));
                    let result = i32::from(left > right) - i32::from(left < right);
                    if let Some(slot) = state.shared.get_mut(destination as usize) {
                        *slot = result;
                    }
                    continue;
                }
                b"clearstrvar" => {
                    if arguments.len() < 2 {
                        continue;
                    }
                    let first = self.eval_declaration_component(
                        &Some(arguments[1].clone()), 0, state,
                    )?;
                    let last = arguments
                        .get(2)
                        .map(|value| self.eval_declaration_component(&Some(value.clone()), 0, state))
                        .transpose()?
                        .unwrap_or(first);
                    if last == i32::MAX {
                        return Err(scene_error(line.offset, "clearstrvar range overflows"));
                    }
                    if first <= last {
                        state.strings.retain(|key, _| {
                            let key = *key as i32;
                            key < first || key > last
                        });
                    }
                    continue;
                }
                _ => {
                    let event = FesEvent::Command(FesLine {
                        offset: line.offset,
                        bytes: expand_strings(bytes, &state.strings),
                    });
                    self.command_parameters = Some(arguments);
                    self.pending = Some(event);
                    return Ok(FesStep::Event);
                }
            }
        }
        Ok(FesStep::Yield)
    }

    fn next_initialization_event(
        &mut self,
        state: &mut ExpressionState,
    ) -> Result<Option<FesEvent>> {
        if let Some(table) = self
            .initialization
            .keyblocks
            .get(self.initialization_cursor.keyblock)
            .cloned()
        {
            let mut blocks = Vec::with_capacity(table.blocks.len());
            for block in table.blocks {
                let size = [
                    self.eval_declaration_component(&block.size[0], 0, state)?,
                    self.eval_declaration_component(&block.size[1], 0, state)?,
                ];
                let neighbors = [
                    self.eval_declaration_component(&block.neighbors[0], -1, state)?,
                    self.eval_declaration_component(&block.neighbors[1], -1, state)?,
                    self.eval_declaration_component(&block.neighbors[2], -1, state)?,
                    self.eval_declaration_component(&block.neighbors[3], -1, state)?,
                ];
                blocks.push(FesKeyBlock {
                    index: block.index,
                    size,
                    neighbors,
                });
            }
            self.initialization_cursor.keyblock += 1;
            return Ok(Some(FesEvent::KeyBlocks(FesKeyBlockTable {
                offset: table.offset,
                declared_count: table.declared_count,
                blocks,
            })));
        }
        if let Some(mut definition) = self
            .initialization
            .definitions
            .get(self.initialization_cursor.definition)
            .cloned()
        {
            if let Some(expression) = definition.bank_expression.as_deref() {
                let requested = self.eval(expression, state)?;
                if requested < -1 {
                    return Err(scene_error(
                        definition.offset,
                        "FES object bank count is negative",
                    ));
                }
                definition.banks = if requested == 0 { 1 } else { requested };
            }
            if definition.banks > MAX_FES_BLOCKS as i32 {
                return Err(scene_error(
                    definition.offset,
                    "FES object bank count exceeds limit",
                ));
            }
            self.initialization_cursor.definition += 1;
            return Ok(Some(FesEvent::CreateObject(definition)));
        }
        if let Some(mut initializer) = self
            .initialization
            .initializers
            .get(self.initialization_cursor.initializer)
            .cloned()
        {
            let expanded = self.program.expand_macros(&initializer.target)?;
            let (name, bank) = split_object_target(&expanded, initializer.offset)?;
            initializer.target = name;
            if let Some(bank) = bank {
                let bank = self.eval(&bank, state)?;
                initializer.target.extend_from_slice(format!("[{bank}]").as_bytes());
            }
            for (column, value) in initializer.columns.iter().zip(&mut initializer.values) {
                if matches!(column.as_slice(), b"FILE" | b"POS2" | b"SIZE2") {
                    *value = expand_strings(value, &state.strings);
                }
            }
            self.initialization_cursor.initializer += 1;
            return Ok(Some(FesEvent::InitializeObject(initializer)));
        }
        if let Some(binding) = self
            .initialization
            .key_bindings
            .get(self.initialization_cursor.key_binding)
            .cloned()
        {
            let block = self.eval(&binding.block, state)?;
            let position = [
                self.eval_declaration_component(&binding.position[0], 0, state)?,
                self.eval_declaration_component(&binding.position[1], 0, state)?,
            ];
            let expanded = self.program.expand_macros(&binding.target)?;
            let (mut target, bank) = split_object_target(&expanded, binding.offset)?;
            if let Some(bank) = bank {
                let bank = self.eval(&bank, state)?;
                target.extend_from_slice(format!("[{bank}]").as_bytes());
            }
            self.initialization_cursor.key_binding += 1;
            return Ok(Some(FesEvent::KeyBlockBinding(FesKeyBlockBinding {
                offset: binding.offset,
                target,
                block,
                position,
            })));
        }
        if self.initialization_cursor.subcommands_pending {
            self.initialization_cursor.subcommands_pending = false;
            return Ok(Some(FesEvent::Subcommands(
                self.initialization.subcommands.clone(),
            )));
        }
        Ok(None)
    }

    fn eval_declaration_component(
        &mut self,
        value: &Option<Vec<u8>>,
        default: i32,
        state: &mut ExpressionState,
    ) -> Result<i32> {
        match value {
            None => Ok(default),
            Some(value) if value.is_empty() => Ok(0),
            Some(value) => self.eval(value, state),
        }
    }

    fn execute_strvar(&mut self, arguments: &[Vec<u8>], state: &mut ExpressionState) -> Result<()> {
        if arguments.len() < 2 {
            return Ok(());
        }
        if arguments.get(1).is_some_and(|word| word.eq_ignore_ascii_case(b"get")) {
            if arguments.len() < 3 {
                return Ok(());
            }
            let destination = self.eval(&arguments[2], state)? as u32;
            let Some(source) = arguments.get(3) else {
                return Ok(());
            };
            let source = self.eval(source, state)? as u32;
            let value = state.strings.get(&source)
                .map(|value| string_prefix(value).to_vec()).unwrap_or_default();
            state.strings.insert(destination, value);
            return Ok(());
        }
        let destination = self.eval(&arguments[1], state)? as u32;
        let value = arguments.get(2).cloned().unwrap_or_default();
        let formatted = if arguments.len() > 3 {
            let mut values = Vec::with_capacity(4);
            let end = 3 + (arguments.len() - 3).min(4);
            for argument in &arguments[3..end] {
                values.push(self.eval(argument, state)?);
            }
            format_fes_string(&value, &values)?
        } else {
            value
        };
        state
            .strings
            .insert(destination, string_prefix(&formatted).to_vec());
        Ok(())
    }

    fn eval(&mut self, source: &[u8], state: &mut ExpressionState) -> Result<i32> {
        if source.is_empty() {
            return Ok(0);
        }
        let current = state.current;
        std::mem::swap(&mut self.local, &mut state.local);
        std::mem::swap(&mut self.named, &mut state.named);
        let result = eval_expression(source, state);
        std::mem::swap(&mut self.named, &mut state.named);
        std::mem::swap(&mut self.local, &mut state.local);
        state.current = current;
        result.map_err(|error| self.expression_error_context(error))
    }

    pub fn eval_fallback(&mut self, source: &[u8], state: &mut ExpressionState) -> Result<i32> {
        let current = state.current;
        std::mem::swap(&mut self.local, &mut state.local);
        std::mem::swap(&mut self.named, &mut state.named);
        let result = eval_expression_fallback_with_text(source, state);
        std::mem::swap(&mut self.named, &mut state.named);
        std::mem::swap(&mut self.local, &mut state.local);
        state.current = current;
        result.map_err(|error| self.expression_error_context(error))
    }

    fn expression_error_context(&self, error: ScriptError) -> ScriptError {
        let Some(block) = self.program.blocks.get(self.block) else { return error; };
        let Some(line) = self.pc.checked_sub(1).and_then(|pc| block.lines.get(pc)) else {
            return error;
        };
        match error {
            ScriptError::InvalidExpression { offset, message } => ScriptError::InvalidExpression {
                offset,
                message: format!("{message}; FES offset={:#x} block={}",
                    line.offset, String::from_utf8_lossy(&block.name)),
            },
            error => error,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CstChoice {
    pub id: i32,
    pub target: Vec<u8>,
    pub text: Vec<u8>,
    pub source_text: Vec<u8>,
    pub text_encoding: CstTextEncoding,
    pub index: usize,
}

impl CstChoice {
    pub fn text_tokens(&self) -> Result<Vec<CstTextToken>> {
        parse_cst_text_with_encoding(&self.text, self.text_encoding)
    }

    pub fn text_tokens_with_encoding(
        &self,
        encoding: CstTextEncoding,
    ) -> Result<Vec<CstTextToken>> {
        parse_cst_text_with_encoding(&self.text, encoding)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CstSceneTransfer {
    pub call: bool,
    pub target: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CstEvent {
    Speaker(Vec<u8>),
    Message(Vec<u8>),
    LocalizedSpeaker {
        text: Vec<u8>,
        language: String,
    },
    LocalizedMessage {
        text: Vec<u8>,
        language: String,
    },
    WaitControl {
        kind: Vec<u8>,
        arguments: Vec<Vec<u8>>,
    },
    Input {
        kind: u8,
    },
    Command(Vec<u8>),
    Choices(Vec<CstChoice>),
    Transfer(CstSceneTransfer),
    Returned,
    Debug {
        kind: u8,
        text: Vec<u8>,
    },
    Record {
        kind: u8,
        text: Vec<u8>,
    },
    Wait(i32),
    Finished,
}

impl CstEvent {
    pub fn message_tokens(&self) -> Result<Option<Vec<CstTextToken>>> {
        match self {
            Self::Message(text) | Self::Speaker(text) => Ok(Some(parse_cst_text(text)?)),
            Self::LocalizedMessage { text, .. } | Self::LocalizedSpeaker { text, .. } => Ok(Some(
                parse_cst_text_with_encoding(text, CstTextEncoding::Unicode)?,
            )),
            _ => Ok(None),
        }
    }

    pub fn message_tokens_with_encoding(
        &self,
        encoding: CstTextEncoding,
    ) -> Result<Option<Vec<CstTextToken>>> {
        match self {
            Self::Message(text) | Self::Speaker(text) => {
                Ok(Some(parse_cst_text_with_encoding(text, encoding)?))
            }
            Self::LocalizedMessage { text, .. } | Self::LocalizedSpeaker { text, .. } => Ok(Some(
                parse_cst_text_with_encoding(text, CstTextEncoding::Unicode)?,
            )),
            _ => Ok(None),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CstTextToken {
    Text(Vec<u8>),
    Newline,
    Continue,
    Character(u16),
    WaitCharacters(u32),
    WaitFrames(u32),
    WaitReset,
    Align(Vec<u8>),
    Position { axis: u8, value: u32 },
    Unicode(u16),
    Style { bold: bool, italic: bool },
    Identifier { key: i32, value: u32 },
    Ruby { base: Vec<u8>, ruby: Vec<u8> },
    NoBreak(Vec<u8>),
    Control { name: Vec<u8>, value: Vec<u8> },
}

/// Parse the message control layer without decoding the surrounding script
/// bytes. This keeps CP932, GBK, UTF-8, and mixed legacy payloads lossless.
pub fn parse_cst_text(input: &[u8]) -> Result<Vec<CstTextToken>> {
    parse_cst_text_with_encoding(input, CstTextEncoding::Cp932)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CstTextEncoding {
    Cp932,
    Unicode,
}

pub fn parse_cst_text_with_encoding(
    input: &[u8],
    encoding: CstTextEncoding,
) -> Result<Vec<CstTextToken>> {
    let mut tokens = Vec::new();
    let mut text = Vec::new();
    let mut flush = |tokens: &mut Vec<CstTextToken>, text: &mut Vec<u8>| {
        if !text.is_empty() {
            tokens.push(CstTextToken::Text(std::mem::take(text)));
        }
    };
    let mut cursor = 0;
    while cursor < input.len() {
        if input[cursor] == b'\\' {
            if let Some(next) = input.get(cursor + 1).copied() {
                match next {
                    b'n' => {
                        flush(&mut tokens, &mut text);
                        tokens.push(CstTextToken::Newline);
                        cursor += 2;
                        continue;
                    }
                    b'@' => {
                        flush(&mut tokens, &mut text);
                        tokens.push(CstTextToken::Continue);
                        cursor += 2;
                        continue;
                    }
                    b'\\' | b'[' => {
                        text.push(next);
                        cursor += 2;
                        continue;
                    }
                    b']' | b'/' => {
                        text.push(next);
                        cursor += 2;
                        continue;
                    }
                    b'$' => {
                        let start = cursor + 2;
                        let end = input[start..]
                            .iter()
                            .position(|&byte| byte == b';')
                            .map_or(input.len(), |offset| start + offset);
                        let value = parse_unsigned_prefix(&input[start..end]);
                        flush(&mut tokens, &mut text);
                        if end > start {
                            tokens.push(CstTextToken::Unicode(value as u16));
                        }
                        cursor = if end < input.len() { end + 1 } else { end };
                        continue;
                    }
                    _ => {}
                }
            }
            let start = cursor + 1;
            if input.get(start) == Some(&b't') {
                let mut style_cursor = start + 1;
                let mut bold = false;
                let mut italic = false;
                while let Some(byte) = input.get(style_cursor).copied() {
                    match byte.to_ascii_lowercase() {
                        b'b' => bold = true,
                        b'i' => italic = true,
                        _ => break,
                    }
                    style_cursor += 1;
                }
                if style_cursor > start + 1 {
                    flush(&mut tokens, &mut text);
                    tokens.push(CstTextToken::Style { bold, italic });
                    cursor = style_cursor + usize::from(input.get(style_cursor) == Some(&b';'));
                    continue;
                }
            }
            let mut name_end = start;
            while input
                .get(name_end)
                .is_some_and(|byte| byte.is_ascii_alphabetic())
            {
                name_end += 1;
            }
            if name_end > start {
                let name = input[start..name_end].to_ascii_lowercase();
                if name.as_slice() == b"p" {
                    flush(&mut tokens, &mut text);
                    tokens.push(CstTextToken::Align(b"l".to_vec()));
                    cursor = name_end;
                    continue;
                }
                if name.as_slice() == b"pl" || name.as_slice() == b"pc" || name.as_slice() == b"pr"
                {
                    flush(&mut tokens, &mut text);
                    tokens.push(CstTextToken::Align(name[1..].to_vec()));
                    cursor = name_end;
                    continue;
                }
                if name.as_slice() == b"wr" {
                    flush(&mut tokens, &mut text);
                    tokens.push(CstTextToken::WaitReset);
                    cursor = name_end;
                    continue;
                }
                if name.as_slice() == b"w" || name.as_slice() == b"wf" {
                    let value_start = name_end;
                    let value_end = consume_unsigned(input, value_start);
                    flush(&mut tokens, &mut text);
                    let value = parse_unsigned_prefix(&input[value_start..value_end]);
                    if name.as_slice() == b"wf" {
                        tokens.push(CstTextToken::WaitFrames(value));
                    } else {
                        tokens.push(CstTextToken::WaitCharacters(value));
                    }
                    cursor = value_end + usize::from(input.get(value_end) == Some(&b';'));
                    continue;
                }
                if name.as_slice() == b"x" || name.as_slice() == b"y" {
                    let value_start = name_end;
                    let value_end = consume_unsigned(input, value_start);
                    if value_end > value_start {
                        flush(&mut tokens, &mut text);
                        tokens.push(CstTextToken::Position {
                            axis: name[0],
                            value: parse_unsigned_prefix(&input[value_start..value_end]),
                        });
                        cursor = value_end + usize::from(input.get(value_end) == Some(&b';'));
                        continue;
                    }
                }
                if name.as_slice() == b"t" {
                    let mut style_cursor = name_end;
                    let mut bold = false;
                    let mut italic = false;
                    while let Some(byte) = input.get(style_cursor).copied() {
                        match byte.to_ascii_lowercase() {
                            b'b' => bold = true,
                            b'i' => italic = true,
                            _ => break,
                        }
                        style_cursor += 1;
                    }
                    if style_cursor > name_end {
                        flush(&mut tokens, &mut text);
                        tokens.push(CstTextToken::Style { bold, italic });
                        cursor = style_cursor + usize::from(input.get(style_cursor) == Some(&b';'));
                        continue;
                    }
                }
                if name.as_slice() == b"id" {
                    let key_start = name_end;
                    let key_end = consume_unsigned(input, key_start);
                    if key_end > key_start {
                        let key = parse_unsigned_prefix(&input[key_start..key_end]) as i32;
                        let mut value = 0;
                        let mut id_cursor =
                            key_end + usize::from(input.get(key_end) == Some(&b';'));
                        if input.get(id_cursor) == Some(&b':') {
                            id_cursor += 1;
                            let value_start = id_cursor;
                            let value_end = consume_unsigned(input, value_start);
                            value = parse_unsigned_prefix(&input[value_start..value_end]);
                            id_cursor =
                                value_end + usize::from(input.get(value_end) == Some(&b';'));
                        }
                        flush(&mut tokens, &mut text);
                        tokens.push(CstTextToken::Identifier { key, value });
                        cursor = id_cursor;
                        continue;
                    }
                }
                if name.as_slice() == b"s" {
                    flush(&mut tokens, &mut text);
                    tokens.push(CstTextToken::Character(match encoding {
                        CstTextEncoding::Cp932 => 0x3000,
                        CstTextEncoding::Unicode => 0x8140,
                    }));
                    tokens.push(CstTextToken::Newline);
                    cursor = name_end;
                    continue;
                }
                if name.as_slice() == b"u" && encoding == CstTextEncoding::Unicode {
                    flush(&mut tokens, &mut text);
                    tokens.push(CstTextToken::Control {
                        name,
                        value: Vec::new(),
                    });
                    cursor = name_end;
                    continue;
                }
                if name.as_slice() == b"r" || name.as_slice() == b"i" {
                    flush(&mut tokens, &mut text);
                    tokens.push(CstTextToken::Control {
                        name,
                        value: Vec::new(),
                    });
                    cursor = name_end;
                    continue;
                }
                if let Some(end) = input[name_end..].iter().position(|&byte| byte == b';') {
                    flush(&mut tokens, &mut text);
                    let end = name_end + end;
                    tokens.push(CstTextToken::Control {
                        name,
                        value: input[name_end..end].to_vec(),
                    });
                    cursor = end + 1;
                    continue;
                }
            }
            if input[cursor] == b'\\' && input.get(cursor + 1).is_none() {
                flush(&mut tokens, &mut text);
                tokens.push(CstTextToken::Newline);
                cursor += 1;
                continue;
            }
            if input[cursor] == b'\\' && input.get(cursor + 1).is_some() {
                text.push(input[cursor + 1]);
                cursor += 2;
                continue;
            }
        } else if input[cursor] == b'[' {
            if let Some(relative_end) = input[cursor + 1..].iter().position(|&byte| byte == b']') {
                let end = cursor + 1 + relative_end;
                let body = &input[cursor + 1..end];
                flush(&mut tokens, &mut text);
                if let Some(separator) = body.iter().position(|&byte| byte == b'/') {
                    tokens.push(CstTextToken::Ruby {
                        base: body[..separator].to_vec(),
                        ruby: body[separator + 1..].to_vec(),
                    });
                } else {
                    tokens.push(CstTextToken::NoBreak(body.to_vec()));
                }
                cursor = end + 1;
                continue;
            }
        }
        let byte = input[cursor];
        if byte == b'\n' {
            flush(&mut tokens, &mut text);
            tokens.push(CstTextToken::Newline);
            cursor += 1;
        } else if is_cp932_pair(input, cursor) {
            text.extend_from_slice(&input[cursor..cursor + 2]);
            cursor += 2;
        } else {
            text.push(byte);
            cursor += 1;
        }
    }
    flush(&mut tokens, &mut text);
    Ok(tokens)
}

fn consume_unsigned(input: &[u8], start: usize) -> usize {
    let mut cursor = start;
    if input
        .get(cursor..)
        .is_some_and(|bytes| bytes.starts_with(b"0x") || bytes.starts_with(b"0X"))
    {
        cursor += 2;
        while input.get(cursor).is_some_and(u8::is_ascii_hexdigit) {
            cursor += 1;
        }
    } else {
        while input.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
    }
    cursor
}

fn parse_unsigned_prefix(input: &[u8]) -> u32 {
    let (mut cursor, radix) = if input.starts_with(b"0x") || input.starts_with(b"0X") {
        (2, 16)
    } else {
        (0, 10)
    };
    let mut value = 0u32;
    while let Some(byte) = input.get(cursor) {
        let Some(digit) = (*byte as char).to_digit(radix) else {
            break;
        };
        value = value.wrapping_mul(radix).wrapping_add(digit);
        cursor += 1;
    }
    value
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CstStep {
    Yield,
    Waiting,
    Event,
    Finished,
}

struct CstCallFrame {
    document: CstDocument,
    scene_name: Vec<u8>,
    localization: Option<CstlLocalization>,
    input: usize,
    record: usize,
    message_fragment: usize,
}

pub struct CstInterpreter {
    pub document: CstDocument,
    pub scene_name: Vec<u8>,
    input: usize,
    record: usize,
    waiting: bool,
    advance_input_on_resume: bool,
    finished: bool,
    wait_frames: u32,
    pending: Option<CstEvent>,
    choices: Option<Vec<CstChoice>>,
    call_stack: Vec<CstCallFrame>,
    localization: Option<CstlLocalization>,
    message_fragment: usize,
}

impl CstInterpreter {
    pub fn new(document: CstDocument) -> Self {
        let record = document.inputs.first().map_or(0, |range| range.start);
        Self {
            document,
            scene_name: Vec::new(),
            input: 0,
            record,
            waiting: false,
            advance_input_on_resume: false,
            finished: false,
            wait_frames: 0,
            pending: None,
            choices: None,
            call_stack: Vec::new(),
            localization: None,
            message_fragment: 0,
        }
    }

    pub fn position(&self) -> (usize, usize) {
        (self.input, self.record)
    }

    pub fn pending(&self) -> Option<&CstEvent> {
        self.pending.as_ref()
    }

    pub fn waiting(&self) -> bool {
        self.waiting
    }

    pub fn finished(&self) -> bool {
        self.finished
    }

    pub fn set_localization(&mut self, localization: Option<CstlLocalization>) {
        self.localization = localization;
    }

    pub fn set_localization_table(
        &mut self,
        table: Option<crate::cstl::CstlTable>,
        language: Option<&str>,
        fallback: Option<&str>,
    ) {
        self.localization = table.map(|table| CstlLocalization::new(table, language, fallback));
    }

    pub fn set_language(&mut self, language: Option<&str>) {
        if let Some(localization) = &mut self.localization {
            localization.set_language(language);
        }
    }

    pub fn language(&self) -> Option<&str> {
        self.localization
            .as_ref()
            .and_then(CstlLocalization::language)
    }

    pub fn resume(&mut self) {
        self.pending = None;
        if self.waiting {
            self.waiting = false;
            self.choices = None;
            if self.advance_input_on_resume {
                self.input = self.input.saturating_add(1);
                self.message_fragment = 0;
                self.record = self
                    .document
                    .inputs
                    .get(self.input)
                    .map_or(self.document.records.len(), |range| range.start);
            }
            self.advance_input_on_resume = false;
        }
    }

    pub fn choose(&mut self, choice: usize) -> Result<()> {
        let choices = self
            .choices
            .as_ref()
            .ok_or_else(|| scene_error(self.record, "CST has no pending choices"))?;
        if choices.get(choice).is_none() {
            return Err(scene_error(
                self.record,
                "CST choice index is out of bounds",
            ));
        }
        let target = choices[choice].target.clone();
        self.choices = None;
        self.waiting = false;
        self.pending = Some(CstEvent::Transfer(CstSceneTransfer {
            call: false,
            target,
        }));
        Ok(())
    }

    pub fn load_scene(
        &mut self,
        document: CstDocument,
        name: Vec<u8>,
        transfer: CstSceneTransfer,
    ) -> Result<()> {
        if transfer.call {
            if self.call_stack.len() >= MAX_CST_CALL_DEPTH {
                return Err(scene_error(self.record, "CST call stack is too deep"));
            }
            self.call_stack.push(CstCallFrame {
                document: self.document.clone(),
                scene_name: self.scene_name.clone(),
                localization: self.localization.clone(),
                input: self.input,
                record: self.record,
                message_fragment: self.message_fragment,
            });
        } else {
            self.call_stack.clear();
        }
        self.document = document;
        self.scene_name = name;
        // A CSTL table belongs to one scene. The host may bind the new table
        // after loading; a called scene must never read its caller's table.
        self.localization = None;
        self.input = 0;
        self.record = self.document.inputs.first().map_or(0, |range| range.start);
        self.waiting = false;
        self.advance_input_on_resume = false;
        self.finished = false;
        self.wait_frames = 0;
        self.message_fragment = 0;
        self.pending = None;
        self.choices = None;
        Ok(())
    }

    pub fn run(&mut self, state: &mut ExpressionState, budget: usize) -> Result<CstStep> {
        if self.pending.is_some() {
            return Ok(CstStep::Event);
        }
        if self.finished {
            return Ok(CstStep::Finished);
        }
        if self.waiting {
            return Ok(CstStep::Waiting);
        }
        if self.wait_frames != 0 {
            self.wait_frames -= 1;
            return Ok(CstStep::Yield);
        }
        for _ in 0..budget.max(1) {
            let Some(range) = self.document.inputs.get(self.input) else {
                if self.restore_call() {
                    continue;
                }
                self.finished = true;
                self.pending = Some(CstEvent::Finished);
                return Ok(CstStep::Event);
            };
            if self.record >= range.end {
                if self.input + 1 < self.document.inputs.len() {
                    self.waiting = true;
                    self.advance_input_on_resume = true;
                    let event = CstEvent::Input { kind: 0 };
                    self.pending = Some(event);
                    return Ok(CstStep::Event);
                }
                if self.restore_call() {
                    continue;
                }
                self.finished = true;
                self.pending = Some(CstEvent::Finished);
                return Ok(CstStep::Event);
            }
            let record = self.document.records[self.record].clone();
            self.record += 1;
            match record.kind {
                0x02 | 0x03 => {
                    self.waiting = true;
                    self.advance_input_on_resume = true;
                    let event = CstEvent::Input { kind: record.kind };
                    self.pending = Some(event);
                    return Ok(CstStep::Event);
                }
                0x20 => {
                    let source = expand_strings(&record.text, &state.strings);
                    let event = self
                        .localization
                        .as_ref()
                        .and_then(|localization| {
                            Some(CstEvent::LocalizedMessage {
                                text: localization
                                    .content_fragment(self.input, self.message_fragment)?
                                    .as_bytes()
                                    .to_vec(),
                                language: localization.language()?.to_owned(),
                            })
                        })
                        .unwrap_or(CstEvent::Message(source));
                    self.message_fragment = self.message_fragment.saturating_add(1);
                    self.pending = Some(event);
                    return Ok(CstStep::Event);
                }
                0x21 => {
                    let source = expand_strings(&record.text, &state.strings);
                    let event = self
                        .localization
                        .as_ref()
                        .and_then(|localization| {
                            Some(CstEvent::LocalizedSpeaker {
                                text: localization.name(self.input)?.as_bytes().to_vec(),
                                language: localization.language()?.to_owned(),
                            })
                        })
                        .unwrap_or(CstEvent::Speaker(source));
                    self.pending = Some(event);
                    return Ok(CstStep::Event);
                }
                0x30 => {
                    if let Some(event) = self.execute_command(&record.text, state)? {
                        self.pending = Some(event);
                        return Ok(CstStep::Event);
                    }
                }
                0xf0 | 0xf1 => {
                    let event = CstEvent::Debug {
                        kind: record.kind,
                        text: record.text,
                    };
                    self.pending = Some(event);
                    return Ok(CstStep::Event);
                }
                kind => {
                    let event = CstEvent::Record {
                        kind,
                        text: record.text,
                    };
                    self.pending = Some(event);
                    return Ok(CstStep::Event);
                }
            }
        }
        Ok(CstStep::Yield)
    }

    fn execute_command(
        &mut self,
        source: &[u8],
        state: &mut ExpressionState,
    ) -> Result<Option<CstEvent>> {
        let expanded = cst_hex_arguments(&expand_strings(source, &state.strings));
        let bytes = expanded.as_slice();
        if let Some((condition, command)) = split_cst_condition(bytes)? {
            if eval_expression(condition, state)? == 0 {
                return Ok(None);
            }
            return self.execute_command(command, state);
        }
        if bytes.first() == Some(&b'#') {
            eval_expression(bytes, state)?;
            return Ok(None);
        }
        let arguments = split_cst_arguments(bytes)?;
        let Some(command) = arguments.first().map(Vec::as_slice) else {
            return Ok(None);
        };
        match command {
            b"next" | b"call" => {
                if arguments.len() != 2 {
                    return Err(scene_error(0, "CST transfer requires one target"));
                }
                Ok(Some(CstEvent::Transfer(CstSceneTransfer {
                    call: command == b"call",
                    target: arguments[1].clone(),
                })))
            }
            b"return" => {
                if self.restore_call() {
                    Ok(Some(CstEvent::Returned))
                } else {
                    self.finished = true;
                    Ok(Some(CstEvent::Finished))
                }
            }
            b"wait" => {
                if arguments.len() < 2 {
                    self.waiting = true;
                    self.advance_input_on_resume = false;
                    return Ok(Some(CstEvent::WaitControl {
                        kind: Vec::new(),
                        arguments: Vec::new(),
                    }));
                }
                if is_numeric_fes_argument(&arguments[1]) {
                    let frames = eval_expression_parameter(&arguments[1], state)?;
                    self.wait_frames = frames.max(0) as u32;
                    Ok(Some(CstEvent::Wait(frames)))
                } else if arguments[1] == b"key" {
                    self.waiting = true;
                    self.advance_input_on_resume = false;
                    Ok(Some(CstEvent::Input { kind: 0x02 }))
                } else {
                    self.waiting = true;
                    self.advance_input_on_resume = false;
                    Ok(Some(CstEvent::WaitControl {
                        kind: arguments[1].clone(),
                        arguments: arguments[2..].to_vec(),
                    }))
                }
            }
            b"str" | b"strn" => {
                if arguments.len() < 3 {
                    return Err(scene_error(0, "CST string command is missing an argument"));
                }
                let index = eval_expression_parameter(&arguments[1], state)? as u32;
                let value = if command == b"strn" {
                    eval_expression_parameter(&arguments[2], state)?
                        .to_string()
                        .into_bytes()
                } else {
                    arguments[2].clone()
                };
                state.strings.insert(index, value);
                Ok(None)
            }
            b"fselect" => {
                let choices = self.collect_choices(state)?;
                if choices.is_empty() {
                    Ok(None)
                } else {
                    self.waiting = true;
                    self.advance_input_on_resume = false;
                    Ok(Some(CstEvent::Choices(choices)))
                }
            }
            _ => Ok(Some(CstEvent::Command(bytes.to_vec()))),
        }
    }

    fn collect_choices(&mut self, state: &mut ExpressionState) -> Result<Vec<CstChoice>> {
        let mut choices = Vec::new();
        let mut cursor = self.record;
        while let Some(record) = self.document.records.get(cursor) {
            if record.kind != 0x30 {
                break;
            }
            if choices.len() >= MAX_CST_CHOICES {
                return Err(scene_error(record.kind as usize, "too many CST choices"));
            }
            let expanded = cst_hex_arguments(&expand_strings(&record.text, &state.strings));
            let (condition, command) = match split_cst_condition(&expanded)? {
                Some((condition, command)) => (Some(condition), command),
                None => (None, expanded.as_slice()),
            };
            if let Some(condition) = condition {
                if eval_expression(condition, state)? == 0 {
                    cursor += 1;
                    continue;
                }
            }
            let arguments = split_cst_arguments(command)?;
            if arguments.len() < 3 {
                break;
            }
            let id = eval_expression_parameter(&arguments[0], state)?;
            let mut text = Vec::new();
            for (index, part) in arguments[2..].iter().enumerate() {
                if index > 0 {
                    text.push(b' ');
                }
                text.extend_from_slice(part);
            }
            let localized = self.localization.as_ref().and_then(|localization| {
                localization.option(self.document.inputs.len(), choices.len())
            });
            choices.push(CstChoice {
                id,
                target: arguments[1].clone(),
                text: localized
                    .as_ref()
                    .map_or_else(|| text.clone(), |value| value.as_bytes().to_vec()),
                source_text: text,
                text_encoding: if localized.is_some() {
                    CstTextEncoding::Unicode
                } else {
                    CstTextEncoding::Cp932
                },
                index: choices.len(),
            });
            cursor += 1;
        }
        self.record = cursor;
        self.choices = Some(choices.clone());
        Ok(choices)
    }

    fn restore_call(&mut self) -> bool {
        let Some(frame) = self.call_stack.pop() else {
            return false;
        };
        self.document = frame.document;
        self.scene_name = frame.scene_name;
        self.localization = frame.localization;
        self.input = frame.input;
        self.record = frame.record;
        self.message_fragment = frame.message_fragment;
        self.waiting = false;
        self.advance_input_on_resume = false;
        self.finished = false;
        self.pending = None;
        self.wait_frames = 0;
        self.choices = None;
        true
    }
}

fn split_cst_condition(input: &[u8]) -> Result<Option<(&[u8], &[u8])>> {
    if !input.starts_with(b"if")
        || !input
            .get(2)
            .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'(')
    {
        return Ok(None);
    }
    let start = input[2..]
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map_or(input.len(), |position| position + 2);
    if input.get(start) != Some(&b'(') {
        return Err(scene_error(
            start,
            "CST if requires a parenthesized condition",
        ));
    }
    let mut depth = 0;
    for (position, byte) in input.iter().enumerate().skip(start) {
        if *byte == b'(' {
            depth += 1;
        } else if *byte == b')' {
            depth -= 1;
            if depth == 0 {
                return Ok(Some((
                    &input[start + 1..position],
                    input[position + 1..].trim_ascii_start(),
                )));
            }
        }
    }
    Err(scene_error(start, "CST if condition is not terminated"))
}

fn split_cst_arguments(input: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut words = Vec::new();
    let mut word = Vec::new();
    let mut cursor = 0;
    while cursor < input.len() {
        if input[cursor] == b' ' {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            cursor += 1;
        } else if input[cursor] == b'(' {
            let start = cursor;
            let mut depth = 0;
            while let Some(byte) = input.get(cursor) {
                if *byte == b'(' {
                    depth += 1;
                } else if *byte == b')' {
                    depth -= 1;
                    if depth == 0 {
                        cursor += 1;
                        break;
                    }
                }
                cursor += 1;
            }
            if depth != 0 {
                return Err(scene_error(
                    start,
                    "CST parameter parenthesis is not terminated",
                ));
            }
            word.extend_from_slice(&input[start..cursor]);
        } else {
            word.push(input[cursor]);
            cursor += 1;
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    Ok(words)
}

fn cst_hex_arguments(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len());
    let mut cursor = 0;
    while cursor < input.len() {
        let byte = input[cursor];
        if is_cp932_pair(input, cursor) {
            output.extend_from_slice(&input[cursor..cursor + 2]);
            cursor += 2;
        } else if byte == b'$' {
            output.extend_from_slice(b"0x");
            cursor += 1;
        } else {
            output.push(byte);
            cursor += 1;
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{
        CstDocument, CstEvent, CstInterpreter, CstTextEncoding, CstTextToken, FesEvent,
        FesInterpreter, FesProgram, parse_cst_text,
    };
    use crate::{CstRecord, CstlLocalization, CstlMessage, CstlTable, expression::ExpressionState};

    #[test]
    fn fes_integer_formatting_keeps_width_precision_and_argument_widths() {
        for (format, arguments, expected) in [
            (b"%+06d".as_slice(), vec![12], b"+00012".as_slice()),
            (b"%#08x", vec![42], b"0x00002a"),
            (b"%*.*d", vec![-6, 3, 7], b"007   "),
            (b"%hhd/%hu", vec![65535, -1], b"-1/65535"),
            (b"%I64u", vec![-1, 1], b"8589934591"),
            (b"%#o/%#.0o", vec![8, 0], b"010/"),
            (b"%03c", vec![65], b"00A"),
            (b"%d\0%p", vec![7], b"7"),
        ] {
            assert_eq!(super::format_fes_string(format, &arguments).unwrap(), expected);
        }
    }

    #[test]
    fn fes_formatting_rejects_large_fields_before_allocating() {
        for (format, arguments) in [
            (b"%*d".as_slice(), vec![i32::MIN, 1]),
            (b"%.2560d", vec![1]),
            (b"%I64d", vec![1]),
            (b"%hc", vec![65]),
            (b"%p", vec![0]),
        ] {
            assert!(super::format_fes_string(format, &arguments).is_err());
        }
    }

    #[test]
    fn cst_return_restores_scene_identity_and_its_own_localization() {
        let mut interpreter = CstInterpreter::new(CstDocument {
            records: Vec::new(), inputs: Vec::new(),
        });
        interpreter.scene_name = b"caller".to_vec();
        interpreter.set_localization_table(Some(CstlTable {
            languages: vec!["ja".to_string()], messages: Vec::new(),
        }), Some("ja"), None);
        interpreter.load_scene(CstDocument {
            records: vec![CstRecord { kind: 0x30, text: b"return".to_vec() }],
            inputs: vec![0..1],
        }, b"callee".to_vec(), super::CstSceneTransfer {
            call: true, target: b"callee".to_vec(),
        }).unwrap();
        assert_eq!(interpreter.language(), None);
        interpreter.run(&mut ExpressionState::default(), 1).unwrap();
        assert_eq!(interpreter.scene_name, b"caller");
        assert_eq!(interpreter.language(), Some("ja"));
        assert_eq!(interpreter.pending(), Some(&CstEvent::Returned));
    }

    #[test]
    fn fes_macro_string_spaces_do_not_create_more_parameters() {
        let program = FesProgram::from_text(
            b"#MACRO\nTEXT $str1\n#START\nstrvar 2 TEXT\nexit\n",
        ).unwrap();
        let mut state = ExpressionState::default();
        state.strings.insert(1, b"hello world".to_vec());
        assert_eq!(program.command_parameters(b"cmd TEXT 9", &state.strings).unwrap(),
            [b"cmd".to_vec(), b"hello world".to_vec(), b"9".to_vec()]);
        let mut interpreter = FesInterpreter::new(program).unwrap();
        interpreter.run(&mut state, 8).unwrap();
        assert_eq!(state.strings.get(&2).unwrap(), b"hello world");
        // A legal CP932 tail may be an ASCII letter or a backslash. Neither
        // belongs to an identifier or escapes the following closing quote.
        let program = FesProgram::from_text(b"#MACRO\nA 7\nNAME object\n#START\n").unwrap();
        assert_eq!(program.expand_macros(b"\x82\x41 A").unwrap(), b"\x82\x41 7");
        assert_eq!(program.expand_macros(b".\x82\x41 A").unwrap(), b".\x82\x41 7");
        assert_eq!(program.expand_macros(b"\"\x82\x5c\" NAME").unwrap(),
            b"\"\x82\x5c\" object");
        assert_eq!(super::fes_parameters(b"a(b)c(d) 1(2) \"\"").unwrap(),
            [b"a".to_vec(), b"(b)c".to_vec(), b"(d)".to_vec(), b"1(2)".to_vec(), Vec::new()]);
        assert_eq!(
            super::fes_parameters(b"cmd \x81\x41(2)").unwrap(),
            [b"cmd".to_vec(), b"\x81\x41(2)".to_vec()],
        );
        let headers = FesProgram::from_text(
            b"#START\ncall \x82\xa0\n#\x82\xa0\nbreak\n#\x82\xa0.PUSH_L\nbreak\n",
        ).unwrap();
        assert!(headers.block(b"\x82\xa0").is_some());
        assert!(headers.block(b"\x82\xa0.PUSH_L").is_some());
    }

    #[test]
    fn fes_get_without_source_retains_target_and_out_of_range_compare_is_ignored() {
        let program = FesProgram::from_text(
            b"#START\nstrvar 7 original\nstrvar GET 7\nstrcmp 999999 a b\nexit\n",
        ).unwrap();
        let mut state = ExpressionState::default();
        let mut interpreter = FesInterpreter::new(program).unwrap();
        interpreter.run(&mut state, 8).unwrap();
        assert_eq!(state.strings.get(&7).unwrap(), b"original");
        assert!(matches!(interpreter.pending(), Some(FesEvent::Exit(0))));
    }

    #[test]
    fn fes_ascii_apostrophe_is_text_not_a_quote_delimiter() {
        let program = FesProgram::from_text(
            b"#SYSTEM_STR\nNekos'\n#START\nlabel \"Nekos' // text\" // comment\n",
        )
        .unwrap();

        assert_eq!(
            program.block(b"SYSTEM_STR").unwrap().lines[0].bytes,
            b"Nekos'"
        );
        assert_eq!(
            program.block(b"START").unwrap().lines[0].bytes,
            b"label \"Nekos' // text\""
        );
    }

    #[test]
    fn fes_macro_source_markers_are_retained_with_diagnostics() {
        let program = FesProgram::from_text(
            b"#MACRO\nqzmacro.h\ntweetmes.sub\nVALUE \"expanded\"\n#START\nVALUE\n",
        )
        .unwrap();

        assert_eq!(program.diagnostics.len(), 2);
        assert!(
            program
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.to_string().contains("source marker retained"))
        );
        assert_eq!(
            program.declarations[0]
                .lines
                .iter()
                .map(|line| line.bytes.as_slice())
                .collect::<Vec<_>>(),
            [
                b"qzmacro.h".as_slice(),
                b"tweetmes.sub",
                b"VALUE \"expanded\""
            ]
        );
        assert_eq!(program.block(b"START").unwrap().lines[0].bytes, b"expanded");
        assert_eq!(program.expand_macros(b"VALUE").unwrap(), b"expanded");
    }

    #[test]
    fn fes_unknown_bare_macro_entry_remains_an_error() {
        assert!(FesProgram::from_text(b"#MACRO\nunknown\n").is_err());
    }

    #[test]
    fn fes_expands_runtime_strings_before_macro_arguments() {
        let program =
            FesProgram::from_text(b"#MACRO\nCOMMAND \"$str1\"\n#START\nstrvar 1 hello\nCOMMAND\n")
                .unwrap();
        let mut interpreter = FesInterpreter::new(program).unwrap();
        let mut state = ExpressionState::default();
        assert!(matches!(
            interpreter.run(&mut state, 8).unwrap(),
            super::FesStep::Event
        ));
        match interpreter.pending() {
            Some(FesEvent::Command(line)) => assert_eq!(line.bytes, b"hello"),
            other => panic!("unexpected FES event: {other:?}"),
        }
    }

    #[test]
    fn fes_message_initializers_expand_shared_resource_fields() {
        let program = FesProgram::from_text(
            b"#DEFINE\nPLANE pl_base\nIMAGE img_bgi\nSTRING str\nSTRING name\n\
              #OBJECT SIZE2\npl_base $str900:0\n\
              #OBJECT FILE\nimg_bgi $str900\n\
              #OBJECT POS2 SIZE2\nstr $str900:10 $str900:10\nname $str900:11 $str900:11\n\
              #START\nexit 0\n",
        )
        .unwrap();
        let mut interpreter = FesInterpreter::new(program).unwrap();
        let mut state = ExpressionState::default();
        state.strings.insert(900, b"window art.hg3".to_vec());
        let mut initializers = Vec::new();
        while interpreter.initialization_pending() {
            interpreter.run(&mut state, 1).unwrap();
            if let Some(FesEvent::InitializeObject(initializer)) = interpreter.pending() {
                initializers.push(initializer.values.clone());
            }
            interpreter.resume();
        }
        assert_eq!(initializers, [
            vec![b"window art.hg3:0".to_vec()],
            vec![b"window art.hg3".to_vec()],
            vec![b"window art.hg3:10".to_vec(); 2],
            vec![b"window art.hg3:11".to_vec(); 2],
        ]);
    }

    #[test]
    fn fes_initialization_emits_structured_declarations_in_runtime_order() {
        let program = FesProgram::from_text(
            b"#PRELOAD\nportrait\n\
              #DEFINE\nIMAGE button[2]\n\
              #OBJECT FILE PRI\nbutton portrait 10\n\
              #KEYBLOCK 1\n0 3,2 -1,0,-1,-1\n\
              #OBJECT KEYBLOCK\nbutton[0] 0,1,2\n\
              #SUBCOMMAND\noneshot move motion px[0] : [0]\n\
              #START\nexit 0\n",
        )
        .unwrap();
        let mut interpreter = FesInterpreter::new(program).unwrap();
        let mut state = ExpressionState::default();

        assert!(matches!(
            interpreter.run(&mut state, 8).unwrap(),
            super::FesStep::Event
        ));
        assert!(matches!(
            interpreter.pending(),
            Some(FesEvent::Declarations(_))
        ));
        interpreter.resume();

        assert!(matches!(
            interpreter.run(&mut state, 8).unwrap(),
            super::FesStep::Event
        ));
        assert!(
            matches!(interpreter.pending(), Some(FesEvent::KeyBlocks(table)) if table.blocks[0].size == [3, 2])
        );
        interpreter.resume();

        assert!(matches!(
            interpreter.run(&mut state, 8).unwrap(),
            super::FesStep::Event
        ));
        assert!(
            matches!(interpreter.pending(), Some(FesEvent::CreateObject(definition)) if definition.kind == b"IMAGE" && definition.name == b"button" && definition.banks == 2)
        );
        interpreter.resume();

        assert!(matches!(
            interpreter.run(&mut state, 8).unwrap(),
            super::FesStep::Event
        ));
        assert!(
            matches!(interpreter.pending(), Some(FesEvent::InitializeObject(initializer)) if initializer.columns == [b"FILE".to_vec(), b"PRI".to_vec()] && initializer.values == [b"portrait".to_vec(), b"10".to_vec()])
        );
        interpreter.resume();

        assert!(matches!(
            interpreter.run(&mut state, 8).unwrap(),
            super::FesStep::Event
        ));
        assert!(
            matches!(interpreter.pending(), Some(FesEvent::KeyBlockBinding(binding)) if binding.target == b"button[0]" && binding.block == 0 && binding.position == [1, 2])
        );
        interpreter.resume();

        assert!(matches!(
            interpreter.run(&mut state, 8).unwrap(),
            super::FesStep::Event
        ));
        assert!(
            matches!(interpreter.pending(), Some(FesEvent::Subcommands(table)) if table.bindings.len() == 1 && table.bindings[0].name == b"move")
        );
    }

    #[test]
    fn cst_localization_selects_names_and_successive_message_fragments() {
        let document = CstDocument {
            records: vec![
                CstRecord {
                    kind: 0x21,
                    text: b"source speaker".to_vec(),
                },
                CstRecord {
                    kind: 0x20,
                    text: b"source first".to_vec(),
                },
                CstRecord {
                    kind: 0x20,
                    text: b"source second".to_vec(),
                },
            ],
            inputs: vec![0..3],
        };
        let table = CstlTable {
            languages: vec!["ja".into(), "zh".into()],
            messages: vec![vec![
                CstlMessage {
                    name: "speaker ja".into(),
                    content: "first ja\u{1f}second ja".into(),
                },
                CstlMessage {
                    name: "姓名".into(),
                    content: "第一段\u{1f}第二段".into(),
                },
            ]],
        };
        let mut interpreter = CstInterpreter::new(document);
        interpreter.set_localization(Some(CstlLocalization::new(table, Some("zh"), Some("ja"))));
        let mut state = ExpressionState::default();

        interpreter.run(&mut state, 1).unwrap();
        assert_eq!(
            interpreter.pending(),
            Some(&CstEvent::LocalizedSpeaker {
                text: "姓名".as_bytes().to_vec(),
                language: "zh".into(),
            })
        );
        interpreter.resume();
        interpreter.run(&mut state, 1).unwrap();
        assert_eq!(
            interpreter.pending(),
            Some(&CstEvent::LocalizedMessage {
                text: "第一段".as_bytes().to_vec(),
                language: "zh".into(),
            })
        );
        assert_eq!(
            interpreter
                .pending()
                .unwrap()
                .message_tokens_with_encoding(CstTextEncoding::Cp932)
                .unwrap()
                .unwrap()[0],
            CstTextToken::Text("第一段".as_bytes().to_vec())
        );
        interpreter.resume();
        interpreter.run(&mut state, 1).unwrap();
        assert_eq!(
            interpreter.pending(),
            Some(&CstEvent::LocalizedMessage {
                text: "第二段".as_bytes().to_vec(),
                language: "zh".into(),
            })
        );
    }

    #[test]
    fn cst_choice_localization_uses_dense_choice_and_input_count_offsets() {
        let document = CstDocument {
            records: vec![
                CstRecord {
                    kind: 0x30,
                    text: b"fselect menu".to_vec(),
                },
                CstRecord {
                    kind: 0x30,
                    text: b"10 first source one".to_vec(),
                },
                CstRecord {
                    kind: 0x30,
                    text: b"20 second source two".to_vec(),
                },
            ],
            inputs: vec![0..3, 3..3, 3..3],
        };
        let table = CstlTable {
            languages: vec!["zh".into()],
            messages: vec![
                vec![CstlMessage {
                    name: String::new(),
                    content: String::new(),
                }],
                vec![CstlMessage {
                    name: String::new(),
                    content: String::new(),
                }],
                vec![CstlMessage {
                    name: String::new(),
                    content: "本地选项一".into(),
                }],
            ],
        };
        let mut interpreter = CstInterpreter::new(document);
        interpreter.set_localization(Some(CstlLocalization::new(table, Some("zh"), None)));
        interpreter.run(&mut ExpressionState::default(), 1).unwrap();
        let Some(CstEvent::Choices(choices)) = interpreter.pending() else {
            panic!("expected localized choices");
        };
        assert_eq!(choices[0].text, "本地选项一".as_bytes());
        assert_eq!(choices[0].text_encoding, CstTextEncoding::Unicode);
        assert_eq!(choices[1].text, b"source two");
        assert_eq!(choices[1].text_encoding, CstTextEncoding::Cp932);
    }

    #[test]
    fn cst_text_parser_preserves_controls_and_multibyte_bytes() {
        let tokens = parse_cst_text(b"A\\n[base/ruby]\\cFF00;\\@\\x81\x40").unwrap();
        assert_eq!(
            tokens,
            vec![
                CstTextToken::Text(b"A".to_vec()),
                CstTextToken::Newline,
                CstTextToken::Ruby {
                    base: b"base".to_vec(),
                    ruby: b"ruby".to_vec(),
                },
                CstTextToken::Control {
                    name: b"c".to_vec(),
                    value: b"FF00".to_vec(),
                },
                CstTextToken::Continue,
                CstTextToken::Position {
                    axis: b'x',
                    value: 0x81,
                },
                CstTextToken::Text(b"@".to_vec()),
            ]
        );
    }

    #[test]
    fn cst_wait_classification_keeps_non_numeric_controls_structured() {
        let document = CstDocument {
            records: vec![CstRecord {
                kind: 0x30,
                text: b"wait tag voice 4".to_vec(),
            }],
            inputs: vec![0..1],
        };
        let mut interpreter = CstInterpreter::new(document);
        let mut state = ExpressionState::default();
        assert_eq!(
            interpreter.run(&mut state, 1).unwrap(),
            super::CstStep::Event
        );
        assert_eq!(
            interpreter.pending(),
            Some(&CstEvent::WaitControl {
                kind: b"tag".to_vec(),
                arguments: vec![b"voice".to_vec(), b"4".to_vec()],
            })
        );
    }
}
