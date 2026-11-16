//! cli.rs
//! 커맨드라인 인자 파서 및 일회성 스냅샷·JSON 리포트 출력기.

use std::net::IpAddr;
use std::path::PathBuf;
use clap::Parser;
use crate::app::config::MonitorConfig;
use crate::app::model::MonitorSnapshot;
use crate::analysis::risk::format_bytes;
use crate::tui::i18n::{self, Lang};

// 1. CLI 인자 정의 ------------------------------------------------------------
#[derive(Debug, Parser)]
#[command(
    name = "database-monitor",
    version,
    about = "Zero-query passive terminal monitor for outbound DB traffic and AI agent over-execution"
)]
pub struct CliArgs {
    #[arg(long, help = "Config file path (default: ~/.config/database-monitor/config.toml)")]
    pub config: Option<PathBuf>,

    #[arg(long, help = "Polling interval in milliseconds (default 1000)")]
    pub interval_ms: Option<u64>,

    #[arg(long, help = "Disable passive Npcap wire capture (Socket-Only mode)")]
    pub no_pcap: bool,

    #[arg(long, help = "Optional network interface substring for passive capture")]
    pub interface: Option<String>,

    #[arg(long, help = "Additional watched DB ports (comma-separated, e.g. 3307,15432)")]
    pub ports: Option<String>,

    #[arg(long, help = "Additional watched DB host alias (format: IP=ALIAS, e.g. 192.0.2.10=DEV-DB)")]
    pub host: Vec<String>,

    #[arg(long, help = "Capture a single non-interactive snapshot and exit")]
    pub once: bool,

    #[arg(long, help = "Print snapshot in JSON format (implies --once)")]
    pub json: bool,
}

impl CliArgs {
    pub fn into_config(&self) -> anyhow::Result<MonitorConfig> {
        let mut cfg = MonitorConfig::load_from_file(self.config.as_deref())?;
        if let Some(interval_ms) = self.interval_ms {
            cfg.poll_interval_ms = interval_ms.clamp(250, 10_000);
        }
        if self.no_pcap {
            cfg.enable_pcap = false;
        }
        if self.interface.is_some() {
            cfg.interface_hint = self.interface.clone();
        }

        if let Some(ref port_csv) = self.ports {
            for part in port_csv.split(',') {
                if let Ok(port) = part.trim().parse::<u16>()
                    && !cfg.watched_ports.contains(&port)
                {
                    cfg.watched_ports.push(port);
                }
            }
        }

        // 설정 파일 hosts 와 같은 규칙: IP 가 아니면 실행 중단, 별칭 생략 시 CUSTOM-DB
        for item in &self.host {
            let (ip_text, alias) = item.split_once('=').unwrap_or((item.as_str(), "CUSTOM-DB"));
            let Ok(ip) = ip_text.trim().parse::<IpAddr>() else {
                anyhow::bail!("--host value is not an IP address: {item}");
            };
            cfg.known_hosts.insert(ip.to_canonical(), alias.trim().to_string());
        }

        Ok(cfg)
    }
}

// 2. 콘솔 텍스트 스냅샷 리포트 출력 -------------------------------------------
// 출력은 영어 고정 (CLI 에는 언어 전환 없음), 사유 문구는 TUI 와 같은 카탈로그
pub fn print_text_snapshot(snap: &MonitorSnapshot) {
    let lang = Lang::En;
    println!("================================================================================");
    println!(
        " DATABASE-MONITOR SNAPSHOT | {} | Mode: {}",
        snap.captured_at, snap.capture_mode
    );
    println!(
        " Overall: [{}] (score: {}/100) - {}",
        snap.overall_risk.label(),
        snap.overall_risk_score,
        i18n::risk_label(lang, snap.overall_risk)
    );
    println!(" Summary: {}", i18n::summary_text(lang, &snap.risk_summary));
    println!(
        " Connections: ESTAB {} / TIME_WAIT {} / SYN_SENT {} ({} local DB/Redis excluded from risk)",
        snap.active_established, snap.active_time_wait, snap.active_syn_sent, snap.local_established
    );
    println!(
        " Traffic: RX {}/s | TX {}/s | {} QPS (total RX {}, {} queries)",
        format_bytes(snap.total_rx_bps),
        format_bytes(snap.total_tx_bps),
        snap.total_qps,
        format_bytes(snap.total_rx_bytes),
        snap.total_queries
    );
    if snap.capture_dropped_frames > 0 {
        println!(
            " Capture drops: {} frames (channel saturated, RX and QPS may be undercounted)",
            snap.capture_dropped_frames
        );
    }
    println!("================================================================================");

    if snap.processes.is_empty() {
        println!(" [Normal] No local process or agent is connected to an external DB.");
    } else {
        println!(" [External DB usage by local process and agent]");
        for p in &snap.processes {
            println!(
                "  - [{}] {} (PID {}) | ESTAB {} / WAIT {} | avg RX {}/s | avg {} QPS | 10s repeat {}x | [{} {} pts] {}",
                p.actor_kind.badge(),
                p.process_name,
                p.pid,
                p.established_conns,
                p.time_wait_conns,
                format_bytes(p.avg_rx_bps),
                p.avg_qps,
                p.max_repeat_10s,
                p.risk_level.label(),
                p.risk_score,
                i18n::reasons_text(lang, &p.risk_reasons, ", ")
            );
            if !p.last_sql.is_empty() {
                println!("    Last SQL: {}", p.last_sql);
            }
        }
    }

    if !snap.top_sql.is_empty() {
        println!("--------------------------------------------------------------------------------");
        println!(" [Top over-query and repeated SQL fingerprints]");
        for s in snap.top_sql.iter().take(5) {
            println!(
                "  - [{}] {} {} | runs {} (10s {}) | total RX {} | {}",
                s.risk_level.label(),
                s.op_kind.label(),
                s.target_table,
                s.exec_count,
                s.repeat_10s,
                format_bytes(s.total_rx_bytes),
                s.sample_sql
            );
        }
    }
    println!("================================================================================");
}
