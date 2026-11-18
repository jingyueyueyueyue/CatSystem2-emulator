//! Resumable FES SUBCOMMAND parameter mapping, independent of window/media
//! services. It emits the raw float DWORDs consumed by the existing KCS motion
//! VM. Property capture, queue insertion and task updates belong to the host.
use cs2_script::{FesSubcommand, FesSubcommandArgument, FesSubcommandParameter};
use crate::{RuntimeError, RuntimeResult};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FesMotionContext {
    pub base: [f32; 3],
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
    pub argb: u32,
}

impl Default for FesMotionContext {
    fn default() -> Self {
        Self {
            base: [0.; 3], position: [0.; 3], velocity: [0.; 3],
            rotation: [0.; 3], scale: [1.; 3], argb: u32::MAX,
        }
    }
}

/// A host stores one cursor for its current bank. Completed expressions and
/// their effects are never evaluated again after a later resource/VM error.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FesMotionArguments {
    context: FesMotionContext,
    input: usize,
    values: Vec<u32>,
}

impl FesMotionArguments {
    pub fn new(context: FesMotionContext) -> Self {
        Self { context, input: 0, values: Vec::new() }
    }

    pub fn values(&self) -> &[u32] { &self.values }

    pub fn validate_checkpoint(&self) -> RuntimeResult<()> {
        if self.input > 16 || self.values.len() > 16 {
            return Err(error("FES motion mapping checkpoint exceeds sixteen arguments"));
        }
        Ok(())
    }

    pub fn complete(
        &mut self, binding: &FesSubcommand, supplied: &[Vec<u8>],
        mut evaluate: impl FnMut(&[u8], i32) -> RuntimeResult<i32>,
    ) -> RuntimeResult<&[u32]> {
        if binding.parameters.len() > 16 || binding.arguments.len() > 16 {
            return Err(error("FES motion declaration exceeds sixteen parameters"));
        }
        while let Some(argument) = binding.arguments.get(self.values.len()) {
            let value = match argument {
                FesSubcommandArgument::Integer(value) => (*value as f32).to_bits(),
                FesSubcommandArgument::Context(key) => {
                    let value = match key.as_slice() {
                        b"cxr" => (self.context.argb >> 16) & 255,
                        b"cxg" => (self.context.argb >> 8) & 255,
                        b"cxb" => self.context.argb & 255,
                        b"cxa" | b"CA" => self.context.argb >> 24,
                        _ => return Err(error("unrecovered FES motion mapping context")),
                    };
                    (value as f32).to_bits()
                }
                FesSubcommandArgument::Parameter(index) => {
                    let parameter = binding.parameters.get(*index)
                        .ok_or_else(|| error("FES motion mapping references an absent descriptor"))?;
                    // Actual inputs are consumed in mapping order, not indexed
                    // by [n]. Missing input uses descriptor n's default; an
                    // explicitly empty input uses the output-position default.
                    let source = match supplied.get(self.input) {
                        Some(source) if source.is_empty() => binding.parameters.get(self.values.len())
                            .map(|parameter| parameter.default.as_slice())
                            .ok_or_else(|| error("empty FES motion input has no output-position default"))?,
                        Some(source) => source.as_slice(),
                        None => parameter.default.as_slice(),
                    };
                    let value = convert(parameter, source, &self.context, &mut evaluate)?;
                    self.input += 1;
                    value
                }
            };
            self.values.push(value);
        }
        Ok(&self.values)
    }
}

fn convert(
    parameter: &FesSubcommandParameter, source: &[u8], context: &FesMotionContext,
    evaluate: &mut impl FnMut(&[u8], i32) -> RuntimeResult<i32>,
) -> RuntimeResult<u32> {
    let descriptor = parameter.descriptor.as_slice();
    if matches!(descriptor, b"CX" | b"cx") {
        return Ok(0_f32.to_bits());
    }
    let (current, base, degrees, relative) = if descriptor == b"p" {
        (0., 0., false, false)
    } else if descriptor.len() == 2 && matches!(descriptor[0], b'C' | b'c') {
        let shift = match descriptor[1].to_ascii_uppercase() {
            b'R' => 16, b'G' => 8, b'B' => 0, b'A' => 24,
            _ => return Err(error("invalid FES motion color descriptor")),
        };
        let current = ((context.argb >> shift) & 255) as f32;
        (current, current, false, descriptor[0] == b'c')
    } else if descriptor.len() == 2 {
        let axis = match descriptor[1].to_ascii_uppercase() {
            b'X' => 0, b'Y' => 1, b'Z' => 2,
            _ => return Err(error("invalid FES motion vector axis")),
        };
        let value = match descriptor[0].to_ascii_uppercase() {
            b'B' => context.base[axis], b'P' => context.position[axis],
            b'V' => context.velocity[axis], b'R' => context.rotation[axis],
            b'S' => context.scale[axis],
            _ => return Err(error("invalid FES motion parameter descriptor")),
        };
        (value, if descriptor[0].eq_ignore_ascii_case(&b'S') { 1. } else { value },
            descriptor[0].eq_ignore_ascii_case(&b'R'), descriptor[0].is_ascii_lowercase())
    } else { return Err(error("invalid FES motion parameter descriptor")); };
    let percent = source.last() == Some(&b'%');
    let source = if percent { &source[..source.len() - 1] } else { source };
    let integer = evaluate(source, x86_integer(current))?;
    let mut value = integer as f32;
    if degrees { value = value * std::f32::consts::PI / 180.; }
    if percent { value = value * base / 100.; }
    if relative { value -= current; }
    Ok(value.to_bits())
}

pub fn x86_integer(value: f32) -> i32 {
    if !value.is_finite() || value < -2_147_483_648.0 || value >= 2_147_483_648.0 {
        i32::MIN
    } else { value.trunc() as i32 }
}

fn error(message: &str) -> RuntimeError { RuntimeError::Host(message.into()) }
