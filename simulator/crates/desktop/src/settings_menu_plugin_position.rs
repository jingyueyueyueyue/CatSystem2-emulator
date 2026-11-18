//! cs2confx130 message placement, consumed by741 and the original MES_SHIFT.
use super::*;

impl HeadlessPlayer {
    pub(super) fn prepare_plugin_position_page(&mut self, source: &UiResources) -> Result<HostSettingsPage> {
        let (title, controls) = source.dialog(130)?;
        let (value, default) = self.with_host(|host| {
            host.ensure_fes_settings()?;
            let startup = host.read_legacy_sound_xml("startup.xml")?;
            //1c1c7 copies the pre-user default global+fbf8 into scratch+fc00.
            // Keep it distinct from the live private FULL_SCREEN/mw_pos.
            let default = startup.as_ref().and_then(|xml| super::super::xml_integer(xml,
                "document/SCREEN/mw_pos")).unwrap_or(0);
            Ok((host.data_services.fes_settings.message_window_position(), default))
        })?;
        if value > 2 || default > 2 {
            bail!("plugin message position has no inspected radio selection");
        }
        let label = |command| -> Result<String> {
            Ok(self.resolve_ui_label(&controls.iter().find(|(id, _)| *id == command)
                .context("plugin position page lacks an inspected control")?.1))
        };
        let group_token: Vec<_> = "$0510-0010$".encode_utf16().collect();
        let caption = &controls.iter().find(|(_, text)| text.starts_with(&group_token))
            .context("plugin position page lacks its message-window group")?.1;
        Ok(HostSettingsPage { resource: 130, title: self.resolve_ui_label(&title),
            reset_label: label(1102)?, controls: vec![HostSettingControl {
                command: 1137, label: self.resolve_ui_label(caption),
                kind: HostSettingKind::Choice([1137, 1138, 1139].into_iter()
                    .map(label).collect::<Result<_>>()?),
                value, default, hidden: false, disabled: false,
                enabled_by: None, reset_changes_value: true,
            }], input: None, font: None, sound: None })
    }
}
