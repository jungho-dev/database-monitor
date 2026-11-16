//! common/mod.rs
//! 통합 테스트 공용 픽스처: 실측 사례 기반 엔진 입력, 스냅샷, 유휴 프로세스, 행 수 확장.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Instant;
use database_monitor::collector::MonitorEngine;
use database_monitor::collector::process::ProcMetadata;
use database_monitor::collector::socket::RawDbSocket;
use database_monitor::app::config::MonitorConfig;
use database_monitor::analysis::decoder::{WireFrame, analyze_sql_text};
use database_monitor::app::model::{
    ActorKind, DbEngine, MonitorSnapshot, ProcessSummary, RiskLevel, SqlOpKind, TcpConnState,
};

// 1. 공용 픽스처 --------------------------------------------------------------
pub const SPRING_POLL_SQL: &str = "SELECT EVENT_ID, EVENT_TYPE, AGGREGATE_ID, PAYLOAD, CREATED_DT, \
     RETRY_CNT, LAST_ERROR_MSG, PROCESSED_YN FROM TB_EVENT_QUEUE WHERE PROCESSED_YN = 'N' \
     AND RETRY_CNT < 5 ORDER BY CREATED_DT ASC LIMIT 500";

pub fn build_offline_engine() -> MonitorEngine {
    let cfg = MonitorConfig {
        enable_pcap: false,
        ..MonitorConfig::default()
    };
    MonitorEngine::new(cfg)
}

// 실측 로그 사례를 가상 주소로 옮긴 한 tick 분량 입력
// with_agent: MCP 에이전트가 응답 3MB SELECT * 를 tick당 16회 (10초 평균 4.8MB/s, 위험)
// Spring 앱: 커넥션 풀 ESTAB 7 + TIME_WAIT 14, 폴링 SELECT 10회·조건 있는 INSERT (위험 아님)
pub fn build_live_fixture(
    with_agent: bool,
) -> (Vec<RawDbSocket>, HashMap<u32, ProcMetadata>, Vec<WireFrame>) {
    let local_ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 25));
    let db_ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));
    let loopback_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let now = Instant::now();
    let socket = |local_addr, local_port, remote_addr, remote_port, state, pid| RawDbSocket {
        local_addr,
        local_port,
        remote_addr,
        remote_port,
        state,
        pid,
    };
    let frame = |src_ip, src_port, dst_ip, dst_port, payload_len, sql: Option<&str>| WireFrame {
        src_ip,
        src_port,
        dst_ip,
        dst_port,
        payload_len,
        decoded_sql: sql.map(|text| analyze_sql_text(DbEngine::MySql, text)),
        captured_at: now,
    };

    // 1. 프로세스 메타데이터
    let mut meta = HashMap::new();
    for (pid, name, cmdline, actor_kind) in [
        (61480, "bun.exe", "bun.exe C:/Users/dev/mcp/mysql-mcp.ts", ActorKind::AiAgentMcp),
        (61920, "java.exe", "java.exe -jar event-consumer.jar", ActorKind::JvmSpringApp),
        (67700, "node.exe", "node.exe local-batch.js", ActorKind::ScriptWorker),
    ] {
        meta.insert(
            pid,
            ProcMetadata {
                pid,
                name: name.to_string(),
                cmdline: cmdline.to_string(),
                actor_kind,
            },
        );
    }

    // 2. 소켓 테이블 (로컬 DB 1개는 위험 제외 대상)
    let local_db = socket(loopback_ip, 57002, loopback_ip, 3306, TcpConnState::Established, 67700);
    let mut sockets = vec![local_db];
    for idx in 0..7u16 {
        sockets.push(socket(local_ip, 56000 + idx, db_ip, 3306, TcpConnState::Established, 61920));
    }
    for idx in 0..14u16 {
        sockets.push(socket(local_ip, 56100 + idx, db_ip, 3306, TcpConnState::TimeWait, 61920));
    }

    // 3. 와이어 프레임
    let mut frames = Vec::new();
    for _ in 0..10 {
        frames.push(frame(local_ip, 56000, db_ip, 3306, 220, Some(SPRING_POLL_SQL)));
        frames.push(frame(db_ip, 3306, local_ip, 56000, 2 * 1024, None));
    }
    frames.push(frame(
        local_ip,
        56001,
        db_ip,
        3306,
        120,
        Some("INSERT INTO TB_ACCESS_LOG (USER_ID, MENU_ID) VALUES ('u1', 'm1')"),
    ));
    for _ in 0..30 {
        let local_sql = Some("DELETE FROM TB_LOCAL_CACHE");
        frames.push(frame(loopback_ip, 57002, loopback_ip, 3306, 60, local_sql));
        frames.push(frame(loopback_ip, 3306, loopback_ip, 57002, 512 * 1024, None));
    }

    if with_agent {
        sockets.push(socket(local_ip, 55101, db_ip, 3306, TcpConnState::Established, 61480));
        sockets.push(socket(local_ip, 55102, db_ip, 3306, TcpConnState::Established, 61480));
        for q_idx in 0..16 {
            let sql = if q_idx % 4 == 0 {
                "SELECT * FROM TB_OUTBOX_EVENT WHERE STATUS = 'READY' AND RETRY_CNT < 3"
            } else {
                "SELECT * FROM TB_PAYMENT_SETTLEMENT WHERE SETTLE_DT >= '2026-10-01'"
            };
            frames.push(frame(local_ip, 55101, db_ip, 3306, 180, Some(sql)));
            frames.push(frame(db_ip, 3306, local_ip, 55101, 3 * 1024 * 1024, None));
        }
    }

    (sockets, meta, frames)
}

pub fn build_fixture_snapshot(with_agent: bool) -> MonitorSnapshot {
    let mut engine = build_offline_engine();
    let (sockets, meta, frames) = build_live_fixture(with_agent);
    engine.tick_with(&sockets, &meta, frames)
}

pub fn build_idle_process(pid: u32, name: &str, actor_kind: ActorKind) -> ProcessSummary {
    ProcessSummary {
        pid,
        process_name: name.to_string(),
        actor_kind,
        command_line: String::new(),
        target_endpoints: Vec::new(),
        established_conns: 1,
        time_wait_conns: 0,
        syn_conns: 0,
        cache_churn_conns: 0,
        tx_bps: 0,
        rx_bps: 0,
        tx_bytes_total: 0,
        rx_bytes_total: 0,
        qps: 0,
        avg_rx_bps: 0,
        avg_qps: 0,
        total_queries: 0,
        max_repeat_10s: 0,
        last_op: SqlOpKind::SocketConnect,
        last_sql: String::new(),
        recent_pattern_risk: RiskLevel::Safe,
        recent_pattern_note: None,
        risk_level: RiskLevel::Safe,
        risk_score: 0,
        risk_rule: None,
        risk_reasons: Vec::new(),
    }
}


// 2. 행 수 확장 ---------------------------------------------------------------
// 스크롤바·위치 표시 검증용: 표마다 count 행이 되도록 복제 (식별 값은 서로 다르게)
#[allow(dead_code)]
pub fn bloat_snapshot(base: &MonitorSnapshot, count: usize) -> MonitorSnapshot {
    let mut snap = base.clone();
    let process = snap.processes[0].clone();
    let connection = snap.connections[0].clone();
    let event = snap.recent_events[0].clone();
    let fingerprint = snap.top_sql[0].clone();
    let alert = snap.alerts[0].clone();
    for index in 0..count {
        let offset = u32::try_from(index).unwrap_or(0);
        let mut next_process = process.clone();
        next_process.pid = 70_000 + offset;
        next_process.process_name = format!("worker-{index}.exe");
        snap.processes.push(next_process);
        let mut next_connection = connection.clone();
        next_connection.pid = 70_000 + offset;
        next_connection.process_name = format!("worker-{index}.exe");
        snap.connections.push(next_connection);
        let mut next_event = event.clone();
        next_event.id += 1_000 + index as u64;
        snap.recent_events.push(next_event);
        let mut next_fingerprint = fingerprint.clone();
        next_fingerprint.fingerprint = format!("{} #{index}", fingerprint.fingerprint);
        next_fingerprint.exec_count += index as u64;
        snap.top_sql.push(next_fingerprint);
        let mut next_alert = alert.clone();
        next_alert.id += 1_000 + index as u64;
        snap.alerts.push(next_alert);
    }
    snap
}
