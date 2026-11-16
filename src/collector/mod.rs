//! mod.rs
//! 소켓 스캐너·패시브 와이어 스니퍼·위험도 엔진을 통합하는 실시간 수집 코어.

pub mod process;
pub mod socket;
pub mod wire;

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::time::{Duration, Instant};
use chrono::Local;
use crossbeam_channel::{Receiver, bounded};
use crate::app::applog;
use crate::collector::process::{ProcMetadata, terminate_related_local_processes};
use crate::collector::socket::{RawDbSocket, SocketScanner};
use crate::collector::wire::{WireCaptureGuard, start_passive_wire_capture};
use crate::app::config::{MonitorConfig, is_local_endpoint};
use crate::analysis::decoder::WireFrame;
use crate::app::model::{
    ActorKind, AlertEntry, DbConnectionEntry, DbEngine, MonitorSnapshot, ProcessSummary,
    RecentSql, RiskLevel, RiskReason, RiskRule, RiskSummary, SqlActivityEvent,
    SqlFingerprintStats, SqlOpKind, StopFailure, StopReport, StopSuccess, TcpConnState,
};
use crate::analysis::risk::evaluate_process_risk;
use crate::tui::i18n::{self, Lang};

// 위험 판정 평균 창, 같은 경보 재기록 간격
const AVG_WINDOW: Duration = Duration::from_secs(10);
const ALERT_REPEAT_GAP: Duration = Duration::from_secs(60);
// 캡처 워커 → 엔진 채널 용량: 1초 tick 기준 약 16k 세그먼트(MTU 기준 20 MB/s 이상)까지 누락 없이 버팀
const WIRE_CHANNEL_CAPACITY: usize = 16_384;

// 1. 소켓 4-튜플 키 및 누적 상태 ----------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FlowKey {
    local_addr: IpAddr,
    local_port: u16,
    remote_addr: IpAddr,
    remote_port: u16,
}

// 표본은 10초 평균 창만큼 보관, 1초 속도는 그중 최근 구간만 합산
struct ConnTrackState {
    first_seen: Instant,
    last_seen: Instant,
    tx_bytes_total: u64,
    rx_bytes_total: u64,
    tx_samples: VecDeque<(Instant, u64)>,
    rx_samples: VecDeque<(Instant, u64)>,
    query_samples: VecDeque<Instant>,
    query_count: u64,
    last_op: SqlOpKind,
    last_sql: String,
    last_sql_event_id: Option<u64>,
}

struct SqlTrackState {
    fingerprint: String,
    sample_sql: String,
    op_kind: SqlOpKind,
    target_table: String,
    last_pid: u32,
    last_process: String,
    actor_kind: ActorKind,
    last_endpoint: String,
    exec_count: u64,
    exec_timestamps: VecDeque<Instant>,
    rx_samples: VecDeque<(Instant, u64)>,
    total_rx_bytes: u64,
    max_single_rx_bytes: u64,
    last_seen: String,
    last_seen_at: Instant,
    pattern_risk: RiskLevel,
    pattern_note: Option<RiskReason>,
}

// 2. 통합 관제 엔진 -----------------------------------------------------------
pub struct MonitorEngine {
    pub config: MonitorConfig,
    started_at: Instant,
    last_tick_at: Option<Instant>,
    scanner: SocketScanner,
    wire_rx: Option<Receiver<WireFrame>>,
    wire_guard: Option<WireCaptureGuard>,
    capture_mode: String,
    capture_dropped_frames: u64,
    logged_drops: u64,
    drop_logged_at: Option<Instant>,
    conn_states: HashMap<FlowKey, ConnTrackState>,
    sql_stats: HashMap<String, SqlTrackState>,
    recent_events: VecDeque<SqlActivityEvent>,
    alerts: VecDeque<AlertEntry>,
    last_alert_by_key: HashMap<(u32, RiskRule), (Instant, RiskLevel)>,
    rx_history_kb: VecDeque<u64>,
    qps_history: VecDeque<u64>,
    total_tx_bytes: u64,
    total_rx_bytes: u64,
    total_queries: u64,
    next_event_id: u64,
    next_alert_id: u64,
    last_stop: Option<(Instant, Result<StopSuccess, StopFailure>)>,
}

impl MonitorEngine {
    pub fn new(config: MonitorConfig) -> Self {
        let (tx, rx) = bounded(WIRE_CHANNEL_CAPACITY);
        let (wire_rx, wire_guard, capture_mode) = match start_passive_wire_capture(&config, tx) {
            Ok(guard) => {
                let dev_count = guard.active_devices.len();
                (
                    Some(rx),
                    Some(guard),
                    format!("Passive Wire+Socket ({dev_count} IF, 0 DB Query)"),
                )
            }
            Err(_) => (None, None, "Passive Socket-Only (0 DB Query)".to_string()),
        };

        Self {
            config,
            started_at: Instant::now(),
            last_tick_at: None,
            scanner: SocketScanner::new(),
            wire_rx,
            wire_guard,
            capture_mode,
            capture_dropped_frames: 0,
            logged_drops: 0,
            drop_logged_at: None,
            conn_states: HashMap::new(),
            sql_stats: HashMap::new(),
            recent_events: VecDeque::with_capacity(200),
            alerts: VecDeque::with_capacity(200),
            last_alert_by_key: HashMap::new(),
            rx_history_kb: VecDeque::from(vec![0; 60]),
            qps_history: VecDeque::from(vec![0; 60]),
            total_tx_bytes: 0,
            total_rx_bytes: 0,
            total_queries: 0,
            next_event_id: 1,
            next_alert_id: 1,
            last_stop: None,
        }
    }

    pub fn capture_mode(&self) -> &str {
        &self.capture_mode
    }

    // 실제 OS 소켓 테이블과 캡처 워커 프레임 수집 후 반영
    pub fn tick(&mut self) -> MonitorSnapshot {
        let (raw_sockets, proc_meta) = self.scanner.scan(&self.config);

        // tick 시작 시점에 쌓인 프레임은 모두 반영 (상한 2048 은 고트래픽에서 RX 를 과소 집계했음)
        // 수집 중 새로 들어오는 프레임은 다음 tick 몫이라 UI 루프를 붙잡지 않음
        let mut frames = Vec::new();
        if let Some(ref rx) = self.wire_rx {
            let queued = rx.len();
            frames.reserve(queued);
            frames.extend(rx.try_iter().take(queued));
        }

        // 채널 포화 누락은 화면 표시, 로그는 늘었을 때 60초에 한 번
        if let Some(guard) = &self.wire_guard {
            let dropped = guard.dropped_frames();
            let log_due = self
                .drop_logged_at
                .is_none_or(|at| at.elapsed() >= ALERT_REPEAT_GAP);
            if dropped > self.logged_drops && log_due {
                applog::warn(&format!(
                    "Capture frames dropped, total {dropped} (channel of {WIRE_CHANNEL_CAPACITY} saturated, RX and QPS may be undercounted)"
                ));
                self.logged_drops = dropped;
                self.drop_logged_at = Some(Instant::now());
            }
            self.capture_dropped_frames = dropped;
        }
        self.tick_with(&raw_sockets, &proc_meta, frames)
    }

    // 수집 결과 반영 단계 (통합 테스트는 OS 테이블 대신 고정 소켓·프레임을 주입)
    pub fn tick_with(
        &mut self,
        raw_sockets: &[RawDbSocket],
        proc_meta: &HashMap<u32, ProcMetadata>,
        frames: Vec<WireFrame>,
    ) -> MonitorSnapshot {
        let now = Instant::now();
        let now_str = Local::now().format("%H:%M:%S").to_string();
        let rate_window = self
            .last_tick_at
            .map(|last| now.duration_since(last))
            .unwrap_or_default()
            .max(Duration::from_secs(1));
        self.last_tick_at = Some(now);

        // 1. 활성 소켓 인덱스 구성
        let mut active_keys = HashMap::new();
        for sock in raw_sockets {
            let key = FlowKey {
                local_addr: sock.local_addr,
                local_port: sock.local_port,
                remote_addr: sock.remote_addr,
                remote_port: sock.remote_port,
            };
            active_keys.insert(key, sock.clone());
            self.ensure_conn_track(key, now, sock, proc_meta, &now_str);
        }

        // 2. 패시브 와이어 프레임 반영
        for frame in frames {
            self.ingest_wire_frame(now, &now_str, frame, &active_keys, proc_meta);
        }

        // 3. 만료 윈도우 정리 (10초 평균 창)
        self.prune_sliding_windows(now, rate_window);

        // 4. 스냅샷 조립 및 위험도 산출
        self.build_snapshot(now, rate_window, &now_str, raw_sockets, proc_meta)
    }

    // 3. 신규 소켓 트래킹 등록 ------------------------------------------------
    fn ensure_conn_track(
        &mut self,
        key: FlowKey,
        now: Instant,
        sock: &RawDbSocket,
        proc_meta: &HashMap<u32, ProcMetadata>,
        now_str: &str,
    ) {
        if let Some(existing) = self.conn_states.get_mut(&key) {
            existing.last_seen = now;
            return;
        }

        let engine = DbEngine::from_port(sock.remote_port);
        let endpoint = self.config.endpoint_label(&sock.remote_addr, sock.remote_port);
        let (proc_name, actor_kind) = proc_meta
            .get(&sock.pid)
            .map(|m| (m.name.clone(), m.actor_kind))
            .unwrap_or_else(|| (format!("pid-{}", sock.pid), ActorKind::SystemOther));

        self.conn_states.insert(
            key,
            ConnTrackState {
                first_seen: now,
                last_seen: now,
                tx_bytes_total: 0,
                rx_bytes_total: 0,
                tx_samples: VecDeque::new(),
                rx_samples: VecDeque::new(),
                query_samples: VecDeque::new(),
                query_count: 0,
                last_op: SqlOpKind::SocketConnect,
                last_sql: format!("[{} connected: {}]", engine.label(), sock.state.label()),
                last_sql_event_id: None,
            },
        );

        if sock.state == TcpConnState::Established
            && !is_local_endpoint(&sock.local_addr, &sock.remote_addr)
        {
            let ev_id = self.next_event_id;
            self.next_event_id += 1;
            self.push_event(SqlActivityEvent {
                id: ev_id,
                timestamp: now_str.to_string(),
                pid: sock.pid,
                process_name: proc_name,
                actor_kind,
                remote_endpoint: endpoint,
                op_kind: SqlOpKind::SocketConnect,
                target_table: "-".to_string(),
                sql_fingerprint: "TCP_ESTABLISHED".to_string(),
                sql_preview: format!(
                    "Local port {} -> {}:{} connection active",
                    sock.local_port, sock.remote_addr, sock.remote_port
                ),
                tx_bytes: 0,
                rx_bytes: 0,
                repeat_10s: 1,
                risk_level: RiskLevel::Safe,
                risk_note: None,
            });
        }
    }

    // 4. 와이어 프레임 집계 ---------------------------------------------------
    fn ingest_wire_frame(
        &mut self,
        now: Instant,
        now_str: &str,
        frame: WireFrame,
        active_keys: &HashMap<FlowKey, RawDbSocket>,
        proc_meta: &HashMap<u32, ProcMetadata>,
    ) {
        let is_outbound = self
            .config
            .is_watched_endpoint(&frame.dst_ip, frame.dst_port);
        let is_inbound = self
            .config
            .is_watched_endpoint(&frame.src_ip, frame.src_port);
        if !is_outbound && !is_inbound {
            return;
        }

        let key = if is_outbound {
            FlowKey {
                local_addr: frame.src_ip,
                local_port: frame.src_port,
                remote_addr: frame.dst_ip,
                remote_port: frame.dst_port,
            }
        } else {
            FlowKey {
                local_addr: frame.dst_ip,
                local_port: frame.dst_port,
                remote_addr: frame.src_ip,
                remote_port: frame.src_port,
            }
        };

        let is_local = is_local_endpoint(&key.local_addr, &key.remote_addr);
        let track = self.conn_states.entry(key).or_insert_with(|| ConnTrackState {
            first_seen: now,
            last_seen: now,
            tx_bytes_total: 0,
            rx_bytes_total: 0,
            tx_samples: VecDeque::new(),
            rx_samples: VecDeque::new(),
            query_samples: VecDeque::new(),
            query_count: 0,
            last_op: SqlOpKind::SocketConnect,
            last_sql: String::new(),
            last_sql_event_id: None,
        });
        track.last_seen = now;

        let bytes = frame.payload_len as u64;
        let captured_at = frame.captured_at;
        if is_outbound {
            track.tx_bytes_total += bytes;
            if bytes > 0 {
                track.tx_samples.push_back((captured_at, bytes));
                if !is_local {
                    self.total_tx_bytes += bytes;
                }
            }

            if let Some(decoded) = frame.decoded_sql {
                // 상세 패널용 원문, TLS·Redis 처럼 원문이 없으면 미리보기
                let sql_text = if decoded.full_sql.is_empty() {
                    decoded.preview.clone()
                } else {
                    decoded.full_sql.clone()
                };
                track.last_op = decoded.op_kind;
                track.last_sql = sql_text.clone();

                // TLS·바이너리·Redis 패킷, 로컬 통신, 드라이버·커넥션 풀 관리 명령 집계 제외
                // (COM_PING, SET autocommit, COMMIT, SELECT 1, SELECT @@변수 등은 요청마다 반복됨)
                let sql_head = decoded
                    .fingerprint
                    .strip_prefix("/*")
                    .and_then(|rest| rest.split_once("*/"))
                    .map_or(decoded.fingerprint.as_str(), |(_, body)| body.trim_start());
                let is_overhead = matches!(
                    decoded.op_kind,
                    SqlOpKind::PingQuit | SqlOpKind::Transaction
                ) || sql_head.starts_with("SET ")
                    || sql_head.starts_with("USE ")
                    || sql_head.starts_with("SELECT @@")
                    || sql_head == "SELECT ?"
                    || sql_head == "SELECT ? FROM DUAL";
                if decoded.op_kind == SqlOpKind::EncryptedFlow || is_local || is_overhead {
                    return;
                }

                // 행위자·엔드포인트 문자열은 집계 대상 SQL 에서만 생성 (응답 프레임 경로 할당 제거)
                let pid = active_keys.get(&key).map(|s| s.pid).unwrap_or(0);
                let (proc_name, actor_kind) = proc_meta
                    .get(&pid)
                    .map(|m| (m.name.clone(), m.actor_kind))
                    .unwrap_or_else(|| (format!("pid-{pid}"), ActorKind::SystemOther));
                let endpoint = self.config.endpoint_label(&key.remote_addr, key.remote_port);
                track.query_count += 1;
                track.query_samples.push_back(captured_at);
                self.total_queries += 1;

                // 핑거프린트별 반복 횟수 기록 (원문은 최근 실행 값으로 갱신)
                let stat = self
                    .sql_stats
                    .entry(decoded.fingerprint.clone())
                    .or_insert_with(|| SqlTrackState {
                        fingerprint: decoded.fingerprint.clone(),
                        sample_sql: String::new(),
                        op_kind: decoded.op_kind,
                        target_table: decoded.target_table.clone(),
                        last_pid: pid,
                        last_process: proc_name.clone(),
                        actor_kind,
                        last_endpoint: endpoint.clone(),
                        exec_count: 0,
                        exec_timestamps: VecDeque::new(),
                        rx_samples: VecDeque::new(),
                        total_rx_bytes: 0,
                        max_single_rx_bytes: 0,
                        last_seen: now_str.to_string(),
                        last_seen_at: now,
                        pattern_risk: decoded.pattern_risk,
                        pattern_note: decoded.pattern_note,
                    });
                stat.sample_sql = sql_text;
                stat.exec_count += 1;
                stat.exec_timestamps.push_back(captured_at);
                stat.last_pid = pid;
                stat.last_process = proc_name.clone();
                stat.actor_kind = actor_kind;
                stat.last_endpoint = endpoint.clone();
                stat.last_seen = now_str.to_string();
                stat.last_seen_at = now;
                // COM_STMT_EXECUTE 는 문장 구분 없이 한 핑거프린트라 반복 판정 제외
                let repeat_10s = if stat.fingerprint == "EXECUTE_STMT" {
                    1
                } else {
                    stat.exec_timestamps.len() as u32
                };

                // 반복 자체는 주의까지만, 위험은 문장 패턴으로만
                let mut ev_risk = decoded.pattern_risk;
                let mut ev_note = decoded.pattern_note;
                if repeat_10s >= self.config.thresholds.crit_repeat_10s
                    && ev_risk < RiskLevel::Warning
                {
                    ev_risk = RiskLevel::Warning;
                    ev_note = Some(RiskReason::RepeatLoopSuspected { repeat: repeat_10s });
                } else if repeat_10s >= self.config.thresholds.warn_repeat_10s
                    && ev_risk < RiskLevel::Caution
                {
                    ev_risk = RiskLevel::Caution;
                    ev_note = Some(RiskReason::RepeatSql { repeat: repeat_10s });
                }

                let ev_id = self.next_event_id;
                self.next_event_id += 1;
                track.last_sql_event_id = Some(ev_id);

                self.push_event(SqlActivityEvent {
                    id: ev_id,
                    timestamp: now_str.to_string(),
                    pid,
                    process_name: proc_name,
                    actor_kind,
                    remote_endpoint: endpoint,
                    op_kind: decoded.op_kind,
                    target_table: decoded.target_table,
                    sql_fingerprint: decoded.fingerprint,
                    sql_preview: decoded.preview,
                    tx_bytes: bytes,
                    rx_bytes: 0,
                    repeat_10s,
                    risk_level: ev_risk,
                    risk_note: ev_note,
                });
            }
        } else if bytes > 0 {
            track.rx_bytes_total += bytes;
            track.rx_samples.push_back((captured_at, bytes));
            if is_local {
                return;
            }
            self.total_rx_bytes += bytes;

            // 직전 쿼리 이벤트에 응답 RX 바이트 귀속 (단건 대용량 응답은 주의까지만)
            // 이벤트 ID 는 증가 순서로 쌓이므로 이분 탐색 (응답 프레임마다 150건 역순 순회 제거)
            let Some(last_id) = track.last_sql_event_id else {
                return;
            };
            let Ok(idx) = self.recent_events.binary_search_by_key(&last_id, |ev| ev.id) else {
                return;
            };
            let single_warn = self.config.thresholds.single_rx_warn_bytes;
            let ev = &mut self.recent_events[idx];
            ev.rx_bytes += bytes;
            if ev.rx_bytes >= single_warn && ev.risk_level < RiskLevel::Warning {
                ev.risk_level = RiskLevel::Warning;
                ev.risk_note = Some(RiskReason::LargeSingleResponse { bytes: ev.rx_bytes });
            }
            if let Some(stat) = self.sql_stats.get_mut(&ev.sql_fingerprint) {
                stat.total_rx_bytes += bytes;
                stat.rx_samples.push_back((captured_at, bytes));
                stat.max_single_rx_bytes = stat.max_single_rx_bytes.max(ev.rx_bytes);
            }
        }
    }

    // 5. 슬라이딩 윈도우 정리 -------------------------------------------------
    fn prune_sliding_windows(&mut self, now: Instant, rate_window: Duration) {
        let keep = AVG_WINDOW.max(rate_window);
        let stale_conn = Duration::from_secs(30);

        self.conn_states
            .retain(|_, st| now.duration_since(st.last_seen) <= stale_conn);

        for st in self.conn_states.values_mut() {
            st.tx_samples.retain(|(ts, _)| now.duration_since(*ts) <= keep);
            st.rx_samples.retain(|(ts, _)| now.duration_since(*ts) <= keep);
            st.query_samples.retain(|ts| now.duration_since(*ts) <= keep);
        }

        for stat in self.sql_stats.values_mut() {
            stat.exec_timestamps.retain(|ts| now.duration_since(*ts) <= AVG_WINDOW);
            stat.rx_samples.retain(|(ts, _)| now.duration_since(*ts) <= AVG_WINDOW);
        }

        // 30분 유휴 핑거프린트 제거 (메모리 상한)
        self.sql_stats.retain(|_, stat| {
            !stat.exec_timestamps.is_empty()
                || now.duration_since(stat.last_seen_at) <= Duration::from_secs(1800)
        });
    }

    // 6. 스냅샷 생성 및 경보 발령 ---------------------------------------------
    fn build_snapshot(
        &mut self,
        now: Instant,
        rate_window: Duration,
        now_str: &str,
        raw_sockets: &[RawDbSocket],
        proc_meta: &HashMap<u32, ProcMetadata>,
    ) -> MonitorSnapshot {
        let th = &self.config.thresholds;
        let mut connections = Vec::new();
        let mut proc_map: HashMap<u32, ProcessSummary> = HashMap::new();

        let mut active_established = 0usize;
        let mut active_time_wait = 0usize;
        let mut active_syn_sent = 0usize;
        let mut local_established = 0usize;
        let window_ms = (rate_window.as_millis() as u64).max(1);
        let avg_secs = AVG_WINDOW.as_secs();

        // 1. 소켓 엔트리 및 프로세스 집계
        for sock in raw_sockets {
            let is_local = is_local_endpoint(&sock.local_addr, &sock.remote_addr);
            match (is_local, sock.state) {
                (true, TcpConnState::Established) => local_established += 1,
                (true, _) => {}
                (false, TcpConnState::Established) => active_established += 1,
                (false, TcpConnState::TimeWait) => active_time_wait += 1,
                (false, TcpConnState::SynSent) => active_syn_sent += 1,
                _ => {}
            }

            let key = FlowKey {
                local_addr: sock.local_addr,
                local_port: sock.local_port,
                remote_addr: sock.remote_addr,
                remote_port: sock.remote_port,
            };

            let meta = proc_meta.get(&sock.pid);
            let proc_name = meta
                .map(|m| m.name.clone())
                .unwrap_or_else(|| format!("pid-{}", sock.pid));
            let actor_kind = meta.map(|m| m.actor_kind).unwrap_or(ActorKind::SystemOther);
            let cmdline = meta
                .map(|m| m.cmdline.clone())
                .unwrap_or_else(|| proc_name.clone());
            let mut endpoint_label = self
                .config
                .endpoint_label(&sock.remote_addr, sock.remote_port);
            if is_local {
                endpoint_label.push_str(" [LOCAL]");
            }
            let engine = DbEngine::from_port(sock.remote_port);

            // 1초 속도는 최근 rate_window 표본만, 평균은 10초 창 전체를 10초로 나눔
            let mut tx_bps = 0;
            let mut rx_bps = 0;
            let mut qps = 0;
            let mut avg_rx_bps = 0;
            let mut avg_queries = 0;
            let mut tx_total = 0;
            let mut rx_total = 0;
            let mut q_cnt = 0;
            let mut last_op = SqlOpKind::SocketConnect;
            let mut last_sql = String::new();
            let mut age_secs = 0;
            if let Some(st) = self.conn_states.get(&key) {
                let in_rate = |ts: &Instant| now.duration_since(*ts) <= rate_window;
                let tx_recent: u64 = st
                    .tx_samples
                    .iter()
                    .filter(|(ts, _)| in_rate(ts))
                    .map(|(_, b)| b)
                    .sum();
                let rx_recent: u64 = st
                    .rx_samples
                    .iter()
                    .filter(|(ts, _)| in_rate(ts))
                    .map(|(_, b)| b)
                    .sum();
                let q_recent = st.query_samples.iter().filter(|ts| in_rate(ts)).count() as u64;
                tx_bps = tx_recent * 1000 / window_ms;
                rx_bps = rx_recent * 1000 / window_ms;
                qps = (q_recent * 1000 / window_ms) as u32;
                avg_rx_bps = st.rx_samples.iter().map(|(_, b)| b).sum::<u64>() / avg_secs;
                avg_queries = st.query_samples.len() as u64;
                tx_total = st.tx_bytes_total;
                rx_total = st.rx_bytes_total;
                q_cnt = st.query_count;
                last_op = st.last_op;
                last_sql = st.last_sql.clone();
                age_secs = now.duration_since(st.first_seen).as_secs();
            }

            let mut conn_risk = RiskLevel::Safe;
            let mut conn_reason = RiskReason::Normal;
            if is_local {
                conn_reason = RiskReason::LocalExcluded;
            } else if avg_rx_bps >= th.crit_rx_bps {
                conn_risk = RiskLevel::Critical;
                conn_reason = RiskReason::RxSustained { avg_rx_bps };
            } else if avg_rx_bps >= th.warn_rx_bps {
                conn_risk = RiskLevel::Warning;
                conn_reason = RiskReason::RxHigh { avg_rx_bps };
            } else if avg_rx_bps >= th.caution_rx_bps {
                conn_risk = RiskLevel::Caution;
                conn_reason = RiskReason::RxRising { avg_rx_bps };
            }

            connections.push(DbConnectionEntry {
                local_addr: sock.local_addr,
                local_port: sock.local_port,
                remote_addr: sock.remote_addr,
                remote_port: sock.remote_port,
                endpoint_label: endpoint_label.clone(),
                engine,
                state: sock.state,
                pid: sock.pid,
                process_name: proc_name.clone(),
                actor_kind,
                command_line: cmdline.clone(),
                tx_bps,
                rx_bps,
                tx_bytes_total: tx_total,
                rx_bytes_total: rx_total,
                qps,
                query_count: q_cnt,
                last_op,
                last_sql_preview: last_sql.clone(),
                age_secs,
                is_local,
                risk_level: conn_risk,
                risk_reason: conn_reason,
            });

            // 로컬 DB·Redis, 소유 PID 미상(TIME_WAIT PID 0) 소켓은 위험·종료 집계 제외
            if is_local || sock.pid == 0 {
                continue;
            }

            let entry = proc_map.entry(sock.pid).or_insert_with(|| ProcessSummary {
                pid: sock.pid,
                process_name: proc_name,
                actor_kind,
                command_line: cmdline,
                target_endpoints: Vec::new(),
                established_conns: 0,
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
                last_op,
                last_sql: String::new(),
                recent_pattern_risk: RiskLevel::Safe,
                recent_pattern_note: None,
                risk_level: RiskLevel::Safe,
                risk_score: 0,
                risk_rule: None,
                risk_reasons: Vec::new(),
            });

            if !entry.target_endpoints.contains(&endpoint_label) {
                entry.target_endpoints.push(endpoint_label);
            }
            match sock.state {
                TcpConnState::Established => entry.established_conns += 1,
                TcpConnState::TimeWait => entry.time_wait_conns += 1,
                TcpConnState::SynSent => entry.syn_conns += 1,
                _ => {}
            }
            if engine == DbEngine::Redis
                && matches!(sock.state, TcpConnState::TimeWait | TcpConnState::SynSent)
            {
                entry.cache_churn_conns += 1;
            }
            entry.tx_bps += tx_bps;
            entry.rx_bps += rx_bps;
            entry.tx_bytes_total += tx_total;
            entry.rx_bytes_total += rx_total;
            entry.qps += qps;
            entry.avg_rx_bps += avg_rx_bps;
            // 연결별 쿼리 수 합계를 임시 보관, 집계 후 10초 평균으로 환산
            entry.avg_qps += avg_queries as u32;
            entry.total_queries += q_cnt;
            // 연결 자리표시보다 실제 SQL 우선
            if !last_sql.is_empty()
                && (entry.last_sql.is_empty() || last_op != SqlOpKind::SocketConnect)
            {
                entry.last_op = last_op;
                entry.last_sql = last_sql;
            }
        }
        for proc_item in proc_map.values_mut() {
            proc_item.avg_qps = (proc_item.avg_qps as u64).div_ceil(avg_secs) as u32;
        }

        // 2. SQL 통계 및 반복 횟수 반영
        let mut top_sql: Vec<SqlFingerprintStats> = self
            .sql_stats
            .values()
            .map(|st| {
                let repeat_10s = st.exec_timestamps.len() as u32;
                let avg_rx_bps = st.rx_samples.iter().map(|(_, b)| b).sum::<u64>() / avg_secs;
                if let Some(p) = proc_map.get_mut(&st.last_pid) {
                    if st.fingerprint != "EXECUTE_STMT" {
                        p.max_repeat_10s = p.max_repeat_10s.max(repeat_10s);
                    }
                    if repeat_10s > 0 && st.pattern_risk > p.recent_pattern_risk {
                        p.recent_pattern_risk = st.pattern_risk;
                        p.recent_pattern_note = st.pattern_note;
                    }
                }

                // 문장 패턴 위에 실측 부하(10초 평균 응답량·반복) 판정을 덧씌움
                let load = if repeat_10s >= th.crit_repeat_10s && avg_rx_bps >= th.warn_rx_bps {
                    Some((
                        RiskLevel::Critical,
                        RiskReason::RxLoop { repeat: repeat_10s, avg_rx_bps },
                    ))
                } else if avg_rx_bps >= th.crit_rx_bps {
                    Some((RiskLevel::Critical, RiskReason::RxSustained { avg_rx_bps }))
                } else if repeat_10s >= th.crit_repeat_10s {
                    Some((
                        RiskLevel::Warning,
                        RiskReason::RepeatLoopSuspected { repeat: repeat_10s },
                    ))
                } else if avg_rx_bps >= th.warn_rx_bps {
                    Some((RiskLevel::Warning, RiskReason::RxHigh { avg_rx_bps }))
                } else if repeat_10s >= th.warn_repeat_10s {
                    Some((RiskLevel::Caution, RiskReason::RepeatSql { repeat: repeat_10s }))
                } else if st.max_single_rx_bytes >= th.single_rx_warn_bytes {
                    Some((
                        RiskLevel::Caution,
                        RiskReason::LargeSingleResponse { bytes: st.max_single_rx_bytes },
                    ))
                } else {
                    None
                };
                let (risk_level, risk_reason) = match load {
                    Some((level, reason)) if level > st.pattern_risk => (level, Some(reason)),
                    _ => (st.pattern_risk, st.pattern_note),
                };

                SqlFingerprintStats {
                    fingerprint: st.fingerprint.clone(),
                    sample_sql: st.sample_sql.clone(),
                    op_kind: st.op_kind,
                    target_table: st.target_table.clone(),
                    last_pid: st.last_pid,
                    last_process: st.last_process.clone(),
                    actor_kind: st.actor_kind,
                    last_endpoint: st.last_endpoint.clone(),
                    exec_count: st.exec_count,
                    repeat_10s,
                    avg_rx_bps,
                    total_rx_bytes: st.total_rx_bytes,
                    max_single_rx_bytes: st.max_single_rx_bytes,
                    last_seen: st.last_seen.clone(),
                    risk_level,
                    risk_reason,
                }
            })
            .collect();

        top_sql.sort_by(|a, b| {
            b.risk_level
                .cmp(&a.risk_level)
                .then_with(|| b.total_rx_bytes.cmp(&a.total_rx_bytes))
                .then_with(|| b.exec_count.cmp(&a.exec_count))
        });
        top_sql.truncate(50);

        // 3. 프로세스별 위험도 산출, 주의 이상은 경보 이력 (팝업은 TUI 에서 위험만)
        let mut processes: Vec<ProcessSummary> = proc_map.into_values().collect();
        for proc_item in &mut processes {
            evaluate_process_risk(proc_item, &self.config);

            let Some(rule_code) = proc_item.risk_rule else {
                continue;
            };
            if proc_item.risk_level >= RiskLevel::Warning {
                let ep = proc_item
                    .target_endpoints
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "-".to_string());
                self.maybe_emit_alert(
                    now,
                    now_str,
                    AlertDraft {
                        level: proc_item.risk_level,
                        rule_code,
                        title: format!(
                            "[{}] {} (PID {})",
                            proc_item.actor_kind.badge(),
                            proc_item.process_name,
                            proc_item.pid
                        ),
                        detail: proc_item.risk_reasons.clone(),
                        pid: proc_item.pid,
                        process_name: proc_item.process_name.clone(),
                        remote_endpoint: ep,
                    },
                );
            }
        }

        processes.sort_by(|a, b| {
            b.risk_level
                .cmp(&a.risk_level)
                .then_with(|| b.risk_score.cmp(&a.risk_score))
                .then_with(|| b.avg_rx_bps.cmp(&a.avg_rx_bps))
                .then_with(|| b.established_conns.cmp(&a.established_conns))
        });

        connections.sort_by(|a, b| {
            b.risk_level
                .cmp(&a.risk_level)
                .then_with(|| b.rx_bps.cmp(&a.rx_bps))
                .then_with(|| (a.state as u8).cmp(&(b.state as u8)))
        });

        // 4. 시스템 전체 요약 지표 계산
        let total_tx_bps: u64 = processes.iter().map(|p| p.tx_bps).sum();
        let total_rx_bps: u64 = processes.iter().map(|p| p.rx_bps).sum();
        let total_qps: u32 = processes.iter().map(|p| p.qps).sum();

        if self.rx_history_kb.len() >= 60 {
            self.rx_history_kb.pop_front();
        }
        self.rx_history_kb.push_back(total_rx_bps / 1024);

        if self.qps_history.len() >= 60 {
            self.qps_history.pop_front();
        }
        self.qps_history.push_back(total_qps as u64);

        let overall_risk = processes
            .iter()
            .map(|p| p.risk_level)
            .max()
            .unwrap_or(RiskLevel::Safe);
        let overall_risk_score = processes
            .iter()
            .map(|p| p.risk_score)
            .max()
            .unwrap_or(0);

        let risk_summary = if let Some(top) = processes.first() {
            if top.risk_level >= RiskLevel::Caution {
                RiskSummary::TopProcess {
                    process_name: top.process_name.clone(),
                    pid: top.pid,
                    reasons: top.risk_reasons.clone(),
                }
            } else {
                RiskSummary::ExternalNormal { active_established }
            }
        } else if local_established > 0 {
            RiskSummary::LocalOnly { local_established }
        } else {
            RiskSummary::Idle
        };

        MonitorSnapshot {
            captured_at: now_str.to_string(),
            uptime_secs: now.duration_since(self.started_at).as_secs(),
            capture_mode: self.capture_mode.clone(),
            capture_dropped_frames: self.capture_dropped_frames,
            zero_db_query_guaranteed: true,
            overall_risk,
            overall_risk_score,
            risk_summary,
            active_established,
            active_time_wait,
            active_syn_sent,
            local_established,
            total_tx_bps,
            total_rx_bps,
            total_tx_bytes: self.total_tx_bytes,
            total_rx_bytes: self.total_rx_bytes,
            total_qps,
            total_queries: self.total_queries,
            rx_history_kb: self.rx_history_kb.iter().copied().collect(),
            qps_history: self.qps_history.iter().copied().collect(),
            processes,
            connections,
            recent_events: self.recent_events.iter().rev().cloned().collect(),
            top_sql,
            alerts: self.alerts.iter().rev().cloned().collect(),
        }
    }

    fn push_event(&mut self, event: SqlActivityEvent) {
        if self.recent_events.len() >= 150 {
            self.recent_events.pop_front();
        }
        self.recent_events.push_back(event);
    }

    // 같은 PID·규칙 경보는 등급이 오를 때 즉시, 같은 등급이면 60초마다 한 번만 기록
    fn maybe_emit_alert(&mut self, now: Instant, now_str: &str, draft: AlertDraft) {
        let dedup_key = (draft.pid, draft.rule_code);
        if let Some(&(last_ts, last_level)) = self.last_alert_by_key.get(&dedup_key)
            && draft.level <= last_level
            && now.duration_since(last_ts) < ALERT_REPEAT_GAP
        {
            return;
        }
        self.last_alert_by_key.insert(dedup_key, (now, draft.level));
        if self.last_alert_by_key.len() > 256 {
            self.last_alert_by_key
                .retain(|_, (ts, _)| now.duration_since(*ts) < ALERT_REPEAT_GAP);
        }

        // 경보 원인 추적용: 이 PID 의 최근 10초 SQL 상위 3개 (상세 패널은 원문, 로그는 핑거프린트)
        let mut recent: Vec<&SqlTrackState> = self
            .sql_stats
            .values()
            .filter(|st| st.last_pid == draft.pid && !st.exec_timestamps.is_empty())
            .collect();
        recent.sort_by_key(|st| std::cmp::Reverse(st.exec_timestamps.len()));
        recent.truncate(3);
        let sql_hint = recent
            .iter()
            .map(|st| format!("{}x {}", st.exec_timestamps.len(), st.fingerprint))
            .collect::<Vec<_>>()
            .join(" / ");
        let recent_sql: Vec<RecentSql> = recent
            .iter()
            .map(|st| RecentSql {
                repeat_10s: st.exec_timestamps.len() as u32,
                sql: st.sample_sql.clone(),
            })
            .collect();

        let log_line = format!(
            "Alert {:?} {} {}: {} -> {} | recent 10s SQL: {}",
            draft.level,
            draft.rule_code.code(),
            draft.title,
            i18n::reasons_text(Lang::En, &draft.detail, ", "),
            draft.remote_endpoint,
            if sql_hint.is_empty() { "-" } else { &sql_hint }
        );
        if draft.level >= RiskLevel::Critical {
            applog::warn(&log_line);
        } else {
            applog::info(&log_line);
        }
        if self.alerts.len() >= 150 {
            self.alerts.pop_front();
        }
        let id = self.next_alert_id;
        self.next_alert_id += 1;
        self.alerts.push_back(AlertEntry {
            id,
            timestamp: now_str.to_string(),
            level: draft.level,
            rule_code: draft.rule_code,
            title: draft.title,
            detail: draft.detail,
            pid: draft.pid,
            process_name: draft.process_name,
            remote_endpoint: draft.remote_endpoint,
            recent_sql,
        });
    }

    // 성공·실패와 중복 여부를 값으로 돌려주는 긴급 일괄 중단 (화면이 결과 문구를 표시 언어로 만듦)
    pub fn emergency_stop_checked(&mut self, processes: &[ProcessSummary]) -> StopReport {
        // 1. taskkill 대기 중 쌓인 키 반복·연속 클릭은 직전 처리 완료 후 2초간 무시
        if let Some((done, last)) = &self.last_stop
            && done.elapsed() < Duration::from_secs(2)
        {
            return StopReport {
                outcome: last.clone(),
                repeated: true,
            };
        }

        // 2. 일괄 종료 실행과 상태 정리
        applog::warn(&format!(
            "Emergency stop requested: {} displayed processes",
            processes.len()
        ));
        let outcome = terminate_related_local_processes(processes);
        if outcome.is_ok() {
            self.conn_states.clear();
            self.sql_stats.clear();
        }
        let report = StopReport {
            outcome,
            repeated: false,
        };
        applog::warn(&format!(
            "Emergency stop result: {}",
            i18n::stop_report_text(Lang::En, &report)
        ));
        self.last_stop = Some((Instant::now(), report.outcome.clone()));
        report
    }
}

struct AlertDraft {
    level: RiskLevel,
    rule_code: RiskRule,
    title: String,
    detail: Vec<RiskReason>,
    pid: u32,
    process_name: String,
    remote_endpoint: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use crate::analysis::decoder::{DecodedSql, analyze_sql_text, decode_wire_sql};

    fn build_offline_engine() -> MonitorEngine {
        let cfg = MonitorConfig {
            enable_pcap: false,
            ..MonitorConfig::default()
        };
        MonitorEngine::new(cfg)
    }

    #[test]
    fn tls_flow_is_not_counted_as_repeated_query() {
        let mut engine = build_offline_engine();
        let local_ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 25));
        let db_ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));
        let tls_payload = [
            0x17u8, 0x03, 0x03, 0x00, 0x40, 0x9a, 0x01, 0xff, 0x02, 0x00, 0x11, 0x87, 0x05, 0x06,
            0x07, 0x08,
        ];
        let now = Instant::now();

        for _ in 0..40 {
            let frame = WireFrame {
                src_ip: local_ip,
                src_port: 55101,
                dst_ip: db_ip,
                dst_port: 3306,
                payload_len: tls_payload.len(),
                decoded_sql: decode_wire_sql(3306, &tls_payload),
                captured_at: now,
            };
            engine.ingest_wire_frame(now, "00:00:00", frame, &HashMap::new(), &HashMap::new());
        }

        assert_eq!(engine.total_queries, 0);
        assert!(engine.sql_stats.is_empty());
        assert_eq!(engine.total_tx_bytes, 40 * tls_payload.len() as u64);
    }

    #[test]
    fn driver_overhead_commands_are_not_counted_as_queries() {
        let mut engine = build_offline_engine();
        let local_ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 25));
        let db_ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));
        let now = Instant::now();
        let overhead_sql = [
            "SET autocommit=0",
            "SET autocommit=1",
            "COMMIT",
            "ROLLBACK",
            "SELECT 1",
            "SELECT @@session.transaction_read_only",
            "/* mysql-connector-j-8.4.0 */SELECT @@session.auto_increment_increment",
        ];

        let com_ping = [0x01u8, 0x00, 0x00, 0x00, 0x0e];

        for _ in 0..10 {
            let mut decoded_list: Vec<DecodedSql> = overhead_sql
                .iter()
                .map(|sql| analyze_sql_text(DbEngine::MySql, sql))
                .collect();
            decoded_list.push(decode_wire_sql(3306, &com_ping).expect("com_ping"));
            for decoded in decoded_list {
                let frame = WireFrame {
                    src_ip: local_ip,
                    src_port: 55101,
                    dst_ip: db_ip,
                    dst_port: 3306,
                    payload_len: 32,
                    decoded_sql: Some(decoded),
                    captured_at: now,
                };
                engine.ingest_wire_frame(now, "00:00:00", frame, &HashMap::new(), &HashMap::new());
            }
        }
        assert_eq!(engine.total_queries, 0);
        assert!(engine.sql_stats.is_empty());

        let select_sql = "SELECT menu_id FROM tb_menu WHERE user_id = 'u1'";
        let frame = WireFrame {
            src_ip: local_ip,
            src_port: 55101,
            dst_ip: db_ip,
            dst_port: 3306,
            payload_len: select_sql.len(),
            decoded_sql: Some(analyze_sql_text(DbEngine::MySql, select_sql)),
            captured_at: now,
        };
        engine.ingest_wire_frame(now, "00:00:00", frame, &HashMap::new(), &HashMap::new());
        assert_eq!(engine.total_queries, 1);
    }

    #[test]
    fn response_bytes_attach_to_latest_sql_event_after_ring_rotation() {
        let mut engine = build_offline_engine();
        let local_ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 25));
        let db_ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));
        let now = Instant::now();
        let reply_len = 5 * 1024 * 1024;

        for idx in 0..160 {
            let sql = format!("SELECT name FROM tb_menu WHERE menu_id = {idx}");
            let frame = WireFrame {
                src_ip: local_ip,
                src_port: 55101,
                dst_ip: db_ip,
                dst_port: 3306,
                payload_len: sql.len(),
                decoded_sql: Some(analyze_sql_text(DbEngine::MySql, &sql)),
                captured_at: now,
            };
            engine.ingest_wire_frame(now, "00:00:00", frame, &HashMap::new(), &HashMap::new());
        }
        let reply = WireFrame {
            src_ip: db_ip,
            src_port: 3306,
            dst_ip: local_ip,
            dst_port: 55101,
            payload_len: reply_len,
            decoded_sql: None,
            captured_at: now,
        };
        engine.ingest_wire_frame(now, "00:00:00", reply, &HashMap::new(), &HashMap::new());

        // 링은 150건 유지, 응답은 마지막 SQL 이벤트와 그 핑거프린트에만 귀속
        assert_eq!(engine.recent_events.len(), 150);
        let last = engine.recent_events.back().expect("last event");
        assert_eq!(last.rx_bytes, reply_len as u64);
        assert!(last.risk_level >= RiskLevel::Warning);
        assert!(engine.recent_events.iter().rev().skip(1).all(|ev| ev.rx_bytes == 0));
        let stat = engine.sql_stats.values().next().expect("fingerprint");
        assert_eq!(engine.sql_stats.len(), 1);
        assert_eq!(stat.max_single_rx_bytes, reply_len as u64);
        assert_eq!(engine.total_rx_bytes, reply_len as u64);
    }

    #[test]
    fn redis_short_connections_are_caution_without_alert() {
        let mut engine = build_offline_engine();
        let local_ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 25));
        let cache_ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 30));
        let mut sockets: Vec<RawDbSocket> = (0..320)
            .map(|idx| RawDbSocket {
                local_addr: local_ip,
                local_port: 50_000 + idx,
                remote_addr: cache_ip,
                remote_port: 6379,
                state: TcpConnState::TimeWait,
                pid: 4242,
            })
            .collect();
        sockets.push(RawDbSocket {
            local_addr: local_ip,
            local_port: 51_000,
            remote_addr: cache_ip,
            remote_port: 6379,
            state: TcpConnState::Established,
            pid: 4242,
        });

        let snap = engine.build_snapshot(
            Instant::now(),
            Duration::from_secs(1),
            "00:00:00",
            &sockets,
            &HashMap::new(),
        );

        assert_eq!(snap.overall_risk, RiskLevel::Caution);
        assert!(snap.alerts.is_empty());
        assert_eq!(
            snap.processes[0].risk_reasons[0],
            RiskReason::CacheChurn { churn: 320 }
        );
    }

    #[test]
    fn repeated_emergency_stop_within_guard_is_ignored() {
        let mut engine = build_offline_engine();
        let first = engine.emergency_stop_checked(&[]);
        let second = engine.emergency_stop_checked(&[]);
        assert!(!first.repeated);
        assert!(second.repeated);
        assert_eq!(first.outcome, second.outcome);
        assert_eq!(first.outcome, Err(StopFailure::NoCritical { shown: 0 }));
    }

    #[test]
    fn ownerless_time_wait_is_not_aggregated_as_pid_zero_process() {
        let mut engine = build_offline_engine();
        let local_ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 25));
        let db_ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));
        let mut sockets: Vec<RawDbSocket> = (0..20)
            .map(|idx| RawDbSocket {
                local_addr: local_ip,
                local_port: 50_000 + idx,
                remote_addr: db_ip,
                remote_port: 3306,
                state: TcpConnState::TimeWait,
                pid: 0,
            })
            .collect();
        sockets.push(RawDbSocket {
            local_addr: local_ip,
            local_port: 51_000,
            remote_addr: db_ip,
            remote_port: 3306,
            state: TcpConnState::Established,
            pid: 4242,
        });

        let snap = engine.build_snapshot(
            Instant::now(),
            Duration::from_secs(1),
            "00:00:00",
            &sockets,
            &HashMap::new(),
        );

        assert_eq!(snap.active_time_wait, 20);
        assert!(snap.processes.iter().all(|p| p.pid != 0));
        assert_eq!(snap.overall_risk, RiskLevel::Safe);
        assert!(snap.alerts.is_empty());
    }

    #[test]
    fn same_rule_alert_is_logged_once_per_minute_unless_escalated() {
        let mut engine = build_offline_engine();
        let now = Instant::now();
        let draft = |level| AlertDraft {
            level,
            rule_code: RiskRule::QueryLoop,
            title: "[SPRING] java.exe (PID 4242)".to_string(),
            detail: vec![RiskReason::RepeatLoopSuspected { repeat: 120 }],
            pid: 4242,
            process_name: "java.exe".to_string(),
            remote_endpoint: "192.0.2.10:3306".to_string(),
        };

        // (경과 초, 등급) -> 누적 경보 수: 8초 뒤 같은 등급 생략, 상승 즉시, 60초 뒤 재기록
        for (secs, level, expected) in [
            (0, RiskLevel::Warning, 1),
            (8, RiskLevel::Warning, 1),
            (9, RiskLevel::Critical, 2),
            (70, RiskLevel::Critical, 3),
        ] {
            engine.maybe_emit_alert(now + Duration::from_secs(secs), "00:00:00", draft(level));
            assert_eq!(engine.alerts.len(), expected, "{secs}s {level:?}");
        }
    }
}
