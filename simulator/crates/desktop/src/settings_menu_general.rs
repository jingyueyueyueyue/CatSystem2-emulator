//! Inspected cs2confx General107 over the host's existing configuration.
use super::*;

const PLUGIN_SHA: &str = "baefc568b127d0dda738758b32b9a70a40a7241cb26996e8c58c34be61d3eb19";
const FIELDS: [&str; 4] = ["WINDOW/setmode", "WINDOW/screensaver", "WINDOW/camouflage", "WINDOW/out_info"];

impl HeadlessPlayer {
    pub(crate) fn prepare_general_settings(&mut self) -> Result<()> {
        //49153a ->483a20 persists before attempting even a missing plugin.
        self.with_host(|host| host.persist_configuration())?;
        let binary = self.scan.engine.engine_binaries.iter().find(|binary|
            binary.relative_path.file_name().is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("cs2confx.dll")))
            .context("40057 has no inspected configuration-plugin resource")?;
        if binary.sha256 != PLUGIN_SHA { bail!("40057 configuration-plugin layout is not inspected"); }
        if binary.relative_path.is_absolute() || binary.relative_path.components().any(|part|
            !matches!(part, std::path::Component::Normal(_))) { bail!("configuration-plugin resource escapes game root"); }
        let source = UiResources::read(&self.scan.root.join(&binary.relative_path))?;
        let (title, controls) = source.dialog(107)?;
        let label = |id| -> Result<String> {
            Ok(self.resolve_ui_label(&controls.iter().find(|(command, _)| *command == id)
                .context("General107 lacks an inspected control")?.1))
        };
        let current = FIELDS.map(|field| self.data_services.configuration.platform_value(field).unwrap());
        let defaults = self.data_services.configuration.general_defaults();
        let mut fields = vec![HostSettingControl { command: 1001, label: String::new(),
            kind: HostSettingKind::Choice([1001, 1002, 1003].into_iter().map(label).collect::<Result<_>>()?),
            value: current[0], default: 0, hidden: false, disabled: false,
            enabled_by: None, reset_changes_value: true }];
        for (index, command) in [1006, 1007, 1008].into_iter().enumerate() {
            fields.push(HostSettingControl { command, label: label(command)?, kind: HostSettingKind::Toggle,
                value: u32::from(current[index + 1] != 0), default: u32::from(defaults[index + 1] != 0), hidden: false, disabled: false,
                enabled_by: None, reset_changes_value: true });
        }
        let title = self.resolve_ui_label(&title);
        let page = HostSettingsPage { resource: 107, title: title.clone(), reset_label: label(1102)?,
            controls: fields, input: None, font: None, sound: None };
        let position = self.prepare_plugin_position_page(&source)?;
        let image = self.prepare_plugin_image_page(&source)?;
        let sound = self.prepare_plugin_sound_page(&source)?;
        let request = HostEffectSettings { id: self.next_ui_id()?, title, pages: vec![page, position, image, sound] };
        self.data_services.window_control.ui.effects = Some(request);
        self.data_services.window_control.ui.effects_applied = false;
        self.data_services.window_control.ui.general = true;
        Ok(())
    }

    /// Packed presentation policy, read directly from the shared settings.
    pub fn host_general_window_policy(&self) -> u32 {
        u32::from(self.data_services.configuration.platform_value(FIELDS[1]).unwrap_or(0) != 0)
            | (u32::from(self.data_services.configuration.platform_value(FIELDS[2]).unwrap_or(0) != 0) << 1)
    }

    pub fn host_general_camouflage_title(&mut self) -> Result<String> {
        if let Some(title) = &self.data_services.window_control.ui.camouflage_title { return Ok(title.clone()); }
        let source = self.original_ui_resources()?;
        let title = self.resolve_ui_label(&source.wide_label("$0000-0045$")?);
        self.data_services.window_control.ui.camouflage_title = Some(title.clone());
        Ok(title)
    }

    /// Resolve remembered/forced mode before publishing the actual initial
    /// window mode. Setmode changes only affect this next-start operation.
    pub fn initialize_general_window_host(&mut self, actual_windowed: u32) -> Result<()> {
        self.with_host(|host| host.ensure_configuration_state())?;
        self.data_services.configuration.initialize_sound_focus_policy();
        let config = &self.data_services.configuration;
        let windowed = match config.platform_value(FIELDS[0]).unwrap_or(0) {
            0 => u32::from(config.platform_value("WINDOW/mode").unwrap_or(0) != 1),
            1 => 0, 2 => 1,
            _ => bail!("startup window mode is outside the inspected0..2 domain"),
        };
        self.set_host_windowed_mode(actual_windowed);
        if windowed != actual_windowed { self.queue_menu_window_mode(windowed)?; }
        if self.host_general_window_policy() & 2 != 0 { self.host_general_camouflage_title()?; }
        if self.data_services.configuration.platform_value(FIELDS[3]).unwrap_or(1) != 0 {
            let [width, height] = self.logical_size();
            let binary = self.scan.engine.primary_executable.as_ref();
            // Portable-host facts, not invented DirectX/device capabilities.
            // Output remains in the same private root as setup.xml.
            let info = format!("CatSystem2 portable host\r\nplatform : {} {}\r\nengine : {}\r\nengine SHA256 : {}\r\nlogical screen : {width} x {height}\r\nstartup screen : {}\r\n",
                std::env::consts::OS, std::env::consts::ARCH,
                binary.map_or_else(String::new, |binary| binary.relative_path.display().to_string()),
                binary.map_or("", |binary| binary.sha256.as_str()),
                self.data_services.configuration.platform_value(FIELDS[0]).unwrap_or(0));
            self.with_host(|host| { host.write_general_information(info.as_bytes()); Ok(()) })?;
        }
        Ok(())
    }
}
