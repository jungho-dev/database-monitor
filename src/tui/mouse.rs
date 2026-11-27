//! mouse.rs
//! 포인터 제스처 상태 전이(시각 주입): 호버, 클릭 포커스·선택·정렬·칩, 경계·열 폭·썸 드래그, 카드 제목 길게 누름 이동, 휠.

use std::time::{Duration, Instant};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;
use crate::tui::cards::{
    CardPress, ColumnDrag, MAX_TABLE_COLUMNS, ScrollDrag, UiAction, scroll_for_thumb, scroll_thumb,
};
use crate::tui::i18n::{self, Msg};
use crate::tui::layout::{SplitAxis, SplitDrag, SplitGroup};
use crate::tui::sort::SortChange;
use crate::tui::state::{AppState, ToastLevel, UiCommand};
use crate::tui::tab_move::TabDrag;

// 휠 한 칸에 움직일 줄 수, 열 경계 더블클릭 인정 간격
const WHEEL_STEP: isize = 3;
const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(500);

impl AppState {
    // 1. 마우스 이벤트 분배 ---------------------------------------------------
    // 직전 그리기에서 등록한 영역으로 판정, 반환 = 실행 요청
    pub fn handle_mouse_event(&mut self, mouse: MouseEvent) -> UiCommand {
        self.handle_mouse_event_at(mouse, Instant::now())
    }

    // 시각을 주입하는 입구 (카드 제목 길게 누름·더블클릭 판정이 now 를 기준으로 함)
    pub fn handle_mouse_event_at(&mut self, mouse: MouseEvent, now: Instant) -> UiCommand {
        self.now = now;
        // 움직임 없이 오래 누른 뒤 처음 오는 이벤트도 이동 모드 진입 뒤에 처리
        self.tick(now);

        // 도움말이 열려 있으면 클릭(닫기)과 버튼 뗌만 받음
        if self.show_help && !matches!(mouse.kind, MouseEventKind::Down(_) | MouseEventKind::Up(_))
        {
            return UiCommand::None;
        }
        // 종료 확인이 열려 있으면 확인·취소 버튼 클릭만 받음
        if self.kill_confirm.is_some() {
            return self.press_confirm_button(mouse);
        }
        // 카드 이동 중: 버튼 없는 움직임·새 누름은 놓은 것으로 보고 미리보기 순서를 확정, 휠은 무시
        if self.card_move.is_some() {
            match mouse.kind {
                MouseEventKind::Moved | MouseEventKind::Down(MouseButton::Left) => {
                    return self.commit_card_move();
                }
                MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => return UiCommand::None,
                _ => {}
            }
        }
        let position = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Moved => {
                // 버튼 없는 움직임 = 이미 뗌, 대기 중 누름은 일반 클릭으로 끝 (포커스는 누를 때 반영)
                // 드래그 중이던 경계·열 폭·스크롤바도 뗀 것으로 보고 Up 과 같이 끝냄 (마우스를 창 밖에서 놓은 경우)
                self.card_press = None;
                let drag_moved = self.end_drags(Some(now));
                self.track_hover(position);
                if drag_moved {
                    UiCommand::SaveState
                } else {
                    UiCommand::None
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.tab_drag.is_some() {
                    self.preview_tab_move(position);
                } else if self.grid_move.is_some() {
                    self.preview_grid_move(position);
                } else if let Some(press) = self.card_press {
                    // 누른 칸을 벗어나 시간이 차기 전에 끌면 취소 (같은 칸의 흔들림은 유지)
                    if position != press.cell {
                        self.card_press = None;
                    }
                } else if self.scroll_drag.is_some() {
                    self.drag_scrollbar(position);
                } else if self.column_drag.is_some() {
                    self.drag_column(position);
                } else {
                    self.drag_split(position);
                }
                UiCommand::None
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // 시간이 차기 전에 놓으면 일반 클릭 (포커스는 누를 때 이미 반영)
                self.card_press = None;
                if self.card_move.is_some() {
                    return self.commit_card_move();
                }
                if self.end_drags(Some(now)) {
                    UiCommand::SaveState
                } else {
                    UiCommand::None
                }
            }
            MouseEventKind::ScrollUp => {
                self.wheel_card(position, -WHEEL_STEP);
                UiCommand::None
            }
            MouseEventKind::ScrollDown => {
                self.wheel_card(position, WHEEL_STEP);
                UiCommand::None
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // 놓기를 놓친 이전 드래그는 새 누름 전에 모두 끝냄, 이미 움직인 것이 있었으면 저장하고 이 누름은 클릭으로 실행하지 않음
                if self.end_drags(None) {
                    return UiCommand::SaveState;
                }
                self.press_left(position, now)
            }
            _ => UiCommand::None,
        }
    }

    // 1-1. 경계·열 폭·스크롤바 드래그 끝내기 ----------------------------------
    // Up·버튼 없는 Moved·새 누름·탭 전환·크기 변경이 공유, 반환 = 실제로 움직인 드래그가 있었는지 (저장 필요)
    // click_at 이 있으면 끌지 않고 뗀 클릭을 처리: 경계 옆 머리글 칸은 그 열 정렬, 경계 칸은 더블클릭 후보로 기록
    pub fn end_drags(&mut self, click_at: Option<Instant>) -> bool {
        let split_moved = self.split_drag.take().is_some_and(|drag| drag.moved);
        self.scroll_drag = None;
        let column_drag = self.column_drag.take();
        let column_moved = column_drag.is_some_and(|drag| drag.moved);
        if column_moved {
            self.last_column_click = None;
        }
        let still_click = column_drag.filter(|drag| !drag.moved).zip(click_at);
        if let Some((drag, at)) = still_click {
            match drag.click_sort {
                Some(column) => {
                    self.last_column_click = None;
                    self.change_card_sort(drag.card, SortChange::SelectColumn(column));
                }
                None => self.last_column_click = Some((drag.card, drag.column, at)),
            }
        }
        let tab_moved = self.finish_tab_drag();
        split_moved || column_moved || tab_moved
    }

    // 2. 종료 확인 버튼 -------------------------------------------------------
    fn press_confirm_button(&mut self, mouse: MouseEvent) -> UiCommand {
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return UiCommand::None;
        }
        let position = Position::new(mouse.column, mouse.row);
        let action = self
            .hit_areas
            .button_rects
            .iter()
            .find(|(action, rect)| {
                matches!(action, UiAction::ConfirmKill | UiAction::CancelKill)
                    && rect.contains(position)
            })
            .map(|(action, _)| *action);
        match action {
            Some(action) => self.perform_action(action),
            None => UiCommand::None,
        }
    }

    // 3. 호버 강조 ------------------------------------------------------------
    // 스크롤바 트랙 위면 썸만, 카드 경계·열 경계 위면 그 경계만, 아니면 마우스 아래 카드
    fn track_hover(&mut self, position: Position) {
        if self.show_help {
            return;
        }
        let track_card = self
            .hit_areas
            .scroll_hits
            .iter()
            .find(|hit| hit.track.contains(position))
            .map(|hit| hit.card);
        if track_card.is_some() {
            self.scroll_hover = track_card;
            self.hover_card = track_card;
            self.split_hover = None;
            self.column_hover = None;
            return;
        }
        self.scroll_hover = None;
        self.column_hover = self
            .hit_areas
            .column_hits
            .iter()
            .find(|hit| hit.grab.contains(position))
            .map(|hit| (hit.card, hit.column));
        let split = self
            .hit_areas
            .split_hits
            .iter()
            .find(|hit| hit.grab.contains(position))
            .map(|hit| (hit.group, hit.index));
        let card = if split.is_some() {
            None
        } else {
            self.hit_areas
                .card_rects
                .iter()
                .find(|(_, rect)| rect.contains(position))
                .map(|(card, _)| *card)
        };
        self.split_hover = split;
        self.hover_card = card;
    }

    // 4. 경계 드래그 ----------------------------------------------------------
    // 잡은 경계를 포인터 위치로 이동, 앞뒤 카드 크기를 알림으로 표시
    fn drag_split(&mut self, position: Position) {
        let Some(drag) = self.split_drag else {
            return;
        };
        let grabbed = self
            .hit_areas
            .split_hits
            .iter()
            .find(|hit| hit.group == drag.group && hit.index == drag.index)
            .copied();
        let Some(hit) = grabbed else {
            self.split_drag = None;
            return;
        };
        let coord = match hit.axis {
            SplitAxis::Columns => i32::from(position.x),
            SplitAxis::Rows => i32::from(position.y),
        };
        if let Some((before, after)) = self.card_layout.drag_cut(&hit, coord - drag.grip) {
            self.split_drag = Some(SplitDrag { moved: true, ..drag });
            self.show_toast(
                &i18n::card_resized(self.lang, hit.axis, before, after),
                ToastLevel::Info,
            );
        }
    }

    // 5. 스크롤바 드래그 ------------------------------------------------------
    // 썸을 포인터 위치로 이동, 트랙 밖은 끝으로 제한 (선택 행은 유지)
    fn drag_scrollbar(&mut self, position: Position) {
        let Some(drag) = self.scroll_drag else {
            return;
        };
        let grabbed = self
            .hit_areas
            .scroll_hits
            .iter()
            .find(|hit| hit.card == drag.card)
            .copied();
        let Some(hit) = grabbed else {
            self.scroll_drag = None;
            return;
        };
        let thumb_top = i32::from(position.y) - i32::from(hit.track.y) - i32::from(drag.grip);
        let thumb_start = u16::try_from(thumb_top.max(0)).unwrap_or(u16::MAX);
        let scroll = scroll_for_thumb(hit.total, hit.visible, hit.track.height, thumb_start);
        self.set_card_scroll(drag.card, scroll);
    }

    // 6. 휠 스크롤 ------------------------------------------------------------
    // 마우스 아래 카드의 화면 위치만 이동 (선택 행은 유지)
    fn wheel_card(&mut self, position: Position, delta: isize) {
        let card = self
            .hit_areas
            .card_rects
            .iter()
            .find(|(_, rect)| rect.contains(position))
            .map(|(card, _)| *card);
        if let Some(card) = card {
            self.scroll_card_view(card, delta);
        }
    }

    // 7. 왼쪽 버튼 누름 -------------------------------------------------------
    // 우선순위: 도움말 닫기, 스크롤바, 카드 제목, 카드 경계 잡기, 열 경계 잡기, 카드 포커스, 버튼, 탭, 머리글 정렬, 행 선택
    fn press_left(&mut self, position: Position, now: Instant) -> UiCommand {
        // 1. 도움말은 클릭 한 번에 닫기
        if self.show_help {
            self.show_help = false;
            return UiCommand::None;
        }

        // 놓기를 놓친 이전 누름이 남았으면 버림 (이동 중이던 순서는 입구에서 이미 확정)
        self.cancel_card_move();

        // 2. 스크롤바: 썸 잡기, 트랙 클릭은 한 화면 이동 (경계 잡기보다 우선)
        self.scroll_drag = None;
        let track_hit = self
            .hit_areas
            .scroll_hits
            .iter()
            .find(|hit| hit.track.contains(position))
            .copied();
        if let Some(hit) = track_hit {
            self.focused_card = Some((self.tab, hit.card));
            let offset = position.y - hit.track.y;
            let page = hit.visible as isize;
            if let Some(thumb) = scroll_thumb(hit.total, hit.visible, hit.scroll, hit.track.height) {
                if offset < thumb.start {
                    self.scroll_card_view(hit.card, -page);
                } else if offset >= thumb.start + thumb.len {
                    self.scroll_card_view(hit.card, page);
                } else {
                    self.scroll_drag = Some(ScrollDrag {
                        card: hit.card,
                        grip: offset - thumb.start,
                    });
                }
            }
            return UiCommand::None;
        }

        // 3. 카드 제목 글자 누름: 카드 포커스 후 길게 누름 대기 (카드 경계 잡기보다 우선, 이동 모드는 tick 이 진입)
        // 이번 프레임 격자 줄에 그려진 카드만, 나머지 테두리는 계속 크기 조절
        let title_card = self
            .hit_areas
            .title_hits
            .iter()
            .find(|hit| hit.rect.contains(position))
            .map(|hit| hit.card);
        let grid_title = title_card.and_then(|card| {
            self.hit_areas
                .grid_lines
                .iter()
                .filter(|line| line.tab == self.tab)
                .find_map(|line| {
                    line.cards
                        .iter()
                        .position(|(placed, _)| *placed == card)
                        .map(|column| (card, line.row, column))
                })
        });
        if let Some((card, row, column)) = grid_title {
            self.focused_card = Some((self.tab, card));
            self.card_press = Some(CardPress {
                card,
                group: SplitGroup::GridCols(self.tab, row as u8),
                index: column,
                started: now,
                cell: position,
            });
            return UiCommand::None;
        }

        // 4. 카드 경계 잡기 (행·머리글 클릭보다 우선)
        self.split_drag = None;
        let grabbed = self
            .hit_areas
            .split_hits
            .iter()
            .find(|hit| hit.grab.contains(position))
            .copied();
        if let Some(hit) = grabbed {
            let coord = match hit.axis {
                SplitAxis::Columns => position.x,
                SplitAxis::Rows => position.y,
            };
            let cut_at = hit.origin + hit.cuts[hit.index];
            self.split_drag = Some(SplitDrag {
                group: hit.group,
                index: hit.index,
                grip: i32::from(coord) - i32::from(cut_at),
                moved: false,
            });
            self.split_hover = Some((hit.group, hit.index));
            self.show_toast(i18n::text(self.lang, Msg::ToastSplitDrag), ToastLevel::Info);
            return UiCommand::None;
        }

        // 5. 열 경계 잡기: 머리글 정렬보다 우선, 경계 칸을 끌지 않고 뗀 직후 다시 누르면 자동 폭 복원
        // 경계 옆 머리글 칸은 끌면 열 폭, 끌지 않고 떼면 그 열 정렬 (떼는 순간 end_drags 가 처리)
        self.column_drag = None;
        let column_hit = self
            .hit_areas
            .column_hits
            .iter()
            .find(|hit| hit.grab.contains(position))
            .copied();
        if let Some(hit) = column_hit {
            self.focused_card = Some((self.tab, hit.card));
            let click_sort = self
                .hit_areas
                .header_cells
                .iter()
                .find(|header| header.rect.contains(position))
                .map(|header| header.column);
            let double_click = click_sort.is_none()
                && self.last_column_click.is_some_and(|(card, column, at)| {
                    card == hit.card
                        && column == hit.column
                        && now.saturating_duration_since(at) < DOUBLE_CLICK_WINDOW
                });
            self.last_column_click = None;
            if double_click {
                self.column_widths[hit.card as usize] = [None; MAX_TABLE_COLUMNS];
                self.show_toast(i18n::text(self.lang, Msg::ToastColumnsAuto), ToastLevel::Info);
                return UiCommand::SaveState;
            }
            self.column_drag = Some(ColumnDrag {
                card: hit.card,
                column: hit.column,
                start_x: position.x,
                start_width: hit.width,
                moved: false,
                click_sort,
            });
            self.column_hover = Some((hit.card, hit.column));
            if click_sort.is_none() {
                self.show_toast(i18n::text(self.lang, Msg::ToastColumnDrag), ToastLevel::Info);
            }
            return UiCommand::None;
        }

        // 6. 카드 클릭: 테두리 강조 대상 변경, 카드 안 행·머리글 처리는 아래에서 계속
        let clicked_card = self
            .hit_areas
            .card_rects
            .iter()
            .find(|(_, rect)| rect.contains(position))
            .map(|(card, _)| *card);
        if let Some(card) = clicked_card {
            self.focused_card = Some((self.tab, card));
        }

        // 7. 하단 칩·경보 카드
        let button = self
            .hit_areas
            .button_rects
            .iter()
            .find(|(_, rect)| rect.contains(position))
            .map(|(action, _)| *action);
        if let Some(action) = button {
            return self.perform_action(action);
        }

        // 8. 탭 전환, 누른 채 끄면 탭 순서 이동 (놓을 때 end_drags 가 확정)
        let tab = self
            .hit_areas
            .tab_rects
            .iter()
            .find(|(_, rect)| rect.contains(position))
            .map(|(tab, _)| *tab);
        if let Some(tab) = tab {
            let command = self.switch_tab(tab);
            self.tab_drag = Some(TabDrag {
                tab,
                baseline: self.tab_order,
            });
            return command;
        }

        // 9. 표 머리글 클릭 정렬 (숫자·시간 내림 -> 오름, 문자 오름 -> 내림, 세 번째 기본, 선택 행은 같은 항목을 따라감)
        let header = self
            .hit_areas
            .header_cells
            .iter()
            .find(|hit| hit.rect.contains(position))
            .map(|hit| (hit.card, hit.column));
        if let Some((card, column)) = header {
            self.change_card_sort(card, SortChange::SelectColumn(column));
            return UiCommand::None;
        }

        // 10. 표 본문 행 클릭: 선택 행 변경, 하단 상세 카드가 따라감
        let row_hit = self
            .hit_areas
            .row_hits
            .iter()
            .find(|hit| hit.rect.contains(position))
            .copied();
        if let Some(hit) = row_hit {
            let index = hit.offset + usize::from(position.y - hit.rect.y);
            if index < hit.len {
                self.select_card_row(hit.card, index);
            }
        }
        UiCommand::None
    }

    // 8. 열 경계 드래그 -------------------------------------------------------
    // 왼쪽 열 폭 = 누른 순간 폭 + 포인터 이동량 (열 최소·최대 폭으로 제한), 움직이지 않은 클릭은 폭을 고정하지 않음
    fn drag_column(&mut self, position: Position) {
        let Some(drag) = self.column_drag else {
            return;
        };
        let grabbed = self
            .hit_areas
            .column_hits
            .iter()
            .find(|hit| hit.card == drag.card && hit.column == drag.column)
            .copied();
        let Some(hit) = grabbed else {
            self.column_drag = None;
            return;
        };
        if !drag.moved && position.x == drag.start_x {
            return;
        }
        let wanted = i32::from(drag.start_width) + i32::from(position.x) - i32::from(drag.start_x);
        let width = wanted.clamp(i32::from(hit.min), i32::from(hit.max.max(hit.min))) as u16;
        self.column_widths[drag.card as usize][drag.column] = Some(width);
        self.column_drag = Some(ColumnDrag { moved: true, ..drag });
        self.show_toast(&i18n::column_width(self.lang, width), ToastLevel::Info);
    }

    // 9. 크기 변경 ------------------------------------------------------------
    // 맞추어 그린 영역이 모두 바뀌므로 카드 이동 취소와 강조·끌기 해제, 끌어 움직인 경계·열 폭이 있었으면 저장 요청
    // 새 크기가 최소 크기 미달이면 열려 있던 종료 확인도 취소 (파괴적 키가 동작하지 않는 화면)
    pub fn handle_resize(&mut self, width: u16, height: u16) -> UiCommand {
        self.apply_screen_size(width, height);
        self.cancel_card_move();
        let drag_moved = self.end_drags(None);
        self.last_column_click = None;
        self.split_hover = None;
        self.column_hover = None;
        self.hover_card = None;
        self.scroll_hover = None;
        if drag_moved {
            UiCommand::SaveState
        } else {
            UiCommand::None
        }
    }
}
