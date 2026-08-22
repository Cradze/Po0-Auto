use std::{
    collections::{BTreeMap, HashSet},
    env,
    net::Ipv4Addr,
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local, TimeDelta, Utc};
use po0_auto::{
    config::{Config, ServerConfig},
    http::ApiClient,
    model::{AppState, ChangeRecord, History, PendingAttempt, PendingNotification, PendingSync},
    scheduler,
    storage::{load_history, load_state, save_history, save_state},
};
use tokio::{
    signal::unix::{SignalKind, signal},
    time::sleep,
};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

struct Cli {
    config_path: PathBuf,
    check_config: bool,
}

struct Application {
    config: Config,
    state: AppState,
    history: History,
    api: ApiClient,
    next_check_at: DateTime<Utc>,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_logging();
    let cli = parse_cli()?;
    let config = Config::load(&cli.config_path)?;
    if cli.check_config {
        println!("配置有效: {}", cli.config_path.display());
        return Ok(());
    }

    let state = load_state(&config.storage.state_file)?;
    let history = load_history(&config.storage.history_file)?;
    validate_data_versions(&state, &history)?;
    let api = ApiClient::new(&config)?;
    let mut app = Application {
        config,
        state,
        history,
        api,
        next_check_at: Utc::now(),
    };
    app.normalize_versions();
    app.persist_all()?;
    app.run().await
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

fn parse_cli() -> Result<Cli> {
    let mut config_path = PathBuf::from("/etc/po0-auto/config.yaml");
    let mut check_config = false;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--config" | "-c" => {
                let value = arguments.next().context("--config 后缺少文件路径")?;
                config_path = PathBuf::from(value);
            }
            "--check-config" => check_config = true,
            "--help" | "-h" => {
                println!(
                    "po0-auto {}\n\n用法: po0-auto [--config PATH] [--check-config]",
                    env!("CARGO_PKG_VERSION")
                );
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("po0-auto {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            _ => bail!("未知参数: {argument}"),
        }
    }
    Ok(Cli {
        config_path,
        check_config,
    })
}

fn validate_data_versions(state: &AppState, history: &History) -> Result<()> {
    if state.version > 1 {
        bail!("state.json 版本 {} 高于当前程序支持的版本 1", state.version);
    }
    if history.version > 1 {
        bail!(
            "history.json 版本 {} 高于当前程序支持的版本 1",
            history.version
        );
    }
    Ok(())
}

impl Application {
    fn normalize_versions(&mut self) {
        self.state.version = 1;
        self.history.version = 1;
    }

    async fn run(&mut self) -> Result<()> {
        let mut terminate = signal(SignalKind::terminate()).context("无法监听 SIGTERM")?;
        let mut interrupt = signal(SignalKind::interrupt()).context("无法监听 SIGINT")?;
        info!("po0-auto 已启动");

        loop {
            self.run_due_work().await?;
            let wake_at = self.next_wake_at();
            let wait = duration_until(wake_at).max(Duration::from_millis(50));
            tokio::select! {
                _ = sleep(wait) => {}
                _ = terminate.recv() => {
                    info!("收到 SIGTERM，正在退出");
                    break;
                }
                _ = interrupt.recv() => {
                    info!("收到 SIGINT，正在退出");
                    break;
                }
            }
        }
        self.persist_all()?;
        Ok(())
    }

    async fn run_due_work(&mut self) -> Result<()> {
        let now = Utc::now();
        if now >= self.next_check_at {
            self.check_ip(now).await;
            let scheduled_at = Utc::now();
            let interval = scheduler::check_interval(
                scheduled_at,
                self.state.current_since,
                self.state.typical_change_seconds,
                &self.config.detection,
            );
            self.next_check_at = add_duration(scheduled_at, interval);
            self.persist_all()?;
        }

        self.process_syncs().await?;
        self.process_notification().await?;
        Ok(())
    }

    async fn check_ip(&mut self, detected_at: DateTime<Utc>) {
        self.state.last_check_at = Some(detected_at);
        let removed = scheduler::prune_history(
            &mut self.history,
            detected_at,
            self.config.detection.retention_days,
        );
        if removed {
            self.recalculate_typical_period();
        }

        match self.api.public_ipv4(&self.config.ip_sources).await {
            Ok((ipv4, source)) => {
                info!(ipv4 = %ipv4, source = %source, "公网 IPv4 检测成功");
                self.accept_observation(ipv4, detected_at);
            }
            Err(error) => {
                warn!(error = %error, "本轮无法获取公网 IPv4");
            }
        }
    }

    fn accept_observation(&mut self, ipv4: Ipv4Addr, detected_at: DateTime<Utc>) {
        match self.state.current_ipv4 {
            None => {
                info!(ipv4 = %ipv4, "首次检测到公网 IPv4，将立即同步白名单");
                self.state.current_ipv4 = Some(ipv4);
                self.state.current_since = Some(detected_at);
                self.state.pending_notification = None;
            }
            Some(old_ipv4) if old_ipv4 != ipv4 => {
                let interval_seconds = self
                    .state
                    .current_since
                    .map(|since| {
                        detected_at
                            .signed_duration_since(since)
                            .num_seconds()
                            .max(0) as u64
                    })
                    .unwrap_or(0);
                info!(old_ipv4 = %old_ipv4, new_ipv4 = %ipv4, "检测到公网 IPv4 变化");
                self.history.records.push(ChangeRecord {
                    old_ipv4,
                    new_ipv4: ipv4,
                    detected_at,
                    interval_seconds,
                });
                self.state.current_ipv4 = Some(ipv4);
                self.state.current_since = Some(detected_at);
                self.state.pending_notification = None;
                scheduler::prune_history(
                    &mut self.history,
                    detected_at,
                    self.config.detection.retention_days,
                );
                self.recalculate_typical_period();
            }
            Some(_) => {}
        }
        self.reconcile_servers(ipv4, detected_at);
    }

    fn recalculate_typical_period(&mut self) {
        self.state.typical_change_seconds = scheduler::typical_change_seconds(
            &self.history,
            self.config.detection.history_sample_size,
        );
    }

    fn reconcile_servers(&mut self, ipv4: Ipv4Addr, detected_at: DateTime<Utc>) {
        let configured: HashSet<&str> = self
            .config
            .servers
            .iter()
            .map(|server| server.name.as_str())
            .collect();
        self.state
            .synced_servers
            .retain(|name, _| configured.contains(name.as_str()));

        let replace_pending = self
            .state
            .pending_sync
            .as_ref()
            .is_some_and(|pending| pending.ipv4 != ipv4);
        if replace_pending {
            warn!(ipv4 = %ipv4, "公网 IP 再次变化，旧的待同步任务已被最新 IP 替代");
            self.state.pending_sync = None;
        }

        let mut missing = BTreeMap::new();
        for server in &self.config.servers {
            let already_synced = self.state.synced_servers.get(&server.name) == Some(&ipv4);
            let already_pending = self
                .state
                .pending_sync
                .as_ref()
                .is_some_and(|pending| pending.servers.contains_key(&server.name));
            if !already_synced && !already_pending {
                missing.insert(
                    server.name.clone(),
                    PendingAttempt {
                        attempts: 0,
                        next_retry_at: detected_at,
                        last_error: None,
                    },
                );
            }
        }

        if missing.is_empty() {
            return;
        }
        let pending = self.state.pending_sync.get_or_insert_with(|| PendingSync {
            ipv4,
            detected_at,
            servers: BTreeMap::new(),
        });
        pending.servers.extend(missing);
    }

    async fn process_syncs(&mut self) -> Result<()> {
        let now = Utc::now();
        let due_names: Vec<String> = self
            .state
            .pending_sync
            .as_ref()
            .map(|pending| {
                pending
                    .servers
                    .iter()
                    .filter(|(_, attempt)| attempt.next_retry_at <= now)
                    .map(|(name, _)| name.clone())
                    .collect()
            })
            .unwrap_or_default();

        for name in due_names {
            let Some(server) = self
                .config
                .servers
                .iter()
                .find(|server| server.name == name)
                .cloned()
            else {
                continue;
            };
            self.sync_one_server(&server).await;
            save_state(&self.config.storage.state_file, &self.state)?;
        }

        let completed = self
            .state
            .pending_sync
            .as_ref()
            .is_some_and(|pending| pending.servers.is_empty());
        if completed {
            let pending = self.state.pending_sync.take().expect("pending_sync 已检查");
            info!(ipv4 = %pending.ipv4, "所有代理服务器白名单均已更新");
            if self.config.telegram.enabled {
                self.state.pending_notification = Some(PendingNotification {
                    ipv4: pending.ipv4,
                    detected_at: pending.detected_at,
                    message: notification_message(
                        pending.ipv4,
                        pending.detected_at,
                        self.state.typical_change_seconds,
                    ),
                    attempts: 0,
                    next_retry_at: Utc::now(),
                    last_error: None,
                });
            }
            save_state(&self.config.storage.state_file, &self.state)?;
        }
        Ok(())
    }

    async fn sync_one_server(&mut self, server: &ServerConfig) {
        let Some(target_ipv4) = self.state.pending_sync.as_ref().map(|pending| pending.ipv4) else {
            return;
        };
        let timeout = Duration::from_secs(self.config.firewall.timeout_seconds);
        match self
            .api
            .update_whitelist(&self.config.firewall.base_url, timeout, server)
            .await
        {
            Ok(()) => {
                info!(server = %server.name, ipv4 = %target_ipv4, "白名单更新成功");
                self.state
                    .synced_servers
                    .insert(server.name.clone(), target_ipv4);
                if let Some(pending) = self.state.pending_sync.as_mut() {
                    pending.servers.remove(&server.name);
                }
            }
            Err(error) => {
                let Some(attempt) = self
                    .state
                    .pending_sync
                    .as_mut()
                    .and_then(|pending| pending.servers.get_mut(&server.name))
                else {
                    return;
                };
                attempt.attempts = attempt.attempts.saturating_add(1);
                let delay = scheduler::retry_delay(
                    attempt.attempts,
                    self.config.detection.retry_initial_seconds,
                    self.config.detection.retry_max_seconds,
                );
                attempt.next_retry_at = add_duration(Utc::now(), delay);
                attempt.last_error = Some(error.to_string());
                warn!(server = %server.name, retry_seconds = delay.as_secs(), error = %error, "白名单更新失败，将重试");
            }
        }
    }

    async fn process_notification(&mut self) -> Result<()> {
        let Some(notification) = self.state.pending_notification.as_ref() else {
            return Ok(());
        };
        if notification.next_retry_at > Utc::now() {
            return Ok(());
        }
        let message = notification.message.clone();
        match self
            .api
            .send_telegram(&self.config.telegram, &message)
            .await
        {
            Ok(()) => {
                info!("Telegram 通知发送成功");
                self.state.pending_notification = None;
            }
            Err(error) => {
                let notification = self
                    .state
                    .pending_notification
                    .as_mut()
                    .expect("pending_notification 已检查");
                notification.attempts = notification.attempts.saturating_add(1);
                let delay = scheduler::retry_delay(
                    notification.attempts,
                    self.config.detection.retry_initial_seconds,
                    self.config.detection.retry_max_seconds,
                );
                notification.next_retry_at = add_duration(Utc::now(), delay);
                notification.last_error = Some(error.to_string());
                warn!(retry_seconds = delay.as_secs(), error = %error, "Telegram 通知失败，将重试");
            }
        }
        save_state(&self.config.storage.state_file, &self.state)
    }

    fn next_wake_at(&self) -> DateTime<Utc> {
        let mut next = self.next_check_at;
        if let Some(pending) = &self.state.pending_sync
            && let Some(retry) = scheduler::next_pending_retry(pending.servers.values())
        {
            next = next.min(retry);
        }
        if let Some(notification) = &self.state.pending_notification {
            next = next.min(notification.next_retry_at);
        }
        next
    }

    fn persist_all(&self) -> Result<()> {
        save_history(&self.config.storage.history_file, &self.history)?;
        save_state(&self.config.storage.state_file, &self.state)
    }
}

fn add_duration(at: DateTime<Utc>, duration: Duration) -> DateTime<Utc> {
    at + TimeDelta::from_std(duration).unwrap_or(TimeDelta::MAX)
}

fn duration_until(at: DateTime<Utc>) -> Duration {
    at.signed_duration_since(Utc::now())
        .to_std()
        .unwrap_or(Duration::ZERO)
}

fn notification_message(
    ipv4: Ipv4Addr,
    detected_at: DateTime<Utc>,
    typical_seconds: Option<u64>,
) -> String {
    let local = detected_at.with_timezone(&Local);
    format!(
        "【来自ImmortalWrt的消息】\n\n监测到宽带IPv4地址变更为：\n\n{ipv4}\n\n检测时间：\n\n{}\n\n已成功更新代理服务器白名单\n\n当前IP地址典型变化周期：\n\n{}",
        local.format("%Y年%-m月%-d日 %H:%M"),
        scheduler::format_period(typical_seconds)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_application() -> Application {
        let config = Config {
            servers: vec![
                ServerConfig {
                    name: "one".to_owned(),
                    token: "token-one".to_owned(),
                },
                ServerConfig {
                    name: "two".to_owned(),
                    token: "token-two".to_owned(),
                },
            ],
            ..Config::default()
        };
        let api = ApiClient::new(&config).unwrap();
        Application {
            config,
            state: AppState::new(),
            history: History::new(),
            api,
            next_check_at: Utc::now(),
        }
    }

    #[test]
    fn notification_contains_required_fields() {
        let detected = DateTime::parse_from_rfc3339("2026-08-22T19:30:00+08:00")
            .unwrap()
            .with_timezone(&Utc);
        let message = notification_message(Ipv4Addr::new(8, 8, 8, 8), detected, Some(3660));
        assert!(message.contains("8.8.8.8"));
        assert!(message.contains("已成功更新代理服务器白名单"));
        assert!(message.contains("1小时1分"));
    }

    #[test]
    fn first_observation_syncs_all_and_change_replaces_pending_target() {
        let mut app = test_application();
        let first_at = DateTime::parse_from_rfc3339("2026-08-22T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let first_ip = Ipv4Addr::new(8, 8, 8, 8);
        app.accept_observation(first_ip, first_at);
        assert_eq!(app.state.current_ipv4, Some(first_ip));
        assert_eq!(app.history.records.len(), 0);
        assert_eq!(app.state.pending_sync.as_ref().unwrap().servers.len(), 2);

        let second_ip = Ipv4Addr::new(1, 1, 1, 1);
        app.accept_observation(second_ip, first_at + TimeDelta::hours(6));
        assert_eq!(app.state.current_ipv4, Some(second_ip));
        assert_eq!(app.history.records.len(), 1);
        assert_eq!(app.history.records[0].interval_seconds, 6 * 3600);
        assert_eq!(app.state.typical_change_seconds, Some(6 * 3600));
        let pending = app.state.pending_sync.as_ref().unwrap();
        assert_eq!(pending.ipv4, second_ip);
        assert_eq!(pending.servers.len(), 2);
    }
}
