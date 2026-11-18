//! Pair the existing SOUND banks with the same task/driver/physical stores.
//! No declaration replay, channel allocation or device work occurs here.
use super::*;
use crate::sound_task::checkpoint::{PhysicalSoundIdentity, PreparedPhysicalSoundState};
use crate::{HeadlessPlayer, ScriptTextEncoding};
use crate::fes_tasks::FesTaskStore;

impl FesSoundStore {
    pub(crate) fn validate_live_physical_checkpoint(&self, host: &HeadlessPlayer,
        tasks: &FesTaskStore, encoding: ScriptTextEncoding,
    ) -> Result<BTreeSet<Key>> {
        self.validate_physical_links(tasks, encoding,
            &host.physical_sound_task_ids().copied().collect(),
            |channel| host.physical_sound_channel_identity(channel))
    }

    pub(crate) fn validate_physical_checkpoint(&self, physical: &PreparedPhysicalSoundState,
        tasks: &FesTaskStore, encoding: ScriptTextEncoding,
    ) -> Result<BTreeSet<Key>> {
        self.validate_physical_links(tasks, encoding,
            &physical.task_ids().copied().collect(), |channel| physical.channel_identity(channel))
    }

    fn validate_physical_links(&self, tasks: &FesTaskStore, _encoding: ScriptTextEncoding,
        physical_tasks: &BTreeSet<u32>, channel: impl Fn(u32) -> Option<PhysicalSoundIdentity>,
    ) -> Result<BTreeSet<Key>> {
        // Object names remain exact original encoded bytes. Resolved resource
        // names/PCM are validated by the physical component, not re-expanded
        // against today's category settings or a different language prefix.
        let mut motion = BTreeSet::new();
        for (key, record) in &self.banks {
            let driver = tasks.runtime(key.0).and_then(|runtime| runtime.driver.as_ref())
                .context("SOUND checkpoint has no actual FES driver owner")?;
            if !record.deleted {
                let definition = usize::try_from(key.2).ok()
                    .and_then(|bank| driver.adapter.object_bank_definition(&key.1, bank))
                    .context("SOUND checkpoint bank has no live adapter declaration")?;
                if !matches!(definition.kind_name.as_slice(), b"SOUND" | b"SNDTABLE")
                    || record.playlist.is_some() != (definition.kind_name == b"SNDTABLE") {
                    bail!("SOUND checkpoint bank/playlist belongs to another declared kind");
                }
                motion.insert(key.clone());
            }
            // CHANNEL is a signed script property: -1 and other invalid
            // values are real no-source states, and declarations need not
            // have allocated a physical receiver yet. Shared channels may
            // have been reloaded by another bank; never equate FILE to the
            // currently resident source or synthesize an ownership parent.
            let mut references = vec![record.channel];
            if let Some(result) = record.media_result { references.push(result.channel); }
            if let Some(submission) = &record.submission {
                submission.validate_checkpoint(record.action_stage == 3)?;
                if !record.action || !matches!(record.action_stage, 1 | 3) {
                    bail!("SOUND checkpoint submission has no owning media action");
                }
                references.push(submission.checkpoint_channel() as i32);
            } else if record.action && record.action_stage != 0 {
                bail!("SOUND checkpoint action lost its accepted submission");
            }
            for index in references.into_iter().filter(|&index| valid_channel(index)) {
                let Some(identity) = channel(index as u32) else { continue; };
                if identity.owner.is_some_and(|owner| !physical_tasks.contains(&owner))
                    || identity.handle.is_some() && (identity.resource.is_none() || identity.resource_id == 0) {
                    bail!("SOUND checkpoint bank references an unowned physical source/handle");
                }
            }
            if let Some(notice) = &record.notice {
                let alias = matches!(notice.waiting, Some(NoticeWaiting::Alias));
                let object = matches!(notice.waiting, Some(NoticeWaiting::Object));
                notice.state.validate_checkpoint(&key.1, key.2, notice.notification,
                    alias, object, record.deleted)?;
                if alias || object {
                    if self.callbacks.get(&key.0) != Some(key) {
                        bail!("SOUND checkpoint running notice lost its callback bank");
                    }
                    tasks.validate_sound_notice_checkpoint(key.0, &key.1, key.2,
                        notice.notification.event(), alias.then(|| notice.state.checkpoint_alias()),
                        record.callback_done)?;
                }
            } else if self.callbacks.get(&key.0) == Some(key) || record.callback_done {
                bail!("SOUND checkpoint callback receipt has no media notice");
            }
        }
        Ok(motion)
    }
}
