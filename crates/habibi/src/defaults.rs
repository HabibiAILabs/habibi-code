//! Frontend-owned user defaults. The library remains free of global configuration.
use anyhow::{Context, Result, ensure};
use libhabibi::{Preferences, Provider, SessionConfig};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Defaults {
    pub provider: Option<Provider>,
    models: BTreeMap<String, String>,
    preferences: Preferences,
    ollama_url: Option<String>,
}

pub(crate) fn path() -> Result<PathBuf> {
    Ok(crate::base_dir("XDG_CONFIG_HOME", ".config")?.join("habibi/settings.json"))
}

impl Defaults {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).with_context(|| format!("Read defaults {}", path.display())),
        };
        let value: Self = serde_json::from_slice(&bytes).with_context(|| {
            format!(
                "Invalid defaults {}; fix or remove this file",
                path.display()
            )
        })?;
        ensure!(
            value.models.values().all(|model| !model.trim().is_empty()),
            "Default model IDs must not be empty"
        );
        Ok(value)
    }

    pub fn apply(&self, config: &mut SessionConfig) {
        if let Some(model) = self.models.get(config.provider.name()) {
            config.model = model.clone();
        }
        config.preferences = self.preferences.clone();
        if config.provider == Provider::Ollama
            && let Some(url) = &self.ollama_url
        {
            config.ollama_url = url.clone();
        }
    }

    pub fn remember(&mut self, config: &SessionConfig, model_only: bool) {
        self.provider = Some(config.provider);
        if config.provider == Provider::Ollama {
            self.ollama_url = Some(config.ollama_url.clone());
        }
        self.models
            .insert(config.provider.name().into(), config.model.clone());
        // Changing only the model must not restore old permission defaults from a resumed session.
        if !model_only {
            self.preferences = config.preferences.clone();
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let parent = path.parent().context("Defaults path needs a parent")?;
        fs::create_dir_all(parent)?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let temp = parent.join(format!(".settings-{}-{nonce}.tmp", std::process::id()));
        let result = (|| -> Result<()> {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            file.write_all(&serde_json::to_vec_pretty(self)?)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temp, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result.with_context(|| format!("Save user defaults {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_user_not_workspace_and_keep_models_per_provider() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/settings.json");
        let mut defaults = Defaults::load(&path).unwrap();
        let mut codex = SessionConfig::new(Provider::Codex, "/first");
        codex.model = "custom-codex".into();
        codex.preferences.yolo = true;
        defaults.remember(&codex, false);
        let mut claude = SessionConfig::new(Provider::Claude, "/second");
        claude.model = "custom-claude".into();
        defaults.remember(&claude, true);
        defaults.save(&path).unwrap();
        let defaults = Defaults::load(&path).unwrap();
        assert_eq!(defaults.provider, Some(Provider::Claude));
        for (provider, model) in [
            (Provider::Codex, "custom-codex"),
            (Provider::Claude, "custom-claude"),
        ] {
            let mut config = SessionConfig::new(provider, "/third");
            defaults.apply(&mut config);
            assert_eq!(config.model, model);
            assert!(config.preferences.yolo);
        }
        claude.preferences.yolo = false;
        defaults_disable_yolo(defaults, &claude, &path);
    }

    fn defaults_disable_yolo(mut defaults: Defaults, config: &SessionConfig, path: &Path) {
        defaults.remember(config, false);
        defaults.save(path).unwrap();
        let mut next = SessionConfig::new(Provider::Codex, "/fourth");
        Defaults::load(path).unwrap().apply(&mut next);
        assert!(!next.preferences.yolo);
    }

    #[test]
    fn missing_defaults_are_safe_and_saved_files_are_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let defaults = Defaults::load(&path).unwrap();
        let mut config = SessionConfig::new(Provider::Codex, dir.path());
        defaults.apply(&mut config);
        assert!(!config.preferences.yolo);
        assert_eq!(config.model, Provider::Codex.default_model());
        defaults.save(&path).unwrap();
        defaults.save(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn corrupt_defaults_are_not_silently_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, "invalid").unwrap();
        assert!(Defaults::load(&path).is_err());
        fs::write(&path, r#"{"models":{"codex":" "}}"#).unwrap();
        assert!(Defaults::load(&path).is_err());
    }
}
