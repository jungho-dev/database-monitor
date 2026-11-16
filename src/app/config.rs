//! config.rs
//! 감시 대상 DB 포트·호스트, 과조회·과작동 임계치, ~/.config/database-monitor/config.toml 설정 파일 로더.

use std::collections::HashMap;
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use anyhow::{Context, bail};
use serde::Deserialize;

pub const CONFIG_FILE_NAME: &str = "config.toml";
// TUI 상태 파일: 설정 파일 옆, 표시 언어·카드 배치·열 폭 저장
pub const TUI_STATE_FILE_NAME: &str = "tui.toml";
// 실행 이력 로그 폴더: 설정 폴더 안 logs (--config 로 다른 설정 파일을 고를 때도 항상 홈 기준)
pub const LOG_DIR_NAME: &str = "logs";
// 설정 폴더: <홈>/.config/database-monitor
const CONFIG_HOME_DIR: &str = ".config";
const CONFIG_APP_DIR: &str = "database-monitor";
// 클래식 콘솔(conhost) 실행 시 적용할 글꼴, Windows Terminal 은 프로필 글꼴 사용
pub const DEFAULT_CONSOLE_FONT: &str = "D2Coding ligature";

// 1. 감시 설정 구조체 ---------------------------------------------------------
#[derive(Debug, Clone)]
pub struct MonitorConfig {
    pub poll_interval_ms: u64,
    pub enable_pcap: bool,
    pub interface_hint: Option<String>,
    pub watched_ports: Vec<u16>,
    // 키는 정규화 IP (IPv4-mapped IPv6 는 IPv4), 소켓·프레임 판별마다 문자열 변환 없이 조회
    pub known_hosts: HashMap<IpAddr, String>,
    pub thresholds: RiskThresholds,
    pub console_font: String,
    pub source_path: Option<PathBuf>,
}

// 2. 위험도 임계치 ------------------------------------------------------------
// RX·QPS는 10초 평균, 반복은 10초 내 동일 핑거프린트 실행 수 기준
// 단계: caution 관찰(위험 아님) / warn 주의 / crit 위험(경보 카드·팝업·일괄 중단 대상)
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RiskThresholds {
    pub caution_rx_bps: u64,
    pub warn_rx_bps: u64,
    pub crit_rx_bps: u64,
    pub single_rx_warn_bytes: u64,
    pub caution_qps: u32,
    pub warn_qps: u32,
    pub crit_qps: u32,
    pub warn_repeat_10s: u32,
    pub crit_repeat_10s: u32,
    pub warn_estab_conns: usize,
    pub crit_estab_conns: usize,
    pub warn_churn_conns: usize,
}

// 3. 기본값 생성 --------------------------------------------------------------
impl Default for RiskThresholds {
    fn default() -> Self {
        Self {
            caution_rx_bps: 256 * 1024,
            warn_rx_bps: 1024 * 1024,
            crit_rx_bps: 4 * 1024 * 1024,
            single_rx_warn_bytes: 4 * 1024 * 1024,
            caution_qps: 30,
            warn_qps: 100,
            crit_qps: 300,
            warn_repeat_10s: 30,
            crit_repeat_10s: 100,
            warn_estab_conns: 35,
            crit_estab_conns: 70,
            warn_churn_conns: 30,
        }
    }
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            poll_interval_ms: 1000,
            enable_pcap: true,
            interface_hint: None,
            watched_ports: vec![3306, 33060, 5002, 6033, 5432, 1433, 1521, 6379, 27017],
            known_hosts: HashMap::new(),
            thresholds: RiskThresholds::default(),
            console_font: DEFAULT_CONSOLE_FONT.to_string(),
            source_path: None,
        }
    }
}

// 4. config.toml 파일 스키마 --------------------------------------------------
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigFile {
    poll_interval_ms: Option<u64>,
    enable_pcap: Option<bool>,
    interface: Option<String>,
    ports: Vec<u16>,
    hosts: HashMap<String, String>,
    thresholds: Option<RiskThresholds>,
    console_font: Option<String>,
}

// 5. 설정 파일 경로 탐색 ------------------------------------------------------
// 사용자 홈: USERPROFILE(Windows) 우선, 없으면 HOME
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

// 명시 경로가 있으면 그대로, 없으면 <홈>/.config/database-monitor/config.toml 이 파일일 때만
pub fn resolve_config_path(explicit_path: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit_path {
        return Some(path.to_path_buf());
    }
    let default_path = home?
        .join(CONFIG_HOME_DIR)
        .join(CONFIG_APP_DIR)
        .join(CONFIG_FILE_NAME);
    default_path.is_file().then_some(default_path)
}

// TUI 상태 파일 경로: <홈>/.config/database-monitor/tui.toml, 홈이 없으면 None
pub fn resolve_tui_state_path(home: Option<&Path>) -> Option<PathBuf> {
    Some(
        home?
            .join(CONFIG_HOME_DIR)
            .join(CONFIG_APP_DIR)
            .join(TUI_STATE_FILE_NAME),
    )
}

// 로그 폴더 경로: <홈>/.config/database-monitor/logs, 홈이 없으면 None (현재 폴더 대체 없이 로그 끈)
pub fn resolve_log_dir(home: Option<&Path>) -> Option<PathBuf> {
    Some(
        home?
            .join(CONFIG_HOME_DIR)
            .join(CONFIG_APP_DIR)
            .join(LOG_DIR_NAME),
    )
}

// 6. 설정 파일 로드 및 병합 ---------------------------------------------------
impl MonitorConfig {
    pub fn load_from_file(explicit_path: Option<&Path>) -> anyhow::Result<Self> {
        let mut cfg = Self::default();

        // 1. 설정 파일 탐색 (명시 경로 > ~/.config/database-monitor/config.toml)
        let home = home_dir();
        let Some(path) = resolve_config_path(explicit_path, home.as_deref()) else {
            return Ok(cfg);
        };

        // 2. TOML 파싱
        let text = fs::read_to_string(&path)
            .with_context(|| format!("Config file read failed: {}", path.display()))?;
        let file_cfg: ConfigFile = toml::from_str(&text)
            .with_context(|| format!("Config file format error: {}", path.display()))?;

        // 3. 기본값 위 병합
        if let Some(interval_ms) = file_cfg.poll_interval_ms {
            cfg.poll_interval_ms = interval_ms.clamp(250, 10_000);
        }
        if let Some(enable_pcap) = file_cfg.enable_pcap {
            cfg.enable_pcap = enable_pcap;
        }
        cfg.interface_hint = file_cfg.interface;
        for port in file_cfg.ports {
            if !cfg.watched_ports.contains(&port) {
                cfg.watched_ports.push(port);
            }
        }
        for (ip_text, alias) in file_cfg.hosts {
            let Ok(ip) = ip_text.trim().parse::<IpAddr>() else {
                bail!("Config hosts key is not an IP address: {ip_text} ({})", path.display());
            };
            cfg.known_hosts.insert(ip.to_canonical(), alias.trim().to_string());
        }
        if let Some(thresholds) = file_cfg.thresholds {
            cfg.thresholds = thresholds;
        }
        // 빈 문자열은 글꼴 변경 끔
        if let Some(console_font) = file_cfg.console_font {
            cfg.console_font = console_font.trim().to_string();
        }
        cfg.source_path = Some(std::path::absolute(&path).unwrap_or(path));
        Ok(cfg)
    }
}

// 7. 감시 대상 판별 -----------------------------------------------------------
impl MonitorConfig {
    pub fn is_watched_endpoint(&self, remote_ip: &IpAddr, remote_port: u16) -> bool {
        if self.watched_ports.contains(&remote_port) {
            return true;
        }
        remote_port != 443 && remote_port != 80 && self.known_hosts.contains_key(remote_ip)
    }

    pub fn endpoint_label(&self, remote_ip: &IpAddr, remote_port: u16) -> String {
        if let Some(alias) = self.known_hosts.get(remote_ip) {
            format!("{remote_ip}:{remote_port} [{alias}]")
        } else {
            format!("{remote_ip}:{remote_port}")
        }
    }
}

// 8. 로컬 통신 판별 -----------------------------------------------------------
pub fn is_local_endpoint(local_ip: &IpAddr, remote_ip: &IpAddr) -> bool {
    let remote = remote_ip.to_canonical();
    remote.is_loopback() || remote.is_unspecified() || remote == local_ip.to_canonical()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_file_parses_and_merges() {
        let example = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("docs")
            .join("config.example.toml");
        let cfg = MonitorConfig::load_from_file(Some(&example)).expect("example config");
        let dev_ip: IpAddr = "192.0.2.10".parse().unwrap();

        assert!(cfg.watched_ports.contains(&3306));
        assert!(cfg.watched_ports.contains(&3307));
        assert!(cfg.is_watched_endpoint(&dev_ip, 15000));
        assert!(!cfg.is_watched_endpoint(&dev_ip, 443));
        assert!(cfg.endpoint_label(&dev_ip, 3306).contains("Dev DB"));
        assert_eq!(cfg.thresholds.warn_qps, 100);
        assert_eq!(cfg.console_font, DEFAULT_CONSOLE_FONT);
        assert!(cfg.source_path.is_some());
    }

    #[test]
    fn mapped_ipv6_host_key_matches_ipv4_socket_and_font_can_be_disabled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(CONFIG_FILE_NAME);
        fs::write(&path, "console_font = \"\"\n[hosts]\n\"::ffff:192.0.2.40\" = \"매핑 DB\"\n")
            .expect("write");
        let cfg = MonitorConfig::load_from_file(Some(&path)).expect("config");
        let v4: IpAddr = "192.0.2.40".parse().unwrap();

        assert!(cfg.is_watched_endpoint(&v4, 15000));
        assert_eq!(cfg.endpoint_label(&v4, 15000), "192.0.2.40:15000 [매핑 DB]");
        assert!(cfg.console_font.is_empty());
    }

    #[test]
    fn invalid_host_key_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(CONFIG_FILE_NAME);
        fs::write(&path, "[hosts]\n\"db.internal\" = \"이름\"\n").expect("write");
        assert!(MonitorConfig::load_from_file(Some(&path)).is_err());
    }

    #[test]
    fn tui_state_path_sits_next_to_config_under_the_injected_home() {
        let home = tempfile::tempdir().expect("tempdir");
        let expected = home
            .path()
            .join(".config")
            .join("database-monitor")
            .join(TUI_STATE_FILE_NAME);
        assert_eq!(resolve_tui_state_path(Some(home.path())), Some(expected));
        assert_eq!(resolve_tui_state_path(None), None);
        assert_eq!(TUI_STATE_FILE_NAME, "tui.toml");
    }

    #[test]
    fn log_dir_sits_under_the_config_folder_of_the_injected_home_and_is_none_without_a_home() {
        let home = tempfile::tempdir().expect("tempdir");
        let expected = home
            .path()
            .join(".config")
            .join("database-monitor")
            .join("logs");
        assert_eq!(resolve_log_dir(Some(home.path())), Some(expected));
        assert_eq!(resolve_log_dir(None), None);
        assert_eq!(LOG_DIR_NAME, "logs");
    }

    #[test]
    fn default_path_resolves_under_injected_home() {
        let home = tempfile::tempdir().expect("tempdir");
        let config_dir = home.path().join(".config").join("database-monitor");

        // 파일이 없으면 None
        assert_eq!(resolve_config_path(None, Some(home.path())), None);
        assert_eq!(resolve_config_path(None, None), None);

        // 파일이 있으면 <홈>/.config/database-monitor/config.toml
        fs::create_dir_all(&config_dir).expect("mkdir");
        let expected = config_dir.join(CONFIG_FILE_NAME);
        fs::write(&expected, "poll_interval_ms = 2000\n").expect("write");
        assert_eq!(resolve_config_path(None, Some(home.path())), Some(expected.clone()));

        // 명시 경로는 홈과 무관하게 우선
        let explicit = home.path().join("other.toml");
        assert_eq!(
            resolve_config_path(Some(&explicit), Some(home.path())),
            Some(explicit)
        );
    }
}
