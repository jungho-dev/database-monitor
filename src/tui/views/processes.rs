//! views/processes.rs
//! 에이전트·프로세스 탭: 프로세스 표와 선택 프로세스 인스펙터 (위아래, 순서 변경 가능).

use std::sync::Arc;
use ratatui::Frame;
use ratatui::layout::Rect;
use crate::app::model::MonitorSnapshot;
use crate::tui::layout::{SplitAxis, SplitGroup};
use crate::tui::state::AppState;
use crate::tui::views::overview::{render_inspector, render_process_table};

// 1. 에이전트·프로세스 탭 -----------------------------------------------------
pub fn render_processes_tab(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    snap: &Arc<MonitorSnapshot>,
) {
    let inspector_h = (area.height / 2)
        .clamp(6, 12)
        .min(area.height.saturating_sub(5));
    let cards = state.card_layout.split_cards(
        &mut state.hit_areas,
        SplitGroup::ProcessesRows,
        SplitAxis::Rows,
        area,
        &[area.height.saturating_sub(inspector_h)],
        &[5, 3],
    );
    render_process_table(frame, cards[0], state, snap);
    render_inspector(frame, cards[1], state);
}
