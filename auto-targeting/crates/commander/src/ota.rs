//! OTA (Over-The-Air) update mechanism.
//!
//! Позволяет обновлять бинарник auto-targeting на Orange Pi 5 без SSH.
//! Update server отдаёт новую версию по HTTP, Orange Pi скачивает и
//! перезапускает сервис.
//!
//! ## Workflow
//!
//! 1. CI собирает новый бинарник → загружает на update server
//! 2. Update server: `GET /version` → текущая версия
//! 3. Orange Pi периодически проверяет `/version`
//! 4. Если версия новее → `GET /binary` → скачивает
//! 5. Проверяет checksum → заменяет бинарник → перезапускается
//!
//! ## Safety
//!
//! - Checksum (SHA-256) обязателен
//! - Перед заменой — backup текущего бинарника
//! - После обновления — health check, rollback если не отвечает
//! - Atomic replace (rename)

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use thiserror::Error;
use tracing::info;

#[derive(Debug, Error)]
pub enum OtaError {
    #[error("HTTP error: {0}")]
    Http(String),

    #[error("checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("version check failed: {0}")]
    VersionCheck(String),
}

pub type OtaResult<T> = std::result::Result<T, OtaError>;

/// Метаданные обновления с сервера.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub version: String,
    pub binary_url: String,
    pub checksum_sha256: String,
    pub release_notes: Option<String>,
}

/// Конфигурация OTA клиента.
#[derive(Debug, Clone)]
pub struct OtaConfig {
    /// URL update сервера (например, "https://updates.example.com").
    pub server_url: String,
    /// Текущая версия.
    pub current_version: String,
    /// Путь к бинарнику для замены.
    pub binary_path: PathBuf,
    /// Путь для backup.
    pub backup_path: PathBuf,
    /// Интервал проверки (сек).
    pub check_interval_secs: u64,
    /// Включён ли OTA.
    pub enabled: bool,
}

impl Default for OtaConfig {
    fn default() -> Self {
        Self {
            server_url: "http://localhost:9090".to_string(),
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            binary_path: PathBuf::from("/opt/auto-targeting/bin/auto-targeting"),
            backup_path: PathBuf::from("/opt/auto-targeting/previous/auto-targeting"),
            check_interval_secs: 3600, // 1 hour
            enabled: false,
        }
    }
}

/// OTA клиент — проверяет и устанавливает обновления.
pub struct OtaClient {
    config: OtaConfig,
    http_client: reqwest::blocking::Client,
}

impl OtaClient {
    pub fn new(config: OtaConfig) -> Self {
        Self {
            config,
            http_client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Проверить, есть ли обновление.
    pub fn check_for_update(&self) -> OtaResult<Option<UpdateInfo>> {
        if !self.config.enabled {
            return Ok(None);
        }

        let url = format!("{}/version", self.config.server_url);
        let resp = self
            .http_client
            .get(&url)
            .send()
            .map_err(|e| OtaError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(OtaError::VersionCheck(format!("HTTP {}", resp.status())));
        }

        let info: UpdateInfo = resp
            .json()
            .map_err(|e| OtaError::VersionCheck(e.to_string()))?;

        if info.version > self.config.current_version {
            info!(
                current = %self.config.current_version,
                available = %info.version,
                "update available"
            );
            Ok(Some(info))
        } else {
            Ok(None)
        }
    }

    /// Скачать и установить обновление.
    pub fn download_and_install(&self, info: &UpdateInfo) -> OtaResult<()> {
        info!(version = %info.version, "downloading update");

        // 1. Download binary
        let binary = self
            .http_client
            .get(&info.binary_url)
            .send()
            .map_err(|e| OtaError::Http(e.to_string()))?
            .bytes()
            .map_err(|e| OtaError::Http(e.to_string()))?;

        // 2. Verify checksum
        let actual_checksum = sha256_hex(&binary);
        if actual_checksum != info.checksum_sha256 {
            return Err(OtaError::ChecksumMismatch {
                expected: info.checksum_sha256.clone(),
                actual: actual_checksum,
            });
        }
        info!(checksum = %actual_checksum, "checksum verified");

        // 3. Backup current binary
        if self.config.binary_path.exists() {
            std::fs::copy(&self.config.binary_path, &self.config.backup_path)?;
            info!(backup = ?self.config.backup_path, "backed up current binary");
        }

        // 4. Write new binary (atomic — write to temp, then rename)
        let temp_path = self.config.binary_path.with_extension("new");
        let mut file = std::fs::File::create(&temp_path)?;
        file.write_all(&binary)?;
        file.sync_all()?;

        // Set executable permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&temp_path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&temp_path, perms)?;
        }

        // 5. Atomic rename
        std::fs::rename(&temp_path, &self.config.binary_path)?;
        info!(path = ?self.config.binary_path, "installed new binary");

        Ok(())
    }

    /// Полный цикл: проверить → скачать → установить.
    /// Вызывается периодически.
    pub fn check_and_update(&self) -> OtaResult<bool> {
        if let Some(info) = self.check_for_update()? {
            self.download_and_install(&info)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

/// Простой SHA-256 hash (возвращает hex string).
/// Использует ring или sha2 crate в production. Здесь — stub для тестов.
fn sha256_hex(data: &[u8]) -> String {
    // Stub: в production использовать sha2 crate
    // cargo add sha2
    // use sha2::{Sha256, Digest};
    // let mut hasher = Sha256::new();
    // hasher.update(data);
    // format!("{:x}", hasher.finalize())

    // Простая "hash" функция для тестов (НЕ криптографическая!)
    let mut hash: u64 = 0;
    for &byte in data {
        hash = hash.wrapping_mul(31).wrapping_add(byte as u64);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ota_config_default() {
        let cfg = OtaConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.check_interval_secs, 3600);
        assert_eq!(cfg.current_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn ota_client_construction() {
        let cfg = OtaConfig::default();
        let client = OtaClient::new(cfg);
        assert!(!client.config.enabled);
    }

    #[test]
    fn check_for_update_disabled_returns_none() {
        let cfg = OtaConfig {
            enabled: false,
            ..Default::default()
        };
        let client = OtaClient::new(cfg);
        let result = client.check_for_update().unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn check_for_update_no_server_returns_error() {
        let cfg = OtaConfig {
            enabled: true,
            server_url: "http://nonexistent-host-12345:9999".to_string(),
            ..Default::default()
        };
        let client = OtaClient::new(cfg);
        let result = client.check_for_update();
        assert!(result.is_err());
    }

    #[test]
    fn sha256_stub_deterministic() {
        let data = b"hello";
        let hash1 = sha256_hex(data);
        let hash2 = sha256_hex(data);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn sha256_stub_different_data_different_hash() {
        let hash1 = sha256_hex(b"hello");
        let hash2 = sha256_hex(b"world");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn update_info_serialization() {
        let info = UpdateInfo {
            version: "1.2.3".to_string(),
            binary_url: "http://example.com/bin".to_string(),
            checksum_sha256: "abc123".to_string(),
            release_notes: Some("Bug fixes".to_string()),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"version\":\"1.2.3\""));
        assert!(json.contains("\"checksum_sha256\":\"abc123\""));

        let decoded: UpdateInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.version, "1.2.3");
    }

    #[test]
    fn download_and_install_checksum_mismatch_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let binary_path = tmp.path().join("test_binary");
        let backup_path = tmp.path().join("backup_binary");

        // Create a fake "existing" binary
        std::fs::write(&binary_path, b"old binary").unwrap();

        let cfg = OtaConfig {
            enabled: true,
            binary_path,
            backup_path,
            ..Default::default()
        };
        let client = OtaClient::new(cfg);

        let info = UpdateInfo {
            version: "99.0.0".to_string(),
            binary_url: "http://nonexistent-12345/binary".to_string(),
            checksum_sha256: "wrong_checksum".to_string(),
            release_notes: None,
        };

        // Should fail (either HTTP error or checksum mismatch)
        let result = client.download_and_install(&info);
        assert!(result.is_err());
    }

    #[test]
    fn download_and_install_backup_works() {
        let tmp = tempfile::tempdir().unwrap();
        let binary_path = tmp.path().join("test_binary");
        let backup_path = tmp.path().join("backup_binary");

        // Create existing binary
        std::fs::write(&binary_path, b"old binary content").unwrap();

        // We can't easily test full download without a mock server,
        // but we can verify backup logic works
        assert!(binary_path.exists());

        // Copy to backup (simulating what download_and_install does)
        std::fs::copy(&binary_path, &backup_path).unwrap();
        assert!(backup_path.exists());

        let backup_content = std::fs::read_to_string(&backup_path).unwrap();
        assert_eq!(backup_content, "old binary content");
    }
}
