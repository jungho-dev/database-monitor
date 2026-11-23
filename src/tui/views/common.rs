//! views/common.rs
//! 탭 공용 그리기: 클릭·마우스 강조 카드, 안내 카드, 표·텍스트 카드, 열 폭 결정, 정렬 머리글, 스크롤바, 경계 손잡이.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Padding, Paragraph, Row, Table};
use crate::tui::cards::{
    CardId, CardKind, ColumnHit, HeaderHit, MAX_COLUMN_WIDTH, MAX_TABLE_COLUMNS, RowHit,
    ScrollHit, TitleHit, scroll_thumb,
};
use crate::tui::format::{char_width, fit_display, text_width};
use crate::tui::i18n::{self, Msg};
use crate::tui::layout::SplitAxis;
use crate::tui::state::AppState;
use crate::tui::theme;

// 글자 열이 줄어드는 최소 폭 (드래그 하한 포함), 열 사이 간격, 열 경계 잡는 칸이 양옆 열로 넓어지는 칸 수
const MIN_COLUMN_WIDTH: u16 = 3;
const TABLE_COLUMN_GAP: u16 = 1;
const COLUMN_GRAB_REACH: u16 = 1;

// 표 열 맞춤: 숫자·짧은 열은 고정, 글자 열은 좁아지면 먼저 줄고(Shrink), 남는 폭은 맨 오른쪽 Grow 열이 가짐
// Pinned 는 위험 등급처럼 끝까지 남길 고정 폭 열 (다른 열이 모두 숨은 뒤에야 오른쪽부터 숨김)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnFit {
    Fixed,
    Pinned,
    Shrink,
    Grow,
}

// 열 명세 (머리글 문구 키, 고정 열 최소 폭, 맞춤 규칙, 정렬)
// 글자 열의 폭은 머리글·가장 넓은 칸의 표시 폭(자연 폭)이 정함
pub type ColumnSpec = (Msg, u16, ColumnFit, Alignment);

// 열 폭 계획 한 칸 (natural = 자연 폭, floor = 줄일 수 있는 최소 폭, user = 끌어 정한 폭)
#[derive(Debug, Clone, Copy)]
pub struct ColumnPlan {
    pub natural: u16,
    pub floor: u16,
    pub fit: ColumnFit,
    pub user: Option<u16>,
}

// 한 칸의 글자 조각들
pub type TableCell = Vec<Span<'static>>;

// 표 한 행 (style = 행 전체 바탕·글자색)
pub struct TableRow {
    pub cells: Vec<TableCell>,
    pub style: Style,
}

// 표 카드 그리기 입력 (머리글 1행 고정, rows 는 정렬을 마친 전체 행, empty = 빈 표 안내와 다음 행동)
pub struct CardTable {
    pub card: CardId,
    // 평소 외곽선 색 (위험 점멸 위상이면 위험색)
    pub tone: Color,
    pub title: String,
    pub hint: String,
    pub columns: Vec<ColumnSpec>,
    pub rows: Vec<TableRow>,
    pub empty: (Msg, Msg),
}

// 1. 클릭·마우스 강조 카드 ----------------------------------------------------
// 영역 등록, 선택 카드는 굵은 테두리·반전 제목, 마우스 아래는 밝은 외곽선
// tone = 평소 외곽선 색, 위험(COLOR_CRIT) 적색이면 선택·마우스 강조보다 우선
// hint = 우측 상단 보조 문구, 빈 문자열이거나 제목과 겹칠 폭이면 생략
pub fn interactive_card(
    state: &mut AppState,
    id: CardId,
    area: Rect,
    title: &str,
    hint: &str,
    tone: Color,
) -> Block<'static> {
    state.hit_areas.card_rects.push((id, area));

    // 1. 이동 중인 카드는 제목에 이동 표시, 제목 글자 영역은 길게 누르기 대상으로 등록 (좌우 테두리 안쪽까지만)
    let lifted = state.card_move.is_some_and(|moving| moving.card == id);
    let shown_title = if lifted {
        format!(" {} ", i18n::card_moving(state.lang, title))
    } else {
        format!(" {title} ")
    };
    let title_width = text_width(&shown_title) as u16;
    let title_rect = Rect::new(
        area.x.saturating_add(1),
        area.y,
        title_width.min(area.width.saturating_sub(2)),
        1.min(area.height),
    );
    if !title_rect.is_empty() {
        state.hit_areas.title_hits.push(TitleHit {
            card: id,
            rect: title_rect,
        });
    }

    // 2. 강조 단계: 클릭한 카드는 그 탭에서만 유효, 없으면 탭의 주 카드
    let focused = state.focused_card_id() == id;
    let hovered = state.hover_card == Some(id);
    let border = if lifted {
        theme::FG_ACCENT
    } else if tone == theme::COLOR_CRIT {
        tone
    } else if focused {
        theme::BORDER_FOCUS
    } else if hovered {
        theme::BORDER_HOVER
    } else {
        tone
    };

    // 3. 선택 카드는 색 외에도 선 굵기·제목 반전으로 구분, 이동 중 카드는 점선·강조색 반전 제목 (이중선은 겹침 창 전용)
    let (border_type, title_style) = if lifted {
        (
            BorderType::LightDoubleDashed,
            Style::default()
                .fg(theme::FG_ON_ACCENT)
                .bg(theme::FG_ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    } else if focused {
        (
            BorderType::Thick,
            Style::default()
                .fg(theme::FG_ON_ACCENT)
                .bg(border)
                .add_modifier(Modifier::BOLD),
        )
    } else if hovered {
        (
            BorderType::Rounded,
            Style::default()
                .fg(theme::TITLE_HOVER)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            BorderType::Rounded,
            Style::default()
                .fg(theme::TITLE_IDLE)
                .add_modifier(Modifier::BOLD),
        )
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(border))
        .title(Span::styled(shown_title, title_style))
        .style(Style::default().bg(theme::BG_PANEL));

    // 4. 보조 문구
    let needed = title_width as usize + text_width(&format!(" {hint} ")) + 4;
    if hint.is_empty() || needed > area.width as usize {
        return block;
    }
    block.title_top(Line::from(Span::styled(format!(" {hint} "), theme::STYLE_DIM)).right_aligned())
}

// 2. 안내 카드 ----------------------------------------------------------------
// 제목 한 줄과 다음 행동 한 줄을 가운데에 (headline_style = 상태 색)
pub fn render_notice_card(
    frame: &mut Frame,
    state: &mut AppState,
    id: CardId,
    area: Rect,
    title: &str,
    notice: (Msg, Msg),
) {
    let block = interactive_card(state, id, area, title, "", theme::BORDER_DIM);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let headline = Span::styled(i18n::text(state.lang, notice.0), theme::STYLE_DIM);
    render_empty_state(frame, inner, headline, i18n::text(state.lang, notice.1));
}

// 3. 표 카드 ------------------------------------------------------------------
// 자연 폭 기반 열 폭 결정, 정렬 머리글·열 경계·본문 행 클릭 영역 등록, 선택 행 띠, 위치 표시, 오른쪽 거터 스크롤바
pub fn render_card_table(frame: &mut Frame, state: &mut AppState, area: Rect, table: CardTable) {
    let CardTable {
        card,
        tone,
        title,
        hint,
        columns,
        rows,
        empty,
    } = table;
    let sort_table = card.sort_table();
    let sort = sort_table.and_then(|kind| state.sorts.get(kind));
    let lang = state.lang;
    let labels: Vec<&'static str> = columns.iter().map(|spec| i18n::text(lang, spec.0)).collect();

    // 1. 제목 (정렬 열과 방향 덧붙임)
    let title = match sort.and_then(|current| labels.get(current.column).map(|label| (*label, current))) {
        Some((label, current)) => i18n::sort_title(lang, &title, label, current.arrow()),
        None => title,
    };
    let block = interactive_card(state, card, area, &title, &hint, tone)
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    let total = rows.len();
    let visible = (inner.height as usize).saturating_sub(1);
    let selectable = card.kind() == CardKind::Selectable;

    // 2. 보기 상태 보정
    let view = &mut state.card_views[card as usize];
    view.total = total;
    view.visible = visible;
    view.scroll = view.scroll.min(total.saturating_sub(visible));
    view.selected = view.selected.min(total.saturating_sub(1));
    let (scroll, selected) = (view.scroll, view.selected);

    // 3. 열 폭 계획 (자연 폭 = 머리글·가장 넓은 칸, 숫자·고정 열은 선호 폭 이상으로 줄지 않음)
    let plans: Vec<ColumnPlan> = columns
        .iter()
        .enumerate()
        .map(|(index, spec)| {
            let label_width = text_width(labels[index]) + usize::from(sort_table.is_some());
            let content_width = rows
                .iter()
                .map(|row| {
                    row.cells
                        .get(index)
                        .map_or(0, |cell| cell.iter().map(Span::width).sum::<usize>())
                })
                .max()
                .unwrap_or(0);
            let widest = u16::try_from(label_width.max(content_width)).unwrap_or(u16::MAX);
            let fixed = matches!(spec.2, ColumnFit::Fixed | ColumnFit::Pinned);
            let natural = if fixed { spec.1.max(widest) } else { widest };
            ColumnPlan {
                natural,
                floor: if fixed {
                    natural
                } else {
                    MIN_COLUMN_WIDTH.min(natural)
                },
                fit: spec.2,
                user: state.column_widths[card as usize]
                    .get(index)
                    .copied()
                    .flatten(),
            }
        })
        .collect();
    let widths = resolve_column_widths(&plans, inner.width);
    let shown: Vec<usize> = (0..columns.len())
        .filter(|index| widths[*index] > 0)
        .collect();
    let shown_floors = shown
        .iter()
        .fold(0u16, |sum, index| sum.saturating_add(plans[*index].floor));
    let shown_gaps = (shown.len().saturating_sub(1)) as u16 * TABLE_COLUMN_GAP;

    // 4. 머리글 (정렬 열 화살표 유지, 정렬·열 경계 클릭 영역은 열 좌표와 동일)
    let mut header_cells: Vec<Cell<'static>> = Vec::new();
    let mut cursor = inner.x;
    for (position, index) in shown.iter().enumerate() {
        let spec = columns[*index];
        let width = widths[*index];
        let active = sort.is_some_and(|current| current.column == *index);
        let name = labels[*index];
        let label = match sort {
            Some(current) if active && text_width(name) + 1 > usize::from(width) => {
                let head = fit_display(name, usize::from(width).saturating_sub(1));
                format!("{}{}", head.trim_end(), current.arrow())
            }
            Some(current) if active => format!("{name}{}", current.arrow()),
            _ if text_width(name) > usize::from(width) => {
                fit_display(name, usize::from(width)).trim_end().to_string()
            }
            _ => name.to_string(),
        };
        let cell = Cell::from(Line::from(label).alignment(spec.3));
        header_cells.push(if active {
            cell.style(theme::STYLE_ACCENT)
        } else {
            cell
        });
        if sort_table.is_some() && inner.height > 0 {
            state.hit_areas.header_cells.push(HeaderHit {
                card,
                column: *index,
                rect: Rect::new(cursor, inner.y, width, 1),
            });
        }

        // 열 경계 (마지막 보이는 열 오른쪽은 경계 없음, 폭 범위 = 다른 열 최소 폭을 남긴 만큼)
        // 잡는 칸은 경계 칸 양옆으로 한 칸씩 넓힘 (옆 열이 최소 폭보다 넓을 때만, 이웃 경계 잡는 칸과 겹치지 않음)
        // 세로선은 머리글과 그려진 본문 행 높이
        if position + 1 < shown.len() && *index < MAX_TABLE_COLUMNS && inner.height > 0 {
            let others_floor = (shown_floors - plans[*index].floor).saturating_add(shown_gaps);
            let edge_x = cursor + width;
            let next_width = widths[shown[position + 1]];
            let reach_left = if width > MIN_COLUMN_WIDTH {
                COLUMN_GRAB_REACH
            } else {
                0
            };
            let reach_right = if next_width > MIN_COLUMN_WIDTH {
                COLUMN_GRAB_REACH
            } else {
                0
            };
            let drawn_rows = total.saturating_sub(scroll).min(visible) as u16;
            state.hit_areas.column_hits.push(ColumnHit {
                card,
                column: *index,
                grab: Rect::new(
                    edge_x - reach_left,
                    inner.y,
                    reach_left + TABLE_COLUMN_GAP + reach_right,
                    1,
                ),
                line: Rect::new(edge_x, inner.y, TABLE_COLUMN_GAP, 1 + drawn_rows),
                width,
                min: plans[*index].floor,
                max: inner
                    .width
                    .saturating_sub(others_floor)
                    .max(plans[*index].floor),
            });
        }
        cursor += width + TABLE_COLUMN_GAP;
    }

    // 5. 본문 행 클릭 영역 (머리글 1행 제외)
    if selectable && visible > 0 {
        state.hit_areas.row_hits.push(RowHit {
            card,
            rect: Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1),
            offset: scroll,
            len: total,
        });
    }

    // 6. 표 그리기 (보이는 행만, 칸은 폭에 맞춰 끝 말줄임)
    let mut block = block;
    if total > visible && visible > 0 {
        block = block.title_bottom(position_label(scroll, visible, total));
    }
    let body_rows: Vec<Row<'static>> = rows
        .into_iter()
        .skip(scroll)
        .take(visible)
        .map(|row| {
            let cells: Vec<Cell<'static>> = row
                .cells
                .into_iter()
                .enumerate()
                .filter(|(index, _)| widths.get(*index).is_some_and(|width| *width > 0))
                .map(|(index, spans)| {
                    let clipped = clip_spans_to_width(spans, usize::from(widths[index]));
                    Cell::from(Line::from(clipped).alignment(columns[index].3))
                })
                .collect();
            Row::new(cells).style(row.style)
        })
        .collect();
    let constraints: Vec<Constraint> = shown
        .iter()
        .map(|index| Constraint::Length(widths[*index]))
        .collect();
    let drawn = Table::new(body_rows, constraints)
        .header(Row::new(header_cells).style(theme::STYLE_TABLE_HEADER))
        .block(block)
        .column_spacing(1);
    frame.render_widget(drawn, area);
    if total == 0 && inner.height > 1 {
        let headline = Span::styled(i18n::text(lang, empty.0), theme::STYLE_TEXT);
        let empty_area = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
        render_empty_state(frame, empty_area, headline, i18n::text(lang, empty.1));
    }

    // 7. 선택 행 띠 (머리글 행 제외)
    let body = Rect::new(inner.x, inner.y + 1, inner.width, inner.height.saturating_sub(1));
    if selectable && total > 0 && (scroll..scroll + visible).contains(&selected) {
        let focused = state.focused_card_id() == card;
        let row_y = body.y + (selected - scroll) as u16;
        paint_selection_band(frame.buffer_mut(), area, row_y, focused);
    }

    // 8. 오른쪽 거터 스크롤바
    render_card_scrollbar(frame, state, card, area, body);
}

// 하단 테두리 위치 표시 (` a-b / total `)
fn position_label(scroll: usize, visible: usize, total: usize) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {}-{} / {} ", scroll + 1, (scroll + visible).min(total), total),
        theme::STYLE_DIM,
    ))
    .right_aligned()
}

// 4. 텍스트 카드 --------------------------------------------------------------
// 줄 단위 스크롤, 일반 문장 줄바꿈 (이어지는 줄은 앞 공백만큼 들여쓰기), 오른쪽 거터는 안쪽 여백 열
pub fn render_text_card(
    frame: &mut Frame,
    state: &mut AppState,
    id: CardId,
    area: Rect,
    title: &str,
    lines: Vec<Line<'static>>,
) {
    let block = interactive_card(state, id, area, title, "", theme::BORDER_DIM)
        .padding(Padding::horizontal(1));
    let body = block.inner(area);

    // 1. 줄 배치와 보기 상태 보정
    let laid_out: Vec<Line<'static>> = lines
        .into_iter()
        .flat_map(|line| wrap_styled_line(line, usize::from(body.width)))
        .collect();
    let total = laid_out.len();
    let visible = body.height as usize;
    let view = &mut state.card_views[id as usize];
    view.total = total;
    view.visible = visible;
    view.scroll = view.scroll.min(total.saturating_sub(visible));
    let scroll = view.scroll;

    // 2. 줄 그리기와 위치 표시
    let mut block = block;
    if total > visible && visible > 0 {
        block = block.title_bottom(position_label(scroll, visible, total));
    }
    let shown: Vec<Line<'static>> = laid_out.into_iter().skip(scroll).take(visible).collect();
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(shown), body);

    // 3. 오른쪽 거터 스크롤바
    render_card_scrollbar(frame, state, id, area, body);
}

// 5. 카드 스크롤바 ------------------------------------------------------------
// 오른쪽 거터(테두리 바로 안쪽 열)가 ░ 트랙, 썸은 █, 그린 경우만 클릭 영역 등록
// body = 스크롤되는 본문 행 범위 (표는 머리글 행 제외)
pub fn render_card_scrollbar(
    frame: &mut Frame,
    state: &mut AppState,
    card: CardId,
    area: Rect,
    body: Rect,
) {
    // 1. 그릴 조건 (고정 카드·내용이 들어가는 카드는 없음)
    if card.kind() == CardKind::Static || body.is_empty() || area.width < 4 {
        return;
    }
    let view = state.card_views[card as usize];
    let Some(thumb) = scroll_thumb(view.total, view.visible, view.scroll, body.height) else {
        return;
    };

    // 2. 색: 트랙은 호버·드래그만 밝게, 썸은 드래그, 호버, 포커스 카드, 평소 순
    let dragging = state.scroll_drag.is_some_and(|drag| drag.card == card);
    let hovered = state.scroll_hover == Some(card);
    let track_color = if dragging || hovered {
        theme::SCROLL_TRACK_HOT
    } else {
        theme::SCROLL_TRACK
    };
    let thumb_color = if dragging {
        theme::SCROLL_THUMB_DRAG
    } else if hovered {
        theme::SCROLL_THUMB_HOVER
    } else if state.focused_card_id() == card {
        theme::SCROLL_THUMB_FOCUS
    } else {
        theme::SCROLL_THUMB
    };

    // 3. 거터 쓰기 (테두리 열은 건드리지 않음)
    let gutter_x = area.right() - 2;
    let buf = frame.buffer_mut();
    for y in body.y..body.bottom() {
        buf[(gutter_x, y)].set_symbol("░").set_fg(track_color);
    }
    let thumb_top = body.y + thumb.start;
    for y in thumb_top..thumb_top + thumb.len {
        buf[(gutter_x, y)].set_symbol("█").set_fg(thumb_color);
    }

    // 4. 클릭 영역 등록 (거터와 테두리 2열)
    state.hit_areas.scroll_hits.push(ScrollHit {
        card,
        track: Rect::new(gutter_x, body.y, 2, body.height),
        total: view.total,
        visible: view.visible,
        scroll: view.scroll,
    });
}

// 6. 카드 경계 드래그 손잡이 --------------------------------------------------
// 평소에는 아무것도 그리지 않고, 마우스 아래·드래그 중인 경계만 강조
pub fn render_split_handles(frame: &mut Frame, state: &AppState) {
    let dragging = state.split_drag.map(|drag| (drag.group, drag.index));
    let buf = frame.buffer_mut();
    for hit in &state.hit_areas.split_hits {
        let key = Some((hit.group, hit.index));
        let dragged = dragging == key;
        let active = dragged || (dragging.is_none() && state.split_hover == key);
        let line = hit.line;
        if !active || line.is_empty() {
            continue;
        }
        let color = if dragged {
            theme::SPLIT_GRIP_DRAG
        } else {
            theme::SPLIT_GRIP_HOT
        };
        match hit.axis {
            // 카드 사이 빈 칸 세로 손잡이
            SplitAxis::Columns => {
                for y in line.y..line.bottom() {
                    buf[(line.x, y)].set_symbol("┃").set_fg(color);
                }
            }
            // 위 카드 아래 테두리 가로 손잡이
            SplitAxis::Rows => {
                buf.set_style(line, Style::default().fg(color));
                let grip_w = line.width.min(6);
                let grip_x = line.x + (line.width - grip_w) / 2;
                for x in grip_x..grip_x + grip_w {
                    buf[(x, line.y)]
                        .set_symbol("━")
                        .set_fg(color)
                        .set_style(Style::default().add_modifier(Modifier::BOLD));
                }
            }
        }
    }
}

// 7. 열 경계 손잡이 -----------------------------------------------------------
// 평소에는 머리글 행 경계 칸에 흐린 │ 로 위치만 표시, 마우스 아래·끄는 동안은 머리글과 본문 행 전체 세로선 ┃
pub fn render_column_handles(frame: &mut Frame, state: &AppState) {
    let dragging = state.column_drag.map(|drag| (drag.card, drag.column));
    let buf = frame.buffer_mut();
    for hit in &state.hit_areas.column_hits {
        let key = Some((hit.card, hit.column));
        let line = hit.line;
        let color = if dragging == key {
            theme::SPLIT_GRIP_DRAG
        } else if dragging.is_none() && state.column_hover == key {
            theme::SPLIT_GRIP_HOT
        } else {
            buf[(line.x, line.y)]
                .set_symbol("│")
                .set_fg(theme::FG_FAINT);
            continue;
        };
        for y in line.y..line.bottom() {
            buf[(line.x, y)].set_symbol("┃").set_fg(color);
        }
    }
}

// 8. 선택 행 띠 ---------------------------------------------------------------
// 왼쪽 여백 열부터 거터 직전까지 한 줄, 포커스 카드는 진한 바탕·흰 글자, 아니면 바탕만
fn paint_selection_band(buf: &mut Buffer, area: Rect, row_y: u16, focused: bool) {
    let band = Rect::new(area.x + 1, row_y, area.width.saturating_sub(3), 1);
    let style = if focused {
        theme::STYLE_SELECTED_ROW
    } else {
        theme::STYLE_SELECTED_ROW_IDLE
    };
    buf.set_style(band, style);
}

// 9. 빈 상태 안내 -------------------------------------------------------------
// 영역 가운데에 제목 한 줄과 흐린 안내 한 줄 (높이 1이면 제목만)
pub fn render_empty_state(frame: &mut Frame, rect: Rect, headline: Span<'static>, hint: &str) {
    if rect.is_empty() {
        return;
    }
    let mut lines = vec![Line::from(headline)];
    if !hint.is_empty() && rect.height >= 2 {
        lines.push(Line::from(Span::styled(hint.to_string(), theme::STYLE_DIM)));
    }
    let used = (lines.len() as u16).min(rect.height);
    let body = Rect::new(rect.x, rect.y + (rect.height - used) / 2, rect.width, used);
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), body);
}

// 10. 표 열 폭 결정 -----------------------------------------------------------
// 자연 폭(또는 끌어 정한 폭)으로 시작, 모자라면 끌지 않은 글자 열부터 가장 넓은 순서로 최소 폭까지 줄이고 끈 열은 마지막
// 그래도 모자라면 맨 오른쪽 열부터 숨김 (Pinned 열은 마지막, 숫자 열은 줄지 않음), 남는 폭은 맨 오른쪽 Grow 열이 가짐 (끌어 정한 열은 받지 않음)
pub fn resolve_column_widths(plans: &[ColumnPlan], total: u16) -> Vec<u16> {
    let total = u32::from(total);

    // 1. 숨김 순서 (Pinned 아닌 열이 오른쪽부터, 그다음 Pinned 열이 오른쪽부터)
    let hide_order: Vec<usize> = (0..plans.len())
        .rev()
        .filter(|index| plans[*index].fit != ColumnFit::Pinned)
        .chain(
            (0..plans.len())
                .rev()
                .filter(|index| plans[*index].fit == ColumnFit::Pinned),
        )
        .collect();
    for hidden in 0..plans.len() {
        // 2. 보이는 열의 시작 폭과 사이 간격 합 (열 폭 상한 적용)
        let visible: Vec<usize> = (0..plans.len())
            .filter(|index| !hide_order[..hidden].contains(index))
            .collect();
        let mut widths: Vec<u16> = visible
            .iter()
            .map(|index| {
                let plan = &plans[*index];
                plan.user
                    .unwrap_or(plan.natural)
                    .min(MAX_COLUMN_WIDTH)
                    .max(plan.floor)
            })
            .collect();
        let gaps = (visible.len() as u32 - 1) * u32::from(TABLE_COLUMN_GAP);
        let used = widths.iter().map(|width| u32::from(*width)).sum::<u32>() + gaps;

        // 3. 부족분은 가장 넓은 열부터 한 칸씩 줄임 (끌지 않은 열 먼저)
        if used > total {
            let mut deficit = used - total;
            for dragged in [false, true] {
                while deficit > 0 {
                    let widest = (0..visible.len())
                        .filter(|pos| {
                            let plan = &plans[visible[*pos]];
                            plan.user.is_some() == dragged && widths[*pos] > plan.floor
                        })
                        .max_by_key(|pos| widths[*pos]);
                    let Some(pos) = widest else {
                        break;
                    };
                    widths[pos] -= 1;
                    deficit -= 1;
                }
            }
            if deficit > 0 {
                continue;
            }
        }

        // 4. 남는 폭은 맨 오른쪽 Grow 열 (끌어 정한 열이면 그대로 둠)
        let absorber = (0..visible.len())
            .rev()
            .find(|pos| plans[visible[*pos]].fit == ColumnFit::Grow)
            .filter(|pos| plans[visible[*pos]].user.is_none());
        if let Some(pos) = absorber {
            widths[pos] += total.saturating_sub(used) as u16;
        }

        // 5. 숨긴 열은 폭 0
        let mut full = vec![0; plans.len()];
        for (pos, index) in visible.iter().enumerate() {
            full[*index] = widths[pos];
        }
        return full;
    }
    vec![0; plans.len()]
}

// 11. 칸 말줄임 ---------------------------------------------------------------
// 넓이를 넘으면 끝에 … 하나 (마지막으로 자른 조각의 스타일), 넘지 않으면 그대로
pub fn clip_spans_to_width(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let total: usize = spans.iter().map(Span::width).sum();
    if total <= width {
        return spans;
    }
    if width == 0 {
        return Vec::new();
    }
    let limit = width - 1;
    let mut clipped: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    let mut last_style = Style::default();
    for span in spans {
        last_style = span.style;
        let mut kept = String::new();
        let mut cut = false;
        for letter in span.content.chars() {
            let letter_width = char_width(letter);
            if used + letter_width > limit {
                cut = true;
                break;
            }
            kept.push(letter);
            used += letter_width;
        }
        if !kept.is_empty() {
            clipped.push(Span::styled(kept, span.style));
        }
        if cut {
            break;
        }
    }
    clipped.push(Span::styled("…", last_style));
    clipped
}

// 12. 스타일 유지 줄바꿈 ------------------------------------------------------
// 공백 뒤 줄나눔, 긴 낱말은 글자 단위, 이어지는 줄은 앞 공백만큼 들여쓰기 (폭 0이면 그대로)
pub fn wrap_styled_line(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 || line.width() <= width {
        return vec![line];
    }
    let Line {
        spans,
        style,
        alignment,
    } = line;

    // 1. 글자별 (글자, 스타일, 표시 폭)
    let letters: Vec<(char, Style, usize)> = spans
        .iter()
        .flat_map(|span| {
            span.content
                .chars()
                .map(move |letter| (letter, span.style, char_width(letter)))
        })
        .collect();

    // 2. 이어지는 줄 들여쓰기 (앞 공백, 폭의 절반 이하)
    let leading_spaces = letters
        .iter()
        .take_while(|(letter, _, _)| *letter == ' ')
        .count();
    let hang = leading_spaces.min(width / 2);
    let hang_cells: Vec<(char, Style, usize)> = vec![(' ', Style::default(), 1); hang];

    // 3. 행 채움 (넘치면 마지막 공백 뒤에서, 없으면 현재 위치에서 나눠 다음 줄로)
    let mut rows: Vec<Vec<(char, Style, usize)>> = Vec::new();
    let mut row: Vec<(char, Style, usize)> = Vec::new();
    let mut row_width = 0usize;
    let mut break_at: Option<usize> = None;
    for cell in letters {
        if cell.0 == ' ' && row_width + cell.2 > width && row.len() > hang {
            rows.push(std::mem::replace(&mut row, hang_cells.clone()));
            row_width = hang;
            break_at = None;
            continue;
        }
        while cell.0 != ' ' && row_width + cell.2 > width && row.len() > hang {
            let split = break_at.filter(|at| *at > hang).unwrap_or(row.len());
            let carried = row.split_off(split);
            rows.push(std::mem::replace(&mut row, hang_cells.clone()));
            row.extend(carried);
            row_width = row.iter().map(|(_, _, shown)| *shown).sum();
            break_at = None;
        }
        if cell.0 == ' ' && row.len() >= hang {
            break_at = Some(row.len() + 1);
        }
        row_width += cell.2;
        row.push(cell);
    }
    if row.len() > hang || rows.is_empty() {
        rows.push(row);
    }

    // 4. 같은 스타일 글자를 조각으로 묶어 줄 생성
    rows.into_iter()
        .map(|cells| {
            let mut pieces: Vec<Span<'static>> = Vec::new();
            let mut piece = String::new();
            let mut piece_style = Style::default();
            for (letter, letter_style, _) in cells {
                if !piece.is_empty() && letter_style != piece_style {
                    pieces.push(Span::styled(std::mem::take(&mut piece), piece_style));
                }
                piece_style = letter_style;
                piece.push(letter);
            }
            if !piece.is_empty() {
                pieces.push(Span::styled(piece, piece_style));
            }
            Line {
                spans: pieces,
                style,
                alignment,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(natural: u16, fit: ColumnFit, user: Option<u16>) -> ColumnPlan {
        let floor = if matches!(fit, ColumnFit::Fixed | ColumnFit::Pinned) {
            natural
        } else {
            MIN_COLUMN_WIDTH.min(natural)
        };
        ColumnPlan {
            natural,
            floor,
            fit,
            user,
        }
    }

    // 1. 넉넉한 폭
    #[test]
    fn spare_width_goes_to_the_rightmost_grow_column_only() {
        let plans = [
            plan(6, ColumnFit::Fixed, None),
            plan(10, ColumnFit::Shrink, None),
            plan(12, ColumnFit::Grow, None),
        ];
        // 간격 2칸 포함 사용 30, 남는 10칸은 Grow 열로
        assert_eq!(resolve_column_widths(&plans, 40), [6, 10, 22]);
    }

    // 2. 좁아질 때
    #[test]
    fn shrinking_takes_the_widest_text_column_first_down_to_the_floor() {
        let plans = [
            plan(6, ColumnFit::Fixed, None),
            plan(20, ColumnFit::Shrink, None),
            plan(12, ColumnFit::Shrink, None),
        ];
        // 사용 6+20+12+2 = 40 -> 30: 가장 넓은 열부터 줄임
        let widths = resolve_column_widths(&plans, 30);
        assert_eq!(widths[0], 6);
        assert_eq!(widths.iter().map(|width| u32::from(*width)).sum::<u32>() + 2, 30);
        assert!(widths[1] <= 20 && widths[2] <= 12);
    }

    // 3. 오른쪽 열 숨김
    #[test]
    fn rightmost_columns_hide_only_after_every_shrinkable_column_hit_its_floor() {
        let plans = [
            plan(8, ColumnFit::Fixed, None),
            plan(8, ColumnFit::Fixed, None),
            plan(8, ColumnFit::Fixed, None),
        ];
        assert_eq!(resolve_column_widths(&plans, 26), [8, 8, 8]);
        assert_eq!(resolve_column_widths(&plans, 25), [8, 8, 0]);
        assert_eq!(resolve_column_widths(&plans, 17), [8, 8, 0]);
        assert_eq!(resolve_column_widths(&plans, 16), [8, 0, 0]);
        assert_eq!(resolve_column_widths(&plans, 7), [0, 0, 0]);
    }

    // 3-1. Pinned 열 유지
    #[test]
    fn pinned_columns_hide_after_every_other_column_and_keep_their_width() {
        let plans = [
            plan(8, ColumnFit::Fixed, None),
            plan(12, ColumnFit::Shrink, None),
            plan(8, ColumnFit::Fixed, None),
            plan(6, ColumnFit::Pinned, None),
        ];
        // 간격 3칸 포함 사용 8+12+8+6+3 = 37, 줄일 수 있는 글자 열이 먼저 줄고 Pinned 열은 그대로
        assert_eq!(resolve_column_widths(&plans, 37), [8, 12, 8, 6]);
        assert_eq!(resolve_column_widths(&plans, 31), [8, 6, 8, 6]);
        // 모자라면 오른쪽 일반 열부터 숨기고 Pinned 열은 남김
        assert_eq!(resolve_column_widths(&plans, 22), [8, 6, 0, 6]);
        assert_eq!(resolve_column_widths(&plans, 17), [8, 0, 0, 6]);
        assert_eq!(resolve_column_widths(&plans, 6), [0, 0, 0, 6]);
        assert_eq!(resolve_column_widths(&plans, 5), [0, 0, 0, 0]);
    }

    // 4. 끌어 정한 폭
    #[test]
    fn dragged_width_survives_slack_and_shrinks_last_and_caps_at_512() {
        let plans = [
            plan(10, ColumnFit::Shrink, Some(30)),
            plan(10, ColumnFit::Grow, None),
        ];
        // 남는 폭은 끌지 않은 Grow 열이 가짐
        assert_eq!(resolve_column_widths(&plans, 60), [30, 29]);
        // 모자라면 끌지 않은 열이 최소 폭까지 먼저 줄고 그다음 끈 열
        assert_eq!(resolve_column_widths(&plans, 30), [26, 3]);
        let huge = [plan(10, ColumnFit::Shrink, Some(9_999))];
        assert_eq!(resolve_column_widths(&huge, 2_000), [MAX_COLUMN_WIDTH]);
    }

    // 5. 칸 말줄임
    #[test]
    fn clipping_ends_with_one_ellipsis_and_keeps_wide_glyphs_whole() {
        let spans = vec![Span::raw("도구목록")];
        let clipped = clip_spans_to_width(spans, 6);
        let text: String = clipped.iter().map(|span| span.content.as_ref()).collect();
        assert_eq!(text, "도구…");
        assert_eq!(clip_spans_to_width(vec![Span::raw("abc")], 3).len(), 1);
        assert!(clip_spans_to_width(vec![Span::raw("abc")], 0).is_empty());
    }

    // 6. 줄바꿈
    #[test]
    fn wrapping_breaks_at_spaces_and_hangs_continuation_lines() {
        let line = Line::from(Span::raw("  alpha beta gamma delta"));
        let rows = wrap_styled_line(line, 12);
        let texts: Vec<String> = rows
            .iter()
            .map(|row| row.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();
        assert!(texts.len() >= 2, "{texts:?}");
        assert!(texts.iter().all(|text| text_width(text) <= 12), "{texts:?}");
        assert!(texts[1].starts_with("  "), "{texts:?}");
    }
}
