//! cs2confx109: only controls with an inspected, existing sound consumer.
use super::*;

// Explicit control/path pairs from1d746, not arithmetic on resource IDs.
// root+f57c/f580 are shared-global+f574/f578 (scratch begins at root+8).
const SOUND_FIELDS: [(u32, &str); 2] = [
    (1027, "SOUND/sticky"),
    (1123, "SOUND/stereo"),
];

impl HeadlessPlayer {
    pub(super) fn prepare_plugin_sound_page(&self, source: &UiResources) -> Result<HostSettingsPage> {
        let (title, controls) = source.dialog(109)?;
        let label = |command| -> Result<String> {
            Ok(self.resolve_ui_label(&controls.iter().find(|(id, _)| *id == command)
                .context("sound109 lacks an inspected control")?.1))
        };
        let defaults = self.data_services.configuration.plugin_sound_defaults();
        let controls = SOUND_FIELDS.into_iter().zip(defaults).map(|((command, field), default)| {
            let caption = if command == 1027 { format!("{}\n{}", label(command)?, label(1112)?) }
                else { label(command)? };
            Ok(HostSettingControl { command, label: caption, kind: HostSettingKind::Toggle,
                value: u32::from(self.data_services.configuration.platform_value(field)
                    .context("plugin sound field is absent from shared configuration")? != 0),
                default: u32::from(default != 0), hidden: false, disabled: false,
                enabled_by: None, reset_changes_value: true })
        }).collect::<Result<_>>()?;
        Ok(HostSettingsPage { resource: 109, title: self.resolve_ui_label(&title),
            reset_label: label(1102)?, controls, input: None, font: None, sound: None })
    }

    pub(super) fn apply_configuration_plugin_settings(
        &mut self, pages: &[HostSettingsPage], edits: &[HostSettingsEdit],
    ) -> Result<()> {
        if pages.len() != 4 || edits.len() != 4 || pages[0].resource != 107
            || pages[1].resource != 130
            || !matches!(pages[2].resource, 128 | 131) || pages[3].resource != 109
            || pages[0].controls.iter().map(|field| field.command).collect::<Vec<_>>() != [1001, 1006, 1007, 1008]
            || pages[1].controls.iter().map(|field| field.command).collect::<Vec<_>>() != [1137]
            || pages[2].controls.iter().map(|field| field.command).collect::<Vec<_>>() != [1120]
            || pages[3].controls.iter().map(|field| field.command).collect::<Vec<_>>() != [1027, 1123]
        { bail!("configuration-plugin Apply differs from its prepared inspected pages"); }
        let general = &edits[0].words;
        let position = &edits[1].words;
        let image = &edits[2].words;
        let sound = &edits[3].words;
        if general.len() != 4 || general[0] > 2 || general[1..].iter().any(|&value| value > 1)
            || position.len() != 1 || position[0] > 2
            || image.len() != 1 || image[0] > 1
            || sound.len() != 2 || sound.iter().any(|&value| value > 1)
        { bail!("configuration-plugin Apply has invalid inspected fields"); }
        // Resolve a fallible original resource before accepting its setting.
        if general[2] != 0 { self.host_general_camouflage_title()?; }
        self.with_host(|host| {
            host.ensure_fes_settings()?;
            host.data_services.configuration.set_general_values(general.as_slice().try_into().unwrap());
            for ((_, field), &value) in SOUND_FIELDS.into_iter().zip(sound) {
                host.data_services.configuration.set_plugin_sound_value(field, value)?;
            }
            //1c53b/1d0f8 -> root+fbcc/global+fbc4.5c552a reads
            // that same live gate before reusing or growing the auxiliary pool.
            // Keep retained surfaces/owners: Apply is not a device reset.
            host.data_services.configuration.set_plugin_backbuffer(image[0])?;
            //1c0b7/1c0d1/1c0eb -> scratch+fc00/global+fbf8.
            //741 gates this shared field on the actual fullscreen receipt;
            // the sample uses645/630 -> MES_SHIFT -> existing PLANE position.
            host.data_services.fes_settings.set_message_window_position(position[0])?;
            //1df70 copies scratch,31c00(1) persists,483630 reloads shared fields.
            host.persist_configuration()?;
            //523090/5230c0 read the live global stereo flag for pan operations.
            // Keep the actual channel, handle, paused bit and source cursor.
            host.data_services.legacy_sound.stereo = sound[1] != 0;
            host.refresh_sound_task_options();
            host.data_services.language.system_settings_dirty = 1;
            Ok(())
        })?;
        // Sticky focus is a device-creation flag at492bd2..492c23. Its new
        // saved value must not reconfigure the already-created host policy.
        Ok(())
    }
}
