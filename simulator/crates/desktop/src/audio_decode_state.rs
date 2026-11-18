//! Owned PCM checkpoint wire; decoded content is verified against the resource
//! during prepare, never replaced by an empty/default source.
use super::*;
use cs2_runtime::graph_serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct PcmState { probe: AudioProbe, samples: Vec<i16> }
impl PcmState {
    pub(crate) fn matches(&self, other: &Self) -> bool { self.probe == other.probe && self.samples == other.samples }
    pub(crate) fn capture(value: &DecodedAudio) -> RuntimeResult<Self> {
        let result = Self { probe: value.probe, samples: value.samples.to_vec() };
        result.validate()?;
        Ok(result)
    }
    fn validate(&self) -> RuntimeResult<()> {
        if !(1..=384_000).contains(&self.probe.sample_rate) || !matches!(self.probe.channels, 1 | 2)
            || self.samples.is_empty() || self.samples.len() > MAX_PCM / 2
            || self.samples.len() % usize::from(self.probe.channels) != 0
            || self.samples.len() / usize::from(self.probe.channels) != self.probe.frames as usize
            || self.probe.codec == AudioCodec::SpeechPcm {
            return Err(error("SOUND checkpoint has invalid/unsupported PCM source or speech provider"));
        }
        Ok(())
    }
    pub(crate) fn restore(self) -> RuntimeResult<Arc<DecodedAudio>> {
        self.validate()?;
        Ok(Arc::new(DecodedAudio { probe: self.probe, samples: self.samples.into() }))
    }
}
pub(crate) mod optional {
    use super::*;
    pub(crate) fn serialize<S: cs2_runtime::graph_serde::Serializer>(
        value: &Option<Arc<DecodedAudio>>, s: S,
    ) -> Result<S::Ok, S::Error> {
        value.as_ref().map(|v| PcmState::capture(v)).transpose()
            .map_err(cs2_runtime::graph_serde::ser::Error::custom)?.serialize(s)
    }
    pub(crate) fn deserialize<'de, D: cs2_runtime::graph_serde::Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Arc<DecodedAudio>>, D::Error> {
        Option::<PcmState>::deserialize(d)?.map(PcmState::restore).transpose()
            .map_err(cs2_runtime::graph_serde::de::Error::custom)
    }
}
