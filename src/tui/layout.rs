//! layout.rs
//! 카드 배치: 탭 격자·경계 비율 보관, 카드 영역 분할, 경계 드래그 영역 계산.

pub mod grid;

use ratatui::layout::Rect;
use crate::tui::cards::{CardId, GridLineHit, UiHitAreas};
use crate::tui::layout::grid::{CardSlot, GRID_SLOTS, TabGrid, default_rows, plan_lines};
use crate::tui::state::{ActiveTab, TAB_COUNT};

// 그룹당 최대 칸 수(격자 줄·카드 수 상한)와 경계 수, 경계 비율 단위(만분율), 탭당 격자 비율 칸 수
pub const MAX_SLOTS: usize = GRID_SLOTS;
pub const MAX_CUTS: usize = MAX_SLOTS - 1;
const RATIO_SCALE: u32 = 10_000;
const GRID_GROUPS_PER_TAB: usize = MAX_SLOTS * 2;

// 1. 분할 그룹과 드래그 영역 --------------------------------------------------
// 한 축으로 나란한 카드 묶음
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitGroup {
    // 탭 격자: 화면 줄 높이 (보이는 줄 수), 격자 줄 안 카드 폭 (격자 줄 번호)
    GridRows(ActiveTab, u8),
    GridCols(ActiveTab, u8),
}

impl SplitGroup {
    // 탭·줄별 고유 번호 (탭 격자 비율 표 위치)
    pub fn index(self) -> usize {
        match self {
            Self::GridRows(tab, lines) => {
                tab as usize * GRID_GROUPS_PER_TAB + usize::from(lines).clamp(1, MAX_SLOTS) - 1
            }
            Self::GridCols(tab, row) => {
                tab as usize * GRID_GROUPS_PER_TAB + MAX_SLOTS + usize::from(row).min(MAX_SLOTS - 1)
            }
        }
    }

    pub fn axis(self) -> SplitAxis {
        match self {
            Self::GridCols(..) => SplitAxis::Columns,
            Self::GridRows(..) => SplitAxis::Rows,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitAxis {
    // 좌우 카드, 사이 빈 칸 1열
    Columns,
    // 상하 카드, 테두리 맞닿음
    Rows,
}

// 경계 하나의 드래그 영역 (위치는 origin 기준 셀)
#[derive(Debug, Clone, Copy)]
pub struct SplitHit {
    pub group: SplitGroup,
    pub index: usize,
    pub axis: SplitAxis,
    // 마우스로 잡는 영역, 손잡이 표시 줄
    pub grab: Rect,
    pub line: Rect,
    // 분할 구간 시작 좌표와 길이
    pub origin: u16,
    pub span_len: u16,
    // 그룹 전체 경계 위치, 이웃 카드 범위, 이 경계의 이동 범위
    pub cuts: [u16; MAX_CUTS],
    pub prev_end: u16,
    pub next_cut: u16,
    pub low: u16,
    pub high: u16,
}

// 진행 중 드래그 (grip = 누른 지점과 경계의 거리, moved = 실제로 끌었는지)
#[derive(Debug, Clone, Copy)]
pub struct SplitDrag {
    pub group: SplitGroup,
    pub index: usize,
    pub grip: i32,
    pub moved: bool,
}

// 2. 카드 배치 상태 -----------------------------------------------------------
// 격자 카드 이동 시작 때 탭의 격자 경계 비율 (취소 때 복원)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GridMoveSnapshot {
    tab: ActiveTab,
    ratios: [Option<[u16; MAX_CUTS]>; GRID_GROUPS_PER_TAB],
}

// 탭별 격자와 격자 경계 비율, None 이면 기본 배치
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CardLayout {
    // 탭별 격자 (None 이면 기본 격자)
    grids: [Option<TabGrid>; TAB_COUNT],
    // 탭별 격자 비율 (앞 MAX_SLOTS = 보이는 줄 수별 높이, 뒤 = 격자 줄별 폭)
    grid_ratios: [[Option<[u16; MAX_CUTS]>; GRID_GROUPS_PER_TAB]; TAB_COUNT],
    grid_move: Option<GridMoveSnapshot>,
}

impl CardLayout {
    // 카드 그룹 분할: 경계 위치 결정, 드래그 영역 등록, 칸 영역 반환
    // 반환 = 칸 순서의 영역 (칸 크기·경계 비율·칸 최소 크기는 칸에 남음)
    // defaults = 기본 경계 위치(셀), mins = 칸별 최소 크기 (defaults 보다 1개 많음)
    pub fn split_cards(
        &self,
        hit_areas: &mut UiHitAreas,
        group: SplitGroup,
        axis: SplitAxis,
        area: Rect,
        defaults: &[u16],
        mins: &[u16],
    ) -> Vec<Rect> {
        debug_assert_eq!(mins.len(), defaults.len() + 1);
        let (origin, span_len, gap) = match axis {
            SplitAxis::Columns => (area.x, area.width, 1),
            SplitAxis::Rows => (area.y, area.height, 0),
        };
        let count = defaults.len().min(MAX_CUTS);

        // 1. 경계 위치 (조절한 그룹만 비율·최소 크기 적용)
        let ratios = self.ratio_cell(group);
        let mut cuts = [0u16; MAX_CUTS];
        let mut pane_start = 0u16;
        for (idx, default_cut) in defaults.iter().take(count).enumerate() {
            cuts[idx] = match ratios {
                Some(ratio) => {
                    let scaled = u32::from(ratio[idx]) * u32::from(span_len);
                    let wanted = ((scaled + RATIO_SCALE / 2) / RATIO_SCALE) as u16;
                    let tail_min: u16 = mins[idx + 1..].iter().map(|min| min + gap).sum();
                    let low = pane_start.saturating_add(mins[idx]);
                    wanted
                        .min(span_len.saturating_sub(tail_min))
                        .max(low)
                        .min(span_len)
                }
                None => (*default_cut).min(span_len),
            };
            pane_start = cuts[idx].saturating_add(gap);
        }

        // 2. 칸 영역 (칸 순서)
        let mut slots = Vec::with_capacity(count + 1);
        let mut pane_start = 0u16;
        for pane_end in cuts[..count].iter().copied().chain([span_len]) {
            let pane_len = pane_end.saturating_sub(pane_start);
            slots.push(match axis {
                SplitAxis::Columns => Rect::new(origin + pane_start, area.y, pane_len, area.height),
                SplitAxis::Rows => Rect::new(area.x, origin + pane_start, area.width, pane_len),
            });
            pane_start = pane_end.saturating_add(gap).min(span_len);
        }

        // 3. 경계 드래그 영역 (이웃 칸 최소 크기 사이)
        for (index, cut) in cuts[..count].iter().copied().enumerate() {
            let prev_end = if index == 0 { 0 } else { cuts[index - 1] + gap };
            let next_cut = if index + 1 < count {
                cuts[index + 1]
            } else {
                span_len
            };
            let low = prev_end.saturating_add(mins[index]);
            let high = next_cut.saturating_sub(gap + mins[index + 1]);
            if low > high || cut == 0 {
                continue;
            }
            // 최소 크기 밖 기본 경계는 첫 드래그에 튀지 않게 범위 확장
            let (low, high) = (low.min(cut), high.max(cut));
            let (grab, line) = match axis {
                // 빈 칸과 양옆 카드 테두리
                SplitAxis::Columns => (
                    Rect::new(origin + cut - 1, area.y, 3, area.height),
                    Rect::new(origin + cut, area.y, 1, area.height),
                ),
                // 위 카드 아래 테두리, 아래 카드 위 테두리
                SplitAxis::Rows => (
                    Rect::new(area.x, origin + cut - 1, area.width, 2),
                    Rect::new(area.x, origin + cut - 1, area.width, 1),
                ),
            };
            hit_areas.split_hits.push(SplitHit {
                group,
                index,
                axis,
                grab: grab.intersection(area),
                line: line.intersection(area),
                origin,
                span_len,
                cuts,
                prev_end,
                next_cut,
                low,
                high,
            });
        }

        slots
    }

    // 경계 이동: pointer = 경계가 놓일 화면 좌표, 그룹 전체를 비율로 보관
    // 반환 = 경계 앞뒤 카드 크기
    pub fn drag_cut(&mut self, hit: &SplitHit, pointer: i32) -> Option<(u16, u16)> {
        if hit.span_len == 0 || hit.low > hit.high {
            return None;
        }
        let offset = pointer - i32::from(hit.origin);
        let cut = offset.clamp(i32::from(hit.low), i32::from(hit.high)) as u16;
        let mut cuts = hit.cuts;
        cuts[hit.index] = cut;
        let span_len = u32::from(hit.span_len);
        *self.ratio_cell_mut(hit.group) =
            Some(cuts.map(|cut| ((u32::from(cut) * RATIO_SCALE + span_len / 2) / span_len) as u16));

        let gap = match hit.axis {
            SplitAxis::Columns => 1,
            SplitAxis::Rows => 0,
        };
        Some((cut - hit.prev_end, hit.next_cut.saturating_sub(cut + gap)))
    }

    // 탭 격자 배치: 화면 줄 계획, 줄 높이·카드 폭 분할 (경계 드래그 영역 등록), 보이는 카드 영역 반환
    pub fn arrange_tab(
        &self,
        hit_areas: &mut UiHitAreas,
        tab: ActiveTab,
        area: Rect,
        slots: &[CardSlot],
    ) -> Vec<(CardId, Rect)> {
        // 1. 화면 줄 계획
        let plan = plan_lines(&self.grid_of(tab), slots, area.width, area.height);
        let line_count = plan.lines.len();
        if line_count == 0 {
            return Vec::new();
        }

        // 2. 줄 높이 분할 (줄 최소 높이 = 카드 최소 높이 중 최대)
        let mut row_cuts = Vec::with_capacity(line_count);
        let mut line_mins = Vec::with_capacity(line_count);
        let mut line_end = 0u16;
        for line in &plan.lines {
            line_end = line_end.saturating_add(line.height);
            row_cuts.push(line_end);
            let min_h = slots
                .iter()
                .filter(|slot| line.cards.contains(&slot.card))
                .map(|slot| slot.min_h)
                .max()
                .unwrap_or(0);
            line_mins.push(min_h);
        }
        row_cuts.pop();
        let line_rects = self.split_cards(
            hit_areas,
            SplitGroup::GridRows(tab, line_count as u8),
            SplitAxis::Rows,
            area,
            &row_cuts,
            &line_mins,
        );

        // 3. 줄마다 카드 폭 분할 (기본 폭 = weight 비례, 넘긴 격자 줄은 비율 미적용)
        // 넘김 판정은 격자 줄 카드 수 기준 (숨긴 화면 줄 포함)
        // 칸 크기는 칸에 남김: 줄 카드들의 weight 를 기본 격자 위치 순으로 정렬해 칸 순서대로 배정
        let grid = self.grid_of(tab);
        let default_cells: Vec<Option<CardId>> =
            default_rows(tab).rows.iter().flatten().copied().collect();
        let plain = CardLayout::default();
        let mut placed = Vec::with_capacity(slots.len());
        for (line, line_rect) in plan.lines.iter().zip(&line_rects) {
            let row_members = grid.rows[line.row]
                .iter()
                .flatten()
                .filter(|card| slots.iter().any(|slot| slot.card == **card))
                .count();
            let wrapped = line.cards.len() < row_members;
            let source = if wrapped { &plain } else { self };
            let line_slots: Vec<&CardSlot> = line
                .cards
                .iter()
                .filter_map(|card| slots.iter().find(|slot| slot.card == *card))
                .collect();
            let mut ranked_weights: Vec<(usize, u32)> = line_slots
                .iter()
                .map(|slot| {
                    let rank = default_cells
                        .iter()
                        .position(|cell| *cell == Some(slot.card))
                        .unwrap_or(usize::MAX);
                    (rank, u32::from(slot.weight.max(1)))
                })
                .collect();
            ranked_weights.sort_by_key(|(rank, _)| *rank);
            let total_weight: u32 = ranked_weights.iter().map(|(_, weight)| weight).sum();
            let gaps = line_slots.len().saturating_sub(1) as u16;
            let usable = u32::from(line_rect.width.saturating_sub(gaps));
            let mut col_cuts = Vec::with_capacity(line_slots.len());
            let mut weight_sum = 0u32;
            for (index, (_, weight)) in ranked_weights.iter().enumerate() {
                weight_sum += weight;
                col_cuts.push((usable * weight_sum / total_weight) as u16 + index as u16);
            }
            col_cuts.pop();
            let col_mins: Vec<u16> = line_slots.iter().map(|slot| slot.min_w).collect();
            let hits_before = hit_areas.split_hits.len();
            let card_rects = source.split_cards(
                hit_areas,
                SplitGroup::GridCols(tab, line.row as u8),
                SplitAxis::Columns,
                *line_rect,
                &col_cuts,
                &col_mins,
            );
            // 넘긴 격자 줄은 폭 경계 잡기 없음 (비율 미적용)
            if wrapped {
                hit_areas.split_hits.truncate(hits_before);
            }
            let line_cards: Vec<(CardId, Rect)> =
                line.cards.iter().copied().zip(card_rects).collect();
            placed.extend_from_slice(&line_cards);
            hit_areas.grid_lines.push(GridLineHit {
                tab,
                row: line.row,
                rect: *line_rect,
                cards: line_cards,
            });
        }
        placed
    }

    // 탭의 지금 격자 (바꾸지 않았으면 기본 격자)
    pub fn grid_of(&self, tab: ActiveTab) -> TabGrid {
        self.grids[tab as usize].unwrap_or_else(|| default_rows(tab))
    }

    // 격자 지정: 줄 수가 바뀌면 탭 격자 비율 전체, 아니면 구성이 바뀐 줄의 폭 비율만 초기화
    pub fn set_grid(&mut self, tab: ActiveTab, grid: TabGrid) {
        // 1. 줄 수 비교
        let previous = self.grid_of(tab);
        let old_rows = previous
            .rows
            .iter()
            .filter(|cells| cells.iter().any(Option::is_some))
            .count();
        let new_rows = grid
            .rows
            .iter()
            .filter(|cells| cells.iter().any(Option::is_some))
            .count();

        // 2. 비율 초기화
        let tab_ratios = &mut self.grid_ratios[tab as usize];
        for (row, (before, after)) in previous.rows.iter().zip(&grid.rows).enumerate() {
            let kept = old_rows == new_rows
                && before.iter().flatten().count() == after.iter().flatten().count()
                && before
                    .iter()
                    .flatten()
                    .all(|card| after.contains(&Some(*card)));
            if !kept {
                tab_ratios[MAX_SLOTS + row] = None;
            }
        }
        if old_rows != new_rows {
            tab_ratios[..MAX_SLOTS].fill(None);
        }

        // 3. 기본 격자와 같으면 보관하지 않음
        self.grids[tab as usize] = (grid != default_rows(tab)).then_some(grid);
    }

    // 이동 중 미리보기 격자 (비율 유지)
    pub fn preview_grid(&mut self, tab: ActiveTab, grid: TabGrid) {
        self.grids[tab as usize] = (grid != default_rows(tab)).then_some(grid);
    }

    // 탭을 기본 격자로 (격자 비율도 비움)
    pub fn reset_tab(&mut self, tab: ActiveTab) {
        self.grids[tab as usize] = None;
        self.grid_ratios[tab as usize] = [None; GRID_GROUPS_PER_TAB];
    }

    // 격자 카드 이동 시작: 탭 격자 비율 보관, 반환 = 이동 전 격자
    pub fn begin_grid_move(&mut self, tab: ActiveTab) -> TabGrid {
        self.grid_move = Some(GridMoveSnapshot {
            tab,
            ratios: self.grid_ratios[tab as usize],
        });
        self.grid_of(tab)
    }

    // 격자 카드 이동 끝: restore 가 있으면 그 격자와 시작 때 비율로 되돌림
    pub fn end_grid_move(&mut self, tab: ActiveTab, restore: Option<TabGrid>) {
        let snapshot = self.grid_move.take();
        let Some(baseline) = restore else {
            return;
        };
        self.grids[tab as usize] = (baseline != default_rows(tab)).then_some(baseline);
        if let Some(snapshot) = snapshot
            && snapshot.tab == tab
        {
            self.grid_ratios[tab as usize] = snapshot.ratios;
        }
    }

    // 배치 파일 저장용 격자 비율: <탭>_rows_<보이는 줄 수>, <탭>_cols_<격자 줄 번호>
    pub fn grid_ratio_entries(&self) -> Vec<(String, Vec<u16>)> {
        let cells = ActiveTab::ALL
            .iter()
            .flat_map(|tab| (0..GRID_GROUPS_PER_TAB).map(move |slot| (*tab, slot)));
        let mut entries = Vec::new();
        for (tab, slot) in cells {
            let Some(ratio) = self.grid_ratios[tab as usize][slot] else {
                continue;
            };
            let (kind, number, cuts) = if slot < MAX_SLOTS {
                ("rows", slot + 1, slot)
            } else {
                let row = slot - MAX_SLOTS;
                let members = self.grid_of(tab).rows[row].iter().flatten().count();
                ("cols", row, members.saturating_sub(1))
            };
            if cuts == 0 {
                continue;
            }
            let tab_key = tab.tab_key();
            entries.push((
                format!("{tab_key}_{kind}_{number}"),
                ratio[..cuts.min(MAX_CUTS)].to_vec(),
            ));
        }
        entries
    }

    // 배치 파일 복원용: 키·길이·범위가 맞을 때만 반영 (폭 비율 길이는 지금 격자 줄의 카드 수 - 1)
    pub fn restore_grid_ratios(&mut self, key: &str, values: &[i64]) -> bool {
        // 1. 키 해석
        let Some((head, number)) = key.rsplit_once('_') else {
            return false;
        };
        let Some((tab_key, kind)) = head.rsplit_once('_') else {
            return false;
        };
        let Some(tab) = ActiveTab::ALL
            .into_iter()
            .find(|tab| tab.tab_key() == tab_key)
        else {
            return false;
        };
        let Ok(number) = number.parse::<usize>() else {
            return false;
        };
        let (slot, cuts) = match kind {
            "rows" if (2..=MAX_SLOTS).contains(&number) => (number - 1, number - 1),
            "cols" if number < MAX_SLOTS => {
                let members = self.grid_of(tab).rows[number].iter().flatten().count();
                (MAX_SLOTS + number, members.saturating_sub(1))
            }
            _ => return false,
        };

        // 2. 길이·범위
        if cuts == 0 || values.len() != cuts {
            return false;
        }
        let mut ratios = [0u16; MAX_CUTS];
        for (index, value) in values.iter().enumerate() {
            let Ok(ratio) = u16::try_from(*value) else {
                return false;
            };
            if u32::from(ratio) > RATIO_SCALE {
                return false;
            }
            ratios[index] = ratio;
        }
        self.grid_ratios[tab as usize][slot] = Some(ratios);
        true
    }

    // 그룹 경계 비율 보관 칸 (탭 격자 표)
    fn ratio_cell(&self, group: SplitGroup) -> Option<[u16; MAX_CUTS]> {
        let index = group.index();
        self.grid_ratios[index / GRID_GROUPS_PER_TAB][index % GRID_GROUPS_PER_TAB]
    }

    fn ratio_cell_mut(&mut self, group: SplitGroup) -> &mut Option<[u16; MAX_CUTS]> {
        let index = group.index();
        &mut self.grid_ratios[index / GRID_GROUPS_PER_TAB][index % GRID_GROUPS_PER_TAB]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;
    use crate::tui::layout::grid::{DropTarget, SlotHeight, apply_drop, shift_card};

    fn split_rows(layout: &CardLayout, height: u16) -> (Vec<Rect>, UiHitAreas) {
        let mut hit_areas = UiHitAreas::default();
        let area = Rect::new(0, 10, 100, height);
        let panes = layout.split_cards(
            &mut hit_areas,
            SplitGroup::GridRows(ActiveTab::Overview, 3),
            SplitAxis::Rows,
            area,
            &[height / 2, height - 6],
            &[5, 3, 3],
        );
        (panes, hit_areas)
    }

    // 개요 기본 격자 카드 조건 (차트 줄 rank 2 reserve 5, 최소 폭 합 63 이라 폭 60 에서 넘김, 스트림 줄 rank 3 reserve 9)
    fn grid_slots() -> Vec<CardSlot> {
        let grid = default_rows(ActiveTab::Overview);
        let mut slots = Vec::new();
        for (row, cells) in grid.rows.iter().enumerate() {
            let (min_w, min_h, height, rank, reserve) = match row {
                0 => (14, 3, SlotHeight::Fixed(3), 0, 0),
                1 => (20, 3, SlotHeight::Fixed(3), 0, 0),
                2 => (31, 5, SlotHeight::Fixed(5), 2, 5),
                3 => (30, 6, SlotHeight::Fixed(10), 3, 9),
                _ => (20, 3, SlotHeight::Fill, 0, 0),
            };
            for card in cells.iter().flatten() {
                slots.push(CardSlot {
                    card: *card,
                    min_w,
                    min_h,
                    height,
                    weight: 1,
                    rank,
                    reserve,
                });
            }
        }
        slots
    }

    fn assert_disjoint_inside(placed: &[(CardId, Rect)], area: Rect) {
        for (index, (card, rect)) in placed.iter().enumerate() {
            assert!(
                rect.is_empty() || area.union(*rect) == area,
                "{card:?} {rect:?}"
            );
            for (other, other_rect) in &placed[index + 1..] {
                assert!(!rect.intersects(*other_rect), "{card:?} {other:?}");
            }
        }
    }

    #[test]
    fn default_layout_keeps_given_cuts() {
        let (panes, hit_areas) = split_rows(&CardLayout::default(), 30);
        let heights: Vec<u16> = panes.iter().map(|pane| pane.height).collect();
        assert_eq!(heights, [15, 9, 6]);
        assert_eq!(panes[2].y, 34);
        assert_eq!(hit_areas.split_hits.len(), 2);
        // 위 카드 아래 테두리 + 아래 카드 위 테두리
        assert_eq!(hit_areas.split_hits[0].grab, Rect::new(0, 24, 100, 2));
        assert_eq!(hit_areas.split_hits[0].line, Rect::new(0, 24, 100, 1));
    }

    #[test]
    fn dragged_cut_clamps_to_neighbor_minimum_and_keeps_ratio_on_resize() {
        let mut layout = CardLayout::default();
        let (_, hit_areas) = split_rows(&layout, 30);

        // 아래 경계를 위로 끝까지: 가운데 카드 최소 3행에서 멈춤, 위 경계는 그대로
        assert_eq!(layout.drag_cut(&hit_areas.split_hits[1], 0), Some((3, 12)));
        let (panes, hit_areas) = split_rows(&layout, 30);
        let heights: Vec<u16> = panes.iter().map(|pane| pane.height).collect();
        assert_eq!(heights, [15, 3, 12]);

        // 위 경계를 아래로 끝까지: 가운데 카드를 밀지 않음
        layout.drag_cut(&hit_areas.split_hits[0], 1000);
        let (panes, _) = split_rows(&layout, 30);
        assert_eq!(panes[0].height, 15);

        // 터미널 높이가 두 배가 되면 같은 비율
        let (panes, _) = split_rows(&layout, 60);
        let heights: Vec<u16> = panes.iter().map(|pane| pane.height).collect();
        assert_eq!(heights, [30, 6, 24]);
    }

    #[test]
    fn column_split_leaves_one_cell_gap_and_grabs_both_borders() {
        let mut hit_areas = UiHitAreas::default();
        let area = Rect::new(2, 5, 81, 8);
        let panes = CardLayout::default().split_cards(
            &mut hit_areas,
            SplitGroup::GridCols(ActiveTab::Overview, 2),
            SplitAxis::Columns,
            area,
            &[44],
            &[20, 20],
        );
        let hits = &hit_areas.split_hits;
        assert_eq!(panes[0], Rect::new(2, 5, 44, 8));
        assert_eq!(panes[1], Rect::new(47, 5, 36, 8));
        assert_eq!(hits[0].line, Rect::new(46, 5, 1, 8));
        assert_eq!(hits[0].grab, Rect::new(45, 5, 3, 8));
        assert_eq!((hits[0].low, hits[0].high), (20, 60));
    }

    #[test]
    fn shrunk_terminal_keeps_leading_cards_at_minimum() {
        let mut layout = CardLayout::default();
        let (_, hit_areas) = split_rows(&layout, 30);
        layout.drag_cut(&hit_areas.split_hits[0], 10 + 20);

        // 조절한 배치는 좁아져도 앞 카드부터 최소 크기 확보
        let (panes, _) = split_rows(&layout, 9);
        let heights: Vec<u16> = panes.iter().map(|pane| pane.height).collect();
        assert_eq!(heights, [5, 3, 1]);
    }

    #[test]
    fn reset_tab_restores_default_cuts_for_that_tab_only() {
        let mut layout = CardLayout::default();
        let (_, hit_areas) = split_rows(&layout, 30);
        layout.drag_cut(&hit_areas.split_hits[0], 10 + 20);
        assert_ne!(layout, CardLayout::default());

        layout.reset_tab(ActiveTab::Alerts);
        assert_ne!(layout, CardLayout::default());

        layout.reset_tab(ActiveTab::Overview);
        assert_eq!(layout, CardLayout::default());
    }

    #[test]
    fn six_card_row_supports_five_cuts() {
        let mut hit_areas = UiHitAreas::default();
        let area = Rect::new(0, 0, 100, 4);
        let layout = CardLayout::default();
        let panes = layout.split_cards(
            &mut hit_areas,
            SplitGroup::GridCols(ActiveTab::Overview, 1),
            SplitAxis::Columns,
            area,
            &[15, 31, 47, 63, 79],
            &[8; 6],
        );
        assert_eq!(panes.len(), 6);
        assert_eq!(hit_areas.split_hits.len(), 5);
        assert_eq!(panes[5].right(), 100);
    }

    #[test]
    fn grid_group_indices_are_unique_per_tab_line_count_and_row() {
        let mut grid_indices = Vec::new();
        for tab in ActiveTab::ALL {
            for lines in 1..=MAX_SLOTS as u8 {
                grid_indices.push(SplitGroup::GridRows(tab, lines).index());
            }
            for row in 0..MAX_SLOTS as u8 {
                grid_indices.push(SplitGroup::GridCols(tab, row).index());
            }
        }
        grid_indices.sort_unstable();
        grid_indices.dedup();
        assert_eq!(grid_indices.len(), TAB_COUNT * GRID_GROUPS_PER_TAB);
        assert!(
            grid_indices
                .iter()
                .all(|index| *index < TAB_COUNT * GRID_GROUPS_PER_TAB)
        );

        let rows = SplitGroup::GridRows(ActiveTab::Overview, 3);
        let cols = SplitGroup::GridCols(ActiveTab::Overview, 1);
        assert_eq!(
            (rows.axis(), cols.axis()),
            (SplitAxis::Rows, SplitAxis::Columns)
        );
    }

    // 60x12 본문: 지표 네 장 한 줄, 표 두 장 한 줄, 인스펙터만 겹치지 않게
    #[test]
    fn minimum_body_shows_metrics_tables_and_inspector_without_overlap() {
        let mut hit_areas = UiHitAreas::default();
        let area = Rect::new(0, 3, 60, 12);
        let placed = CardLayout::default().arrange_tab(
            &mut hit_areas,
            ActiveTab::Overview,
            area,
            &grid_slots(),
        );
        let cards: Vec<CardId> = placed.iter().map(|(card, _)| *card).collect();
        assert_eq!(
            cards,
            [
                CardId::Risk,
                CardId::Connections,
                CardId::Traffic,
                CardId::QueryRate,
                CardId::Processes,
                CardId::Sockets,
                CardId::Inspector,
            ]
        );
        let rows: Vec<(u16, u16)> = placed
            .iter()
            .map(|(_, rect)| (rect.y, rect.height))
            .collect();
        assert!(rows[..4].iter().all(|row| *row == (3, 3)));
        assert_eq!(rows[4..6], [(6, 3), (6, 3)]);
        assert_eq!(rows[6], (9, 6));
        assert_disjoint_inside(&placed, area);
        assert_eq!(placed[3].1.right(), 60);

        // 줄 경계 드래그 영역 둘
        let row_hits = hit_areas
            .split_hits
            .iter()
            .filter(|hit| hit.group == SplitGroup::GridRows(ActiveTab::Overview, 3))
            .count();
        assert_eq!(row_hits, 2);
    }

    // 모든 카드를 여섯 줄(첫 줄 여섯 장)에 채운 격자도 60x12 에서 패닉 없이 겹치지 않음
    #[test]
    fn a_full_grid_of_every_card_renders_disjoint_rects_at_the_minimum_body() {
        let row_sizes = [6, 2, 2, 2, 2, 1];
        let mut grid = TabGrid::default();
        let mut cards = CardId::ALL.iter();
        for (row, size) in row_sizes.iter().enumerate() {
            for (column, card) in cards.by_ref().take(*size).enumerate() {
                grid.rows[row][column] = Some(*card);
            }
        }
        let mut layout = CardLayout::default();
        layout.set_grid(ActiveTab::Overview, grid);
        let area = Rect::new(0, 3, 60, 12);
        for (min_w, min_h, height) in [
            (12, 3, SlotHeight::Fixed(3)),
            (8, 1, SlotHeight::Fill),
            (70, 20, SlotHeight::Fixed(30)),
        ] {
            let slots: Vec<CardSlot> = CardId::ALL
                .iter()
                .map(|card| CardSlot {
                    card: *card,
                    min_w,
                    min_h,
                    height,
                    weight: 1,
                    rank: 0,
                    reserve: 0,
                })
                .collect();
            let mut hit_areas = UiHitAreas::default();
            let placed = layout.arrange_tab(&mut hit_areas, ActiveTab::Overview, area, &slots);
            assert_disjoint_inside(&placed, area);
            assert!(placed.len() <= CardId::ALL.len());
        }
    }

    // 넘긴 격자 줄은 폭 비율을 쓰지 않고, 한 줄에 들어가면 비율 적용
    #[test]
    fn width_ratios_apply_only_to_rows_that_did_not_wrap() {
        let mut layout = CardLayout::default();
        // 차트 줄(격자 줄 2, 최소 폭 합 63)은 폭 60 에서 넘김, 130 에서는 한 줄
        assert!(layout.restore_grid_ratios("overview_cols_2", &[3_000]));
        let chart_rects = |layout: &CardLayout, width: u16| {
            let mut hit_areas = UiHitAreas::default();
            let area = Rect::new(0, 0, width, 40);
            let placed =
                layout.arrange_tab(&mut hit_areas, ActiveTab::Overview, area, &grid_slots());
            placed
                .into_iter()
                .filter(|(card, _)| *card == CardId::ChartSqlMix || *card == CardId::ChartTopTables)
                .collect::<Vec<(CardId, Rect)>>()
        };
        let default_layout = CardLayout::default();
        assert_eq!(chart_rects(&layout, 60), chart_rects(&default_layout, 60));
        assert_eq!(chart_rects(&layout, 60).len(), 2);
        let wide = chart_rects(&layout, 130);
        assert_eq!(wide[0].1.width, 39);
        assert_ne!(wide, chart_rects(&default_layout, 130));
    }

    #[test]
    fn set_grid_resets_ratios_by_row_count_and_row_membership() {
        let mut layout = CardLayout::default();
        assert!(layout.restore_grid_ratios("overview_rows_5", &[1_000, 2_000, 4_000, 6_000]));
        assert!(layout.restore_grid_ratios("overview_cols_0", &[2_500, 5_000, 7_500]));
        assert!(layout.restore_grid_ratios("overview_cols_1", &[5_000]));
        let keys = |layout: &CardLayout| {
            let mut keys: Vec<String> = layout
                .grid_ratio_entries()
                .into_iter()
                .map(|(key, _)| key)
                .collect();
            keys.sort_unstable();
            keys
        };
        assert_eq!(
            keys(&layout),
            ["overview_cols_0", "overview_cols_1", "overview_rows_5"]
        );

        // 줄 안 교환: 구성이 같아 비율 유지
        let grid = layout.grid_of(ActiveTab::Overview);
        let swapped = shift_card(
            &grid,
            CardId::Connections,
            KeyCode::Right,
            &[CardId::Traffic],
        )
        .unwrap();
        layout.set_grid(ActiveTab::Overview, swapped);
        assert_eq!(keys(&layout).len(), 3);

        // 줄 수 그대로 합류: 구성이 바뀐 줄의 폭 비율만 초기화
        let grid = layout.grid_of(ActiveTab::Overview);
        let joined = apply_drop(
            &grid,
            CardId::Sockets,
            DropTarget::After(CardId::ChartTopTables),
        )
        .unwrap();
        layout.set_grid(ActiveTab::Overview, joined);
        assert_eq!(keys(&layout), ["overview_cols_0", "overview_rows_5"]);

        // 줄 수가 바뀜: 탭 격자 비율 전체 초기화
        let grid = layout.grid_of(ActiveTab::Overview);
        let fewer = apply_drop(
            &grid,
            CardId::Processes,
            DropTarget::After(CardId::ChartTopTables),
        )
        .unwrap();
        layout.set_grid(ActiveTab::Overview, fewer);
        assert!(layout.grid_ratio_entries().is_empty());

        // 기본 격자로 되돌리면 보관하지 않음
        layout.set_grid(ActiveTab::Overview, default_rows(ActiveTab::Overview));
        assert_eq!(layout, CardLayout::default());
    }

    #[test]
    fn grid_ratio_entries_use_tab_kind_number_keys_and_restore_validates_them() {
        // 줄 경계 드래그는 보이는 줄 수 키에 저장
        let mut layout = CardLayout::default();
        let mut hit_areas = UiHitAreas::default();
        let area = Rect::new(0, 0, 130, 36);
        layout.arrange_tab(&mut hit_areas, ActiveTab::Overview, area, &grid_slots());
        let hit = hit_areas
            .split_hits
            .iter()
            .find(|hit| hit.group == SplitGroup::GridRows(ActiveTab::Overview, 5))
            .copied()
            .unwrap();
        assert!(layout.drag_cut(&hit, 2).is_some());
        let entries = layout.grid_ratio_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "overview_rows_5");
        assert_eq!(entries[0].1.len(), 4);

        // 폭 비율 키는 격자 줄 번호, 길이 = 줄 카드 수 - 1
        assert!(layout.restore_grid_ratios("overview_cols_1", &[5_000]));
        let entries = layout.grid_ratio_entries();
        assert_eq!(entries[1], ("overview_cols_1".to_string(), vec![5_000]));
        let mut restored = CardLayout::default();
        for (key, values) in &entries {
            let values: Vec<i64> = values.iter().map(|value| i64::from(*value)).collect();
            assert!(restored.restore_grid_ratios(key, &values), "{key}");
        }
        assert_eq!(restored, layout);

        // 길이·범위·키가 맞지 않으면 거부
        let mut layout = CardLayout::default();
        for (key, values) in [
            ("overview_cols_0", &[1_000, 2_000][..]),
            ("overview_cols_0", &[1_000, 2_000, 10_001][..]),
            ("overview_cols_1", &[-1][..]),
            ("overview_cols_4", &[][..]),
            ("overview_cols_6", &[5_000][..]),
            ("overview_rows_1", &[][..]),
            ("overview_rows_7", &[1, 2, 3, 4, 5, 6][..]),
            ("overview_wide_2", &[5_000][..]),
            ("nowhere_rows_2", &[5_000][..]),
            ("alerts_cols_1", &[5_000][..]),
            ("overview_rows_x", &[5_000][..]),
            ("rows", &[5_000][..]),
        ] {
            assert!(!layout.restore_grid_ratios(key, values), "{key} {values:?}");
        }
        assert_eq!(layout, CardLayout::default());
    }

    // 이동 취소는 격자와 비율을 되돌리고, 확정은 새 격자 유지, reset_tab 은 기본 배치
    #[test]
    fn grid_move_cancel_restores_baseline_and_reset_tab_clears_the_tab() {
        let mut layout = CardLayout::default();
        assert!(layout.restore_grid_ratios("overview_cols_0", &[2_500, 5_000, 7_500]));
        let before = layout;
        let baseline = layout.begin_grid_move(ActiveTab::Overview);
        assert_eq!(baseline, default_rows(ActiveTab::Overview));
        let moved = apply_drop(
            &baseline,
            CardId::Connections,
            DropTarget::RowAbove(CardId::Risk),
        )
        .unwrap();
        layout.set_grid(ActiveTab::Overview, moved);
        assert!(layout.grid_ratio_entries().is_empty());
        layout.end_grid_move(ActiveTab::Overview, Some(baseline));
        assert_eq!(layout, before);

        layout.begin_grid_move(ActiveTab::Overview);
        layout.set_grid(ActiveTab::Overview, moved);
        layout.end_grid_move(ActiveTab::Overview, None);
        assert_eq!(layout.grid_of(ActiveTab::Overview), moved);
        assert_eq!(
            layout.grid_of(ActiveTab::Alerts),
            default_rows(ActiveTab::Alerts)
        );

        layout.reset_tab(ActiveTab::Overview);
        assert_eq!(layout, CardLayout::default());
    }
}
