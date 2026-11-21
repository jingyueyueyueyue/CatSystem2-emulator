//! Owned PCM checkpoint wire; decoded content is verified against the resource
//! during prepare, never replaced by an empty/default source.
use super::*;
use cs2_runtime::graph_serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct PcmState {
    probe: AudioProbe,
    samples: Vec<i16>,
}
impl PcmState {
    pub(crate) fn matches_source(&self, source: &PreparedAudio) -> RuntimeResult<bool> {
        Ok(self.probe == source.probe && source.matches_samples(&self.samples)?)
    }
    pub(crate) fn capture(value: &PreparedAudio) -> RuntimeResult<Self> {
        if value.sample_count() * size_of::<i16>() > cs2_runtime::object_graph::MAX_GRAPH_BYTES {
            return Err(error(
                "SOUND checkpoint PCM exceeds the portable graph byte bound",
            ));
        }
        let result = Self {
            probe: value.probe,
            samples: value.checkpoint_samples()?,
        };
        result.validate()?;
        Ok(result)
    }
    fn validate(&self) -> RuntimeResult<()> {
        if !(1..=MAX_SAMPLE_RATE).contains(&self.probe.sample_rate)
            || !matches!(self.probe.channels, 1 | 2)
            || self.samples.is_empty()
            || self.samples.len() > MAX_PCM / 2
            || self.samples.len() % usize::from(self.probe.channels) != 0
            || self.samples.len() / usize::from(self.probe.channels) != self.probe.frames as usize
            || self.probe.codec == AudioCodec::SpeechPcm
        {
            return Err(error(
                "SOUND checkpoint has invalid/unsupported PCM source or speech provider",
            ));
        }
        Ok(())
    }
    pub(crate) fn restore(self) -> RuntimeResult<Arc<PreparedAudio>> {
        self.validate()?;
        Ok(Arc::new(PreparedAudio::from_pcm(DecodedAudio {
            probe: self.probe,
            samples: self.samples.into(),
        })))
    }
}
pub(crate) mod optional {
    use super::*;
    pub(crate) fn serialize<S: cs2_runtime::graph_serde::Serializer>(
        value: &Option<Arc<PreparedAudio>>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        value
            .as_ref()
            .map(|v| PcmState::capture(v))
            .transpose()
            .map_err(cs2_runtime::graph_serde::ser::Error::custom)?
            .serialize(s)
    }
    pub(crate) fn deserialize<'de, D: cs2_runtime::graph_serde::Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Arc<PreparedAudio>>, D::Error> {
        Option::<PcmState>::deserialize(d)?
            .map(PcmState::restore)
            .transpose()
            .map_err(cs2_runtime::graph_serde::de::Error::custom)
    }
}
