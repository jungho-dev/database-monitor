//! risk.rs
//! DB 과조회·쿼리 폭주·커넥션 폭주 위험도 산출 엔진 (위험은 DB 피해 우려 조건만).

use crate::app::config::{MonitorConfig, RiskThresholds};
use crate::app::model::{ProcessSummary, RiskLevel, RiskReason, RiskRule};

// 1. 프로세스 단위 위험도 평가 ------------------------------------------------
// 순간값(1초) 대신 10초 평균으로 판정해 짧은 버스트·정상 폴링을 위험으로 올리지 않음
// 사유는 값(RiskReason)으로만 만들고 문구는 화면·출력 계층이 언어별로 만듦
pub fn evaluate_process_risk(proc_item: &mut ProcessSummary, cfg: &MonitorConfig) {
    let th: &RiskThresholds = &cfg.thresholds;
    let avg_rx = proc_item.avg_rx_bps;
    let avg_qps = proc_item.avg_qps;
    let repeat = proc_item.max_repeat_10s;
    let mut findings: Vec<(RiskLevel, RiskRule, RiskReason)> = Vec::new();

    // 1. 조회량: 대량 수신 지속, 또는 고빈도·반복 실행이 큰 응답을 계속 끌어오는 루프
    let heavy_loop =
        avg_rx >= th.warn_rx_bps && (avg_qps >= th.warn_qps || repeat >= th.crit_repeat_10s);
    if avg_rx >= th.crit_rx_bps {
        findings.push((
            RiskLevel::Critical,
            RiskRule::OverQueryRx,
            RiskReason::RxSustained { avg_rx_bps: avg_rx },
        ));
    } else if heavy_loop {
        findings.push((
            RiskLevel::Critical,
            RiskRule::QueryLoop,
            RiskReason::HeavyLoop { avg_rx_bps: avg_rx, avg_qps, repeat },
        ));
    } else if avg_rx >= th.warn_rx_bps {
        findings.push((
            RiskLevel::Warning,
            RiskRule::OverQueryRx,
            RiskReason::RxHigh { avg_rx_bps: avg_rx },
        ));
    } else if avg_rx >= th.caution_rx_bps {
        findings.push((
            RiskLevel::Caution,
            RiskRule::OverQueryRx,
            RiskReason::RxRising { avg_rx_bps: avg_rx },
        ));
    }

    // 2. 쿼리 빈도
    if avg_qps >= th.crit_qps {
        findings.push((
            RiskLevel::Critical,
            RiskRule::QueryStorm,
            RiskReason::QpsStorm { avg_qps },
        ));
    } else if avg_qps >= th.warn_qps {
        findings.push((
            RiskLevel::Warning,
            RiskRule::QueryStorm,
            RiskReason::QpsHigh { avg_qps },
        ));
    } else if avg_qps >= th.caution_qps {
        findings.push((
            RiskLevel::Caution,
            RiskRule::QueryStorm,
            RiskReason::QpsRising { avg_qps },
        ));
    }

    // 3. 동일 SQL 반복 (스케줄러 폴링·N+1 은 응답이 작으면 DB 피해가 작아 주의까지만)
    // 반복 대량 조회 루프로 이미 위험 판정된 경우 중복 사유 생략
    if repeat >= th.crit_repeat_10s {
        if !heavy_loop {
            findings.push((
                RiskLevel::Warning,
                RiskRule::QueryLoop,
                RiskReason::RepeatLoopSuspected { repeat },
            ));
        }
    } else if repeat >= th.warn_repeat_10s {
        findings.push((
            RiskLevel::Caution,
            RiskRule::QueryLoop,
            RiskReason::RepeatSql { repeat },
        ));
    }

    // 4. 커넥션 점유와 단기 연결 폭주 (Redis 단기 연결은 DB 과조회가 아니라 관찰만)
    let has_activity = avg_qps > 0 || avg_rx > 0 || proc_item.total_queries > 0;
    if proc_item.established_conns >= th.crit_estab_conns {
        findings.push((
            RiskLevel::Warning,
            RiskRule::ConnHog,
            RiskReason::ConnHog { established: proc_item.established_conns },
        ));
    } else if proc_item.established_conns >= th.warn_estab_conns && has_activity {
        findings.push((
            RiskLevel::Caution,
            RiskRule::ConnHog,
            RiskReason::ConnMulti { established: proc_item.established_conns },
        ));
    }
    let churn = (proc_item.time_wait_conns + proc_item.syn_conns)
        .saturating_sub(proc_item.cache_churn_conns);
    if churn >= th.warn_churn_conns * 4 {
        findings.push((
            RiskLevel::Warning,
            RiskRule::ConnChurn,
            RiskReason::ChurnStorm { churn },
        ));
    } else if churn >= th.warn_churn_conns {
        findings.push((
            RiskLevel::Caution,
            RiskRule::ConnChurn,
            RiskReason::ChurnRepeat { churn },
        ));
    }
    if proc_item.cache_churn_conns >= th.warn_churn_conns {
        findings.push((
            RiskLevel::Caution,
            RiskRule::ConnChurn,
            RiskReason::CacheChurn { churn: proc_item.cache_churn_conns },
        ));
    }

    // 5. 최근 10초 SQL 문장 자체의 위험 (파괴적 DDL, 조건 없는 변경 등)
    if proc_item.recent_pattern_risk > RiskLevel::Safe
        && let Some(note) = proc_item.recent_pattern_note
    {
        findings.push((proc_item.recent_pattern_risk, RiskRule::DangerousSql, note));
    }

    // 6. 최고 등급 사유 우선 정렬, 점수는 등급 구간 + 동반 사유 가산
    findings.sort_by_key(|f| std::cmp::Reverse(f.0));
    let level = findings.first().map_or(RiskLevel::Safe, |f| f.0);
    let base: u8 = match level {
        RiskLevel::Safe => 0,
        RiskLevel::Caution => 20,
        RiskLevel::Warning => 50,
        RiskLevel::Critical => 80,
    };
    let extra = (findings.len().saturating_sub(1) * 6).min(19) as u8;
    proc_item.risk_level = level;
    proc_item.risk_score = if level == RiskLevel::Safe { 0 } else { base + extra };
    proc_item.risk_rule = findings.first().map(|f| f.1);
    proc_item.risk_reasons = findings.into_iter().map(|f| f.2).collect();
    if proc_item.risk_reasons.is_empty() && proc_item.established_conns > 0 {
        proc_item.risk_reasons.push(RiskReason::IdleConnected);
    }
}

// 2. 바이트 단위 포맷터 -------------------------------------------------------
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;

    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::model::{ActorKind, SqlOpKind};

    fn build_active_process(actor_kind: ActorKind) -> ProcessSummary {
        ProcessSummary {
            pid: 9001,
            process_name: "proc.exe".to_string(),
            actor_kind,
            command_line: String::new(),
            target_endpoints: vec!["192.0.2.10:3306 [DEV-DB]".to_string()],
            established_conns: 2,
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
            total_queries: 10,
            max_repeat_10s: 0,
            last_op: SqlOpKind::Select,
            last_sql: String::new(),
            recent_pattern_risk: RiskLevel::Safe,
            recent_pattern_note: None,
            risk_level: RiskLevel::Safe,
            risk_score: 0,
            risk_rule: None,
            risk_reasons: Vec::new(),
        }
    }

    #[test]
    fn sustained_bulk_rx_is_critical() {
        let cfg = MonitorConfig::default();
        let mut agent = build_active_process(ActorKind::AiAgentMcp);
        agent.avg_rx_bps = 5 * 1024 * 1024;
        agent.avg_qps = 42;

        evaluate_process_risk(&mut agent, &cfg);
        assert_eq!(agent.risk_level, RiskLevel::Critical);
        assert_eq!(agent.risk_rule, Some(RiskRule::OverQueryRx));
        assert!(agent.risk_score >= 80);
    }

    #[test]
    fn repeated_query_pulling_large_responses_is_critical_loop() {
        let cfg = MonitorConfig::default();
        let mut agent = build_active_process(ActorKind::AiAgentMcp);
        agent.avg_rx_bps = 1536 * 1024;
        agent.avg_qps = 12;
        agent.max_repeat_10s = 120;

        evaluate_process_risk(&mut agent, &cfg);
        assert_eq!(agent.risk_level, RiskLevel::Critical);
        assert_eq!(agent.risk_rule, Some(RiskRule::QueryLoop));
    }

    #[test]
    fn busy_spring_app_with_small_responses_is_not_risky() {
        // 실측 로그 사례: 스케줄러 반복 조회 + 조건 있는 쓰기 + 커넥션 풀 재연결
        let cfg = MonitorConfig::default();
        let mut spring_app = build_active_process(ActorKind::JvmSpringApp);
        spring_app.established_conns = 25;
        spring_app.time_wait_conns = 14;
        spring_app.avg_rx_bps = 95 * 1024;
        spring_app.avg_qps = 8;
        spring_app.max_repeat_10s = 60;

        evaluate_process_risk(&mut spring_app, &cfg);
        assert!(spring_app.risk_level < RiskLevel::Warning, "{:?}", spring_app.risk_reasons);
    }

    #[test]
    fn destructive_sql_is_critical_even_without_load() {
        let cfg = MonitorConfig::default();
        let mut client = build_active_process(ActorKind::DbClientCli);
        client.recent_pattern_risk = RiskLevel::Critical;
        client.recent_pattern_note = Some(RiskReason::UnconditionalWrite);

        evaluate_process_risk(&mut client, &cfg);
        assert_eq!(client.risk_level, RiskLevel::Critical);
        assert_eq!(client.risk_rule, Some(RiskRule::DangerousSql));
    }
}
