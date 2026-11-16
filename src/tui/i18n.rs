//! i18n.rs
//! 화면 문구 카탈로그: 영어·한국어 두 벌, 기본 영어, 값이 들어가는 문구는 형식 함수.

use crate::app::model::{
    KillError, ProcessRef, RecentSql, RiskLevel, RiskReason, RiskSummary, StopFailure,
    StopReport, StopSuccess,
};
use crate::analysis::risk::format_bytes;
use crate::tui::layout::SplitAxis;

// 1. 표시 언어 ----------------------------------------------------------------
// 파일 저장 값은 en, ko
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Lang {
    #[default]
    En,
    Ko,
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Ko => "ko",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "en" => Some(Self::En),
            "ko" => Some(Self::Ko),
            _ => None,
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::En => Self::Ko,
            Self::Ko => Self::En,
        }
    }
}

// 2. 문구 카탈로그 ------------------------------------------------------------
// 항목마다 영어·한국어가 한 줄에 있어 한쪽만 빠질 수 없음, ALL 은 완결성 테스트용
macro_rules! define_messages {
    ($($name:ident => ($en:expr, $ko:expr),)+) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Msg {
            $($name,)+
        }

        impl Msg {
            pub const ALL: &'static [Msg] = &[$(Msg::$name,)+];
        }

        pub fn text(lang: Lang, msg: Msg) -> &'static str {
            match msg {
                $(Msg::$name => match lang {
                    Lang::En => $en,
                    Lang::Ko => $ko,
                },)+
            }
        }
    };
}

define_messages! {
    // 탭
    TabOverview => ("Overview", "통합관제"),
    TabProcesses => ("Agents & Processes", "에이전트·프로세스"),
    TabTopSql => ("Top SQL", "과조회SQL"),
    TabAlerts => ("Alerts", "위험경보"),

    // 앱 바
    AppBarMode => ("Mode", "모드"),
    AppBarPoll => ("Poll", "폴링"),
    AppBarWaiting => ("Waiting", "대기"),
    BadgePaused => ("Paused", "일시정지"),
    BadgePinned => ("Pinned", "최상단"),

    // 하단 칩
    ChipStop => ("Stop risky", "위험 중단"),
    ChipPinOn => ("Pin ON", "최상단 ON"),
    ChipPinOff => ("Pin OFF", "최상단 OFF"),
    ChipBlinkOn => ("Blink ON", "점멸 ON"),
    ChipBlinkOff => ("Blink OFF", "점멸 OFF"),
    ChipPause => ("Pause", "일시정지"),
    ChipResume => ("Resume", "재개"),
    ChipRefresh => ("Refresh", "새로고침"),
    ChipResetLayout => ("Reset layout", "배치 초기화"),
    ChipLanguage => ("EN", "KO"),
    ChipHelp => ("Help", "도움말"),
    ChipQuit => ("Quit", "종료"),

    // 하단 안내 (첫 낱말이 키·동작 글자)
    HintRelease => ("Release to place", "놓기 위치 확정"),
    HintCancel => ("Esc Cancel", "Esc 취소"),
    HintDragCardSize => ("Drag card size", "드래그 카드 크기"),
    HintResetLayout => ("l Reset layout", "l 기본 배치"),
    HintDragColumnWidth => ("Drag column width", "드래그 열 폭"),
    HintDoubleClickAuto => ("Double-click auto width", "더블클릭 자동 폭"),
    HintDragScroll => ("Drag scroll", "드래그 스크롤"),
    HintTrackPage => ("Track-click page", "트랙클릭 한 화면"),
    HintSelectRow => ("↑↓/kj Select row", "↑↓/kj 행 선택"),
    HintPage => ("PgUp/PgDn Page", "PgUp/PgDn 페이지"),
    HintFirstLast => ("Home/End First/Last", "Home/End 처음·끝"),
    HintScroll => ("↑↓/kj Scroll", "↑↓/kj 스크롤"),
    HintSortColumn => ("s Sort column", "s 정렬 열"),
    HintSortOrder => ("S Sort order", "S 정렬 방향"),
    HintKill => ("K Kill selected", "K 선택 종료"),
    HintStop => ("x Stop risky", "x 위험 중단"),
    HintPause => ("Space Pause", "Space 일시정지"),
    HintInterval => ("+/- Interval", "+/- 주기"),
    HintNextTab => ("Tab Next tab", "Tab 다음 탭"),
    HintAllKeys => ("? All keys", "? 전체 단축키"),

    // 도움말 겹침
    HelpTitle => ("Help", "도움말"),
    HelpClose => ("Any key or click: close", "아무 키·클릭: 닫기"),
    HelpTruncated => ("… (enlarge window to show all)", "… (창을 키우면 전체 표시)"),
    HelpSectionKeyboard => ("Keyboard", "키보드"),
    HelpSectionMouse => ("Mouse", "마우스"),
    HelpSwitchTab => ("Switch tab (Overview to Alerts)", "탭 이동 (통합관제 ~ 위험경보)"),
    HelpMoveRows => ("Move rows / scroll focused card", "선택 카드의 행 이동·스크롤"),
    HelpSort => ("Next sort column / sort order, reset", "다음 정렬 열 / 정렬 방향, 기본 순위 복귀"),
    HelpRefresh => ("Collect now", "지금 바로 수집"),
    HelpPause => ("Pause / resume live updates", "실시간 갱신 일시정지·재개"),
    HelpInterval => ("Polling interval (250 ms steps)", "폴링 주기 (250ms 단위)"),
    HelpMoveKj => ("Move up / down (like ↑ ↓)", "위 / 아래 이동 (↑ ↓ 과 동일)"),
    HelpStop => ("Stop all critical local processes (confirm)", "위험 로컬 프로세스 일괄 중단 (확인)"),
    HelpKill => ("Kill selected process (confirm)", "선택 프로세스 종료 (확인)"),
    HelpPin => ("Pin window on top on/off", "창 최상단 고정 켜기·끄기"),
    HelpBlink => ("Alarm blink on/off", "경보 점멸 켜기·끄기"),
    HelpKeyShiftArrows => ("Shift+Arrows", "Shift+방향키"),
    HelpMoveCard => ("Move focused card", "포커스 카드 위치 이동"),
    HelpResetLayout => ("Reset card size, order, widths (tab)", "현재 탭 카드 크기·순서·열 폭 기본 배치"),
    HelpLanguage => ("Switch language (EN / KO)", "표시 언어 전환 (EN / KO)"),
    HelpToggleHelp => ("Open / close help", "도움말 열기·닫기"),
    HelpQuit => ("Quit", "종료"),
    HelpKeyClickTab => ("Click tab / chip", "탭·칩 클릭"),
    HelpClickTab => ("Switch tab, run footer action", "탭 전환, 하단 동작 실행"),
    HelpKeyClickCard => ("Click card / row", "카드·행 클릭"),
    HelpClickCard => ("Focus card (thick border), select row", "카드 선택(굵은 테두리), 표 행 선택"),
    HelpKeyClickHeader => ("Click column header", "열 머리글 클릭"),
    HelpClickHeader => ("Sort: first, flipped, then reset", "정렬: 처음 방향, 반대 방향, 기본 순위"),
    HelpKeyDragHeader => ("Drag header edge", "머리글 경계 드래그"),
    HelpDragHeader => ("Column width (double-click: auto)", "열 폭 조절 (더블클릭: 자동 폭)"),
    HelpKeyDragEdge => ("Drag card edge", "카드 경계 드래그"),
    HelpDragEdge => ("Resize cards", "카드 크기 조절"),
    HelpKeyHoldTitle => ("Hold card title 0.4 s", "카드 제목 0.4초 누름"),
    HelpHoldTitle => ("Move card: drag, release to place", "카드 이동: 끌어서 놓기"),
    HelpKeyScrollbar => ("Scrollbar", "스크롤바"),
    HelpScrollbar => ("Drag thumb, click track to page", "썸 드래그, 트랙 클릭 한 화면"),
    HelpKeyWheel => ("Wheel", "휠"),
    HelpWheel => ("Scroll the card under the pointer", "포인터 아래 카드 스크롤"),

    // 카드 제목과 보조 문구
    CardRisk => ("Risk", "위험도"),
    CardConnections => ("DB Connections", "DB 연결"),
    CardTraffic => ("Inbound Traffic", "수신 트래픽"),
    CardQueryRate => ("Query Rate", "쿼리율"),
    CardProcesses => ("External DB Processes & Agents", "외부 DB 연결 프로세스·에이전트"),
    CardSockets => ("Live DB Sockets", "실시간 DB 소켓"),
    CardSqlStream => ("Live External DB Query Stream", "실시간 외부 DB 작업·쿼리 스트림"),
    CardInspector => ("Selected Process Inspector", "선택 프로세스 인스펙터"),
    CardTopSql => ("Over-query & Loop SQL Fingerprints", "과조회·반복 루프 SQL 핑거프린트 랭킹"),
    CardSqlDetail => ("Selected SQL Detail", "선택 SQL 상세"),
    CardAlertLog => ("Over-query & Over-run Alert Log", "과조회·과작동 경보 로그"),
    CardAlertDetail => ("Selected Alert Detail", "선택 경보 상세"),
    CardGate => ("Collector", "수집기"),
    HintProcesses => ("Click select · x stop risky · K kill selected", "클릭 선택 · x 위험 중단 · K 선택 종료"),
    HintSockets => ("LOCAL excluded from risk", "LOCAL 위험 제외"),
    HintSqlStream => ("Passive sniffing · zero DB load", "패시브 스니핑 · 외부 DB 부하 0"),
    HintTopSql => ("Click a row for details below", "클릭하면 하단 상세"),

    // 패널 상태
    PanelPending => ("Waiting for first poll", "집계 대기 중"),
    PanelPendingHint => ("The first snapshot arrives within a poll interval", "첫 스냅샷은 폴링 주기 안에 도착"),
    EmptyProcesses => ("No local process is connected to a watched external DB", "감시 대상 외부 DB에 연결된 로컬 프로세스 없음"),
    EmptyProcessesHint => ("Rows appear automatically once a connection opens", "연결이 생기면 자동 표시"),
    EmptySockets => ("No live DB sockets", "실시간 DB 소켓 없음"),
    EmptySocketsHint => ("Watched ports and config.toml hosts are checked", "감시 포트와 config.toml 호스트 확인 중"),
    EmptyStream => ("No query seen yet", "관찰된 쿼리 없음"),
    EmptyStreamHint => ("Plain-text MySQL and PostgreSQL traffic shows here", "평문 MySQL·PostgreSQL 통신이 잡히면 표시"),
    EmptyTopSql => ("No external DB SQL observed", "관찰된 외부 DB SQL 없음"),
    EmptyTopSqlHint => ("Plain-text MySQL and PostgreSQL traffic shows here", "평문 MySQL·PostgreSQL 통신이 잡히면 표시"),
    EmptyAlerts => ("No alerts", "발생한 경보 없음"),
    EmptyAlertsHint => ("Only Caution and above are recorded", "관찰 이상 등급만 기록"),
    EmptyAlertDetail => ("No alert selected", "선택된 경보 없음"),
    EmptyAlertDetailHint => ("Select a row in the alert log", "경보 로그에서 행 선택"),
    EmptyInspector => ("No process selected", "선택된 프로세스 없음"),
    EmptyInspectorHint => ("Select a row in the process table", "프로세스 표에서 행 선택"),
    EmptySqlDetail => ("No SQL selected", "선택된 SQL 없음"),
    EmptySqlDetailHint => ("Select a row in the fingerprint ranking", "랭킹 표에서 행 선택"),

    // 표 열 머리글
    ColKind => ("Kind", "구분"),
    ColPid => ("PID", "PID"),
    ColProcess => ("Process", "프로세스"),
    ColEstab => ("ESTAB", "ESTAB"),
    ColWait => ("WAIT", "WAIT"),
    ColAvgRx => ("Avg RX/s", "평균RX/s"),
    ColAvgQps => ("Avg QPS", "평균QPS"),
    ColRepeat10s => ("10s Rep", "10s반복"),
    ColRisk => ("Risk", "위험도"),
    ColRiskShort => ("Risk", "위험"),
    ColEndpoint => ("External DB Endpoint", "외부 DB 엔드포인트"),
    ColState => ("State", "상태"),
    ColRxPerSec => ("RX/s", "RX/s"),
    ColTime => ("Time", "시각"),
    ColActor => ("Actor", "행위자"),
    ColOp => ("Op", "작업"),
    ColTable => ("Table", "테이블"),
    ColRxSize => ("RX Size", "응답크기"),
    ColPayload => ("Live SQL / Wire Payload", "실시간 SQL / 와이어 페이로드"),
    ColExecCount => ("Executions", "실행횟수"),
    ColTotalRx => ("Total RX", "누적응답(RX)"),
    ColMaxSingle => ("Max Single", "최대단일응답"),
    ColLastProcess => ("Last Process", "최근프로세스"),
    ColDiagnosis => ("Diagnosis / Last SQL", "진단 / 최근 SQL"),
    ColLevel => ("Level", "등급"),
    ColRule => ("Rule", "규칙"),
    ColTitle => ("Title", "제목"),
    ColDetail => ("Detail", "내용"),

    // 지표 카드와 값 라벨
    MetricEstab => ("ESTAB", "ESTAB"),
    MetricWait => ("WAIT", "WAIT"),
    MetricSyn => ("SYN", "SYN"),
    MetricRx => ("RX", "RX"),
    MetricTx => ("TX", "TX"),
    MetricQps => ("QPS", "QPS"),
    RiskSafe => ("Safe", "정상"),
    RiskCaution => ("Watch", "관찰"),
    RiskWarning => ("Warning", "주의"),
    RiskCritical => ("Critical", "위험"),
    GuideSafe => ("All clear", "이상 없음"),
    GuideCaution => ("For reference (not a risk)", "참고용 표시 (위험 아님)"),
    GuideWarning => ("Load signs, watch the trend", "부하 징후, 추이 확인"),
    GuideCritical => ("DB at risk, act now", "DB 피해 우려, 즉시 조치"),
    SocketExcluded => ("Excluded", "제외"),

    // 인스펙터·상세 라벨
    LabelProcess => ("Process", "프로세스"),
    LabelTargetDb => ("Target DB", "대상 DB"),
    LabelDiagnosis => ("Diagnosis", "진단 사유"),
    LabelCommand => ("Command", "커맨드라인"),
    LabelLastSql => ("Last SQL", "최근 SQL"),
    LabelExecution => ("Execution", "실행"),
    LabelActor => ("Actor", "행위자"),
    LabelFingerprint => ("Fingerprint", "핑거프린트"),
    LabelFullSql => ("Full SQL", "SQL 원문"),
    LabelReason => ("Reason", "사유"),
    LabelRecentSql => ("Recent SQL", "최근 SQL"),
    PayloadWaiting => ("(waiting for wire payload)", "(와이어 페이로드 대기 중)"),
    NoteNone => ("No findings", "특이사항 없음"),
    NoteNoRecentSql => ("(no plain-text SQL seen at alert time)", "(경보 시점 관찰된 평문 SQL 없음)"),

    // 위험 사유 (값이 없는 것)
    ReasonDestructiveDdl => ("Destructive DDL (DROP/TRUNCATE)", "파괴적 DDL (DROP/TRUNCATE)"),
    ReasonUnconditionalWrite => ("Update/delete without WHERE", "WHERE 절 없는 전체 변경/삭제"),
    ReasonBlockingFunction => ("Blocking or load function call", "DB 블로킹/부하 함수 호출"),
    ReasonSchemaChange => ("Schema change DDL (ALTER)", "스키마 변경 DDL (ALTER)"),
    ReasonFullScan => ("Full select without WHERE or LIMIT", "조건·LIMIT 없는 전체 조회"),
    ReasonNormal => ("Normal", "정상"),
    ReasonLocalExcluded => ("Local traffic (excluded from risk)", "로컬 통신 (위험 제외)"),
    ReasonIdleConnected => ("Connected, idle", "정상 연결 유휴 상태"),
    SummaryIdle => (
        "No local process or agent is connected to an external DB (idle)",
        "현재 외부 DB에 연결된 로컬 프로세스·에이전트 없음 (유휴)"
    ),

    // 위험 경보 카드와 종료 확인
    SirenAction => ("[ Click or X: stop risky processes (confirm) ]", "[ 클릭 또는 X 키: 위험 프로세스 중단 (확인) ]"),
    SirenBadge => ("[CRITICAL]", "[위험 경보]"),
    KillTitle => ("Force-terminate local process", "로컬 프로세스 강제 종료"),
    KillCommand => ("taskkill /T /F", "taskkill /T /F"),
    KillYes => ("Y  Kill now", "Y 즉시 강제 종료"),
    KillNo => ("N / Esc  Cancel", "N / Esc 취소"),
    StopTitle => ("Emergency stop: critical local processes", "긴급 중단: 위험 로컬 프로세스"),
    StopTargets => ("Targets", "대상 수"),
    KillTarget => ("Target", "대상"),
    KillState => ("State", "상태"),

    // 알림
    ToastLanguage => ("Language: English", "표시 언어: 한국어"),
    ToastLayoutReset => ("Layout reset to default", "기본 배치로 복원"),
    ToastCardMoved => ("Card order changed (l: reset)", "카드 순서 변경 (l: 기본 배치)"),
    ToastSplitDrag => ("Drag card edge · release to keep · l to reset", "카드 경계 드래그 · 놓으면 고정 · l 기본 배치"),
    ToastColumnDrag => ("Drag column width · double-click for auto", "열 폭 드래그 · 더블클릭 자동 폭"),
    ToastColumnsAuto => ("Column widths set to auto", "열 폭 자동으로 복원"),
    ToastPaused => ("Live updates paused", "화면 갱신 일시정지"),
    ToastResumed => ("Live monitoring resumed", "실시간 감시 재개"),
    ToastPinned => ("Window pinned on top", "창 최상단 고정"),
    ToastUnpinned => ("Window unpinned", "창 최상단 고정 해제"),
    ToastBlinkOn => ("Alarm blink ON", "경보 점멸 ON"),
    ToastBlinkOff => ("Alarm blink OFF (colors held)", "경보 점멸 OFF (경보 색 고정)"),
    ToastRefresh => ("Collecting now", "지금 수집 요청"),
    ToastStopRequested => ("Emergency stop requested", "긴급 중단 요청"),
    ToastKillRequested => ("Kill requested", "종료 요청"),
    ToastKillCancelled => ("Kill cancelled", "프로세스 종료 취소"),
    ToastNoCritical => ("No critical process to stop", "중단할 위험 프로세스 없음"),
    ToastStateSaved => ("State saved", "상태 저장됨"),
    ToastConfigNone => ("No config file, default ports only", "설정 파일 없음 (기본 감시 포트만 사용)"),
}

// 3. 값이 들어가는 문구 -------------------------------------------------------
pub fn too_small(lang: Lang, min_width: u16, min_height: u16, width: u16, height: u16) -> String {
    match lang {
        Lang::En => format!("Terminal too small (min {min_width}x{min_height}, now {width}x{height})"),
        Lang::Ko => format!("터미널 크기 부족 (최소 {min_width}x{min_height}, 현재 {width}x{height})"),
    }
}

pub fn config_loaded(lang: Lang, path: &str) -> String {
    match lang {
        Lang::En => format!("Config loaded: {path}"),
        Lang::Ko => format!("설정 파일 로드: {path}"),
    }
}

pub fn stale(lang: Lang, age_secs: u64) -> String {
    match lang {
        Lang::En => format!("stale {age_secs}s"),
        Lang::Ko => format!("지연 {age_secs}초"),
    }
}

pub fn uptime(lang: Lang, secs: u64) -> String {
    match lang {
        Lang::En => format!("Up {secs}s"),
        Lang::Ko => format!("가동 {secs}초"),
    }
}

pub fn capture_drops(lang: Lang, dropped: u64) -> String {
    match lang {
        Lang::En => format!("Capture drops {dropped}"),
        Lang::Ko => format!("캡처 누락 {dropped}"),
    }
}

pub fn local_excluded(lang: Lang, count: usize) -> String {
    match lang {
        Lang::En => format!("{count} local excluded"),
        Lang::Ko => format!("로컬 {count}개 제외"),
    }
}

pub fn total_bytes(lang: Lang, size: &str) -> String {
    match lang {
        Lang::En => format!("Total {size}"),
        Lang::Ko => format!("누적 {size}"),
    }
}

pub fn total_queries(lang: Lang, count: u64) -> String {
    match lang {
        Lang::En => format!("Total {count}"),
        Lang::Ko => format!("누적 {count}건"),
    }
}

pub fn alert_count(lang: Lang, count: usize) -> String {
    match lang {
        Lang::En => format!("{count} alerts"),
        Lang::Ko => format!("경보 {count}건"),
    }
}

pub fn alert_log_hint(lang: Lang, count: usize) -> String {
    match lang {
        Lang::En => format!("{count} alerts · click for details below"),
        Lang::Ko => format!("{count}건 · 클릭하면 하단 상세"),
    }
}

pub fn times(lang: Lang, count: impl std::fmt::Display) -> String {
    match lang {
        Lang::En => format!("{count}x"),
        Lang::Ko => format!("{count}회"),
    }
}

pub fn sort_title(lang: Lang, title: &str, label: &str, arrow: &str) -> String {
    match lang {
        Lang::En => format!("{title} · Sort {label}{arrow}"),
        Lang::Ko => format!("{title} · 정렬 {label}{arrow}"),
    }
}

pub fn card_moving(lang: Lang, title: &str) -> String {
    match lang {
        Lang::En => format!("{title} · moving"),
        Lang::Ko => format!("{title} · 이동 중"),
    }
}

pub fn card_resized(lang: Lang, axis: SplitAxis, before: u16, after: u16) -> String {
    let (en_unit, ko_unit) = match axis {
        SplitAxis::Columns => ("cols", "열"),
        SplitAxis::Rows => ("rows", "행"),
    };
    match lang {
        Lang::En => format!("Card size {before}:{after} {en_unit} (l: reset)"),
        Lang::Ko => format!("카드 크기 {before}{ko_unit} : {after}{ko_unit} (l: 기본 배치)"),
    }
}

pub fn column_width(lang: Lang, width: u16) -> String {
    match lang {
        Lang::En => format!("Column width {width}"),
        Lang::Ko => format!("열 폭 {width}"),
    }
}

pub fn interval_changed(lang: Lang, millis: u64) -> String {
    match lang {
        Lang::En => format!("Polling interval {millis}ms"),
        Lang::Ko => format!("폴링 주기 {millis}ms"),
    }
}

pub fn slow_collect(lang: Lang, millis: u128) -> String {
    match lang {
        Lang::En => format!("Slow collection {millis}ms"),
        Lang::Ko => format!("수집 지연 {millis}ms"),
    }
}

pub fn stop_done(lang: Lang, detail: &str) -> String {
    match lang {
        Lang::En => format!("Emergency stop: {detail}"),
        Lang::Ko => format!("긴급 중단: {detail}"),
    }
}

pub fn kill_done(lang: Lang, name: &str, pid: u32) -> String {
    match lang {
        Lang::En => format!("Terminated {name} (PID {pid})"),
        Lang::Ko => format!("{name} (PID {pid}) 강제 종료 완료"),
    }
}

pub fn stop_target_count(lang: Lang, count: usize) -> String {
    match lang {
        Lang::En => format!("{count} processes"),
        Lang::Ko => format!("{count}개 프로세스"),
    }
}

pub fn stop_more(lang: Lang, count: usize) -> String {
    match lang {
        Lang::En => format!("  +{count} more"),
        Lang::Ko => format!("  외 {count}개"),
    }
}

pub fn kill_failed(lang: Lang, detail: &str) -> String {
    match lang {
        Lang::En => format!("Kill failed: {detail}"),
        Lang::Ko => format!("종료 실패: {detail}"),
    }
}

pub fn risk_label(lang: Lang, level: RiskLevel) -> &'static str {
    text(
        lang,
        match level {
            RiskLevel::Safe => Msg::RiskSafe,
            RiskLevel::Caution => Msg::RiskCaution,
            RiskLevel::Warning => Msg::RiskWarning,
            RiskLevel::Critical => Msg::RiskCritical,
        },
    )
}

pub fn risk_guidance(lang: Lang, level: RiskLevel) -> &'static str {
    text(
        lang,
        match level {
            RiskLevel::Safe => Msg::GuideSafe,
            RiskLevel::Caution => Msg::GuideCaution,
            RiskLevel::Warning => Msg::GuideWarning,
            RiskLevel::Critical => Msg::GuideCritical,
        },
    )
}

// 4. 상태 파일 안내 -----------------------------------------------------------
// 읽을 수 없는 사유: 시스템·파서 메시지는 번역하지 않음
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateIssue {
    Detail(String),
    UnsupportedVersion(i64),
}

pub fn state_unreadable(lang: Lang, file: &str, issue: &StateIssue) -> String {
    let reason = match (issue, lang) {
        (StateIssue::Detail(detail), _) => detail.clone(),
        (StateIssue::UnsupportedVersion(version), Lang::En) => format!("unsupported version {version}"),
        (StateIssue::UnsupportedVersion(version), Lang::Ko) => format!("지원하지 않는 버전 {version}"),
    };
    match lang {
        Lang::En => format!("{file} unreadable, using defaults ({reason}), backup kept"),
        Lang::Ko => format!("{file} 읽기 실패, 기본값 사용 ({reason}), 원본 .bak 보존"),
    }
}

pub fn state_entries_skipped(lang: Lang, file: &str, skipped: usize) -> String {
    match lang {
        Lang::En => format!("{file}: {skipped} entries ignored"),
        Lang::Ko => format!("{file}: 일부 항목 무시 {skipped}건"),
    }
}

pub fn state_save_failed(lang: Lang, detail: &str) -> String {
    match lang {
        Lang::En => format!("State save failed: {detail}"),
        Lang::Ko => format!("상태 저장 실패: {detail}"),
    }
}

// 5. 도움말 줄 ----------------------------------------------------------------
// (키, 뜻) 목록, 뜻이 빈 줄은 구역 제목
pub fn help_rows(lang: Lang) -> Vec<(&'static str, &'static str)> {
    let t = |msg: Msg| text(lang, msg);
    vec![
        (t(Msg::HelpSectionKeyboard), ""),
        ("Tab / Shift+Tab / 1-4", t(Msg::HelpSwitchTab)),
        ("↑ ↓ PgUp PgDn Home End", t(Msg::HelpMoveRows)),
        ("k / j", t(Msg::HelpMoveKj)),
        ("s / S", t(Msg::HelpSort)),
        ("r", t(Msg::HelpRefresh)),
        ("Space / p", t(Msg::HelpPause)),
        ("+ / -", t(Msg::HelpInterval)),
        ("x / F9 / F12", t(Msg::HelpStop)),
        ("K", t(Msg::HelpKill)),
        ("t", t(Msg::HelpPin)),
        ("b", t(Msg::HelpBlink)),
        (t(Msg::HelpKeyShiftArrows), t(Msg::HelpMoveCard)),
        ("l", t(Msg::HelpResetLayout)),
        ("L", t(Msg::HelpLanguage)),
        ("?", t(Msg::HelpToggleHelp)),
        ("q / Esc / Ctrl+C", t(Msg::HelpQuit)),
        (t(Msg::HelpSectionMouse), ""),
        (t(Msg::HelpKeyClickTab), t(Msg::HelpClickTab)),
        (t(Msg::HelpKeyClickCard), t(Msg::HelpClickCard)),
        (t(Msg::HelpKeyClickHeader), t(Msg::HelpClickHeader)),
        (t(Msg::HelpKeyDragHeader), t(Msg::HelpDragHeader)),
        (t(Msg::HelpKeyDragEdge), t(Msg::HelpDragEdge)),
        (t(Msg::HelpKeyHoldTitle), t(Msg::HelpHoldTitle)),
        (t(Msg::HelpKeyScrollbar), t(Msg::HelpScrollbar)),
        (t(Msg::HelpKeyWheel), t(Msg::HelpWheel)),
    ]
}

// 6. 위험 사유·진단 요약·종료 결과 --------------------------------------------
// 값(숫자·이름)만 받아 표시 언어 문구를 만듦, 바이트 단위·이름·PID 는 번역하지 않음
pub fn reason_text(lang: Lang, reason: &RiskReason) -> String {
    let rate = |bps: u64| format!("{}/s", format_bytes(bps));
    match *reason {
        RiskReason::DestructiveDdl => text(lang, Msg::ReasonDestructiveDdl).to_string(),
        RiskReason::UnconditionalWrite => text(lang, Msg::ReasonUnconditionalWrite).to_string(),
        RiskReason::BlockingFunction => text(lang, Msg::ReasonBlockingFunction).to_string(),
        RiskReason::SchemaChange => text(lang, Msg::ReasonSchemaChange).to_string(),
        RiskReason::FullScan => text(lang, Msg::ReasonFullScan).to_string(),
        RiskReason::Normal => text(lang, Msg::ReasonNormal).to_string(),
        RiskReason::LocalExcluded => text(lang, Msg::ReasonLocalExcluded).to_string(),
        RiskReason::IdleConnected => text(lang, Msg::ReasonIdleConnected).to_string(),
        RiskReason::RxSustained { avg_rx_bps } => match lang {
            Lang::En => format!("Sustained bulk query (10s avg {})", rate(avg_rx_bps)),
            Lang::Ko => format!("대량 조회 지속 (10초 평균 {})", rate(avg_rx_bps)),
        },
        RiskReason::RxHigh { avg_rx_bps } => match lang {
            Lang::En => format!("High query volume (10s avg {})", rate(avg_rx_bps)),
            Lang::Ko => format!("조회량 많음 (10초 평균 {})", rate(avg_rx_bps)),
        },
        RiskReason::RxRising { avg_rx_bps } => match lang {
            Lang::En => format!("Rising query volume (10s avg {})", rate(avg_rx_bps)),
            Lang::Ko => format!("조회량 증가 (10초 평균 {})", rate(avg_rx_bps)),
        },
        RiskReason::RxLoop { repeat, avg_rx_bps } => match lang {
            Lang::En => format!("Bulk query loop: {repeat}x in 10s, avg {}", rate(avg_rx_bps)),
            Lang::Ko => format!("10초 내 {repeat}회 반복, 평균 {} 대량 조회 루프", rate(avg_rx_bps)),
        },
        RiskReason::HeavyLoop { avg_rx_bps, avg_qps, repeat } => match lang {
            Lang::En => format!(
                "Repeated bulk query loop (10s avg {}, {avg_qps} QPS, same SQL {repeat}x)",
                rate(avg_rx_bps)
            ),
            Lang::Ko => format!(
                "반복 대량 조회 루프 (10초 평균 {}, {avg_qps} QPS, 동일 SQL {repeat}회)",
                rate(avg_rx_bps)
            ),
        },
        RiskReason::QpsStorm { avg_qps } => match lang {
            Lang::En => format!("Query storm (10s avg {avg_qps} QPS)"),
            Lang::Ko => format!("쿼리 폭주 (10초 평균 {avg_qps} QPS)"),
        },
        RiskReason::QpsHigh { avg_qps } => match lang {
            Lang::En => format!("High query rate (10s avg {avg_qps} QPS)"),
            Lang::Ko => format!("쿼리 빈도 높음 (10초 평균 {avg_qps} QPS)"),
        },
        RiskReason::QpsRising { avg_qps } => match lang {
            Lang::En => format!("Rising query rate (10s avg {avg_qps} QPS)"),
            Lang::Ko => format!("쿼리 빈도 증가 (10초 평균 {avg_qps} QPS)"),
        },
        RiskReason::RepeatLoopSuspected { repeat } => match lang {
            Lang::En => format!("Same SQL loop suspected ({repeat}x in 10s)"),
            Lang::Ko => format!("동일 SQL 반복 루프 의심 (10초 내 {repeat}회)"),
        },
        RiskReason::RepeatSql { repeat } => match lang {
            Lang::En => format!("Same SQL repeated ({repeat}x in 10s)"),
            Lang::Ko => format!("동일 SQL 반복 (10초 내 {repeat}회)"),
        },
        RiskReason::LargeSingleResponse { bytes } => match lang {
            Lang::En => format!("Large single response ({})", format_bytes(bytes)),
            Lang::Ko => format!("단건 대용량 응답 ({})", format_bytes(bytes)),
        },
        RiskReason::ConnHog { established } => match lang {
            Lang::En => format!("Connection hog (ESTAB {established})"),
            Lang::Ko => format!("커넥션 과점유 (ESTAB {established}개)"),
        },
        RiskReason::ConnMulti { established } => match lang {
            Lang::En => format!("Many connections held (ESTAB {established})"),
            Lang::Ko => format!("다중 커넥션 점유 (ESTAB {established}개)"),
        },
        RiskReason::ChurnStorm { churn } => match lang {
            Lang::En => format!("Short-lived connection storm (WAIT/SYN {churn})"),
            Lang::Ko => format!("단기 커넥션 폭주 (WAIT/SYN {churn}개)"),
        },
        RiskReason::ChurnRepeat { churn } => match lang {
            Lang::En => format!("Repeated short-lived connections (WAIT/SYN {churn})"),
            Lang::Ko => format!("단기 커넥션 반복 (WAIT/SYN {churn}개)"),
        },
        RiskReason::CacheChurn { churn } => match lang {
            Lang::En => format!("Repeated short Redis connections (WAIT/SYN {churn})"),
            Lang::Ko => format!("Redis 단기 연결 반복 (WAIT/SYN {churn}개)"),
        },
    }
}

pub fn reasons_text(lang: Lang, reasons: &[RiskReason], separator: &str) -> String {
    reasons
        .iter()
        .map(|reason| reason_text(lang, reason))
        .collect::<Vec<_>>()
        .join(separator)
}

pub fn summary_text(lang: Lang, summary: &RiskSummary) -> String {
    match summary {
        RiskSummary::TopProcess { process_name, pid, reasons } => {
            format!("{process_name} (PID {pid}): {}", reasons_text(lang, reasons, " / "))
        }
        RiskSummary::ExternalNormal { active_established } => match lang {
            Lang::En => format!("{active_established} active external DB connections, normal range"),
            Lang::Ko => format!("외부 DB 활성 연결 {active_established}개 정상 범위"),
        },
        RiskSummary::LocalOnly { local_established } => match lang {
            Lang::En => format!(
                "No external DB connection ({local_established} local DB/Redis excluded from risk)"
            ),
            Lang::Ko => format!("외부 DB 연결 없음 (로컬 DB·Redis {local_established}개는 위험 제외)"),
        },
        RiskSummary::Idle => text(lang, Msg::SummaryIdle).to_string(),
    }
}

pub fn recent_sql_text(lang: Lang, entry: &RecentSql) -> String {
    match lang {
        Lang::En => format!("{}x in 10s | {}", entry.repeat_10s, entry.sql),
        Lang::Ko => format!("10초 내 {}회 | {}", entry.repeat_10s, entry.sql),
    }
}

pub fn kill_error_text(lang: Lang, error: &KillError) -> String {
    match (error, lang) {
        (KillError::Protected { process_name, pid }, Lang::En) => {
            format!("Protected system or own process, cannot terminate: {process_name}({pid})")
        }
        (KillError::Protected { process_name, pid }, Lang::Ko) => {
            format!("시스템/자체 보호 프로세스 종료 불가: {process_name}({pid})")
        }
        (KillError::AlreadyExited { process_name, pid }, Lang::En) => {
            format!("Process already exited: {process_name}({pid})")
        }
        (KillError::AlreadyExited { process_name, pid }, Lang::Ko) => {
            format!("이미 종료된 프로세스: {process_name}({pid})")
        }
        (KillError::PidReused { process_name, pid, current_name }, Lang::En) => {
            format!("Kill held, PID may be reused: {process_name}({pid}), now {current_name}")
        }
        (KillError::PidReused { process_name, pid, current_name }, Lang::Ko) => {
            format!("PID 재사용 의심으로 종료 보류: {process_name}({pid}), 현재 {current_name}")
        }
        (KillError::MonitorAncestor { process_name, pid }, Lang::En) => {
            format!("Parent of this monitor, cannot terminate: {process_name}({pid})")
        }
        (KillError::MonitorAncestor { process_name, pid }, Lang::Ko) => {
            format!("모니터 상위 프로세스라 종료 불가: {process_name}({pid})")
        }
        (KillError::AccessDenied { process_name, pid, parent_pid }, Lang::En) => format!(
            "{process_name}({pid}) access denied (parent PID {parent_pid}): if it runs elevated, \
             start the monitor as administrator or stop it in its own app"
        ),
        (KillError::AccessDenied { process_name, pid, parent_pid }, Lang::Ko) => format!(
            "{process_name}({pid}) 종료 권한 없음 (상위 PID {parent_pid}): 관리자 권한 프로세스면 \
             모니터를 관리자 권한으로 실행하거나 해당 앱에서 직접 중지"
        ),
        (KillError::SpawnFailed { process_name, pid, detail }, Lang::En) => {
            format!("taskkill could not start: {process_name}({pid}) {detail}")
        }
        (KillError::SpawnFailed { process_name, pid, detail }, Lang::Ko) => {
            format!("taskkill 실행 실패: {process_name}({pid}) {detail}")
        }
        (KillError::TaskkillFailed { process_name, pid, exit_code, detail }, Lang::En) => format!(
            "{process_name}({pid}) termination failed (taskkill exit code {exit_code}): {detail}"
        ),
        (KillError::TaskkillFailed { process_name, pid, exit_code, detail }, Lang::Ko) => {
            format!("{process_name}({pid}) 종료 실패 (taskkill 종료코드 {exit_code}): {detail}")
        }
    }
}

// 일괄 중단 결과 문구 (직전 결과 재사용이면 끝에 안내)
pub fn stop_report_text(lang: Lang, report: &StopReport) -> String {
    let names = |list: &[ProcessRef]| {
        list.iter()
            .map(|item| format!("{}({})", item.process_name, item.pid))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let errors = |list: &[KillError]| {
        list.iter()
            .map(|error| kill_error_text(lang, error))
            .collect::<Vec<_>>()
            .join(" / ")
    };
    let body = match (&report.outcome, lang) {
        (Ok(StopSuccess::AllStopped { stopped }), Lang::En) => {
            format!("All stopped ({}): {}", stopped.len(), names(stopped))
        }
        (Ok(StopSuccess::AllStopped { stopped }), Lang::Ko) => {
            format!("긴급 일괄 중단 완료 ({}건): {}", stopped.len(), names(stopped))
        }
        (Ok(StopSuccess::Partial { stopped, failed }), Lang::En) => format!(
            "{} stopped ({}) / {} failed: {}",
            stopped.len(),
            names(stopped),
            failed.len(),
            errors(failed)
        ),
        (Ok(StopSuccess::Partial { stopped, failed }), Lang::Ko) => format!(
            "일괄 중단 {}건 성공 ({}) / {}건 실패: {}",
            stopped.len(),
            names(stopped),
            failed.len(),
            errors(failed)
        ),
        (Err(StopFailure::NoCritical { shown }), Lang::En) => format!(
            "No critical process (all {shown} shown are Warning or below, or protected; \
             use K to kill one)"
        ),
        (Err(StopFailure::NoCritical { shown }), Lang::Ko) => format!(
            "위험 등급 프로세스 없음 (표시 {shown}건 모두 주의 이하 또는 보호 대상, 개별 종료는 K)"
        ),
        (Err(StopFailure::AllFailed { failed }), Lang::En) => {
            format!("Stop failed: {}", errors(failed))
        }
        (Err(StopFailure::AllFailed { failed }), Lang::Ko) => {
            format!("일괄 중단 실패: {}", errors(failed))
        }
    };
    match (report.repeated, lang) {
        (false, _) => body,
        (true, Lang::En) => format!("{body} (repeat request within 2s ignored)"),
        (true, Lang::Ko) => format!("{body} (2초 내 중복 요청 무시)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1. 카탈로그 완결성
    #[test]
    fn every_message_has_both_languages_and_english_is_the_default() {
        assert_eq!(Lang::default(), Lang::En);
        for msg in Msg::ALL {
            assert!(!text(Lang::En, *msg).is_empty(), "{msg:?} en");
            assert!(!text(Lang::Ko, *msg).is_empty(), "{msg:?} ko");
        }
    }

    // 2. 영어 문구의 한글 혼입 방지
    #[test]
    fn english_entries_hold_no_hangul_and_korean_entries_differ_where_translated() {
        let hangul = |value: &str| value.chars().any(|ch| ('\u{AC00}'..='\u{D7A3}').contains(&ch));
        for msg in Msg::ALL {
            assert!(!hangul(text(Lang::En, *msg)), "{msg:?}");
        }
        assert_ne!(text(Lang::En, Msg::TabAlerts), text(Lang::Ko, Msg::TabAlerts));
    }

    // 3. 언어 코드 왕복
    #[test]
    fn language_codes_round_trip_and_unknown_codes_are_rejected() {
        for lang in [Lang::En, Lang::Ko] {
            assert_eq!(Lang::from_code(lang.code()), Some(lang));
        }
        assert_eq!(Lang::from_code("EN"), None);
        assert_eq!(Lang::from_code("fr"), None);
        assert_eq!(Lang::En.toggled(), Lang::Ko);
        assert_eq!(Lang::Ko.toggled(), Lang::En);
    }

    // 4. 도움말 구역
    #[test]
    fn help_rows_open_with_keyboard_and_mouse_sections_in_both_languages() {
        for lang in [Lang::En, Lang::Ko] {
            let rows = help_rows(lang);
            let sections: Vec<&str> = rows
                .iter()
                .filter(|(_, meaning)| meaning.is_empty())
                .map(|(title, _)| *title)
                .collect();
            assert_eq!(
                sections,
                [text(lang, Msg::HelpSectionKeyboard), text(lang, Msg::HelpSectionMouse)]
            );
        }
    }

    // 5. 위험 사유 문구: 영어에 한글이 없고 한국어는 영어와 다름
    #[test]
    fn every_reason_reads_in_both_languages_and_english_holds_no_hangul() {
        let hangul = |value: &str| value.chars().any(|ch| ('\u{AC00}'..='\u{D7A3}').contains(&ch));
        let reasons = [
            RiskReason::DestructiveDdl,
            RiskReason::UnconditionalWrite,
            RiskReason::BlockingFunction,
            RiskReason::SchemaChange,
            RiskReason::FullScan,
            RiskReason::Normal,
            RiskReason::LocalExcluded,
            RiskReason::IdleConnected,
            RiskReason::RxSustained { avg_rx_bps: 5 << 20 },
            RiskReason::RxHigh { avg_rx_bps: 600 << 10 },
            RiskReason::RxRising { avg_rx_bps: 300 << 10 },
            RiskReason::RxLoop { repeat: 90, avg_rx_bps: 2 << 20 },
            RiskReason::HeavyLoop { avg_rx_bps: 2 << 20, avg_qps: 30, repeat: 90 },
            RiskReason::QpsStorm { avg_qps: 300 },
            RiskReason::QpsHigh { avg_qps: 120 },
            RiskReason::QpsRising { avg_qps: 60 },
            RiskReason::RepeatLoopSuspected { repeat: 100 },
            RiskReason::RepeatSql { repeat: 40 },
            RiskReason::LargeSingleResponse { bytes: 3 << 20 },
            RiskReason::ConnHog { established: 80 },
            RiskReason::ConnMulti { established: 25 },
            RiskReason::ChurnStorm { churn: 400 },
            RiskReason::ChurnRepeat { churn: 100 },
            RiskReason::CacheChurn { churn: 320 },
        ];
        for reason in &reasons {
            let en = reason_text(Lang::En, reason);
            let ko = reason_text(Lang::Ko, reason);
            assert!(!en.is_empty() && !hangul(&en), "{reason:?} {en}");
            assert!(hangul(&ko) || ko.starts_with("Redis"), "{reason:?} {ko}");
            assert_ne!(en, ko, "{reason:?}");
        }
        assert_eq!(
            reason_text(Lang::En, &RiskReason::CacheChurn { churn: 320 }),
            "Repeated short Redis connections (WAIT/SYN 320)"
        );
        assert_eq!(
            reason_text(Lang::Ko, &RiskReason::CacheChurn { churn: 320 }),
            "Redis 단기 연결 반복 (WAIT/SYN 320개)"
        );
    }

    // 6. 종료 결과 문구: 이름·PID·시스템 상세는 그대로, 문구만 언어별
    #[test]
    fn stop_and_kill_outcomes_render_per_language_with_untranslated_details() {
        let protected = KillError::Protected { process_name: "lsass.exe".to_string(), pid: 700 };
        assert_eq!(
            kill_error_text(Lang::En, &protected),
            "Protected system or own process, cannot terminate: lsass.exe(700)"
        );
        assert_eq!(
            kill_error_text(Lang::Ko, &protected),
            "시스템/자체 보호 프로세스 종료 불가: lsass.exe(700)"
        );
        let failed = StopReport {
            outcome: Err(StopFailure::AllFailed {
                failed: vec![KillError::TaskkillFailed {
                    process_name: "java.exe".to_string(),
                    pid: 9,
                    exit_code: 128,
                    detail: "Access is denied.".to_string(),
                }],
            }),
            repeated: true,
        };
        let en = stop_report_text(Lang::En, &failed);
        assert!(en.starts_with("Stop failed: java.exe(9) termination failed"), "{en}");
        assert!(en.contains("Access is denied.") && en.ends_with("(repeat request within 2s ignored)"));
        let ko = stop_report_text(Lang::Ko, &failed);
        assert!(ko.starts_with("일괄 중단 실패: java.exe(9) 종료 실패"), "{ko}");
        assert!(ko.contains("Access is denied.") && ko.ends_with("(2초 내 중복 요청 무시)"));
        let done = StopReport {
            outcome: Ok(StopSuccess::AllStopped {
                stopped: vec![ProcessRef { process_name: "bun.exe".to_string(), pid: 5 }],
            }),
            repeated: false,
        };
        assert_eq!(stop_report_text(Lang::En, &done), "All stopped (1): bun.exe(5)");
        assert_eq!(stop_report_text(Lang::Ko, &done), "긴급 일괄 중단 완료 (1건): bun.exe(5)");
    }
}
