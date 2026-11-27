//! views/processes.rs
//! 에이전트·프로세스 탭: 프로세스 표와 선택 프로세스 인스펙터 (위아래, 순서 변경 가능).

use std::sync::Arc;
use ratatui::Frame;
use ratatui::layout::Rect;
use crate::app::model::MonitorSnapshot;
use crate::tui::cards::CardId;
use crate::tui::layout::grid::{CardSlot, SlotHeight};
use crate::tui::state::{ActiveTab, AppState};
use crate::tui::views::overview::{
    metric_slots, render_inspector, render_metric_card, render_process_table,
};

// 1. 에이전트·프로세스 탭 -----------------------------------------------------
// 탭 격자: 0행 지표, 프로세스 표, 인스펙터
pub fn render_processes_tab(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    snap: &Arc<MonitorSnapshot>,
) {
    // 1. 카드 조건 (지표 줄을 뺼 높이 기준 인스펙터 높이)
    let rest_h = area.height.saturating_sub(4);
    let inspector_h = (rest_h / 2).clamp(6, 12).min(rest_h.saturating_sub(5));
    let mut slots = metric_slots().to_vec();
    slots.extend([
        CardSlot {
            card: CardId::Processes,
            min_w: 24,
            min_h: 4,
            height: SlotHeight::Fill,
            weight: 1,
            rank: 0,
            reserve: 0,
        },
        CardSlot {
            card: CardId::Inspector,
            min_w: 20,
            min_h: 3,
            height: SlotHeight::Fixed(inspector_h),
            weight: 1,
            rank: 0,
            reserve: 0,
        },
    ]);

    // 2. 격자 배치와 카드 그리기
    let layout = state.card_layout;
    let placed = layout.arrange_tab(&mut state.hit_areas, ActiveTab::Processes, area, &slots);
    for (card, rect) in placed {
        match card {
            CardId::Risk | CardId::Connections | CardId::Traffic | CardId::QueryRate => {
                render_metric_card(frame, rect, state, snap, card)
            }
            CardId::Processes => render_process_table(frame, rect, state, snap),
            CardId::Inspector => render_inspector(frame, rect, state),
            CardId::Sockets
            | CardId::SqlStream
            | CardId::TopSqlRanking
            | CardId::SqlDetail
            | CardId::AlertLog
            | CardId::AlertDetail
            | CardId::Gate
            | CardId::ChartSqlMix
            | CardId::ChartTopTables => {}
        }
    }
}
