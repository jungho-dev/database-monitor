//! cards.rs
//! 카드 식별자·종류, 카드별 스크롤·선택 상태, 클릭 동작, 마우스 적중 영역 모음.

use std::time::{Duration, Instant};
use ratatui::layout::{Position, Rect};
use crate::tui::layout::{SplitGroup, SplitHit};
use crate::tui::sort::SortTable;
use crate::tui::state::ActiveTab;

pub const CARD_COUNT: usize = CardId::ChartTopTables as usize + 1;
// 카드 제목을 누른 채 이 시간이 지나면 이동 모드
pub const CARD_HOLD_TIME: Duration = Duration::from_millis(400);

// 1. 카드 식별자 --------------------------------------------------------------
// 클릭·마우스 강조 대상 카드 (앱 바·탭 바·하단 바·도움말·경보 배너 제외)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardId {
    Risk,
    Connections,
    Traffic,
    QueryRate,
    Processes,
    Sockets,
    SqlStream,
    Inspector,
    TopSqlRanking,
    SqlDetail,
    AlertLog,
    AlertDetail,
    // 첫 집계 대기 안내
    Gate,
    // 막대 차트: SQL 작업 구성, 수신량 상위 테이블
    ChartSqlMix,
    ChartTopTables,
}

// 휠·키 동작 구분: 고정, 화면 위치만 이동, 선택 행과 화면 위치 함께 이동
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    Static,
    Scrollable,
    Selectable,
}

impl CardId {
    pub const ALL: [Self; CARD_COUNT] = [
        Self::Risk,
        Self::Connections,
        Self::Traffic,
        Self::QueryRate,
        Self::Processes,
        Self::Sockets,
        Self::SqlStream,
        Self::Inspector,
        Self::TopSqlRanking,
        Self::SqlDetail,
        Self::AlertLog,
        Self::AlertDetail,
        Self::Gate,
        Self::ChartSqlMix,
        Self::ChartTopTables,
    ];

    pub fn kind(self) -> CardKind {
        match self {
            Self::Processes | Self::TopSqlRanking | Self::AlertLog => CardKind::Selectable,
            Self::Sockets
            | Self::SqlStream
            | Self::Inspector
            | Self::SqlDetail
            | Self::AlertDetail => CardKind::Scrollable,
            Self::Risk
            | Self::Connections
            | Self::Traffic
            | Self::QueryRate
            | Self::Gate
            | Self::ChartSqlMix
            | Self::ChartTopTables => CardKind::Static,
        }
    }

    // 정렬·열 폭 조절 표를 가진 카드의 표 종류
    pub fn sort_table(self) -> Option<SortTable> {
        match self {
            Self::Processes => Some(SortTable::Processes),
            Self::Sockets => Some(SortTable::Sockets),
            Self::SqlStream => Some(SortTable::Events),
            Self::TopSqlRanking => Some(SortTable::TopSql),
            Self::AlertLog => Some(SortTable::Alerts),
            _ => None,
        }
    }
}

// 2. 카드별 보기 상태 ---------------------------------------------------------
// scroll = 첫 표시 행, selected = 선택 행, visible·total = 직전 그리기에서 기록한 값
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CardView {
    pub scroll: usize,
    pub selected: usize,
    pub visible: usize,
    pub total: usize,
}

// 키 이동 종류
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMotion {
    Lines(isize),
    Pages(isize),
    Home,
    End,
}

// 3. 클릭 동작 ----------------------------------------------------------------
// 하단 칩, 경보 카드, 종료 확인 버튼, 단축키가 같이 쓰는 동작
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiAction {
    Quit,
    ShowHelp,
    Refresh,
    TogglePause,
    TogglePin,
    ToggleBlink,
    ResetLayout,
    ToggleLanguage,
    // 새 빌드 감지 때만 동작
    ForceRestart,
    EmergencyStop,
    KillSelected,
    ConfirmKill,
    CancelKill,
}

// 4. 마우스 적중 영역 ---------------------------------------------------------
// 정렬 머리글 셀 (card = 정렬 대상 카드)
#[derive(Debug, Clone, Copy)]
pub struct HeaderHit {
    pub card: CardId,
    pub column: usize,
    pub rect: Rect,
}

// 선택 가능한 표의 본문 영역: 클릭 행 = offset + (클릭 y - rect.y)
#[derive(Debug, Clone, Copy)]
pub struct RowHit {
    pub card: CardId,
    pub rect: Rect,
    pub offset: usize,
    pub len: usize,
}

// 스크롤바 트랙 적중 영역 (거터+테두리 2열, 값은 그릴 때의 보기 상태)
#[derive(Debug, Clone, Copy)]
pub struct ScrollHit {
    pub card: CardId,
    pub track: Rect,
    pub total: usize,
    pub visible: usize,
    pub scroll: usize,
}

// 진행 중 썸 드래그 (grip = 누른 지점과 썸 시작 칸의 거리)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollDrag {
    pub card: CardId,
    pub grip: u16,
}

// 열 폭을 조절할 수 있는 표의 최대 열 수 (카드별 끌어 정한 폭 배열 길이)
pub const MAX_TABLE_COLUMNS: usize = 12;
// 끌어 정한 열 폭의 상한 (복원·열 폭 결정이 함께 씀)
pub const MAX_COLUMN_WIDTH: u16 = 512;

// 표 머리글 두 칸 사이 경계 (column = 왼쪽 열 번호, width = 그릴 때 왼쪽 열 폭)
// grab = 잡는 칸 (머리글 행의 경계 칸과 양옆 열이 최소 폭보다 넓으면 그 열 쪽 한 칸씩)
// line = 경계 칸 세로선 영역 (머리글과 그려진 본문 행 높이, x = 경계 칸)
// min·max = 끌어서 정할 수 있는 왼쪽 열 폭 범위
#[derive(Debug, Clone, Copy)]
pub struct ColumnHit {
    pub card: CardId,
    pub column: usize,
    pub grab: Rect,
    pub line: Rect,
    pub width: u16,
    pub min: u16,
    pub max: u16,
}

// 진행 중 열 경계 드래그 (start_x·start_width = 누른 순간 포인터 열과 왼쪽 열 폭, moved = 실제로 끌었는지)
// click_sort = 경계 옆 머리글 칸을 눌렀을 때 그 열 번호 (끌지 않고 떼면 정렬)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnDrag {
    pub card: CardId,
    pub column: usize,
    pub start_x: u16,
    pub start_width: u16,
    pub moved: bool,
    pub click_sort: Option<usize>,
}

// 카드 제목 글자(` 제목 ` 구간, 위 테두리 위): 길게 누르면 카드 이동
#[derive(Debug, Clone, Copy)]
pub struct TitleHit {
    pub card: CardId,
    pub rect: Rect,
}

// 격자로 그린 탭의 이번 프레임 화면 줄 (row = 격자 줄 번호, cards = 줄 안 카드 영역)
#[derive(Debug, Clone)]
pub struct GridLineHit {
    pub tab: ActiveTab,
    pub row: usize,
    pub rect: Rect,
    pub cards: Vec<(CardId, Rect)>,
}

// 카드 제목을 누른 채 대기 중 (started = 누른 시각, cell = 누른 칸, 시간이 차기 전 다른 칸으로 끌면 취소)
#[derive(Debug, Clone, Copy)]
pub struct CardPress {
    pub card: CardId,
    pub group: SplitGroup,
    pub index: usize,
    pub started: Instant,
    pub cell: Position,
}

// 진행 중 카드 이동 (격자와 놓을 자리는 GridMove)
#[derive(Debug, Clone, Copy)]
pub struct CardMove {
    pub card: CardId,
}

// 매 프레임 그리기에서 다시 채우는 클릭 영역
#[derive(Debug, Default, Clone)]
pub struct UiHitAreas {
    pub tab_rects: Vec<(ActiveTab, Rect)>,
    pub button_rects: Vec<(UiAction, Rect)>,
    pub header_cells: Vec<HeaderHit>,
    pub column_hits: Vec<ColumnHit>,
    pub row_hits: Vec<RowHit>,
    pub split_hits: Vec<SplitHit>,
    pub scroll_hits: Vec<ScrollHit>,
    pub card_rects: Vec<(CardId, Rect)>,
    pub title_hits: Vec<TitleHit>,
    pub grid_lines: Vec<GridLineHit>,
}

// 5. 스크롤바 썸 --------------------------------------------------------------
// 썸 위치·길이 (트랙 안 칸 단위)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollThumb {
    pub start: u16,
    pub len: u16,
}

// 보기 상태 -> 썸, 내용이 들어가거나 트랙이 2칸 미만이면 None
pub fn scroll_thumb(total: usize, visible: usize, scroll: usize, track: u16) -> Option<ScrollThumb> {
    if visible == 0 || total <= visible || track < 2 {
        return None;
    }

    // 1. 길이: 보이는 비율, 1칸 이상 트랙 미만 (끝에서도 움직일 여유 유지)
    let track_len = usize::from(track);
    let len = ((visible * track_len + total / 2) / total).clamp(1, track_len - 1);

    // 2. 시작: 스크롤 비율을 남은 칸에 반올림 대응
    let free = track_len - len;
    let max_scroll = total - visible;
    let start = (scroll.min(max_scroll) * free + max_scroll / 2) / max_scroll;
    Some(ScrollThumb {
        start: start as u16,
        len: len as u16,
    })
}

// 썸 시작 칸 -> 스크롤 (scroll_thumb 의 역변환, 끝점 보존)
pub fn scroll_for_thumb(total: usize, visible: usize, track: u16, thumb_start: u16) -> usize {
    let Some(thumb) = scroll_thumb(total, visible, 0, track) else {
        return 0;
    };
    let free = usize::from(track - thumb.len);
    let max_scroll = total - visible;
    (usize::from(thumb_start).min(free) * max_scroll + free / 2) / free
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2. 썸 비율
    #[test]
    fn scroll_thumb_matches_proportion_and_hits_both_ends() {
        let top = scroll_thumb(50, 10, 0, 10).unwrap();
        let middle = scroll_thumb(50, 10, 20, 10).unwrap();
        let bottom = scroll_thumb(50, 10, 40, 10).unwrap();
        assert_eq!((top.len, middle.len, bottom.len), (2, 2, 2));
        assert_eq!((top.start, middle.start, bottom.start), (0, 4, 8));
        // 범위를 넘는 스크롤은 끝으로 제한
        assert_eq!(scroll_thumb(50, 10, 999, 10).unwrap().start, 8);
    }

    // 3. 내용이 들어갈 때
    #[test]
    fn scroll_thumb_none_when_content_fits() {
        assert_eq!(scroll_thumb(10, 10, 0, 8), None);
        assert_eq!(scroll_thumb(5, 10, 0, 8), None);
        assert_eq!(scroll_thumb(50, 0, 0, 8), None);
        assert_eq!(scroll_thumb(50, 10, 0, 1), None);
    }

    // 4. 썸 역변환
    #[test]
    fn scroll_for_thumb_round_trips_endpoints_and_is_monotonic() {
        for (total, visible, track) in [(50usize, 10usize, 10u16), (200, 15, 15), (17, 9, 9)] {
            let max_scroll = total - visible;
            let mut previous = 0usize;
            for scroll in 0..=max_scroll {
                let thumb = scroll_thumb(total, visible, scroll, track).unwrap();
                let back = scroll_for_thumb(total, visible, track, thumb.start);
                assert!(back >= previous, "{total}/{visible}/{track} at {scroll}");
                previous = back;
            }
            assert_eq!(scroll_for_thumb(total, visible, track, 0), 0);
            let last = scroll_thumb(total, visible, max_scroll, track).unwrap();
            assert_eq!(scroll_for_thumb(total, visible, track, last.start), max_scroll);
            assert_eq!(scroll_for_thumb(total, visible, track, 999), max_scroll);
        }
        assert_eq!(scroll_for_thumb(5, 10, 8, 3), 0);
    }
}
