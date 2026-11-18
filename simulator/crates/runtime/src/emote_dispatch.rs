//! Single source of truth for the identified NEKOPARA ordinary E-mote ABI.
//!
//! A recovered wrapper is not a completed backend operation. The descriptor
//! separates byte packing, return ABI and the receiver's semantic operation.
//! Native925 is a verified constant in native_profile.rs and is not in this set.

use super::{EmoteBuffer, EmoteCommand, EmoteError, EmoteReply, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmoteReturnKind {
    Continue,
    Value,
    ThreeWords,
    ByteString,
    DescriptorColumns,
}

impl EmoteReturnKind {
    pub fn accepts(self, reply: &EmoteReply) -> bool {
        matches!(
            (self, reply),
            (Self::Continue, EmoteReply::Continue)
                | (Self::Value, EmoteReply::Value(_))
                | (Self::ThreeWords, EmoteReply::ThreeWords(_))
                | (Self::ByteString, EmoteReply::ByteString(_))
                | (Self::DescriptorColumns, EmoteReply::DescriptorColumns(_))
        )
    }
}

/// Reusable parameter layouts; numbers here are argument DWORD indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmotePacking {
    Empty,
    /// data0=arg2, buffer1=formatted arg1.
    LoadSpec,
    /// data1=arg1.
    RawData1,
    /// data0=arg1, buffer1=three initialized DWORDs, VM outputs=arg2/3/4.
    VectorQuery,
    /// arg1 is a writable VM byte-string address, never an input record.
    StringOutput,
    /// data0=color arg1, data1=table arg2, with no string input.
    TouchCount,
    /// buffer1=color/table/row args1/2/3; VM outputs are args4/5.
    TouchItem,
    /// buffer1=float bits arg1, null selector.
    FloatAll,
    /// buffer0=formatted arg1, buffer1=float bits arg2.
    FloatNamed,
    /// buffer1=three raw DWORDs arg1/2/3.
    ThreeRawWords,
    /// buffer0=formatted arg1, data1=arg2.
    NamedRaw,
    /// data1=arg1, data0=arg2.
    TwoRawWords,
    /// buffer1=formatted arg1.
    Label,
    ///1028 pops only arg0 (4bytes), but formats the DWORD at storage+4
    ///before discarding the string and querying553. The host must obtain
    ///8 retained bytes via native_argument_storage; it must not increase
    ///the declared pop count or invent an extra zero argument. Keep the
    ///formatted bytes owned in buffer1 so that this read cannot be skipped.
    DiscardedTailString,
    /// buffer1=zeroed80 bytes: formatted arg1 at0, arg2 at72, arg3 at76.
    LabelRecord80,
}

impl EmotePacking {
    fn accepts(self, packet: &EmoteCommand) -> bool {
        use EmoteBuffer::{Bytes, FloatBits, None, ThreeWords};
        let (a, b) = (&packet.buffer0, &packet.buffer1);
        match self {
            Self::Empty | Self::RawData1 | Self::TwoRawWords | Self::StringOutput | Self::TouchCount => {
                matches!((a, b), (None, None))
            }
            Self::LoadSpec | Self::Label | Self::DiscardedTailString => matches!((a, b), (None, Bytes(_))),
            Self::VectorQuery | Self::ThreeRawWords | Self::TouchItem => {
                matches!((a, b), (None, ThreeWords(_)))
            }
            Self::FloatAll => matches!((a, b), (None, FloatBits(_))),
            Self::FloatNamed => matches!((a, b), (Bytes(_), FloatBits(_))),
            Self::NamedRaw => matches!((a, b), (Bytes(_), None)),
            Self::LabelRecord80 => {
                matches!((a, b), (None, Bytes(bytes)) if bytes.len() == 80)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmoteCommandSelector {
    Fixed(u32),
    /// Original1025 chooses548 for nonzero arg1 and549 for zero.
    BooleanArg1 {
        nonzero: u32,
        zero: u32,
    },
}

impl EmoteCommandSelector {
    pub fn select(self, arg1: u32) -> u32 {
        match self {
            Self::Fixed(code) => code,
            Self::BooleanArg1 { nonzero, zero } => {
                if arg1 == 0 {
                    zero
                } else {
                    nonzero
                }
            }
        }
    }

    fn accepts(self, code: u32) -> bool {
        match self {
            Self::Fixed(expected) => code == expected,
            Self::BooleanArg1 { nonzero, zero } => code == nonzero || code == zero,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmoteBatchValue {
    FloatBits,
    RawData1,
    NamesOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmoteDispatchShape {
    /// Direct manager API: no task address and no synthetic command code.
    Manager,
    Single {
        command: EmoteCommandSelector,
        packing: EmotePacking,
    },
    OrderedNameTable {
        command: u32,
        prefix_command: Option<u32>,
        value: EmoteBatchValue,
    },
    Unrecovered(&'static str),
}

/// The CPU receiver matches semantic operations, never a second list of slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmoteOperation {
    LoadResource,
    SelectSecondary,
    QuerySecondaryPatterns,
    QueryTouchCount,
    QueryTouchItem,
    VariableSpeed,
    VariableWeight,
    ResetVariableWeight,
    MaterialHold,
    QueryBusy,
    FinishPending,
    SplineLabel,
    TextureFilter,
    QueryAttributes,
    RecomputeControls,
    DebugBankText,
    SelectProgram,
    StartAction,
    QueryActionState,
    SkipAction,
    ClearActionSaved,
    HitTestControls,
    EnableControls,
    ColorPercentages,
    ControllerA,
    ControllerB,
    SurfaceFade,
    Unrecovered(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct EmoteNativeDescriptor {
    pub index: u32,
    pub wrapper_rva: u32,
    pub argument_bytes: u8,
    pub reply: EmoteReturnKind,
    pub shape: EmoteDispatchShape,
    pub operation: EmoteOperation,
}

impl EmoteNativeDescriptor {
    ///Readable original parameter storage is distinct from KCS's pop count.
    ///1028 intentionally reads one retired/live stack word after its argument.
    pub fn argument_storage_bytes(&self) -> usize {
        if matches!(self.shape, EmoteDispatchShape::Single {
            packing: EmotePacking::DiscardedTailString, ..
        }) {
            8
        } else {
            usize::from(self.argument_bytes)
        }
    }

    /// Some wrappers are known even though their manager semantics are not.
    pub fn wrapper_ready(&self) -> bool {
        !matches!(self.shape, EmoteDispatchShape::Unrecovered(_))
    }

    pub fn target_word(&self) -> Option<usize> {
        if self.argument_bytes == 0 {
            None
        } else {
            Some(0)
        }
    }

    /// Checks the native->command association and buffer shape before any
    /// backend side effect, including calls made outside the desktop planner.
    pub fn packet_operation(&self, packet: &EmoteCommand) -> Result<EmoteOperation> {
        let valid = packet.native == self.index
            && match self.shape {
                EmoteDispatchShape::Single { command, packing } => {
                    command.accepts(packet.command) && packing.accepts(packet)
                }
                EmoteDispatchShape::OrderedNameTable {
                    command,
                    prefix_command,
                    value,
                } => {
                    if Some(packet.command) == prefix_command {
                        EmotePacking::Empty.accepts(packet)
                    } else {
                        packet.command == command
                            && match value {
                                EmoteBatchValue::FloatBits => {
                                    EmotePacking::FloatNamed.accepts(packet)
                                }
                                EmoteBatchValue::RawData1 | EmoteBatchValue::NamesOnly => {
                                    EmotePacking::NamedRaw.accepts(packet)
                                }
                            }
                    }
                }
                EmoteDispatchShape::Manager | EmoteDispatchShape::Unrecovered(_) => false,
            };
        if !valid {
            return Err(EmoteError::Unsupported {
                native: packet.native,
                reason: "command or buffer layout does not match the identified wrapper descriptor",
            });
        }
        Ok(self.operation)
    }
}

macro_rules! single {
    ($index:literal, $rva:literal, $bytes:literal, $reply:ident, $code:expr, $pack:ident, $operation:expr) => {
        EmoteNativeDescriptor {
            index: $index,
            wrapper_rva: $rva,
            argument_bytes: $bytes,
            reply: EmoteReturnKind::$reply,
            shape: EmoteDispatchShape::Single {
                command: $code,
                packing: EmotePacking::$pack,
            },
            operation: $operation,
        }
    };
}
macro_rules! batch {
    ($index:literal, $rva:literal, $code:literal, $prefix:expr, $value:ident, $reason:literal) => {
        batch!($index, $rva, $code, $prefix, $value, $reason, EmoteOperation::Unrecovered($reason))
    };
    ($index:literal, $rva:literal, $code:literal, $prefix:expr, $value:ident, $reason:literal, $operation:expr) => {
        EmoteNativeDescriptor {
            index: $index,
            wrapper_rva: $rva,
            argument_bytes: 8,
            reply: EmoteReturnKind::Continue,
            shape: EmoteDispatchShape::OrderedNameTable {
                command: $code,
                prefix_command: $prefix,
                value: EmoteBatchValue::$value,
            },
            operation: $operation,
        }
    };
}
macro_rules! blocked {
    ($index:literal, $rva:literal, $bytes:literal, $reply:ident, $reason:literal) => {
        EmoteNativeDescriptor {
            index: $index,
            wrapper_rva: $rva,
            argument_bytes: $bytes,
            reply: EmoteReturnKind::$reply,
            shape: EmoteDispatchShape::Unrecovered($reason),
            operation: EmoteOperation::Unrecovered($reason),
        }
    };
}

use EmoteCommandSelector::{BooleanArg1, Fixed};
use EmoteOperation::{
    ClearActionSaved, ColorPercentages, ControllerA, ControllerB, EnableControls, LoadResource,
    QueryActionState, SelectProgram, SkipAction, SplineLabel, StartAction, Unrecovered,
};

/// Sorted, exact-PE descriptors. All RVAs are relative to the SHA-bound image
/// from emote::NEKOPARA_PE_SHA256. Evidence: recovery_A_20261116_emote.json.
#[rustfmt::skip]
pub static EMOTE_NATIVE_DESCRIPTORS: &[EmoteNativeDescriptor] = &[
    single!(906, 0x24d2b0, 12, Continue, Fixed(524), LoadSpec, LoadResource),
    single!(907, 0x24cc40, 8, Continue, Fixed(526), RawData1, EnableControls),
    single!(908, 0x24d730, 4, Continue, Fixed(527), Empty, EmoteOperation::FinishPending),
    single!(909, 0x24cf10, 4, Value, Fixed(528), Empty, EmoteOperation::QueryBusy),
    single!(910, 0x24ccd0, 20, ThreeWords, Fixed(529), VectorQuery, EmoteOperation::QueryAttributes),
    single!(911, 0x24d350, 8, Continue, Fixed(530), FloatAll, EmoteOperation::VariableSpeed),
    single!(912, 0x24d300, 12, Continue, Fixed(530), FloatNamed, EmoteOperation::VariableSpeed),
    batch!(913, 0x24d390, 530, None, FloatBits, "owned variable speed", EmoteOperation::VariableSpeed),
    single!(914, 0x24d570, 8, Continue, Fixed(532), FloatAll, EmoteOperation::VariableWeight),
    single!(915, 0x24d520, 12, Continue, Fixed(532), FloatNamed, EmoteOperation::VariableWeight),
    batch!(916, 0x24d5b0, 532, None, FloatBits, "owned variable weight", EmoteOperation::VariableWeight),
    single!(917, 0x24d230, 12, Continue, Fixed(533), LoadSpec, EmoteOperation::SelectSecondary),
    single!(918, 0x24d280, 8, Continue, Fixed(535), RawData1, EmoteOperation::TextureFilter),
    single!(919, 0x24d670, 16, Continue, Fixed(536), ThreeRawWords, ColorPercentages),
    single!(920, 0x24d030, 12, Continue, Fixed(538), NamedRaw, EmoteOperation::MaterialHold),
    batch!(921, 0x24d450, 538, Some(539), RawData1, "reset material hold then ordered raw holds", EmoteOperation::MaterialHold),
    blocked!(922, 0x24cfd0, 0, Continue, "global manager method0x1807c0 semantics"),
    blocked!(923, 0x24d210, 0, Continue, "global manager method0x180b30 semantics"),
    single!(924, 0x24d0f0, 12, Continue, Fixed(540), TwoRawWords, Unrecovered("sound-root cross-object link211/220/221/222")),
    single!(926, 0x24cd80, 8, ByteString, Fixed(534), StringOutput, EmoteOperation::QuerySecondaryPatterns),
    single!(927, 0x24cc10, 8, Continue, Fixed(541), RawData1, ControllerA),
    single!(928, 0x24cc70, 8, Continue, Fixed(542), RawData1, ControllerB),
    single!(929, 0x24d120, 8, Continue, Fixed(543), Label, SplineLabel),
    single!(930, 0x24d760, 4, Continue, Fixed(544), Empty, SplineLabel),
    single!(931, 0x24cca0, 8, Continue, Fixed(545), RawData1, EmoteOperation::SurfaceFade),
    EmoteNativeDescriptor {
        index: 932, wrapper_rva: 0x24cb50, argument_bytes: 0,
        reply: EmoteReturnKind::Continue, shape: EmoteDispatchShape::Manager,
        operation: EmoteOperation::RecomputeControls,
    },
    EmoteNativeDescriptor {
        index: 933, wrapper_rva: 0x24cbb0, argument_bytes: 0,
        reply: EmoteReturnKind::Continue, shape: EmoteDispatchShape::Manager,
        operation: EmoteOperation::DebugBankText,
    },
    single!(965, 0x24d790, 16, Continue, Fixed(547), LabelRecord80, SelectProgram),
    batch!(966, 0x24d160, 531, None, NamesOnly, "reset all current weights per table entry", EmoteOperation::ResetVariableWeight),
    single!(1025, 0x24d6d0, 8, Continue, BooleanArg1 { nonzero: 548, zero: 549 }, Empty, EmoteOperation::HitTestControls),
    single!(1027, 0x24d080, 8, Value, Fixed(551), Label, StartAction),
    single!(1028, 0x24cf70, 4, Value, Fixed(553), DiscardedTailString, QueryActionState),
    single!(1029, 0x24ceb0, 12, Value, Fixed(554), TouchCount, EmoteOperation::QueryTouchCount),
    single!(1030, 0x24cdc0, 24, DescriptorColumns, Fixed(554), TouchItem, EmoteOperation::QueryTouchItem),
    single!(1047, 0x24cff0, 4, Continue, Fixed(552), Empty, SkipAction),
    single!(1048, 0x24cb70, 4, Continue, Fixed(550), Empty, ClearActionSaved),
];

pub fn emote_native_descriptor(index: u32) -> Option<&'static EmoteNativeDescriptor> {
    EMOTE_NATIVE_DESCRIPTORS
        .binary_search_by_key(&index, |entry| entry.index)
        .ok()
        .map(|i| &EMOTE_NATIVE_DESCRIPTORS[i])
}

/// Exact DWORD arithmetic from M2Plane helperRVA0x239640, including wrapping
/// imul and unsigned magic division. Input percentages are not clamped first.
/// Alpha is preserved. This calculates the uniform vertex tint, not textures.
pub fn m2plane_tint_argb(base_argb: u32, percentages: [u32; 3]) -> u32 {
    fn channel(base: u32, percentage: u32) -> u32 {
        let scaled = percentage.wrapping_mul(255);
        let divided100 = (((u64::from(scaled) * 0x51eb851f) >> 32) as u32) >> 5;
        let product = divided100.wrapping_mul(base);
        let divided255 = (((u64::from(product) * 0x80808081) >> 32) as u32) >> 7;
        divided255.min(255)
    }
    let red = channel((base_argb >> 16) & 255, percentages[0]);
    let green = channel((base_argb >> 8) & 255, percentages[1]);
    let blue = channel(base_argb & 255, percentages[2]);
    (base_argb & 0xff000000) | (red << 16) | (green << 8) | blue
}
