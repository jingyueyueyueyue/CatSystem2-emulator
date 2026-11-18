//! The shared, inspected backbuffer control of cs2confx128/131.
use super::*;

impl HeadlessPlayer {
    pub(super) fn prepare_plugin_image_page(&mut self, source: &UiResources) -> Result<HostSettingsPage> {
        //1eae0 selects128/129 when CONFIG_B/view3d is nonzero,131 otherwise.
        //3ffc9..3fff2 reads view3d with default0 into owner+c624/global+c20c.
        // This is a startup definition, not the separate user CONFIG/type.
        let detailed = self.with_host(|host| {
            let startup = host.read_legacy_sound_xml("startup.xml")?;
            Ok(startup.as_ref().and_then(|xml| super::super::xml_integer(xml,
                "document/CONFIG_B/view3d")).unwrap_or(0) != 0)
        })?;
        let resource = if detailed { 128 } else { 131 };
        let (title, controls) = source.dialog(resource)?;
        let label = |command| -> Result<String> {
            Ok(self.resolve_ui_label(&controls.iter().find(|(id, _)| *id == command)
                .context("plugin image page lacks an inspected control")?.1))
        };
        let value = self.data_services.configuration.platform_value("D3D/backbuffer")
            .context("plugin backbuffer field is absent from shared configuration")?;
        Ok(HostSettingsPage { resource, title: self.resolve_ui_label(&title), reset_label: label(1102)?,
            controls: vec![HostSettingControl { command: 1120, label: label(1120)?,
                kind: HostSettingKind::Toggle, value: u32::from(value != 0), default: 1,
                hidden: false, disabled: false, enabled_by: None, reset_changes_value: true }],
            input: None, font: None, sound: None })
    }
}
