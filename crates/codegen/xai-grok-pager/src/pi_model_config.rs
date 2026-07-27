//! Pi `models.json` compatibility layer for the native model manager.
//!
//! Pi remains the runtime owner. This module only provides a safe, typed edit
//! transaction for the file that Pi's `ModelRuntime` already reloads.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::pi_resource_config::resolve_pi_agent_dir;

const LOCK_RETRIES: usize = 50;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(20);
const STALE_LOCK_AGE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiModelsFile {
    #[serde(default)]
    pub providers: BTreeMap<String, PiProviderConfig>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiProviderConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_header: Option<bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub models: Vec<PiModelConfig>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiModelConfig {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<PiModelCost>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub thinking_level_map: BTreeMap<String, Option<String>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiModelCost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone)]
pub struct PiModelConfigSnapshot {
    pub path: PathBuf,
    pub document: PiModelsFile,
    baseline_document: PiModelsFile,
    baseline_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct PiModelSaveReport {
    pub path: PathBuf,
    pub backup: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct PiModelRestoreReport {
    pub restored_from: PathBuf,
    pub backup_of_replaced: Option<PathBuf>,
}

impl PiModelConfigSnapshot {
    pub fn load() -> Result<Self> {
        let path = resolve_pi_agent_dir()?.join("models.json");
        Self::load_from_path(path)
    }

    pub fn load_from_path(path: PathBuf) -> Result<Self> {
        ensure_regular_or_missing(&path)?;
        let baseline_bytes = read_optional(&path)?;
        let document = parse_document(baseline_bytes.as_deref(), &path)?;
        validate_document(&document)?;
        Ok(Self {
            path,
            baseline_document: document.clone(),
            document,
            baseline_bytes,
        })
    }

    pub fn is_dirty(&self) -> bool {
        self.document != self.baseline_document
    }

    pub fn reload_from_disk(&mut self) -> Result<()> {
        let fresh = Self::load_from_path(self.path.clone())?;
        *self = fresh;
        Ok(())
    }

    pub fn save(&mut self) -> Result<PiModelSaveReport> {
        validate_document(&self.document)?;
        let bytes = format!("{}\n", serde_json::to_string_pretty(&self.document)?).into_bytes();
        let parent = self
            .path
            .parent()
            .context("models.json must have a parent directory")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create Pi agent directory {}", parent.display()))?;
        ensure_regular_or_missing(&self.path)?;
        let _lock = DirectoryLock::acquire(self.path.with_extension("json.lock"))?;
        ensure_regular_or_missing(&self.path)?;
        let current = read_optional(&self.path)?;
        if current != self.baseline_bytes {
            bail!("models.json changed on disk after this editor opened; refresh before saving");
        }
        let backup = current
            .as_deref()
            .map(|raw| write_backup(parent, raw))
            .transpose()?;
        atomic_write_private(&self.path, &bytes)?;
        self.baseline_bytes = Some(bytes);
        self.baseline_document = self.document.clone();
        Ok(PiModelSaveReport {
            path: self.path.clone(),
            backup,
        })
    }

    pub fn latest_backup(&self) -> Result<Option<PathBuf>> {
        latest_backup_for(
            self.path
                .parent()
                .context("models.json must have a parent directory")?,
        )
    }

    pub fn restore_latest(&mut self) -> Result<PiModelRestoreReport> {
        let restored_from = self
            .latest_backup()?
            .context("no models.json backup is available")?;
        ensure_regular_or_missing(&restored_from)?;
        let raw = fs::read(&restored_from)
            .with_context(|| format!("failed to read backup {}", restored_from.display()))?;
        let restored = parse_document(Some(&raw), &restored_from)?;
        validate_document(&restored)?;
        self.document = restored;
        let report = self.save()?;
        Ok(PiModelRestoreReport {
            restored_from,
            backup_of_replaced: report.backup,
        })
    }
}

pub fn validate_document(document: &PiModelsFile) -> Result<()> {
    for (provider_name, provider) in &document.providers {
        if provider_name.trim().is_empty() {
            bail!("provider names must not be empty");
        }
        validate_optional_text(provider.base_url.as_deref(), provider_name, "baseUrl")?;
        validate_optional_text(provider.api.as_deref(), provider_name, "api")?;
        let mut model_ids = BTreeSet::new();
        for (index, model) in provider.models.iter().enumerate() {
            let label = format!("provider '{provider_name}' model #{}", index + 1);
            if model.id.trim().is_empty() {
                bail!("{label} has an empty id");
            }
            if model.name.trim().is_empty() {
                bail!("{label} ({}) has an empty name", model.id);
            }
            if !model_ids.insert(model.id.trim().to_owned()) {
                bail!(
                    "provider '{provider_name}' has duplicate model id '{}'",
                    model.id
                );
            }
            validate_optional_text(model.api.as_deref(), &label, "api")?;
            if model.context_window == Some(0) {
                bail!("{label} contextWindow must be greater than zero");
            }
            if model.max_tokens == Some(0) {
                bail!("{label} maxTokens must be greater than zero");
            }
            if let Some(cost) = &model.cost {
                for (name, value) in [
                    ("input", cost.input),
                    ("output", cost.output),
                    ("cacheRead", cost.cache_read),
                    ("cacheWrite", cost.cache_write),
                ] {
                    if !value.is_finite() || value < 0.0 {
                        bail!("{label} cost.{name} must be a finite non-negative number");
                    }
                }
            }
            if model.input.iter().any(|entry| entry.trim().is_empty()) {
                bail!("{label} input modalities must not contain empty values");
            }
            if model.thinking_level_map.iter().any(|(key, value)| {
                key.trim().is_empty()
                    || value
                        .as_deref()
                        .is_some_and(|value| value.trim().is_empty())
            }) {
                bail!("{label} thinkingLevelMap keys and string values must not be empty");
            }
        }
    }
    Ok(())
}

fn validate_optional_text(value: Option<&str>, owner: &str, field: &str) -> Result<()> {
    if value.is_some_and(|value| value.trim().is_empty()) {
        bail!("{owner} {field} must be omitted or non-empty");
    }
    Ok(())
}

fn parse_document(raw: Option<&[u8]>, path: &Path) -> Result<PiModelsFile> {
    let Some(raw) = raw else {
        return Ok(PiModelsFile::default());
    };
    let value: Value = serde_json::from_slice(raw)
        .with_context(|| format!("invalid models JSON {}", path.display()))?;
    let root = value
        .as_object()
        .with_context(|| format!("{} root must be an object", path.display()))?;
    if root
        .get("providers")
        .is_some_and(|value| !value.is_object())
    {
        bail!("{}.providers must be an object", path.display());
    }
    serde_json::from_value(value)
        .with_context(|| format!("invalid Pi model configuration {}", path.display()))
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn ensure_regular_or_missing(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("refusing to read or replace symlink {}", path.display())
        }
        Ok(metadata) if !metadata.is_file() => {
            bail!("expected a regular file at {}", path.display())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to inspect {}", path.display())),
    }
}

fn backup_dir(agent_dir: &Path) -> PathBuf {
    agent_dir.join("backups/models")
}

fn write_backup(agent_dir: &Path, raw: &[u8]) -> Result<PathBuf> {
    let dir = backup_dir(agent_dir);
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create model backup directory {}", dir.display()))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = dir.join(format!("models-{stamp}-{}.json", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options
        .open(&path)
        .with_context(|| format!("failed to create model backup {}", path.display()))?;
    set_private_permissions(&file)?;
    file.write_all(raw)
        .with_context(|| format!("failed to write model backup {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync model backup {}", path.display()))?;
    Ok(path)
}

fn latest_backup_for(agent_dir: &Path) -> Result<Option<PathBuf>> {
    let dir = backup_dir(agent_dir);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read model backups {}", dir.display()));
        }
    };
    let mut candidates = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("models-") && name.ends_with(".json"))
        })
        .collect::<Vec<_>>();
    candidates.sort();
    Ok(candidates.pop())
}

fn atomic_write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("models.json must have a parent")?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_path = parent.join(format!(".models.json.tmp-{}-{stamp}", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut temp = options
        .open(&temp_path)
        .with_context(|| format!("failed to create temporary file {}", temp_path.display()))?;
    let result = (|| -> Result<()> {
        set_private_permissions(&temp)?;
        temp.write_all(bytes).with_context(|| {
            format!(
                "failed to write temporary models file {}",
                temp_path.display()
            )
        })?;
        temp.sync_all().with_context(|| {
            format!(
                "failed to sync temporary models file {}",
                temp_path.display()
            )
        })?;
        drop(temp);
        atomic_replace(&temp_path, path)
            .with_context(|| format!("failed to atomically replace {}", path.display()))?;
        sync_directory(parent)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn set_private_permissions(file: &fs::File) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .context("failed to set private models.json permissions")
}

#[cfg(not(unix))]
fn set_private_permissions(_file: &fs::File) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("failed to sync directory {}", path.display()))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

struct DirectoryLock {
    path: PathBuf,
}

impl DirectoryLock {
    fn acquire(path: PathBuf) -> Result<Self> {
        for attempt in 0..LOCK_RETRIES {
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if lock_is_stale(&path) {
                        let _ = fs::remove_dir(&path);
                        continue;
                    }
                    if attempt + 1 < LOCK_RETRIES {
                        std::thread::sleep(LOCK_RETRY_DELAY);
                        continue;
                    }
                    bail!("timed out acquiring model config lock {}", path.display());
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to acquire lock {}", path.display()));
                }
            }
        }
        bail!("timed out acquiring model config lock {}", path.display())
    }
}

impl Drop for DirectoryLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.path);
    }
}

fn lock_is_stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age > STALE_LOCK_AGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str) -> PiModelConfig {
        PiModelConfig {
            id: id.to_owned(),
            name: id.to_owned(),
            input: vec!["text".to_owned()],
            context_window: Some(8_192),
            max_tokens: Some(2_048),
            ..PiModelConfig::default()
        }
    }

    fn snapshot(path: PathBuf) -> PiModelConfigSnapshot {
        PiModelConfigSnapshot::load_from_path(path).expect("load snapshot")
    }

    #[test]
    fn missing_file_opens_as_empty_document() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = snapshot(temp.path().join("models.json"));
        assert!(state.document.providers.is_empty());
        assert!(!state.is_dirty());
    }

    #[test]
    fn validation_rejects_duplicate_model_ids() {
        let mut document = PiModelsFile::default();
        document.providers.insert(
            "provider".to_owned(),
            PiProviderConfig {
                models: vec![model("same"), model("same")],
                ..PiProviderConfig::default()
            },
        );
        assert!(validate_document(&document).is_err());
    }

    #[test]
    fn save_creates_private_backup_and_detects_external_conflict() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("models.json");
        fs::write(&path, "{\"providers\":{}}\n").expect("seed");
        let mut state = snapshot(path.clone());
        state.document.providers.insert(
            "provider".to_owned(),
            PiProviderConfig {
                models: vec![model("one")],
                ..PiProviderConfig::default()
            },
        );
        let report = state.save().expect("save");
        assert!(report.backup.is_some_and(|path| path.exists()));
        assert!(!state.is_dirty());

        let mut stale = snapshot(path.clone());
        fs::write(&path, "{\"providers\":{\"external\":{\"models\":[]}}}\n")
            .expect("external edit");
        stale
            .document
            .providers
            .insert("local".to_owned(), PiProviderConfig::default());
        assert!(stale.save().is_err());
    }

    #[test]
    fn restore_latest_replaces_draft_and_backs_up_current_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("models.json");
        fs::write(&path, "{\"providers\":{}}\n").expect("seed");
        let mut state = snapshot(path.clone());
        state
            .document
            .providers
            .insert("first".to_owned(), PiProviderConfig::default());
        state.save().expect("first save");
        state.document.providers.clear();
        state
            .document
            .providers
            .insert("second".to_owned(), PiProviderConfig::default());
        state.save().expect("second save");
        let report = state.restore_latest().expect("restore");
        assert!(report.restored_from.exists());
        assert!(state.document.providers.contains_key("first"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_target_is_rejected() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("real.json");
        fs::write(&target, "{\"providers\":{}}\n").expect("target");
        let link = temp.path().join("models.json");
        symlink(&target, &link).expect("symlink");
        assert!(PiModelConfigSnapshot::load_from_path(link).is_err());
    }
}
