//! reassembly.rs
//! 요청 방향 TCP 세그먼트 재조립: 여러 세그먼트로 나뉜 SQL 을 선언 길이까지 모아 한 번에 해독.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};
use crate::analysis::decoder::{WireFrame, decode_wire_sql};

pub const MAX_FLOW_BYTES: usize = 256 * 1024;
pub const MAX_FLOWS: usize = 256;
pub const FLOW_TIMEOUT: Duration = Duration::from_secs(2);
const SWEEP_INTERVAL: Duration = Duration::from_millis(200);

// 1. 흐름 키와 재조립 버퍼 ----------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TcpFlowKey {
    pub src_ip: IpAddr,
    pub src_port: u16,
    pub dst_ip: IpAddr,
    pub dst_port: u16,
}

struct PendingFlow {
    buffer: Vec<u8>,
    next_seq: u32,
    expected_len: usize,
    started_at: Instant,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SegmentOutcome {
    Whole,
    Held,
    Assembled(Vec<u8>),
}

#[derive(Default)]
pub struct FlowReassembler {
    flows: HashMap<TcpFlowKey, PendingFlow>,
    abandoned: Vec<(TcpFlowKey, PendingFlow)>,
    last_sweep_at: Option<Instant>,
}

// 2. 세그먼트 수용과 만료 흐름 회수 -------------------------------------------
impl FlowReassembler {
    pub fn accept_segment(
        &mut self,
        key: TcpFlowKey,
        seq: u32,
        payload: &[u8],
        declared_len: Option<usize>,
        now: Instant,
    ) -> SegmentOutcome {
        // 1. 진행 중인 흐름에 이어붙이기
        if let Some(flow) = self.flows.get_mut(&key) {
            if seq == flow.next_seq {
                let needed = flow.expected_len - flow.buffer.len();
                flow.buffer.extend_from_slice(&payload[..payload.len().min(needed)]);
                flow.next_seq = flow.next_seq.wrapping_add(payload.len() as u32);
                if flow.buffer.len() < flow.expected_len {
                    return SegmentOutcome::Held;
                }
                if let Some(done) = self.flows.remove(&key) {
                    return SegmentOutcome::Assembled(done.buffer);
                }
            }

            // 순서 어긋남 (누락·재전송·캡처 잘림), 모은 앞부분은 불완전로 포기
            if let Some(flow) = self.flows.remove(&key) {
                self.abandoned.push((key, flow));
            }
        }

        // 2. 선언 길이보다 짧은 시작 세그먼트면 새 흐름 등록
        if let Some(expected_len) = declared_len
            && expected_len > payload.len()
            && expected_len <= MAX_FLOW_BYTES
            && self.flows.len() < MAX_FLOWS
        {
            self.flows.insert(
                key,
                PendingFlow {
                    buffer: payload.to_vec(),
                    next_seq: seq.wrapping_add(payload.len() as u32),
                    expected_len,
                    started_at: now,
                },
            );
            return SegmentOutcome::Held;
        }
        SegmentOutcome::Whole
    }

    pub fn drain_expired_flows(&mut self, now: Instant) -> Vec<WireFrame> {
        // 1. 시간 제한을 넘긴 흐름 포기 (점검은 주기마다)
        let sweep_due = self
            .last_sweep_at
            .is_none_or(|at| now.duration_since(at) >= SWEEP_INTERVAL);
        if sweep_due && !self.flows.is_empty() {
            self.last_sweep_at = Some(now);
            for (key, flow) in std::mem::take(&mut self.flows) {
                if now.duration_since(flow.started_at) >= FLOW_TIMEOUT {
                    self.abandoned.push((key, flow));
                } else {
                    self.flows.insert(key, flow);
                }
            }
        }

        // 2. 포기한 흐름을 불완전 문장 프레임으로 변환 (바이트는 이미 집계됨)
        std::mem::take(&mut self.abandoned)
            .into_iter()
            .filter_map(|(key, flow)| {
                Some(WireFrame {
                    src_ip: key.src_ip,
                    src_port: key.src_port,
                    dst_ip: key.dst_ip,
                    dst_port: key.dst_port,
                    payload_len: 0,
                    decoded_sql: Some(decode_wire_sql(key.dst_port, &flow.buffer)?),
                    captured_at: flow.started_at,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn build_flow_key(src_port: u16) -> TcpFlowKey {
        TcpFlowKey {
            src_ip: IpAddr::V4(Ipv4Addr::new(192, 0, 2, 25)),
            src_port,
            dst_ip: IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)),
            dst_port: 3306,
        }
    }

    // 선언 길이 = 헤더 4바이트 + 시퀀스·커맨드·본문
    fn build_query_head(sql: &str, declared_extra: usize) -> Vec<u8> {
        let pkt_len = sql.len() + 1 + declared_extra;
        let mut packet = vec![pkt_len as u8, (pkt_len >> 8) as u8, (pkt_len >> 16) as u8, 0, 0x03];
        packet.extend_from_slice(sql.as_bytes());
        packet
    }

    #[test]
    fn in_order_segments_assemble_and_extra_bytes_are_cut() {
        let mut flows = FlowReassembler::default();
        let key = build_flow_key(55101);
        let now = Instant::now();

        let first = flows.accept_segment(key, 100, &[1, 2, 3, 4, 5, 6], Some(10), now);
        assert_eq!(first, SegmentOutcome::Held);
        let second = flows.accept_segment(key, 106, &[7, 8, 9, 10, 11, 12, 13, 14], Some(99), now);
        assert_eq!(second, SegmentOutcome::Assembled(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]));
        assert!(flows.flows.is_empty());
    }

    #[test]
    fn sequence_gap_abandons_flow_and_restarts_with_new_segment() {
        let mut flows = FlowReassembler::default();
        let key = build_flow_key(55101);
        let now = Instant::now();
        let head = build_query_head("UPDATE tb_user SET memo = 'a'", 4000);

        assert_eq!(flows.accept_segment(key, 100, &head, Some(4000), now), SegmentOutcome::Held);
        let gap = flows.accept_segment(key, 9000, &[7, 7, 7], None, now);
        assert_eq!(gap, SegmentOutcome::Whole);
        assert!(flows.flows.is_empty());

        let frames = flows.drain_expired_flows(now);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].payload_len, 0);
        assert!(frames[0].decoded_sql.as_ref().is_some_and(|sql| !sql.complete));
    }

    #[test]
    fn stalled_flow_is_dropped_after_timeout_and_memory_is_released() {
        let mut flows = FlowReassembler::default();
        let now = Instant::now();
        let head = build_query_head("DELETE FROM tb_user", 4000);
        let declared = Some(head.len() + 4000);

        assert_eq!(
            flows.accept_segment(build_flow_key(55101), 100, &head, declared, now),
            SegmentOutcome::Held
        );
        assert!(flows.drain_expired_flows(now + Duration::from_secs(1)).is_empty());
        assert_eq!(flows.flows.len(), 1);

        let frames = flows.drain_expired_flows(now + Duration::from_secs(3));
        assert_eq!(frames.len(), 1);
        assert!(frames[0].decoded_sql.as_ref().is_some_and(|sql| !sql.complete));
        assert!(flows.flows.is_empty());
        assert!(flows.drain_expired_flows(now + Duration::from_secs(6)).is_empty());
    }

    #[test]
    fn oversize_message_and_full_table_are_not_tracked() {
        let mut flows = FlowReassembler::default();
        let now = Instant::now();

        let oversize = flows.accept_segment(build_flow_key(1), 1, &[0; 8], Some(MAX_FLOW_BYTES + 1), now);
        assert_eq!(oversize, SegmentOutcome::Whole);

        for port in 0..MAX_FLOWS as u16 {
            let held = flows.accept_segment(build_flow_key(10_000 + port), 1, &[0; 8], Some(64), now);
            assert_eq!(held, SegmentOutcome::Held);
        }
        let overflow = flows.accept_segment(build_flow_key(60_000), 1, &[0; 8], Some(64), now);
        assert_eq!(overflow, SegmentOutcome::Whole);
        assert_eq!(flows.flows.len(), MAX_FLOWS);
    }
}
