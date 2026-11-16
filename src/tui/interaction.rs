//! interaction.rs
//! 포인터 제스처 상태 전이(시각 주입): 호버, 클릭 포커스·선택·정렬·칩, 경계·열 폭·썸 드래그, 카드 제목 길게 누름 이동, 휠.

use std::time::{Duration, Instant};
use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;
use crate::tui::cards::{
    CARD_HOLD_TIME, CardId, CardMove, CardPress, ColumnDrag, MAX_TABLE_COLUMNS, ScrollDrag,
    UiAction, scroll_for_thumb, scroll_thumb,
};
use crate::tui::i18n::{self, Msg};
use crate::tui::layout::{SplitAxis, SplitDrag};
use crate::tui::sort::SortChange;
use crate::tui::state::{AppState, ToastLevel, UiCommand};

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
                let drag_moved = self.end_pointer_drags(now);
                self.track_hover(position);
                if drag_moved {
                    UiCommand::SaveState
                } else {
                    UiCommand::None
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.card_move.is_some() {
                    self.preview_card_move(position);
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
                if self.end_pointer_drags(now) {
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
                if self.end_pointer_drags(now) {
                    return UiCommand::SaveState;
                }
                self.press_left(position, now)
            }
            _ => UiCommand::None,
        }
    }

    // 1-1. 경계·열 폭·스크롤바 드래그 끝내기 ----------------------------------
    // Up·버튼 없는 Moved·새 누름·탭 전환이 공유, 반환 = 실제로 움직인 드래그가 있었는지 (저장 필요)
    // 끌지 않고 뗀 열 경계 클릭만 더블클릭 후보로 기록
    pub fn end_pointer_drags(&mut self, now: Instant) -> bool {
        let split_moved = self.split_drag.take().is_some_and(|drag| drag.moved);
        self.scroll_drag = None;
        let mut column_moved = false;
        if let Some(drag) = self.column_drag.take() {
            column_moved = drag.moved;
            self.last_column_click = if drag.moved {
                None
            } else {
                Some((drag.card, drag.column, now))
            };
        }
        split_moved || column_moved
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
        // 순서를 바꿀 수 있는 그룹이 이번 프레임에 그려진 카드만, 나머지 테두리는 계속 크기 조절
        let title_card = self
            .hit_areas
            .title_hits
            .iter()
            .find(|hit| hit.rect.contains(position))
            .map(|hit| hit.card);
        let title_slot = title_card
            .and_then(|card| card.reorder_slot(self.tab).map(|slot| (card, slot)))
            .filter(|(_, (group, _))| {
                self.hit_areas
                    .slot_groups
                    .iter()
                    .any(|slots| slots.group == *group)
            });
        if let Some((card, (group, index))) = title_slot {
            self.focused_card = Some((self.tab, card));
            self.card_press = Some(CardPress {
                card,
                group,
                index,
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

        // 5. 열 경계 잡기: 머리글 정렬보다 우선, 끌지 않고 뗀 직후 다시 누르면 자동 폭 복원
        self.column_drag = None;
        let column_hit = self
            .hit_areas
            .column_hits
            .iter()
            .find(|hit| hit.grab.contains(position))
            .copied();
        if let Some(hit) = column_hit {
            self.focused_card = Some((self.tab, hit.card));
            let double_click = self.last_column_click.is_some_and(|(card, column, at)| {
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
            });
            self.column_hover = Some((hit.card, hit.column));
            self.show_toast(i18n::text(self.lang, Msg::ToastColumnDrag), ToastLevel::Info);
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

        // 8. 탭 전환
        let tab = self
            .hit_areas
            .tab_rects
            .iter()
            .find(|(_, rect)| rect.contains(position))
            .map(|(tab, _)| *tab);
        if let Some(tab) = tab {
            return self.switch_tab(tab);
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
        let drag_moved = self.end_pointer_drags(self.now);
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

    // 10. 길게 누름 시계 ------------------------------------------------------
    // 제목을 누른 지 CARD_HOLD_TIME 이 지나면 이동 모드 진입 (마우스 움직임과 무관), 반환 = 상태가 바뀌었는지
    pub fn tick(&mut self, now: Instant) -> bool {
        let Some(press) = self.card_press else {
            return false;
        };
        if now.saturating_duration_since(press.started) < CARD_HOLD_TIME {
            return false;
        }
        self.card_press = None;
        let Some(slot) = self.card_layout.slot_of(press.group, press.index) else {
            return false;
        };
        self.card_move = Some(CardMove {
            card: press.card,
            group: press.group,
            index: press.index,
            baseline: self.card_layout.order_of(press.group),
            target: slot,
        });
        true
    }

    // 이동 모드 진입까지 남은 시간 (이벤트 대기 시간 상한 계산용), 대기 중이 아니면 None
    pub fn press_remaining(&self, now: Instant) -> Option<Duration> {
        self.card_press.map(|press| {
            CARD_HOLD_TIME.saturating_sub(now.saturating_duration_since(press.started))
        })
    }

    // 11. 카드 이동 취소·미리보기·확정 ----------------------------------------
    // 대기 중 누름은 버리고, 이동 중이면 누르기 전 순서로 복원, 반환 = 상태가 바뀌었는지
    pub fn cancel_card_move(&mut self) -> bool {
        let pressed = self.card_press.take().is_some();
        match self.card_move.take() {
            Some(moving) => {
                self.card_layout.set_order(moving.group, moving.baseline);
                true
            }
            None => pressed,
        }
    }

    // 포인터가 놓인 칸을 목표로 삼아 누르기 전 순서에서 삽입 이동을 다시 계산, 칸 밖이면 마지막 목표 유지
    pub fn preview_card_move(&mut self, position: Position) {
        let Some(moving) = self.card_move else {
            return;
        };
        let target = self
            .hit_areas
            .slot_groups
            .iter()
            .find(|slots| slots.group == moving.group)
            .and_then(|slots| slots.slots.iter().position(|slot| slot.contains(position)));
        let Some(target) = target else {
            return;
        };
        if target == moving.target {
            return;
        }
        self.card_layout
            .move_card_to_slot(moving.group, &moving.baseline, moving.index, target);
        self.card_move = Some(CardMove { target, ..moving });
    }

    // 순서가 바뀌었을 때만 알림과 저장 요청 (제자리에 놓으면 아무 일 없음)
    pub fn commit_card_move(&mut self) -> UiCommand {
        let Some(moving) = self.card_move.take() else {
            return UiCommand::None;
        };
        if self.card_layout.order_of(moving.group) == moving.baseline {
            return UiCommand::None;
        }
        self.show_toast(i18n::text(self.lang, Msg::ToastCardMoved), ToastLevel::Info);
        UiCommand::SaveState
    }

    // 12. Shift+방향키 한 칸 이동 ---------------------------------------------
    // 세로 나열 그룹은 ↑↓, 가로 나열 그룹은 ←→ 만 받고 나머지는 무시, 바로 확정·저장
    // 이번 프레임에 그려진 그룹만 대상 (안내 카드만 보이는 탭은 순서를 바꾸지 않음)
    pub fn shift_focused_card(&mut self, card: CardId, code: KeyCode) -> UiCommand {
        let Some((group, index)) = card.reorder_slot(self.tab) else {
            return UiCommand::None;
        };
        if !self
            .hit_areas
            .slot_groups
            .iter()
            .any(|slots| slots.group == group)
        {
            return UiCommand::None;
        }
        let delta: isize = match (group.axis(), code) {
            (SplitAxis::Rows, KeyCode::Up) | (SplitAxis::Columns, KeyCode::Left) => -1,
            (SplitAxis::Rows, KeyCode::Down) | (SplitAxis::Columns, KeyCode::Right) => 1,
            _ => return UiCommand::None,
        };
        let Some(slot) = self.card_layout.slot_of(group, index) else {
            return UiCommand::None;
        };
        let Some(target) = slot
            .checked_add_signed(delta)
            .filter(|target| *target < group.card_count())
        else {
            return UiCommand::None;
        };
        let baseline = self.card_layout.order_of(group);
        self.card_layout
            .move_card_to_slot(group, &baseline, index, target);
        self.show_toast(i18n::text(self.lang, Msg::ToastCardMoved), ToastLevel::Info);
        UiCommand::SaveState
    }
}
