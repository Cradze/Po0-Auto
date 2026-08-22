use std::{collections::HashSet, fs, path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use reqwest::Url;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub ip_sources: Vec<IpSourceConfig>,
    pub firewall: FirewallConfig,
    pub servers: Vec<ServerConfig>,
    pub telegram: TelegramConfig,
    pub detection: DetectionConfig,
    pub storage: StorageConfig,
    pub user_agent: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ip_sources: default_ip_sources(),
            firewall: FirewallConfig::default(),
            servers: Vec::new(),
            telegram: TelegramConfig::default(),
            detection: DetectionConfig::default(),
            storage: StorageConfig::default(),
            user_agent: format!("po0-auto/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

impl Config {
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("无法读取配置文件 {}", path.display()))?;
        let config: Self = serde_yaml::from_str(&raw)
            .with_context(|| format!("配置文件 {} 格式无效", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.ip_sources.is_empty() {
            bail!("ip_sources 至少需要配置一个接口");
        }
        let mut source_names = HashSet::new();
        for source in &self.ip_sources {
            if source.name.trim().is_empty() {
                bail!("公网 IP 接口名称不能为空");
            }
            if !source_names.insert(&source.name) {
                bail!("公网 IP 接口名称重复: {}", source.name);
            }
            validate_http_url(&source.url)
                .with_context(|| format!("公网 IP 接口 {} 的 URL 无效", source.name))?;
            if source.timeout_seconds == 0 {
                bail!("公网 IP 接口 {} 的 timeout_seconds 必须大于 0", source.name);
            }
        }

        validate_http_url(&self.firewall.base_url).context("firewall.base_url 无效")?;
        if self.firewall.timeout_seconds == 0 {
            bail!("firewall.timeout_seconds 必须大于 0");
        }

        if self.servers.is_empty() {
            bail!("servers 至少需要配置一台代理服务器");
        }
        let mut names = HashSet::new();
        for server in &self.servers {
            if server.name.trim().is_empty() || server.token.trim().is_empty() {
                bail!("服务器名称和 token 均不能为空");
            }
            if !names.insert(&server.name) {
                bail!("服务器名称重复: {}", server.name);
            }
        }

        if self.telegram.enabled {
            if self.telegram.bot_token.trim().is_empty() || self.telegram.chat_id.trim().is_empty()
            {
                bail!("启用 Telegram 时必须配置 bot_token 和 chat_id");
            }
            validate_http_url(&self.telegram.api_base).context("telegram.api_base 无效")?;
            if self.telegram.timeout_seconds == 0 {
                bail!("telegram.timeout_seconds 必须大于 0");
            }
        }

        let detection = &self.detection;
        if detection.fast_interval_seconds == 0
            || detection.slow_interval_seconds == 0
            || detection.history_sample_size == 0
            || detection.retention_days == 0
            || detection.retry_initial_seconds == 0
            || detection.retry_max_seconds == 0
        {
            bail!("检测周期、保留周期、样本数和重试周期必须大于 0");
        }
        if !(1..=100).contains(&detection.slow_until_percent) {
            bail!("detection.slow_until_percent 必须在 1 到 100 之间");
        }
        if detection.retry_initial_seconds > detection.retry_max_seconds {
            bail!("重试初始间隔不能大于最大重试间隔");
        }
        const MAX_INTERVAL_SECONDS: u64 = 365 * 24 * 60 * 60;
        if detection.fast_interval_seconds > MAX_INTERVAL_SECONDS
            || detection.slow_interval_seconds > MAX_INTERVAL_SECONDS
            || detection.retry_initial_seconds > MAX_INTERVAL_SECONDS
            || detection.retry_max_seconds > MAX_INTERVAL_SECONDS
            || detection.retention_days > 36_500
            || detection.history_sample_size > 10_000
        {
            bail!("检测配置超出安全上限");
        }
        if self.storage.state_file == self.storage.history_file {
            bail!("state_file 和 history_file 不能是同一文件");
        }
        if self.user_agent.trim().is_empty() {
            bail!("user_agent 不能为空");
        }
        Ok(())
    }
}

fn validate_http_url(value: &str) -> Result<()> {
    let url = Url::parse(value)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        bail!("只支持带主机名的 http/https URL");
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct IpSourceConfig {
    pub name: String,
    pub url: String,
    pub response: IpResponseKind,
    pub timeout_seconds: u64,
}

impl Default for IpSourceConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            url: String::new(),
            response: IpResponseKind::PlainText,
            timeout_seconds: 8,
        }
    }
}

impl IpSourceConfig {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds)
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpResponseKind {
    BilibiliJson,
    PlainText,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FirewallConfig {
    pub base_url: String,
    pub timeout_seconds: u64,
}

impl Default for FirewallConfig {
    fn default() -> Self {
        Self {
            base_url: "https://console.po0.io/modules/servers/penguin/api/firewall.php".to_owned(),
            timeout_seconds: 15,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub name: String,
    pub token: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TelegramConfig {
    pub enabled: bool,
    pub api_base: String,
    pub bot_token: String,
    pub chat_id: String,
    pub timeout_seconds: u64,
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_base: "https://api.telegram.org".to_owned(),
            bot_token: String::new(),
            chat_id: String::new(),
            timeout_seconds: 15,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DetectionConfig {
    pub fast_interval_seconds: u64,
    pub slow_interval_seconds: u64,
    pub slow_until_percent: u8,
    pub history_sample_size: usize,
    pub retention_days: u64,
    pub retry_initial_seconds: u64,
    pub retry_max_seconds: u64,
}

impl Default for DetectionConfig {
    fn default() -> Self {
        Self {
            fast_interval_seconds: 5 * 60,
            slow_interval_seconds: 30 * 60,
            slow_until_percent: 75,
            history_sample_size: 10,
            retention_days: 7,
            retry_initial_seconds: 30,
            retry_max_seconds: 15 * 60,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    pub state_file: PathBuf,
    pub history_file: PathBuf,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            state_file: PathBuf::from("/var/lib/po0-auto/state.json"),
            history_file: PathBuf::from("/var/lib/po0-auto/history.json"),
        }
    }
}

fn default_ip_sources() -> Vec<IpSourceConfig> {
    [
        (
            "bilibili",
            "https://api.bilibili.com/x/web-interface/zone",
            IpResponseKind::BilibiliJson,
        ),
        ("ipinfo", "https://ipinfo.io/ip", IpResponseKind::PlainText),
        ("ident", "https://v4.ident.me", IpResponseKind::PlainText),
        (
            "aws-checkip",
            "https://checkip.amazonaws.com",
            IpResponseKind::PlainText,
        ),
        (
            "ifconfig",
            "https://ifconfig.me/ip",
            IpResponseKind::PlainText,
        ),
    ]
    .into_iter()
    .map(|(name, url, response)| IpSourceConfig {
        name: name.to_owned(),
        url: url.to_owned(),
        response,
        timeout_seconds: 8,
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_example_is_valid() {
        let config: Config = serde_yaml::from_str(include_str!("../config.example.yaml")).unwrap();
        config.validate().unwrap();
        assert_eq!(config.ip_sources.len(), 5);
        assert_eq!(config.detection.fast_interval_seconds, 300);
    }

    #[test]
    fn rejects_duplicate_server_names_and_enabled_empty_telegram() {
        let mut config = Config {
            servers: vec![
                ServerConfig {
                    name: "same".to_owned(),
                    token: "one".to_owned(),
                },
                ServerConfig {
                    name: "same".to_owned(),
                    token: "two".to_owned(),
                },
            ],
            ..Config::default()
        };
        assert!(config.validate().is_err());

        config.servers.truncate(1);
        config.telegram.enabled = true;
        assert!(config.validate().is_err());
    }
}
