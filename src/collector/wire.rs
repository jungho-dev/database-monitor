//! wire.rs
//! Npcap 기반 패시브 DB 와이어 패킷 스니퍼 (외부 DB 쿼리 0회 보장).

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Instant;
use crossbeam_channel::{Sender, TrySendError};
use crate::app::config::MonitorConfig;
use crate::analysis::decoder::{WireFrame, decode_packet};
use crate::analysis::reassembly::FlowReassembler;

// 1. 패시브 와이어 캡처 핸들 --------------------------------------------------
pub struct WireCaptureGuard {
    pub active_devices: Vec<String>,
    stop_flag: Arc<AtomicBool>,
    dropped_frames: Arc<AtomicU64>,
    workers: Vec<JoinHandle<()>>,
}

impl WireCaptureGuard {
    // 채널 포화로 버린 프레임 누적 수 (RX·QPS 과소 집계 신호)
    pub fn dropped_frames(&self) -> u64 {
        self.dropped_frames.load(Ordering::Relaxed)
    }

    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        for handle in self.workers.drain(..) {
            let _ = handle.join();
        }
    }
}

impl Drop for WireCaptureGuard {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }
}

// 2. 패시브 스니퍼 기동 -------------------------------------------------------
pub fn start_passive_wire_capture(
    cfg: &MonitorConfig,
    tx: Sender<WireFrame>,
) -> Result<WireCaptureGuard, String> {
    if !cfg.enable_pcap {
        return Err("PCAP disabled by configuration".to_string());
    }

    let devices = pcap::Device::list().map_err(|e| format!("Npcap device listing failed: {e}"))?;
    if devices.is_empty() {
        return Err("No usable network interface".to_string());
    }

    let bpf = build_bpf_filter(cfg);
    // 워커가 감시 대상 방향을 직접 판별 (BPF host 조건에 걸린 비DB 트래픽은 채널에 넣지 않음)
    let watch_cfg = Arc::new(cfg.clone());
    let stop_flag = Arc::new(AtomicBool::new(false));
    let dropped_frames = Arc::new(AtomicU64::new(0));
    let mut active_devices = Vec::new();
    let mut workers = Vec::new();

    for dev in devices {
        if let Some(ref hint) = cfg.interface_hint {
            let desc = dev.desc.as_deref().unwrap_or("");
            if !dev.name.contains(hint) && !desc.to_ascii_lowercase().contains(&hint.to_ascii_lowercase()) {
                continue;
            }
        } else if dev.addresses.is_empty() && !dev.name.to_ascii_lowercase().contains("loopback") {
            continue;
        }

        let label = dev
            .desc
            .clone()
            .unwrap_or_else(|| dev.name.clone());
        let Ok(inactive) = pcap::Capture::from_device(dev) else {
            continue;
        };
        let Ok(mut cap) = inactive
            .snaplen(2048)
            .promisc(false)
            .timeout(100)
            .open()
        else {
            continue;
        };

        if cap.filter(&bpf, true).is_err() {
            continue;
        }

        let datalink = cap.get_datalink().0;
        let worker_tx = tx.clone();
        let worker_stop = Arc::clone(&stop_flag);
        let worker_dropped = Arc::clone(&dropped_frames);
        let worker_cfg = Arc::clone(&watch_cfg);
        active_devices.push(label);

        if let Ok(handle) = std::thread::Builder::new()
            .name("database-monitor-wire".to_string())
            .spawn(move || {
                // 요청 방향 세그먼트 재조립 버퍼 (워커마다 독립)
                let mut flows = FlowReassembler::default();
                while !worker_stop.load(Ordering::Relaxed) {
                    // 1. 패킷 해독 (수신 대기 만료는 해독 없이 만료 점검만)
                    let direct = match cap.next_packet() {
                        Ok(pkt) => decode_packet(pkt.data, datalink, &worker_cfg, &mut flows),
                        Err(pcap::Error::TimeoutExpired) => None,
                        Err(_) => break,
                    };

                    // 2. 포기·만료된 재조립 흐름을 불완전 문장 프레임으로 합류
                    let abandoned = flows.drain_expired_flows(Instant::now());
                    for frame in direct.into_iter().chain(abandoned) {
                        match worker_tx.try_send(frame) {
                            Ok(()) => {}
                            Err(TrySendError::Full(_)) => {
                                worker_dropped.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(TrySendError::Disconnected(_)) => return,
                        }
                    }
                }
            })
        {
            workers.push(handle);
        }
    }

    if workers.is_empty() {
        return Err("PCAP open failed on every active interface (Socket-Only fallback)".to_string());
    }

    Ok(WireCaptureGuard {
        active_devices,
        stop_flag,
        dropped_frames,
        workers,
    })
}

// 3. BPF 필터 문자열 생성 -----------------------------------------------------
pub fn build_bpf_filter(cfg: &MonitorConfig) -> String {
    let mut host_ips: Vec<&IpAddr> = cfg.known_hosts.keys().collect();
    host_ips.sort();
    let terms: Vec<String> = cfg
        .watched_ports
        .iter()
        .map(|port| format!("port {port}"))
        .chain(host_ips.into_iter().map(|ip| format!("host {ip}")))
        .collect();
    if terms.is_empty() {
        return "tcp port 3306".to_string();
    }
    format!("tcp and ({})", terms.join(" or "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bpf_filter_covers_ports_and_known_hosts() {
        let mut cfg = MonitorConfig {
            watched_ports: vec![3306, 6379],
            ..MonitorConfig::default()
        };
        cfg.known_hosts
            .insert("192.0.2.10".parse().unwrap(), "개발 DB".to_string());
        assert_eq!(
            build_bpf_filter(&cfg),
            "tcp and (port 3306 or port 6379 or host 192.0.2.10)"
        );
    }
}
