//! M2 #chain selection from1891c0/1970f0/197760/197e80.
//! Fixed transitions use the actual endpoint write in16d3a1..16d3bb.
//! Timed chains retain their identified kind and duration; they are not
//! approximated by an arbitrary linear tween.

use super::{CpuM2Config, EmoteError, Result};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuM2ChainKind {
    Fix,
    Linear,
    SinFs,
    SinSf,
    SinSs,
    Fade,
    NoMove,
}

impl CpuM2ChainKind {
    fn parse(value: &[u8]) -> Result<Self> {
        Ok(match value {
            b"fix" => Self::Fix,
            b"linear" => Self::Linear,
            b"sin_fs" => Self::SinFs,
            b"sin_sf" => Self::SinSf,
            b"sin_ss" => Self::SinSs,
            b"fade" => Self::Fade,
            b"nomove" => Self::NoMove,
            _ => return Err(invalid("M2 chain interpolation name")),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuM2ChainSelection {
    pub kind: CpuM2ChainKind,
    pub frames: u32,
    pub labels_cp932: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Dimension {
    Direct(CpuM2ChainKind, u32),
    Alias(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Alias {
    default: Option<CpuM2ChainKind>,
    frames: Option<u32>,
    pairs: Vec<(Vec<u8>, Vec<u8>, CpuM2ChainKind, u32, Vec<Vec<u8>>)>,
    fix: BTreeSet<Vec<u8>>,
    fade: BTreeSet<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CpuM2PatternChains {
    dimensions: BTreeMap<u32, Dimension>,
    aliases: BTreeMap<Vec<u8>, Alias>,
}

fn invalid(why: &'static str) -> EmoteError {
    EmoteError::InvalidResource(why)
}
fn integer(value: &[u8]) -> Result<u32> {
    std::str::from_utf8(value)
        .ok()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| invalid("M2 chain integer"))
}

impl CpuM2PatternChains {
    pub fn from_expanded<'a>(configs: impl IntoIterator<Item = &'a CpuM2Config>) -> Result<Self> {
        let mut output = Self::default();
        for config in configs {
            let mut alias = None;
            for row in config.records(b"chain") {
                let first = row.first().ok_or_else(|| invalid("M2 chain record"))?;
                if first.starts_with(b"$") {
                    if row.len() != 1 || first.len() < 2 {
                        return Err(invalid("M2 chain alias header"));
                    }
                    alias = Some(first.clone());
                    output.aliases.entry(first.clone()).or_default();
                    continue;
                }
                if let Some(key) = &alias {
                    let table = output
                        .aliases
                        .get_mut(key)
                        .ok_or_else(|| invalid("M2 chain alias owner"))?;
                    if first == b"default" && row.len() == 2 {
                        table.default = Some(CpuM2ChainKind::parse(&row[1])?);
                    } else if first == b"frame" && row.len() == 2 {
                        table.frames = Some(integer(&row[1])?);
                    } else if matches!(first.as_slice(), b"fix" | b"fade") && row.len() >= 2 {
                        //1973a5/1974d7 insert every following pattern label
                        //into alias+1c(fade) or+14(fix), respectively.
                        let set = if first == b"fade" {
                            &mut table.fade
                        } else {
                            &mut table.fix
                        };
                        set.extend(row[1..].iter().cloned());
                    } else if row.len() >= 4 {
                        table.pairs.push((
                            row[0].clone(),
                            row[1].clone(),
                            CpuM2ChainKind::parse(&row[2])?,
                            integer(&row[3])?,
                            row[4..].to_vec(),
                        ));
                    } else {
                        return Err(invalid("M2 chain alias record"));
                    }
                    continue;
                }
                let dimension = integer(first)?
                    .checked_sub(1)
                    .filter(|n| *n < 4096)
                    .ok_or_else(|| invalid("M2 chain dimension"))?;
                let value = if row.len() == 2 && row[1].starts_with(b"$") {
                    Dimension::Alias(row[1].clone())
                } else if row.len() == 3 {
                    Dimension::Direct(CpuM2ChainKind::parse(&row[1])?, integer(&row[2])?)
                } else {
                    return Err(invalid("M2 chain dimension record"));
                };
                //1891c0 updates the actual dimension record in input order.
                output.dimensions.insert(dimension, value);
            }
        }
        Ok(output)
    }

    ///197760: exact pair, reversible pair, fade set, fix set, then default.
    pub fn select(&self, dimension: u32, from: &[u8], to: &[u8]) -> Result<CpuM2ChainSelection> {
        let labels = || vec![from.to_vec(), to.to_vec()];
        match self.dimensions.get(&dimension) {
            Some(Dimension::Direct(kind, frames)) => Ok(CpuM2ChainSelection {
                kind: *kind,
                frames: *frames,
                labels_cp932: labels(),
            }),
            Some(Dimension::Alias(key)) => {
                let table = self
                    .aliases
                    .get(key)
                    .ok_or_else(|| invalid("M2 selected chain alias is absent"))?;
                if let Some((from, to, kind, frames, intermediate)) =
                    table.pairs.iter().find(|(a, b, ..)| a == from && b == to)
                {
                    let mut sequence = vec![from.clone()];
                    sequence.extend(intermediate.iter().cloned());
                    sequence.push(to.clone());
                    return Ok(CpuM2ChainSelection {
                        kind: *kind,
                        frames: *frames,
                        labels_cp932: sequence,
                    });
                }
                //197932..197b4b: NoMove pairs are deliberately directional.
                if let Some((_, _, kind, frames, intermediate)) =
                    table.pairs.iter().find(|(a, b, kind, ..)| {
                        a == to && b == from && *kind != CpuM2ChainKind::NoMove
                    })
                {
                    let mut sequence = vec![from.to_vec()];
                    sequence.extend(intermediate.iter().rev().cloned());
                    sequence.push(to.to_vec());
                    return Ok(CpuM2ChainSelection {
                        kind: *kind,
                        frames: *frames,
                        labels_cp932: sequence,
                    });
                }
                let kind = if table.fade.contains(from) || table.fade.contains(to) {
                    CpuM2ChainKind::Fade
                } else if table.fix.contains(from) || table.fix.contains(to) {
                    CpuM2ChainKind::Fix
                } else {
                    table
                        .default
                        .ok_or_else(|| invalid("M2 chain default kind is absent"))?
                };
                Ok(CpuM2ChainSelection {
                    kind,
                    frames: table
                        .frames
                        .ok_or_else(|| invalid("M2 chain default duration is absent"))?,
                    labels_cp932: labels(),
                })
            }
            None => Err(invalid("M2 changed dimension has no recovered chain")),
        }
    }
}
