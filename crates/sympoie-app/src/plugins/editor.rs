use std::sync::Arc;

use vst3_host::{PluginWindow, PluginWindowOwner};

use super::{PluginRuntime, Registry, SharedPlugin, poisoned};

pub struct PluginEditor {
    id: u64,
    plugin: SharedPlugin,
    registry: Registry,
    window: PluginWindow,
}

impl PluginEditor {
    pub fn open(
        runtime: &PluginRuntime,
        id: u64,
        owner: Option<&PluginWindowOwner>,
    ) -> Result<Self, String> {
        let (plugin, _) = runtime.handles(id)?;
        let mut window = PluginWindow::new(Arc::clone(&plugin));
        if let Some(owner) = owner {
            window.set_owner(owner.clone()).map_err(|e| e.to_string())?;
        }
        window.open().map_err(|e| e.to_string())?;
        Ok(Self {
            id,
            plugin,
            registry: Arc::clone(&runtime.live),
            window,
        })
    }

    pub fn is_open(&self) -> bool {
        self.window.is_open()
    }

    pub fn focus(&mut self) -> Result<(), String> {
        self.window.focus().map_err(|error| error.to_string())
    }

    pub fn set_visible(&mut self, visible: bool) -> Result<(), String> {
        self.window
            .set_visible(visible)
            .map_err(|error| error.to_string())
    }

    pub fn service(&mut self) -> Result<(), String> {
        let current = self
            .registry
            .lock()
            .map_err(poisoned)?
            .get(&self.id)
            .map(|entry| Arc::clone(&entry.plugin));
        if !current.is_some_and(|current| Arc::ptr_eq(&current, &self.plugin)) {
            self.close();
            return Err("The plugin instance was replaced; reopen its editor".into());
        }
        self.window
            .service_platform_events()
            .map_err(|e| e.to_string())
    }

    pub fn close(&mut self) {
        self.window.close();
    }
}

impl Drop for PluginEditor {
    fn drop(&mut self) {
        self.close();
    }
}
