//! sort.rs
//! 표 정렬: 정렬 열 종류(문자·숫자), 3단계 순환(처음 방향, 반대 방향, 기본 순위), 열별 비교, 표시 순서 계산.

use std::cmp::Ordering;
use crate::app::model::{
    AlertEntry, DbConnectionEntry, MonitorSnapshot, ProcessSummary, SqlActivityEvent,
    SqlFingerprintStats,
};
use crate::tui::i18n::Msg;

// 1. 정렬 대상 표와 열 정보 ---------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortTable {
    Processes,
    Sockets,
    Events,
    TopSql,
    Alerts,
}

// 열 종류: Number 는 숫자·시간·등급 열로 큰·최근 값부터, Text 는 오름차순부터
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKind {
    Text,
    Number,
}

impl SortTable {
    pub const ALL: [Self; 5] = [
        Self::Processes,
        Self::Sockets,
        Self::Events,
        Self::TopSql,
        Self::Alerts,
    ];

    // 열 번호 순서의 머리글 문구 키 (정렬 키는 열 번호, 문구는 표시용)
    pub fn labels(self) -> &'static [Msg] {
        match self {
            Self::Processes => &[
                Msg::ColKind,
                Msg::ColPid,
                Msg::ColProcess,
                Msg::ColEstab,
                Msg::ColWait,
                Msg::ColAvgRx,
                Msg::ColAvgQps,
                Msg::ColRepeat10s,
                Msg::ColRisk,
            ],
            Self::Sockets => &[
                Msg::ColPid,
                Msg::ColProcess,
                Msg::ColEndpoint,
                Msg::ColState,
                Msg::ColRxPerSec,
                Msg::ColRiskShort,
            ],
            Self::Events => &[
                Msg::ColTime,
                Msg::ColActor,
                Msg::ColOp,
                Msg::ColTable,
                Msg::ColRxSize,
                Msg::ColRepeat10s,
                Msg::ColRiskShort,
                Msg::ColPayload,
            ],
            Self::TopSql => &[
                Msg::ColRisk,
                Msg::ColOp,
                Msg::ColTable,
                Msg::ColExecCount,
                Msg::ColRepeat10s,
                Msg::ColTotalRx,
                Msg::ColMaxSingle,
                Msg::ColLastProcess,
                Msg::ColDiagnosis,
            ],
            Self::Alerts => &[
                Msg::ColTime,
                Msg::ColLevel,
                Msg::ColRule,
                Msg::ColTitle,
                Msg::ColDetail,
            ],
        }
    }

    pub fn kinds(self) -> &'static [SortKind] {
        use SortKind::{Number, Text};
        match self {
            Self::Processes => &[Text, Number, Text, Number, Number, Number, Number, Number, Number],
            Self::Sockets => &[Number, Text, Text, Text, Number, Number],
            Self::Events => &[Number, Text, Text, Text, Number, Number, Number, Text],
            Self::TopSql => &[Number, Text, Text, Number, Number, Number, Number, Text, Text],
            Self::Alerts => &[Number, Number, Text, Text, Text],
        }
    }

    pub fn column_count(self) -> usize {
        self.kinds().len()
    }

    // 상태 파일에서 표를 가리키는 고정 이름
    pub fn table_key(self) -> &'static str {
        match self {
            Self::Processes => "processes",
            Self::Sockets => "sockets",
            Self::Events => "sql_stream",
            Self::TopSql => "top_sql",
            Self::Alerts => "alerts",
        }
    }
}

// 2. 정렬 상태 ----------------------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortKey {
    pub column: usize,
    pub descending: bool,
}

impl SortKey {
    pub fn arrow(self) -> &'static str {
        if self.descending { "↓" } else { "↑" }
    }
}

// 정렬 변경 요청: 머리글 클릭, s 키, S 키
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortChange {
    SelectColumn(usize),
    NextColumn,
    ReverseDirection,
}

// 표별 정렬 (None 이면 수집기 기본 순위)
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TableSorts {
    slots: [Option<SortKey>; 5],
}

impl TableSorts {
    pub fn get(&self, table: SortTable) -> Option<SortKey> {
        self.slots[table as usize]
    }

    // 정렬 순환: 새 열은 숫자·시간 열이 내림차순, 문자 열이 오름차순에서 시작하고
    // 같은 열은 처음 방향 -> 반대 방향 -> 기본 순위 (머리글 클릭·S 공용, s 는 다음 열)
    pub fn apply_change(&mut self, table: SortTable, change: SortChange) {
        let kinds = table.kinds();
        let current = self.slots[table as usize];
        let first_descending = |column: usize| kinds.get(column) == Some(&SortKind::Number);
        let click = |column: usize| match current {
            Some(key) if key.column == column => {
                (key.descending == first_descending(column)).then_some(SortKey {
                    column,
                    descending: !key.descending,
                })
            }
            _ => Some(SortKey {
                column,
                descending: first_descending(column),
            }),
        };
        self.slots[table as usize] = match change {
            SortChange::SelectColumn(column) if column >= kinds.len() => current,
            SortChange::SelectColumn(column) => click(column),
            SortChange::ReverseDirection => click(current.map_or(0, |key| key.column)),
            SortChange::NextColumn => {
                let next = current.map_or(0, |key| key.column + 1);
                (next < kinds.len()).then_some(SortKey {
                    column: next,
                    descending: first_descending(next),
                })
            }
        };
    }

    // 상태 복원·초기화용 직접 지정
    pub fn set(&mut self, table: SortTable, key: Option<SortKey>) {
        self.slots[table as usize] = key;
    }
}

// 3. 표시 순서 계산 -----------------------------------------------------------
// 원본 순서는 그대로 두고 참조 순서만 정렬: 동률은 수집기 기본 순위 유지
fn sorted<T>(
    items: &[T],
    key: Option<SortKey>,
    compare: fn(&T, &T, usize) -> Ordering,
) -> Vec<&T> {
    let mut view: Vec<&T> = items.iter().collect();
    if let Some(key) = key {
        view.sort_by(|a, b| {
            let ord = compare(a, b, key.column);
            if key.descending { ord.reverse() } else { ord }
        });
    }
    view
}

pub fn processes<'a>(snap: &'a MonitorSnapshot, sorts: &TableSorts) -> Vec<&'a ProcessSummary> {
    sorted(&snap.processes, sorts.get(SortTable::Processes), compare_process)
}

pub fn sockets<'a>(snap: &'a MonitorSnapshot, sorts: &TableSorts) -> Vec<&'a DbConnectionEntry> {
    sorted(&snap.connections, sorts.get(SortTable::Sockets), compare_socket)
}

pub fn events<'a>(snap: &'a MonitorSnapshot, sorts: &TableSorts) -> Vec<&'a SqlActivityEvent> {
    sorted(&snap.recent_events, sorts.get(SortTable::Events), compare_event)
}

pub fn top_sql<'a>(snap: &'a MonitorSnapshot, sorts: &TableSorts) -> Vec<&'a SqlFingerprintStats> {
    sorted(&snap.top_sql, sorts.get(SortTable::TopSql), compare_top_sql)
}

pub fn alerts<'a>(snap: &'a MonitorSnapshot, sorts: &TableSorts) -> Vec<&'a AlertEntry> {
    sorted(&snap.alerts, sorts.get(SortTable::Alerts), compare_alert)
}

// 4. 열별 비교 기준 -----------------------------------------------------------
// 문자열 열은 대소문자 무시, 등급·사유·규칙 열은 값 순서 (표시 언어와 무관)
fn compare_text(a: &str, b: &str) -> Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
}

fn compare_process(a: &ProcessSummary, b: &ProcessSummary, column: usize) -> Ordering {
    match column {
        0 => compare_text(a.actor_kind.badge(), b.actor_kind.badge()),
        1 => a.pid.cmp(&b.pid),
        2 => compare_text(&a.process_name, &b.process_name),
        3 => a.established_conns.cmp(&b.established_conns),
        4 => a.time_wait_conns.cmp(&b.time_wait_conns),
        5 => a.avg_rx_bps.cmp(&b.avg_rx_bps),
        6 => a.avg_qps.cmp(&b.avg_qps),
        7 => a.max_repeat_10s.cmp(&b.max_repeat_10s),
        8 => (a.risk_level, a.risk_score).cmp(&(b.risk_level, b.risk_score)),
        _ => Ordering::Equal,
    }
}

fn compare_socket(a: &DbConnectionEntry, b: &DbConnectionEntry, column: usize) -> Ordering {
    match column {
        0 => a.pid.cmp(&b.pid),
        1 => compare_text(&a.process_name, &b.process_name),
        2 => compare_text(&a.endpoint_label, &b.endpoint_label),
        3 => compare_text(a.state.label(), b.state.label()),
        4 => a.rx_bps.cmp(&b.rx_bps),
        // 로컬 "제외" 행은 위험 등급보다 낮게
        5 => (!a.is_local, a.risk_level).cmp(&(!b.is_local, b.risk_level)),
        _ => Ordering::Equal,
    }
}

fn compare_event(a: &SqlActivityEvent, b: &SqlActivityEvent, column: usize) -> Ordering {
    match column {
        // 시각 문자열은 초 단위 동률이 많아 발생 순번으로 비교
        0 => a.id.cmp(&b.id),
        1 => compare_text(a.actor_kind.badge(), b.actor_kind.badge())
            .then_with(|| compare_text(&a.process_name, &b.process_name)),
        2 => compare_text(a.op_kind.label(), b.op_kind.label()),
        3 => compare_text(&a.target_table, &b.target_table),
        4 => a.rx_bytes.cmp(&b.rx_bytes),
        5 => a.repeat_10s.cmp(&b.repeat_10s),
        6 => a.risk_level.cmp(&b.risk_level),
        7 => compare_text(&a.sql_preview, &b.sql_preview),
        _ => Ordering::Equal,
    }
}

fn compare_top_sql(a: &SqlFingerprintStats, b: &SqlFingerprintStats, column: usize) -> Ordering {
    match column {
        0 => a.risk_level.cmp(&b.risk_level),
        1 => compare_text(a.op_kind.label(), b.op_kind.label()),
        2 => compare_text(&a.target_table, &b.target_table),
        3 => a.exec_count.cmp(&b.exec_count),
        4 => a.repeat_10s.cmp(&b.repeat_10s),
        5 => a.total_rx_bytes.cmp(&b.total_rx_bytes),
        6 => a.max_single_rx_bytes.cmp(&b.max_single_rx_bytes),
        7 => compare_text(&a.last_process, &b.last_process)
            .then_with(|| a.last_pid.cmp(&b.last_pid)),
        8 => a.risk_reason.cmp(&b.risk_reason)
            .then_with(|| compare_text(&a.sample_sql, &b.sample_sql)),
        _ => Ordering::Equal,
    }
}

fn compare_alert(a: &AlertEntry, b: &AlertEntry, column: usize) -> Ordering {
    match column {
        0 => a.id.cmp(&b.id),
        1 => a.level.cmp(&b.level),
        2 => a.rule_code.cmp(&b.rule_code),
        3 => compare_text(&a.title, &b.title),
        4 => a.detail.cmp(&b.detail),
        _ => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(column: usize, descending: bool) -> Option<SortKey> {
        Some(SortKey { column, descending })
    }

    // 1. 숫자 열 3단계
    #[test]
    fn numeric_column_cycles_descending_ascending_then_resets() {
        let mut sorts = TableSorts::default();
        // 실행횟수(3열): 내림차순 -> 오름차순 -> 기본 순위
        sorts.apply_change(SortTable::TopSql, SortChange::SelectColumn(3));
        assert_eq!(sorts.get(SortTable::TopSql), key(3, true));
        sorts.apply_change(SortTable::TopSql, SortChange::SelectColumn(3));
        assert_eq!(sorts.get(SortTable::TopSql), key(3, false));
        sorts.apply_change(SortTable::TopSql, SortChange::SelectColumn(3));
        assert_eq!(sorts.get(SortTable::TopSql), None);
    }

    // 2. 문자 열 3단계
    #[test]
    fn text_column_cycles_ascending_descending_then_resets() {
        let mut sorts = TableSorts::default();
        // 프로세스 이름(2열)
        sorts.apply_change(SortTable::Processes, SortChange::SelectColumn(2));
        assert_eq!(sorts.get(SortTable::Processes), key(2, false));
        sorts.apply_change(SortTable::Processes, SortChange::SelectColumn(2));
        assert_eq!(sorts.get(SortTable::Processes), key(2, true));
        sorts.apply_change(SortTable::Processes, SortChange::SelectColumn(2));
        assert_eq!(sorts.get(SortTable::Processes), None);
    }

    // 3. 다른 열 전환
    #[test]
    fn selecting_another_column_starts_in_that_columns_natural_direction() {
        let mut sorts = TableSorts::default();
        sorts.apply_change(SortTable::TopSql, SortChange::SelectColumn(3));
        sorts.apply_change(SortTable::TopSql, SortChange::SelectColumn(1));
        assert_eq!(sorts.get(SortTable::TopSql), key(1, false));
        sorts.apply_change(SortTable::TopSql, SortChange::SelectColumn(5));
        assert_eq!(sorts.get(SortTable::TopSql), key(5, true));
        // 범위 밖 열은 무시
        sorts.apply_change(SortTable::TopSql, SortChange::SelectColumn(99));
        assert_eq!(sorts.get(SortTable::TopSql), key(5, true));
    }

    // 4. s 와 S 키
    #[test]
    fn s_walks_columns_then_resets_and_shift_s_follows_the_click_cycle() {
        let mut sorts = TableSorts::default();
        let count = SortTable::Alerts.column_count();
        sorts.apply_change(SortTable::Alerts, SortChange::NextColumn);
        assert_eq!(sorts.get(SortTable::Alerts), key(0, true));
        for column in 1..count {
            sorts.apply_change(SortTable::Alerts, SortChange::NextColumn);
            let expected = SortTable::Alerts.kinds()[column] == SortKind::Number;
            assert_eq!(sorts.get(SortTable::Alerts), key(column, expected));
        }
        sorts.apply_change(SortTable::Alerts, SortChange::NextColumn);
        assert_eq!(sorts.get(SortTable::Alerts), None);

        // S: 정렬이 없으면 첫 열의 처음 방향, 이어서 반대 방향, 기본 순위
        sorts.apply_change(SortTable::Alerts, SortChange::ReverseDirection);
        assert_eq!(sorts.get(SortTable::Alerts), key(0, true));
        sorts.apply_change(SortTable::Alerts, SortChange::ReverseDirection);
        assert_eq!(sorts.get(SortTable::Alerts), key(0, false));
        sorts.apply_change(SortTable::Alerts, SortChange::ReverseDirection);
        assert_eq!(sorts.get(SortTable::Alerts), None);
    }

    // 5. 열 명세 일치
    #[test]
    fn every_table_has_one_label_and_one_kind_per_column() {
        for table in SortTable::ALL {
            assert_eq!(table.labels().len(), table.kinds().len(), "{table:?}");
            assert!(table.column_count() <= crate::tui::cards::MAX_TABLE_COLUMNS);
        }
        let mut names: Vec<&str> = SortTable::ALL.iter().map(|table| table.table_key()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SortTable::ALL.len());
    }
}
