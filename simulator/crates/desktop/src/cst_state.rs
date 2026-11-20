//! Full independent CST state in the host's object save graph. Ordinary385
//! remains its original table-only protocol and does not silently grow fields.
use std::collections::BTreeMap;
use cs2_script::cst_snapshot::{CstContextSnapshot, CstContextStoreSnapshot,
    CstDocumentSnapshot, CstLocalizationSnapshot};
use cs2_script::Result;
use cs2_script::data_snapshot::{CssObjectSnapshot, ScriptDataObjectsSnapshot,
    RecordStackSnapshot, XmlDocumentSnapshot};
use super::{CstContextState, HeadlessPlayer, ScriptTextEncoding, native_state_error,
    parse_cst_table, serialize_cst_table};

/// Prepared restore is intentionally separate from commit so a complete host
/// loader can validate every object's identities before changing live state.
pub(crate) struct PreparedCstContexts(BTreeMap<u32, CstContextState>);

pub(crate) struct PreparedScriptDataObjects {
    contexts: PreparedCstContexts,
    containers: BTreeMap<u32, cs2_formats::CssFile>,
    strings: BTreeMap<u32, Vec<u8>>,
    databases: super::database_native::DatabaseStore,
    windows: super::cst_window_native::CstWindowStore,
    maps: BTreeMap<u32, BTreeMap<Vec<u8>, u32>>,
    raw: super::data_services::PreparedRawStorage,
    tables: super::data_services::PreparedDataTables,
    ordinary: super::ordinary_native::PreparedOrdinaryData,
    record_stacks: BTreeMap<u32, super::DesktopRecordStack>,
    xml_documents: BTreeMap<u32, cs2_formats::ConfigXml>,
}

pub(crate) struct PreparedBacklogs(BTreeMap<u32, cs2_runtime::backlog::Backlog>);

impl HeadlessPlayer {
    pub fn backlog_snapshots(&self) -> Result<BTreeMap<u32, cs2_runtime::backlog::BacklogSnapshot>> {
        self.backlogs.iter().map(|(&handle, log)| {
            Ok((handle, log.snapshot().map_err(|_| native_state_error("backlog object capture failed"))?))
        }).collect()
    }

    pub(crate) fn prepare_backlog_restore(
        snapshots: &BTreeMap<u32, cs2_runtime::backlog::BacklogSnapshot>,
    ) -> Result<PreparedBacklogs> {
        if snapshots.len() > 1_000_000 {
            return Err(native_state_error("backlog object group exceeds its item limit"));
        }
        let mut objects = BTreeMap::new();
        let mut total = 0usize;
        for (&handle, saved) in snapshots {
            if handle == 0 { return Err(native_state_error("backlog object snapshot has a zero identity")); }
            total = total.checked_add(saved.body.len())
                .and_then(|total| total.checked_add(saved.settings.name_first.len()))
                .and_then(|total| total.checked_add(saved.settings.name_last.len()))
                .filter(|&total| total <= 256 * 1024 * 1024)
                .ok_or_else(|| native_state_error("backlog object snapshots exceed 256 MiB"))?;
            let object = cs2_runtime::backlog::Backlog::from_snapshot(saved)
                .map_err(|_| native_state_error("backlog object snapshot is malformed"))?;
            objects.insert(handle, object);
        }
        Ok(PreparedBacklogs(objects))
    }

    pub(crate) fn commit_backlog_restore(&mut self, prepared: PreparedBacklogs) {
        self.backlogs = prepared.0;
    }

    pub fn shared_numeric_state_snapshot(
        &self,
    ) -> Result<cs2_script::shared_numeric_snapshot::SharedNumericStateSnapshot> {
        self.data_services.tasks.shared_numeric_state_snapshot(&self.shared_values)
    }

    pub(crate) fn restore_shared_numeric_state(
        &mut self, state: cs2_script::shared_numeric_snapshot::SharedNumericStateSnapshot,
    ) -> Result<()> {
        self.data_services.tasks.restore_shared_numeric_state(&mut self.shared_values, state)
    }

    pub fn script_data_objects_snapshot(&self) -> Result<ScriptDataObjectsSnapshot> {
        let snapshot = ScriptDataObjectsSnapshot {
            version: 5, contexts: self.cst_context_snapshots()?,
            containers: self.css_handles.iter()
                .map(|(&handle, file)| CssObjectSnapshot::capture(handle, file)).collect(),
            shared_strings: self.string_values.clone(),
            databases: self.data_services.databases.snapshots()?,
            cst_windows: self.data_services.cst_windows.snapshots()?,
            ordered_maps: self.integer_maps.clone(),
            raw_blocks: self.data_services.raw_snapshots(),
            triple_tables: self.data_services.triple_snapshots(),
            name_tables: self.data_services.name_snapshots(),
            sequences: self.data_services.ordinary.sequence_snapshots()?,
            raw_images: self.data_services.ordinary.raw_image_snapshots(),
            pair_tables: self.data_services.ordinary.pair_snapshots(),
            record_stacks: self.record_stacks.iter().map(|(&handle, stack)|
                (handle, RecordStackSnapshot { stride: stack.stride, count: stack.count,
                    growth: stack.growth, capacity: stack.capacity, bytes: stack.bytes.clone() })).collect(),
            xml_documents: self.xml_handles.iter().map(|(&handle, document)|
                (handle, XmlDocumentSnapshot::capture(document))).collect(),
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub(crate) fn prepare_script_data_objects_restore(
        snapshot: &ScriptDataObjectsSnapshot,
    ) -> Result<PreparedScriptDataObjects> {
        snapshot.validate()?;
        let contexts = Self::prepare_cst_context_restore(&snapshot.contexts)?;
        let containers = snapshot.containers.iter()
            .map(|saved| Ok((saved.handle, saved.restore()?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        Ok(PreparedScriptDataObjects {
            contexts, containers, strings: snapshot.shared_strings.clone(),
            databases: super::database_native::DatabaseStore::from_snapshots(&snapshot.databases)?,
            windows: super::cst_window_native::CstWindowStore::from_snapshots(&snapshot.cst_windows)?,
            maps: snapshot.ordered_maps.clone(),
            raw: super::data_services::DataServices::prepare_raw_restore(&snapshot.raw_blocks),
            tables: super::data_services::DataServices::prepare_table_restore(snapshot),
            ordinary: super::ordinary_native::OrdinaryNativeServices::prepare_data_restore(snapshot)?,
            record_stacks: snapshot.record_stacks.iter().map(|(&handle, stack)|
                (handle, super::DesktopRecordStack { stride: stack.stride, count: stack.count,
                    growth: stack.growth, capacity: stack.capacity, bytes: stack.bytes.clone() })).collect(),
            xml_documents: snapshot.xml_documents.iter().map(|(&handle, document)|
                Ok((handle, document.restore()?))).collect::<Result<_>>()?,
        })
    }

    pub(crate) fn commit_script_data_objects_restore(&mut self, prepared: PreparedScriptDataObjects) {
        self.commit_cst_context_restore(prepared.contexts);
        self.css_handles = prepared.containers;
        self.string_values = prepared.strings;
        self.data_services.cst_text_origins = Default::default();
        self.data_services.databases = prepared.databases;
        self.data_services.cst_windows = prepared.windows;
        self.integer_maps = prepared.maps;
        self.data_services.commit_raw_restore(prepared.raw);
        self.data_services.commit_table_restore(prepared.tables);
        self.data_services.ordinary.commit_data_restore(prepared.ordinary);
        self.record_stacks = prepared.record_stacks;
        self.xml_handles = prepared.xml_documents;
    }

    pub fn cst_context_snapshots(&self) -> Result<CstContextStoreSnapshot> {
        let mut contexts = Vec::with_capacity(self.cst_contexts.len());
        for (&handle, context) in &self.cst_contexts {
            contexts.push(CstContextSnapshot {
                handle, position: context.position,
                encoding: match context.encoding { ScriptTextEncoding::Cp932 => 0,
                    ScriptTextEncoding::Gbk => 1, ScriptTextEncoding::Utf8 => 2 },
                document: context.document.as_ref().map(CstDocumentSnapshot::capture).transpose()?,
                registered_languages: context.languages.clone(),
                localization: context.localization.as_ref().map(CstLocalizationSnapshot::capture),
                registrations: serialize_cst_table(&context.registrations, true)?,
                history: serialize_cst_table(&context.history, true)?,
                source_name: context.source_name.clone(), source_line: context.source_line,
            });
        }
        let snapshot = CstContextStoreSnapshot { version: 1, contexts };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub(crate) fn prepare_cst_context_restore(
        snapshot: &CstContextStoreSnapshot,
    ) -> Result<PreparedCstContexts> {
        snapshot.validate()?;
        let mut contexts = BTreeMap::new();
        for saved in &snapshot.contexts {
            let encoding = match saved.encoding { 0 => ScriptTextEncoding::Cp932,
                1 => ScriptTextEncoding::Gbk, 2 => ScriptTextEncoding::Utf8,
                _ => return Err(native_state_error("invalid saved CST text encoding")) };
            contexts.insert(saved.handle, CstContextState {
                document: saved.document.as_ref().map(CstDocumentSnapshot::restore).transpose()?,
                position: saved.position,
                registrations: parse_cst_table(&saved.registrations)?,
                history: parse_cst_table(&saved.history)?,
                languages: saved.registered_languages.clone(),
                localization: saved.localization.as_ref().map(CstLocalizationSnapshot::restore).transpose()?,
                encoding,
                source_name: saved.source_name.clone(), source_line: saved.source_line,
                story: Default::default(),
            });
        }
        Ok(PreparedCstContexts(contexts))
    }

    /// The full graph loader must restore the global handle allocator and
    /// cross-object references together with this already validated CST group.
    pub(crate) fn commit_cst_context_restore(&mut self, prepared: PreparedCstContexts) {
        self.cst_contexts = prepared.0;
    }
}
