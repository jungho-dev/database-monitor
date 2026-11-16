//! monitor.rs
//! 엔진·위험도·소스 구분선 통합 테스트 (TUI 화면 검증은 tui.rs).

#[allow(dead_code)]
mod common;

use std::fs;
use std::path::Path;
use database_monitor::app::config::MonitorConfig;
use database_monitor::app::model::{ActorKind, DbEngine, RiskLevel, RiskReason, RiskRule, SqlOpKind};
use common::{build_fixture_snapshot, build_idle_process, build_offline_engine};

// 1. 공용 픽스처 --------------------------------------------------------------
fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| {
            let code = ch as u32;
            if (0xAC00..=0xD7A3).contains(&code)
                || (0x1100..=0x11FF).contains(&code)
                || (0x3130..=0x318F).contains(&code)
                || (0x4E00..=0x9FFF).contains(&code)
                || (0xFF01..=0xFF60).contains(&code)
            {
                2
            } else {
                1
            }
        })
        .sum()
}

fn verify_dir_dividers(dir: &Path) {
    let entries = fs::read_dir(dir).expect("read src dir");
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            verify_dir_dividers(&path);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            let content = fs::read_to_string(&path).expect("read rs file");
            for (idx, line) in content.lines().enumerate() {
                let trimmed = line.trim_start();
                if trimmed.starts_with("// ") && trimmed.ends_with("----") {
                    let width = display_width(line.trim_end());
                    assert_eq!(
                        width,
                        80,
                        "{}:{} 구분선 폭 불일치 (현재 {}열): {}",
                        path.display(),
                        idx + 1,
                        width,
                        line
                    );
                }
            }
        }
    }
}

// 2. 소스 구분선 폭 -----------------------------------------------------------
#[test]
fn all_numbered_dividers_align_to_column_80() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    verify_dir_dividers(&root.join("src"));
    verify_dir_dividers(&root.join("tests"));
}

// 3. 픽스처·라이브 엔진 스냅샷 ------------------------------------------------
#[test]
fn bulk_agent_is_critical_and_live_snapshot_has_zero_db_queries() {
    let snap = build_fixture_snapshot(true);
    assert!(snap.zero_db_query_guaranteed);
    assert_eq!(snap.overall_risk, RiskLevel::Critical);
    let agent = &snap.processes[0];
    assert_eq!(agent.pid, 61480);
    assert_eq!(agent.risk_rule, Some(RiskRule::OverQueryRx));
    assert!(!snap.top_sql.is_empty());
    assert!(snap.alerts.iter().any(|a| a.pid == 61480 && !a.recent_sql.is_empty()));

    let mut live_engine = build_offline_engine();
    let live_snap = live_engine.tick();
    assert!(live_snap.zero_db_query_guaranteed);
}

// 4. 로컬 DB·Redis 위험 제외 --------------------------------------------------
#[test]
fn local_db_and_redis_are_excluded_from_risk() {
    let snap = build_fixture_snapshot(true);

    // 로컬 전용 프로세스 위험 집계 제외
    assert!(snap.processes.iter().all(|p| p.pid != 67700));
    assert_eq!(snap.local_established, 1);
    let local_conns: Vec<_> = snap.connections.iter().filter(|c| c.is_local).collect();
    assert_eq!(local_conns.len(), 1);
    assert!(local_conns.iter().all(|c| c.risk_level == RiskLevel::Safe));
    assert!(local_conns.iter().all(|c| c.endpoint_label.ends_with("[LOCAL]")));

    // 로컬 SQL 이벤트·핑거프린트 제외
    assert!(snap.recent_events.iter().all(|e| e.pid != 67700));
    assert!(snap.top_sql.iter().all(|s| s.target_table != "TB_LOCAL_CACHE"));
    assert!(snap.top_sql.iter().all(|s| s.op_kind != SqlOpKind::Delete));
}

// 5. 위험 프로세스 없을 때 일괄 중단 거부 -------------------------------------
#[test]
fn emergency_stop_refuses_when_no_risky_process() {
    use database_monitor::collector::process::terminate_related_local_processes;

    let idle = build_idle_process(999_999, "dbeaver.exe", ActorKind::DbClientCli);
    assert!(terminate_related_local_processes(&[idle]).is_err());
}

// 6. 사내 DB 포트 엔진 매핑 ---------------------------------------------------
#[test]
fn default_watched_ports_map_to_db_engines() {
    let cfg = MonitorConfig::default();
    for port in [3306, 5002, 6033, 6379] {
        assert!(cfg.watched_ports.contains(&port));
    }
    assert!(cfg.known_hosts.is_empty());
    assert_eq!(DbEngine::from_port(3306), DbEngine::MySql);
    assert_eq!(DbEngine::from_port(5002), DbEngine::MySql);
    assert_eq!(DbEngine::from_port(6033), DbEngine::MySql);
    assert_eq!(DbEngine::from_port(6379), DbEngine::Redis);
}

// 7. 유휴 커넥션 풀 정상 판정 -------------------------------------------------
#[test]
fn idle_spring_app_with_hikari_pool_is_safe_not_risky() {
    use database_monitor::analysis::risk::evaluate_process_risk;

    let cfg = MonitorConfig::default();
    let mut spring_app = build_idle_process(12345, "java.exe", ActorKind::JvmSpringApp);
    spring_app.command_line =
        "java.exe -Dspring.profiles.active=dev -jar batch-bootRun.jar".to_string();
    spring_app.target_endpoints = vec!["192.0.2.10:5002".to_string()];
    // HikariCP Master 10개 + Slave 10개 풀
    spring_app.established_conns = 20;

    evaluate_process_risk(&mut spring_app, &cfg);

    assert_eq!(spring_app.risk_level, RiskLevel::Safe);
    assert_eq!(spring_app.risk_score, 0);
    assert!(spring_app.risk_reasons.contains(&RiskReason::IdleConnected));
}

// 8. 정상 Spring 앱 폴링·쓰기 오탐 방지 ---------------------------------------
#[test]
fn spring_polling_and_conditional_writes_are_not_flagged_as_risk() {
    let snap = build_fixture_snapshot(false);
    let spring = snap
        .processes
        .iter()
        .find(|p| p.pid == 61920)
        .expect("Spring 앱 집계");

    assert!(spring.risk_level < RiskLevel::Warning, "{:?}", spring.risk_reasons);
    assert!(snap.overall_risk < RiskLevel::Warning);
    assert!(snap.alerts.is_empty());
    assert!(snap.top_sql.iter().all(|s| s.risk_level == RiskLevel::Safe));
}

// 9. --json 필드 이름과 영어 사유 문구 ----------------------------------------
#[test]
fn json_snapshot_keeps_field_names_and_carries_english_reason_text() {
    let snap = build_fixture_snapshot(true);
    let json = serde_json::to_value(&snap).expect("serialize");
    let has_hangul = |value: &str| value.chars().any(|ch| ('\u{AC00}'..='\u{D7A3}').contains(&ch));
    assert!(!has_hangul(&json.to_string()), "JSON 에 한글 문구");

    // 1. 프로세스: 사유 배열·규칙 코드·요약 문자열
    let agent = &json["processes"][0];
    assert_eq!(agent["risk_rule"], "OVER_QUERY_RX");
    assert_eq!(agent["risk_reasons"][0], "Sustained bulk query (10s avg 4.80 MB/s)");
    assert_eq!(agent["recent_pattern_note"], "");
    assert_eq!(
        json["risk_summary"],
        "bun.exe (PID 61480): Sustained bulk query (10s avg 4.80 MB/s)"
    );

    // 2. 경보: 규칙 코드, 사유 이어 붙임, 최근 SQL 문자열
    let alert = &json["alerts"][0];
    assert_eq!(alert["rule_code"], "OVER_QUERY_RX");
    assert_eq!(alert["detail"], "Sustained bulk query (10s avg 4.80 MB/s)");
    let recent = alert["recent_sql"][0].as_str().expect("recent sql text");
    assert!(recent.starts_with("12x in 10s | SELECT * FROM TB_PAYMENT_SETTLEMENT"), "{recent}");

    // 3. 소켓·이벤트·랭킹: 값이 없으면 빈 문자열, 있으면 영어 문구
    assert!(json["connections"].as_array().unwrap().iter().all(|c| c["risk_reason"].is_string()));
    assert!(json["recent_events"].as_array().unwrap().iter().all(|e| e["risk_note"].is_string()));
    assert!(json["top_sql"].as_array().unwrap().iter().all(|t| t["risk_reason"].is_string()));
}
