//! model.rs
//! DB 연결, 프로세스·에이전트 분류, SQL 이벤트, 위험도·사유·종료 결과 상태 모델.

use std::net::IpAddr;
use serde::{Serialize, Serializer};
use crate::tui::i18n::{self, Lang};

// 1. DB 엔진 구분 -------------------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum DbEngine {
    MySql,
    PostgreSql,
    SqlServer,
    Oracle,
    Redis,
    MongoDb,
    Custom(u16),
}

impl DbEngine {
    pub fn from_port(port: u16) -> Self {
        match port {
            3306 | 33060 | 5002 | 6033 => Self::MySql,
            5432 => Self::PostgreSql,
            1433 => Self::SqlServer,
            1521 => Self::Oracle,
            6379 => Self::Redis,
            27017 => Self::MongoDb,
            other => Self::Custom(other),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::MySql => "MySQL",
            Self::PostgreSql => "PGSQL",
            Self::SqlServer => "MSSQL",
            Self::Oracle => "ORACLE",
            Self::Redis => "REDIS",
            Self::MongoDb => "MONGO",
            Self::Custom(_) => "DB-TCP",
        }
    }
}

// 2. 로컬 행위자 구분 ---------------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum ActorKind {
    AiAgentMcp,
    JvmSpringApp,
    DbClientCli,
    ScriptWorker,
    SystemOther,
}

impl ActorKind {
    pub fn badge(self) -> &'static str {
        match self {
            Self::AiAgentMcp => "AGENT",
            Self::JvmSpringApp => "SPRING",
            Self::DbClientCli => "CLIENT",
            Self::ScriptWorker => "SCRIPT",
            Self::SystemOther => "PROC",
        }
    }

    pub fn classify(proc_name: &str, cmdline: &str) -> Self {
        let name_low = proc_name.to_ascii_lowercase();
        let cmd_low = cmdline.to_ascii_lowercase();

        // 에이전트 및 MCP 프로세스 판별
        if cmd_low.contains("mysql-mcp")
            || cmd_low.contains("mcp-server-mysql")
            || cmd_low.contains("antigravity")
            || cmd_low.contains("claude")
            || cmd_low.contains("codex")
            || cmd_low.contains(".gemini")
            || name_low.contains("antigravity")
            || name_low.contains("claude")
        {
            return Self::AiAgentMcp;
        }

        // Bun/Node 기반 MCP 런처 판별
        if (name_low.starts_with("bun") || name_low.starts_with("node"))
            && (cmd_low.contains("mcp") || cmd_low.contains("mysql") || cmd_low.is_empty())
        {
            return Self::AiAgentMcp;
        }

        // Spring / JVM 애플리케이션 판별
        if name_low.starts_with("java")
            || name_low.starts_with("javaw")
            || name_low.starts_with("gradle")
            || name_low.starts_with("mvn")
            || cmd_low.contains("bootrun")
            || cmd_low.contains("sender-test")
            || cmd_low.contains("spring")
        {
            return Self::JvmSpringApp;
        }

        // DB 전용 클라이언트 및 덤프 도구 판별
        if name_low.starts_with("mysql")
            || name_low.starts_with("mariadb")
            || name_low.starts_with("psql")
            || name_low.starts_with("pg_dump")
            || name_low.starts_with("sqlcmd")
            || name_low.starts_with("dbeaver")
            || name_low.starts_with("datagrip")
            || name_low.starts_with("heidisql")
        {
            return Self::DbClientCli;
        }

        // 스크립트 워커 판별
        if name_low.starts_with("python")
            || name_low.starts_with("bun")
            || name_low.starts_with("node")
            || name_low.starts_with("pwsh")
            || name_low.starts_with("powershell")
            || name_low.starts_with("bash")
        {
            return Self::ScriptWorker;
        }

        Self::SystemOther
    }
}

// 3. TCP 소켓 상태 ------------------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum TcpConnState {
    Established,
    SynSent,
    TimeWait,
    CloseWait,
    FinWait,
    Other,
}

impl TcpConnState {
    // MIB_TCP_STATE 코드 매핑
    pub fn from_mib(code: u32) -> Self {
        match code {
            5 => Self::Established,
            3 | 4 => Self::SynSent,
            11 => Self::TimeWait,
            8 => Self::CloseWait,
            6 | 7 | 9 | 10 => Self::FinWait,
            _ => Self::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Established => "ESTAB",
            Self::SynSent => "SYN_SENT",
            Self::TimeWait => "TIME_WAIT",
            Self::CloseWait => "CLOSE_WAIT",
            Self::FinWait => "FIN_WAIT",
            Self::Other => "OTHER",
        }
    }
}

// 4. 위험도 등급 --------------------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum RiskLevel {
    Safe,
    Caution,
    Warning,
    Critical,
}

impl RiskLevel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Safe => "SAFE",
            Self::Caution => "CAUTION",
            Self::Warning => "WARNING",
            Self::Critical => "CRITICAL",
        }
    }
}

// 5. SQL 및 와이어 작업 종류 --------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum SqlOpKind {
    Select,
    Insert,
    Update,
    Delete,
    Ddl,
    ShowMeta,
    Transaction,
    PrepareExec,
    PingQuit,
    EncryptedFlow,
    SocketConnect,
}

impl SqlOpKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "SELECT",
            Self::Insert => "INSERT",
            Self::Update => "UPDATE",
            Self::Delete => "DELETE",
            Self::Ddl => "DDL",
            Self::ShowMeta => "SHOW/META",
            Self::Transaction => "TXN",
            Self::PrepareExec => "PREP/EXEC",
            Self::PingQuit => "PING/QUIT",
            Self::EncryptedFlow => "TLS/DATA",
            Self::SocketConnect => "CONN",
        }
    }

    pub fn is_mutation(self) -> bool {
        matches!(self, Self::Insert | Self::Update | Self::Delete | Self::Ddl)
    }
}

// 6. 개별 DB 소켓 엔트리 ------------------------------------------------------
#[derive(Debug, Clone, Serialize)]
pub struct DbConnectionEntry {
    pub local_addr: IpAddr,
    pub local_port: u16,
    pub remote_addr: IpAddr,
    pub remote_port: u16,
    pub endpoint_label: String,
    pub engine: DbEngine,
    pub state: TcpConnState,
    pub pid: u32,
    pub process_name: String,
    pub actor_kind: ActorKind,
    pub command_line: String,
    pub tx_bps: u64,
    pub rx_bps: u64,
    pub tx_bytes_total: u64,
    pub rx_bytes_total: u64,
    pub qps: u32,
    pub query_count: u64,
    pub last_op: SqlOpKind,
    pub last_sql_preview: String,
    pub age_secs: u64,
    pub is_local: bool,
    pub risk_level: RiskLevel,
    pub risk_reason: RiskReason,
}

// 7. 프로세스·에이전트별 집계 -------------------------------------------------
#[derive(Debug, Clone, Serialize)]
pub struct ProcessSummary {
    pub pid: u32,
    pub process_name: String,
    pub actor_kind: ActorKind,
    pub command_line: String,
    pub target_endpoints: Vec<String>,
    pub established_conns: usize,
    pub time_wait_conns: usize,
    pub syn_conns: usize,
    pub cache_churn_conns: usize,
    pub tx_bps: u64,
    pub rx_bps: u64,
    pub tx_bytes_total: u64,
    pub rx_bytes_total: u64,
    pub qps: u32,
    pub avg_rx_bps: u64,
    pub avg_qps: u32,
    pub total_queries: u64,
    pub max_repeat_10s: u32,
    pub last_op: SqlOpKind,
    pub last_sql: String,
    pub recent_pattern_risk: RiskLevel,
    #[serde(serialize_with = "serialize_reason_option")]
    pub recent_pattern_note: Option<RiskReason>,
    pub risk_level: RiskLevel,
    pub risk_score: u8,
    #[serde(serialize_with = "serialize_rule_option")]
    pub risk_rule: Option<RiskRule>,
    pub risk_reasons: Vec<RiskReason>,
}

// 8. 실시간 SQL 및 작업 이벤트 ------------------------------------------------
#[derive(Debug, Clone, Serialize)]
pub struct SqlActivityEvent {
    pub id: u64,
    pub timestamp: String,
    pub pid: u32,
    pub process_name: String,
    pub actor_kind: ActorKind,
    pub remote_endpoint: String,
    pub op_kind: SqlOpKind,
    pub target_table: String,
    pub sql_fingerprint: String,
    pub sql_preview: String,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub repeat_10s: u32,
    pub risk_level: RiskLevel,
    #[serde(serialize_with = "serialize_reason_option")]
    pub risk_note: Option<RiskReason>,
}

// 9. SQL 핑거프린트 통계 ------------------------------------------------------
#[derive(Debug, Clone, Serialize)]
pub struct SqlFingerprintStats {
    pub fingerprint: String,
    pub sample_sql: String,
    pub op_kind: SqlOpKind,
    pub target_table: String,
    pub last_pid: u32,
    pub last_process: String,
    pub actor_kind: ActorKind,
    pub last_endpoint: String,
    pub exec_count: u64,
    pub repeat_10s: u32,
    pub avg_rx_bps: u64,
    pub total_rx_bytes: u64,
    pub max_single_rx_bytes: u64,
    pub last_seen: String,
    pub risk_level: RiskLevel,
    #[serde(serialize_with = "serialize_reason_option")]
    pub risk_reason: Option<RiskReason>,
}

// 10. 위험 경보 엔트리 --------------------------------------------------------
#[derive(Debug, Clone, Serialize)]
pub struct AlertEntry {
    pub id: u64,
    pub timestamp: String,
    pub level: RiskLevel,
    pub rule_code: RiskRule,
    pub title: String,
    #[serde(serialize_with = "serialize_reasons_joined")]
    pub detail: Vec<RiskReason>,
    pub pid: u32,
    pub process_name: String,
    pub remote_endpoint: String,
    pub recent_sql: Vec<RecentSql>,
}

// 경보 시점 이 PID 의 최근 10초 SQL 한 건 (반복 횟수와 원문, 문구는 카탈로그)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentSql {
    pub repeat_10s: u32,
    pub sql: String,
}

// 11. 전체 관제 스냅샷 --------------------------------------------------------
#[derive(Debug, Clone, Serialize)]
pub struct MonitorSnapshot {
    pub captured_at: String,
    pub uptime_secs: u64,
    pub capture_mode: String,
    // 캡처 채널 포화로 버린 프레임 누적 수 (0 이 아니면 RX·QPS 과소 집계 가능)
    pub capture_dropped_frames: u64,
    pub zero_db_query_guaranteed: bool,
    pub overall_risk: RiskLevel,
    pub overall_risk_score: u8,
    pub risk_summary: RiskSummary,
    pub active_established: usize,
    pub active_time_wait: usize,
    pub active_syn_sent: usize,
    pub local_established: usize,
    pub total_tx_bps: u64,
    pub total_rx_bps: u64,
    pub total_tx_bytes: u64,
    pub total_rx_bytes: u64,
    pub total_qps: u32,
    pub total_queries: u64,
    pub rx_history_kb: Vec<u64>,
    pub qps_history: Vec<u64>,
    pub processes: Vec<ProcessSummary>,
    pub connections: Vec<DbConnectionEntry>,
    pub recent_events: Vec<SqlActivityEvent>,
    pub top_sql: Vec<SqlFingerprintStats>,
    pub alerts: Vec<AlertEntry>,
}

// 12. 위험 사유와 규칙 --------------------------------------------------------
// 수집기·위험 엔진은 문구 없이 값만 만들고, 문구는 tui::i18n 카탈로그가 표시 언어로 만듦
// 순서(Ord)는 종류, 값 순이라 표시 언어와 무관하게 정렬·중복 제거에 씀
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RiskReason {
    DestructiveDdl,
    UnconditionalWrite,
    BlockingFunction,
    SchemaChange,
    FullScan,
    Normal,
    LocalExcluded,
    IdleConnected,
    RxSustained { avg_rx_bps: u64 },
    RxHigh { avg_rx_bps: u64 },
    RxRising { avg_rx_bps: u64 },
    RxLoop { repeat: u32, avg_rx_bps: u64 },
    HeavyLoop { avg_rx_bps: u64, avg_qps: u32, repeat: u32 },
    QpsStorm { avg_qps: u32 },
    QpsHigh { avg_qps: u32 },
    QpsRising { avg_qps: u32 },
    RepeatLoopSuspected { repeat: u32 },
    RepeatSql { repeat: u32 },
    LargeSingleResponse { bytes: u64 },
    ConnHog { established: usize },
    ConnMulti { established: usize },
    ChurnStorm { churn: usize },
    ChurnRepeat { churn: usize },
    CacheChurn { churn: usize },
}

// 경보 종류 (경보 중복 제거 키, 코드 문자열은 JSON·표 표시용)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RiskRule {
    OverQueryRx,
    QueryLoop,
    QueryStorm,
    ConnHog,
    ConnChurn,
    DangerousSql,
}

impl RiskRule {
    pub fn code(self) -> &'static str {
        match self {
            Self::OverQueryRx => "OVER_QUERY_RX",
            Self::QueryLoop => "QUERY_LOOP",
            Self::QueryStorm => "QUERY_STORM",
            Self::ConnHog => "CONN_HOG",
            Self::ConnChurn => "CONN_CHURN",
            Self::DangerousSql => "DANGEROUS_SQL",
        }
    }
}

// 전체 진단 요약 (프로세스 이름·PID·사유는 값 그대로)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RiskSummary {
    TopProcess {
        process_name: String,
        pid: u32,
        reasons: Vec<RiskReason>,
    },
    ExternalNormal {
        active_established: usize,
    },
    LocalOnly {
        local_established: usize,
    },
    Idle,
}

// 13. 종료 결과 ---------------------------------------------------------------
// 종료·일괄 중단 결과도 값으로만 전달, 화면과 로그가 각자 언어로 문구를 만듦
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRef {
    pub process_name: String,
    pub pid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillError {
    Protected {
        process_name: String,
        pid: u32,
    },
    AlreadyExited {
        process_name: String,
        pid: u32,
    },
    PidReused {
        process_name: String,
        pid: u32,
        current_name: String,
    },
    MonitorAncestor {
        process_name: String,
        pid: u32,
    },
    AccessDenied {
        process_name: String,
        pid: u32,
        parent_pid: u32,
    },
    SpawnFailed {
        process_name: String,
        pid: u32,
        detail: String,
    },
    TaskkillFailed {
        process_name: String,
        pid: u32,
        exit_code: i32,
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopSuccess {
    AllStopped {
        stopped: Vec<ProcessRef>,
    },
    Partial {
        stopped: Vec<ProcessRef>,
        failed: Vec<KillError>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopFailure {
    NoCritical { shown: usize },
    AllFailed { failed: Vec<KillError> },
}

// repeated = 직전 처리 직후 중복 요청이라 이전 결과를 다시 돌려줌
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopReport {
    pub outcome: Result<StopSuccess, StopFailure>,
    pub repeated: bool,
}

// 14. JSON 직렬화 -------------------------------------------------------------
// 필드 이름은 그대로, 사유 값은 카탈로그의 영어 문구 (--json 은 언어 전환 없음)
impl Serialize for RiskReason {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&i18n::reason_text(Lang::En, self))
    }
}

impl Serialize for RecentSql {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&i18n::recent_sql_text(Lang::En, self))
    }
}

impl Serialize for RiskRule {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code())
    }
}

impl Serialize for RiskSummary {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&i18n::summary_text(Lang::En, self))
    }
}

// 없는 값은 빈 문자열 (기존 JSON 값 유지)
fn serialize_reason_option<S: Serializer>(
    reason: &Option<RiskReason>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match reason {
        Some(reason) => reason.serialize(serializer),
        None => serializer.serialize_str(""),
    }
}

fn serialize_rule_option<S: Serializer>(
    rule: &Option<RiskRule>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(rule.map_or("", RiskRule::code))
}

fn serialize_reasons_joined<S: Serializer>(
    reasons: &[RiskReason],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&i18n::reasons_text(Lang::En, reasons, ", "))
}
