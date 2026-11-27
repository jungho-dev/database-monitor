//! layout/grid.rs
//! 탭별 카드 격자: 줄 배치, 반응형 화면 줄 계획, 놓기 위치 판정, 카드 이동.

use crate::tui::cards::{CardId, GridLineHit};
use crate::tui::state::ActiveTab;
use crossterm::event::KeyCode;
use ratatui::layout::{Position, Rect};

// 격자 줄 수와 줄당 카드 수 상한
pub const GRID_SLOTS: usize = 6;

// 1. 격자와 카드 조건 ---------------------------------------------------------
// 탭의 격자 줄별 카드 (줄·카드 수 상한 GRID_SLOTS)
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TabGrid {
    pub rows: [[Option<CardId>; GRID_SLOTS]; GRID_SLOTS],
}

// 카드 높이: 고정 행 수, 남는 높이 나눔
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotHeight {
    Fixed(u16),
    Fill,
}

// 이번 프레임에 그릴 카드의 크기 조건 (rank 0 = 항상 표시)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardSlot {
    pub card: CardId,
    pub min_w: u16,
    pub min_h: u16,
    pub height: SlotHeight,
    pub weight: u16,
    pub rank: u8,
    pub reserve: u16,
}

// 보이는 화면 줄 (row = 격자 줄 번호)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridLine {
    pub row: usize,
    pub cards: Vec<CardId>,
    pub height: u16,
}

// 위에서 아래로 보이는 화면 줄
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GridPlan {
    pub lines: Vec<GridLine>,
}

// 카드를 놓을 자리
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropTarget {
    Stay,
    Before(CardId),
    After(CardId),
    RowAbove(CardId),
    RowBelow(CardId),
}

// 진행 중인 격자 카드 이동 (baseline = 누르기 전 격자, lines = 누른 순간 그려진 줄: 놓을 자리는 이 줄 기준으로 판정)
#[derive(Debug, Clone)]
pub struct GridMove {
    pub tab: ActiveTab,
    pub card: CardId,
    pub baseline: TabGrid,
    pub lines: Vec<GridLineHit>,
}

// 화면 줄 후보 (used_w = 최소 폭 합과 빈 칸)
#[derive(Debug, Clone)]
struct LineDraft {
    row: usize,
    cards: Vec<CardId>,
    used_w: u16,
    fixed: u16,
    fill: bool,
    min_h: u16,
    rank: u8,
    reserve: u16,
}

// 2. 기본 격자와 카드 이름 ----------------------------------------------------
// 탭 기본 격자: 모든 탭 0행은 지표 카드 네 장, 개요는 다섯 줄, 다른 탭은 목록·상세가 한 줄씩
pub fn default_rows(tab: ActiveTab) -> TabGrid {
    let mut grid = TabGrid::default();
    let metrics: &[CardId] = &[
        CardId::Risk,
        CardId::Connections,
        CardId::Traffic,
        CardId::QueryRate,
    ];
    let rows: &[&[CardId]] = match tab {
        ActiveTab::Overview => &[
            metrics,
            &[CardId::Processes, CardId::Sockets],
            &[CardId::ChartSqlMix, CardId::ChartTopTables],
            &[CardId::SqlStream],
            &[CardId::Inspector],
        ],
        ActiveTab::Processes => &[metrics, &[CardId::Processes], &[CardId::Inspector]],
        ActiveTab::TopSql => &[metrics, &[CardId::TopSqlRanking], &[CardId::SqlDetail]],
        ActiveTab::Alerts => &[metrics, &[CardId::AlertLog], &[CardId::AlertDetail]],
    };
    for (row, cards) in rows.iter().enumerate() {
        for (column, card) in cards.iter().enumerate() {
            grid.rows[row][column] = Some(*card);
        }
    }
    grid
}

// 배치 파일에서 카드를 가리키는 고정 이름
pub fn card_key(card: CardId) -> &'static str {
    match card {
        CardId::Risk => "risk",
        CardId::Connections => "connections",
        CardId::Traffic => "traffic",
        CardId::QueryRate => "query_rate",
        CardId::Processes => "processes",
        CardId::Sockets => "sockets",
        CardId::SqlStream => "sql_stream",
        CardId::Inspector => "inspector",
        CardId::TopSqlRanking => "top_sql_ranking",
        CardId::SqlDetail => "sql_detail",
        CardId::AlertLog => "alert_log",
        CardId::AlertDetail => "alert_detail",
        CardId::Gate => "gate",
        CardId::ChartSqlMix => "chart_sql_mix",
        CardId::ChartTopTables => "chart_top_tables",
    }
}

// card_key 의 역변환 (모르는 이름은 None)
pub fn card_from_key(key: &str) -> Option<CardId> {
    let card = match key {
        "risk" => CardId::Risk,
        "connections" => CardId::Connections,
        "traffic" => CardId::Traffic,
        "query_rate" => CardId::QueryRate,
        "processes" => CardId::Processes,
        "sockets" => CardId::Sockets,
        "sql_stream" => CardId::SqlStream,
        "inspector" => CardId::Inspector,
        "top_sql_ranking" => CardId::TopSqlRanking,
        "sql_detail" => CardId::SqlDetail,
        "alert_log" => CardId::AlertLog,
        "alert_detail" => CardId::AlertDetail,
        "gate" => CardId::Gate,
        "chart_sql_mix" => CardId::ChartSqlMix,
        "chart_top_tables" => CardId::ChartTopTables,
        _ => return None,
    };
    Some(card)
}

// 3. 반응형 화면 줄 계획 ------------------------------------------------------
// 폭이 모자라면 같은 격자 줄의 다음 화면 줄로, 높이가 모자라면 rank 높은 줄부터 숨김
pub fn plan_lines(grid: &TabGrid, slots: &[CardSlot], width: u16, height: u16) -> GridPlan {
    // 1. 화면 줄 후보 (조건 없는 카드는 빠지고 줄 동료가 폭을 받음)
    let placed = grid
        .rows
        .iter()
        .enumerate()
        .flat_map(|(row, cells)| cells.iter().flatten().map(move |card| (row, *card)));
    let mut drafts: Vec<LineDraft> = Vec::new();
    for (row, card) in placed {
        let Some(slot) = slots.iter().find(|slot| slot.card == card) else {
            continue;
        };
        let fits = drafts.last().is_some_and(|last| {
            last.row == row && last.used_w.saturating_add(1).saturating_add(slot.min_w) <= width
        });
        if !fits {
            drafts.push(LineDraft {
                row,
                cards: Vec::new(),
                used_w: 0,
                fixed: 0,
                fill: false,
                min_h: 0,
                rank: slot.rank,
                reserve: slot.reserve,
            });
        }
        let Some(line) = drafts.last_mut() else {
            continue;
        };
        let gap = u16::from(!line.cards.is_empty());
        line.used_w = line.used_w.saturating_add(gap).saturating_add(slot.min_w);
        line.cards.push(card);
        line.min_h = line.min_h.max(slot.min_h);
        // 줄 rank 는 카드 중 가장 낮은 값 (항상 보여야 하는 카드가 있으면 그 줄은 숨기지 않음), reserve 는 그 rank 카드의 값
        if slot.rank < line.rank {
            line.rank = slot.rank;
            line.reserve = slot.reserve;
        } else if slot.rank == line.rank {
            line.reserve = line.reserve.max(slot.reserve);
        }
        match slot.height {
            SlotHeight::Fixed(rows) => line.fixed = line.fixed.max(rows),
            SlotHeight::Fill => line.fill = true,
        }
    }

    // 2. 줄별 고정 높이와 최소 높이 (채움 줄의 고정 높이는 0)
    let fixed_rows: Vec<u32> = drafts
        .iter()
        .map(|line| {
            if line.fill {
                0
            } else {
                u32::from(line.fixed.max(line.min_h))
            }
        })
        .collect();
    let need_rows: Vec<u32> = drafts
        .iter()
        .zip(&fixed_rows)
        .map(|(line, fixed)| {
            if line.fill {
                u32::from(line.min_h)
            } else {
                *fixed
            }
        })
        .collect();

    // 3. rank 0 줄은 항상, 높은 rank 줄은 고정 높이 합과 reserve 가 들어갈 때만 (낮은 rank 먼저)
    let mut shown: Vec<bool> = drafts.iter().map(|line| line.rank == 0).collect();
    let mut fixed_total: u32 = (0..drafts.len())
        .filter(|index| shown[*index])
        .map(|index| fixed_rows[index])
        .sum();
    let mut ranked: Vec<usize> = (0..drafts.len())
        .filter(|index| drafts[*index].rank > 0)
        .collect();
    ranked.sort_by_key(|index| drafts[*index].rank);
    for index in ranked {
        let with_line = fixed_total + fixed_rows[index];
        if with_line + u32::from(drafts[index].reserve) > u32::from(height) {
            continue;
        }
        shown[index] = true;
        fixed_total = with_line;
    }

    // 4. 최소 높이도 모자라거나 줄이 GRID_SLOTS 를 넘으면 아래 줄부터 숨김
    let mut visible: Vec<usize> = (0..drafts.len()).filter(|index| shown[*index]).collect();
    while visible.len() > GRID_SLOTS
        || visible.iter().map(|index| need_rows[*index]).sum::<u32>() > u32::from(height)
    {
        visible.pop();
    }

    // 5. 줄 높이: 고정 줄은 고정 높이, 채움 줄은 남는 높이를 나눔 (채움 줄이 없으면 마지막 줄)
    let fixed_sum: u32 = visible.iter().map(|index| fixed_rows[*index]).sum();
    let spare = u32::from(height).saturating_sub(fixed_sum);
    let fill_count = visible.iter().filter(|index| drafts[**index].fill).count() as u32;
    let share = if fill_count == 0 {
        0
    } else {
        spare / fill_count
    };
    let mut lines = Vec::with_capacity(visible.len());
    let mut fill_seen = 0u32;
    for index in &visible {
        let draft = &mut drafts[*index];
        fill_seen += u32::from(draft.fill);
        let line_rows = match (draft.fill, fill_seen == fill_count) {
            (true, true) => spare - share * (fill_count - 1),
            (true, false) => share,
            (false, _) => fixed_rows[*index],
        };
        lines.push(GridLine {
            row: draft.row,
            cards: std::mem::take(&mut draft.cards),
            height: line_rows as u16,
        });
    }
    if fill_count == 0
        && let Some(last) = lines.last_mut()
    {
        last.height = last.height.saturating_add(spare as u16);
    }
    GridPlan { lines }
}

// 4. 놓기 위치와 카드 이동 ----------------------------------------------------
// 포인터 위치 -> 놓을 자리 (줄 위·아래 테두리 행은 새 줄, 안쪽은 그 줄에 합류, 옮기는 카드 줄의 위 테두리는 안쪽)
pub fn resolve_drop(
    plan: &GridPlan,
    line_rects: &[Rect],
    card_rects: &[(CardId, Rect)],
    moving: CardId,
    pointer: Position,
) -> DropTarget {
    // 1. 포인터가 놓인 화면 줄
    let Some((line, rect)) = plan
        .lines
        .iter()
        .zip(line_rects)
        .find(|(_, rect)| rect.contains(pointer))
    else {
        return DropTarget::Stay;
    };
    let Some(first) = line.cards.first().copied() else {
        return DropTarget::Stay;
    };

    // 2. 줄 위·아래 테두리 행 (자기 줄 제목 줄을 따라 밀면 줄 안 이동)
    if pointer.y == rect.y && !line.cards.contains(&moving) {
        return DropTarget::RowAbove(first);
    }
    if pointer.y.saturating_add(1) == rect.bottom() {
        return DropTarget::RowBelow(first);
    }

    // 3. 옮기는 카드 위
    let over_moving = card_rects
        .iter()
        .any(|(card, card_rect)| *card == moving && card_rect.contains(pointer));
    if over_moving {
        return DropTarget::Stay;
    }

    // 4. 가운데가 포인터 오른쪽인 첫 카드 앞, 없으면 마지막 카드 뒤
    for card in &line.cards {
        let Some((_, card_rect)) = card_rects.iter().find(|(placed, _)| placed == card) else {
            continue;
        };
        if card_rect.x.saturating_add(card_rect.width / 2) > pointer.x {
            return DropTarget::Before(*card);
        }
    }
    DropTarget::After(line.cards.last().copied().unwrap_or(first))
}

// baseline 에서 card 를 target 자리로 (빈 줄 제거, 7번째 카드·줄이면 None)
pub fn apply_drop(baseline: &TabGrid, card: CardId, target: DropTarget) -> Option<TabGrid> {
    // 1. 기준 카드
    let anchor = match target {
        DropTarget::Stay => return Some(*baseline),
        DropTarget::Before(anchor)
        | DropTarget::After(anchor)
        | DropTarget::RowAbove(anchor)
        | DropTarget::RowBelow(anchor) => anchor,
    };
    let joins_row = matches!(target, DropTarget::Before(_) | DropTarget::After(_));
    if joins_row && anchor == card {
        return Some(*baseline);
    }

    // 2. 줄 목록 (빈 줄 제외)과 두 카드 자리
    let mut rows: Vec<Vec<CardId>> = Vec::with_capacity(GRID_SLOTS);
    for cells in &baseline.rows {
        let row: Vec<CardId> = cells.iter().flatten().copied().collect();
        if !row.is_empty() {
            rows.push(row);
        }
    }
    let (from_row, from_col) = rows.iter().enumerate().find_map(|(index, row)| {
        row.iter()
            .position(|placed| *placed == card)
            .map(|column| (index, column))
    })?;
    let (anchor_row, anchor_col) = rows.iter().enumerate().find_map(|(index, row)| {
        row.iter()
            .position(|placed| *placed == anchor)
            .map(|column| (index, column))
    })?;
    let shifted = if anchor_row == from_row && anchor_col > from_col {
        anchor_col - 1
    } else {
        anchor_col
    };

    // 3. 이동
    rows[from_row].remove(from_col);
    match target {
        DropTarget::Before(_) => rows[anchor_row].insert(shifted, card),
        DropTarget::After(_) => rows[anchor_row].insert(shifted + 1, card),
        DropTarget::RowAbove(_) => rows.insert(anchor_row, vec![card]),
        DropTarget::RowBelow(_) => rows.insert(anchor_row + 1, vec![card]),
        DropTarget::Stay => {}
    }
    rows.retain(|row| !row.is_empty());

    // 4. 상한 확인 후 격자로
    if rows.len() > GRID_SLOTS || rows.iter().any(|row| row.len() > GRID_SLOTS) {
        return None;
    }
    let mut grid = TabGrid::default();
    for (row, cards) in rows.iter().enumerate() {
        for (column, placed) in cards.iter().enumerate() {
            grid.rows[row][column] = Some(*placed);
        }
    }
    Some(grid)
}

// 키보드 이동: 좌우 = 줄 안 그려진 이웃과 교환, 상하 = 자기 새 줄과 이웃 줄 합류를 번갈아
pub fn shift_card(grid: &TabGrid, card: CardId, key: KeyCode, drawn: &[CardId]) -> Option<TabGrid> {
    // 1. 카드 자리
    let (row, column) = grid.rows.iter().enumerate().find_map(|(index, cells)| {
        cells
            .iter()
            .position(|cell| *cell == Some(card))
            .map(|column| (index, column))
    })?;
    let cells = &grid.rows[row];
    let shares_row = cells.iter().flatten().count() > 1;

    // 2. 방향별 놓을 자리
    let target = match key {
        KeyCode::Left => DropTarget::Before(
            *cells[..column]
                .iter()
                .flatten()
                .rev()
                .find(|cell| drawn.contains(cell))?,
        ),
        KeyCode::Right => DropTarget::After(
            *cells[column + 1..]
                .iter()
                .flatten()
                .find(|cell| drawn.contains(cell))?,
        ),
        KeyCode::Up if shares_row => DropTarget::RowAbove(card),
        KeyCode::Down if shares_row => DropTarget::RowBelow(card),
        KeyCode::Up => DropTarget::After(
            *grid.rows[..row]
                .iter()
                .rev()
                .find_map(|above| above.iter().flatten().last())?,
        ),
        KeyCode::Down => DropTarget::After(
            *grid.rows[row + 1..]
                .iter()
                .find_map(|below| below.iter().flatten().last())?,
        ),
        _ => return None,
    };
    apply_drop(grid, card, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: CardId = CardId::Risk;
    const B: CardId = CardId::Connections;
    const C: CardId = CardId::SqlStream;
    const D: CardId = CardId::Inspector;
    const METRICS: [CardId; 4] = [
        CardId::Risk,
        CardId::Connections,
        CardId::Traffic,
        CardId::QueryRate,
    ];

    fn grid_from(rows: &[&[CardId]]) -> TabGrid {
        let mut grid = TabGrid::default();
        for (row, cards) in rows.iter().enumerate() {
            for (column, card) in cards.iter().enumerate() {
                grid.rows[row][column] = Some(*card);
            }
        }
        grid
    }

    fn rows_of(grid: &TabGrid) -> Vec<Vec<CardId>> {
        grid.rows
            .iter()
            .map(|cells| cells.iter().flatten().copied().collect::<Vec<CardId>>())
            .filter(|row| !row.is_empty())
            .collect()
    }

    fn fixed_slot(card: CardId, min_w: u16, rows: u16, rank: u8, reserve: u16) -> CardSlot {
        CardSlot {
            card,
            min_w,
            min_h: rows.min(6),
            height: SlotHeight::Fixed(rows),
            weight: 1,
            rank,
            reserve,
        }
    }

    // 개요 기본 격자 카드 조건: 차트 줄 rank 2 reserve 5, 스트림 줄 rank 3 reserve 9, 인스펙터 줄은 채움
    fn overview_slots() -> Vec<CardSlot> {
        let mut slots = Vec::new();
        for card in METRICS {
            slots.push(fixed_slot(card, 8, 3, 0, 0));
        }
        for card in [CardId::Processes, CardId::Sockets] {
            slots.push(fixed_slot(card, 8, 3, 0, 0));
        }
        for card in [CardId::ChartSqlMix, CardId::ChartTopTables] {
            slots.push(fixed_slot(card, 16, 5, 2, 5));
        }
        slots.push(fixed_slot(CardId::SqlStream, 40, 10, 3, 9));
        slots.push(CardSlot {
            card: CardId::Inspector,
            min_w: 24,
            min_h: 3,
            height: SlotHeight::Fill,
            weight: 1,
            rank: 0,
            reserve: 0,
        });
        slots
    }

    fn line_shape(plan: &GridPlan) -> Vec<(usize, usize, u16)> {
        plan.lines
            .iter()
            .map(|line| (line.row, line.cards.len(), line.height))
            .collect()
    }

    #[test]
    fn default_grids_put_the_metric_cards_on_row_zero_of_every_tab() {
        assert_eq!(
            rows_of(&default_rows(ActiveTab::Overview)),
            vec![
                METRICS.to_vec(),
                vec![CardId::Processes, CardId::Sockets],
                vec![CardId::ChartSqlMix, CardId::ChartTopTables],
                vec![CardId::SqlStream],
                vec![CardId::Inspector],
            ]
        );
        assert_eq!(
            rows_of(&default_rows(ActiveTab::Processes)),
            vec![
                METRICS.to_vec(),
                vec![CardId::Processes],
                vec![CardId::Inspector],
            ]
        );
        assert_eq!(
            rows_of(&default_rows(ActiveTab::TopSql)),
            vec![
                METRICS.to_vec(),
                vec![CardId::TopSqlRanking],
                vec![CardId::SqlDetail],
            ]
        );
        assert_eq!(
            rows_of(&default_rows(ActiveTab::Alerts)),
            vec![
                METRICS.to_vec(),
                vec![CardId::AlertLog],
                vec![CardId::AlertDetail],
            ]
        );
        // 첫 집계 대기 안내 카드는 어느 격자에도 없음
        for tab in ActiveTab::ALL {
            let grid = default_rows(tab);
            assert_ne!(grid, TabGrid::default(), "{tab:?}");
            assert!(!rows_of(&grid).concat().contains(&CardId::Gate), "{tab:?}");
        }
    }

    #[test]
    fn card_keys_are_unique_snake_case_and_round_trip() {
        let mut keys: Vec<&str> = CardId::ALL.iter().map(|card| card_key(*card)).collect();
        for card in CardId::ALL {
            assert_eq!(card_from_key(card_key(card)), Some(card), "{card:?}");
        }
        assert!(keys.iter().all(|key| {
            key.chars()
                .all(|letter| letter.is_ascii_lowercase() || letter == '_')
        }));
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), CardId::ALL.len());
        assert_eq!(card_key(CardId::Risk), "risk");
        assert_eq!(card_key(CardId::AlertDetail), "alert_detail");
        assert_eq!(card_from_key("no_such_card"), None);
    }

    // 여섯 장 줄: 폭 60 은 3 + 3, 폭 100 은 5 + 1, 폭 130 은 한 줄 (카드는 빠지지 않음)
    #[test]
    fn a_six_card_row_wraps_onto_following_lines_without_dropping_cards() {
        let cards = [
            CardId::Risk,
            CardId::Connections,
            CardId::Traffic,
            CardId::QueryRate,
            CardId::Processes,
            CardId::Sockets,
        ];
        let grid = grid_from(&[&cards]);
        let slots: Vec<CardSlot> = cards
            .iter()
            .map(|card| fixed_slot(*card, 16, 3, 0, 0))
            .collect();
        for (width, counts) in [(60, vec![3, 3]), (100, vec![5, 1]), (130, vec![6])] {
            let plan = plan_lines(&grid, &slots, width, 40);
            let line_counts: Vec<usize> = plan.lines.iter().map(|line| line.cards.len()).collect();
            assert_eq!(line_counts, counts, "width {width}");
            assert!(plan.lines.iter().all(|line| line.row == 0));
            let flat: Vec<CardId> = plan
                .lines
                .iter()
                .flat_map(|line| line.cards.clone())
                .collect();
            assert_eq!(flat, cards, "width {width}");
        }

        // 조건 없는 카드는 빠지고 줄 동료가 폭을 받음
        let plan = plan_lines(&grid, &slots[..4], 60, 40);
        let line_counts: Vec<usize> = plan.lines.iter().map(|line| line.cards.len()).collect();
        assert_eq!(line_counts, [3, 1]);
    }

    // 높이 12 는 지표·표·인스펙터, 26 은 차트 추가, 36 은 스트림까지
    #[test]
    fn higher_rank_lines_show_only_when_their_reserve_fits_lower_rank_first() {
        let grid = default_rows(ActiveTab::Overview);
        let slots = overview_slots();
        let plan = plan_lines(&grid, &slots, 130, 12);
        assert_eq!(line_shape(&plan), [(0, 4, 3), (1, 2, 3), (4, 1, 6)]);
        let plan = plan_lines(&grid, &slots, 130, 26);
        assert_eq!(
            line_shape(&plan),
            [(0, 4, 3), (1, 2, 3), (2, 2, 5), (4, 1, 15)]
        );
        let plan = plan_lines(&grid, &slots, 130, 36);
        assert_eq!(
            line_shape(&plan),
            [(0, 4, 3), (1, 2, 3), (2, 2, 5), (3, 1, 10), (4, 1, 15)]
        );
        let total: u16 = plan.lines.iter().map(|line| line.height).sum();
        assert_eq!(total, 36);
    }

    // 높은 rank 카드를 rank 0 줄에 옮겨도 그 줄은 항상 보임 (넓으면 한 화면 줄, 좁으면 넘친 카드만 따로)
    #[test]
    fn a_ranked_card_moved_into_a_rank_zero_row_keeps_that_row_visible() {
        let mut grid = default_rows(ActiveTab::Overview);
        grid = apply_drop(
            &grid,
            CardId::SqlStream,
            DropTarget::After(CardId::Inspector),
        )
        .expect("drop");
        let slots = overview_slots();
        let inspector_row = grid
            .rows
            .iter()
            .position(|row| row.contains(&Some(CardId::Inspector)))
            .expect("inspector row");
        for (width, height) in [(70u16, 12u16), (130, 12), (130, 36)] {
            let plan = plan_lines(&grid, &slots, width, height);
            let inspector_line = plan
                .lines
                .iter()
                .find(|line| line.row == inspector_row && line.cards.contains(&CardId::Inspector))
                .expect("inspector line shown");
            assert!(
                inspector_line.cards.contains(&CardId::SqlStream),
                "{width}x{height}"
            );
        }
        // 60열에서는 스트림 카드만 다음 화면 줄로 넘어가고 인스펙터 줄은 그대로 보임
        let plan = plan_lines(&grid, &slots, 60, 12);
        assert!(
            plan.lines
                .iter()
                .any(|line| line.cards == [CardId::Inspector])
        );
        assert!(
            plan.lines
                .iter()
                .all(|line| !line.cards.contains(&CardId::SqlStream))
        );
    }

    // rank 0 줄의 최소 높이도 모자라면 아래 줄부터 숨김, 줄 수는 GRID_SLOTS 까지
    #[test]
    fn lines_hide_from_the_bottom_when_rank_zero_minimums_do_not_fit() {
        let grid = default_rows(ActiveTab::Overview);
        let plan = plan_lines(&grid, &overview_slots(), 130, 7);
        assert_eq!(line_shape(&plan), [(0, 4, 3), (1, 2, 4)]);
        let plan = plan_lines(&grid, &overview_slots(), 130, 0);
        assert!(plan.lines.is_empty());

        // 세 장 줄 다섯 개는 폭 60 에서 줄마다 화면 줄 두 개, 앞 여섯 화면 줄만 남음
        let rows: Vec<&[CardId]> = CardId::ALL.chunks(3).collect();
        let grid = grid_from(&rows);
        let slots: Vec<CardSlot> = CardId::ALL
            .iter()
            .map(|card| fixed_slot(*card, 29, 3, 0, 0))
            .collect();
        let plan = plan_lines(&grid, &slots, 60, 100);
        assert_eq!(plan.lines.len(), GRID_SLOTS);
        assert_eq!(plan.lines.last().map(|line| line.row), Some(2));
    }

    #[test]
    fn resolve_drop_maps_borders_to_new_rows_and_inner_cells_to_neighbors() {
        let plan = GridPlan {
            lines: vec![
                GridLine {
                    row: 0,
                    cards: vec![A, B],
                    height: 4,
                },
                GridLine {
                    row: 1,
                    cards: vec![C],
                    height: 4,
                },
            ],
        };
        let line_rects = [Rect::new(0, 0, 40, 4), Rect::new(0, 4, 40, 4)];
        let card_rects = [
            (A, Rect::new(0, 0, 20, 4)),
            (B, Rect::new(21, 0, 19, 4)),
            (C, Rect::new(0, 4, 40, 4)),
        ];
        let drop_at = |moving: CardId, x: u16, y: u16| {
            resolve_drop(&plan, &line_rects, &card_rects, moving, Position::new(x, y))
        };
        assert_eq!(drop_at(C, 5, 0), DropTarget::RowAbove(A));
        // 자기 줄 위 테두리는 줄 안: 이웃 앞뒤, 자기 카드 위는 제자리
        assert_eq!(drop_at(A, 25, 0), DropTarget::Before(B));
        assert_eq!(drop_at(A, 35, 0), DropTarget::After(B));
        assert_eq!(drop_at(A, 5, 0), DropTarget::Stay);
        assert_eq!(drop_at(B, 5, 0), DropTarget::Before(A));
        assert_eq!(drop_at(A, 30, 3), DropTarget::RowBelow(A));
        assert_eq!(drop_at(C, 30, 3), DropTarget::RowBelow(A));
        assert_eq!(drop_at(A, 5, 4), DropTarget::RowAbove(C));
        assert_eq!(drop_at(A, 5, 1), DropTarget::Stay);
        assert_eq!(drop_at(C, 5, 1), DropTarget::Before(A));
        assert_eq!(drop_at(C, 15, 2), DropTarget::Before(B));
        assert_eq!(drop_at(C, 35, 2), DropTarget::After(B));
        assert_eq!(drop_at(A, 20, 5), DropTarget::After(C));
        assert_eq!(drop_at(C, 50, 50), DropTarget::Stay);
    }

    #[test]
    fn apply_drop_joins_creates_and_removes_rows_within_the_caps() {
        let base = grid_from(&[&[A, B], &[C], &[D]]);
        assert_eq!(apply_drop(&base, A, DropTarget::Stay), Some(base));
        assert_eq!(apply_drop(&base, A, DropTarget::Before(A)), Some(base));

        // 합류와 빈 줄 제거
        let joined = apply_drop(&base, C, DropTarget::After(B)).map(|grid| rows_of(&grid));
        assert_eq!(joined, Some(vec![vec![A, B, C], vec![D]]));
        let joined = apply_drop(&base, D, DropTarget::Before(A)).map(|grid| rows_of(&grid));
        assert_eq!(joined, Some(vec![vec![D, A, B], vec![C]]));
        let swapped = apply_drop(&base, A, DropTarget::After(B)).map(|grid| rows_of(&grid));
        assert_eq!(swapped, Some(vec![vec![B, A], vec![C], vec![D]]));

        // 위·아래 새 줄
        let above = apply_drop(&base, B, DropTarget::RowAbove(A)).map(|grid| rows_of(&grid));
        assert_eq!(above, Some(vec![vec![B], vec![A], vec![C], vec![D]]));
        let below = apply_drop(&base, A, DropTarget::RowBelow(D)).map(|grid| rows_of(&grid));
        assert_eq!(below, Some(vec![vec![B], vec![C], vec![D], vec![A]]));
        let own = apply_drop(&base, C, DropTarget::RowAbove(C)).map(|grid| rows_of(&grid));
        assert_eq!(own, Some(vec![vec![A, B], vec![C], vec![D]]));

        // 7번째 카드, 7번째 줄은 거부
        let full_row: Vec<CardId> = CardId::ALL[4..10].to_vec();
        let crowded = grid_from(&[&full_row, &[A]]);
        assert_eq!(
            apply_drop(&crowded, A, DropTarget::After(full_row[5])),
            None
        );
        let six_rows = grid_from(&[
            &[A, B],
            &[C],
            &[D],
            &[CardId::Gate],
            &[CardId::ChartSqlMix],
            &[CardId::ChartTopTables],
        ]);
        assert_eq!(apply_drop(&six_rows, B, DropTarget::RowBelow(A)), None);
        let lone = apply_drop(&six_rows, C, DropTarget::RowBelow(D)).map(|grid| rows_of(&grid));
        assert_eq!(lone.map(|rows| rows.len()), Some(6));

        // 격자에 없는 카드
        assert_eq!(
            apply_drop(&base, CardId::AlertLog, DropTarget::After(A)),
            None
        );
    }

    // 상하 이동은 자기 새 줄과 이웃 줄 합류를 번갈아, 끝에서는 None
    #[test]
    fn shift_card_swaps_sideways_and_alternates_own_row_and_join_vertically() {
        let base = grid_from(&[&[A, B], &[C]]);
        let shifted = |grid: &TabGrid, card: CardId, key: KeyCode| {
            shift_card(grid, card, key, &CardId::ALL).map(|moved| rows_of(&moved))
        };
        assert_eq!(
            shifted(&base, A, KeyCode::Right),
            Some(vec![vec![B, A], vec![C]])
        );
        assert_eq!(
            shifted(&base, B, KeyCode::Left),
            Some(vec![vec![B, A], vec![C]])
        );
        assert_eq!(shifted(&base, A, KeyCode::Left), None);
        assert_eq!(shifted(&base, B, KeyCode::Right), None);
        assert_eq!(shifted(&base, A, KeyCode::Char('x')), None);

        // 아래로: 새 줄, 합류, 새 줄, 끝
        let step = shift_card(&base, A, KeyCode::Down, &CardId::ALL).unwrap();
        assert_eq!(rows_of(&step), vec![vec![B], vec![A], vec![C]]);
        let step = shift_card(&step, A, KeyCode::Down, &CardId::ALL).unwrap();
        assert_eq!(rows_of(&step), vec![vec![B], vec![C, A]]);
        let step = shift_card(&step, A, KeyCode::Down, &CardId::ALL).unwrap();
        assert_eq!(rows_of(&step), vec![vec![B], vec![C], vec![A]]);
        assert_eq!(shift_card(&step, A, KeyCode::Down, &CardId::ALL), None);

        // 위로: 혼자인 줄은 위 줄 끝에 합류, 함께인 줄은 위에 새 줄, 맨 위 혼자면 끝
        assert_eq!(shifted(&base, C, KeyCode::Up), Some(vec![vec![A, B, C]]));
        assert_eq!(
            shifted(&base, B, KeyCode::Up),
            Some(vec![vec![B], vec![A], vec![C]])
        );
        let top = grid_from(&[&[A], &[B, C]]);
        assert_eq!(shift_card(&top, A, KeyCode::Up, &CardId::ALL), None);
        assert_eq!(shifted(&top, A, KeyCode::Down), Some(vec![vec![B, C, A]]));
    }
}
