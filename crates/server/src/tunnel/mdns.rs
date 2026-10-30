use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, RwLock};

pub const MDNS_IPV4_MULTICAST: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
pub const MDNS_PORT: u16 = 5353;
pub const DNS_SD_SERVICE_ENUMERATION: &str = "_services._dns-sd._udp.local";

#[derive(Debug, thiserror::Error)]
pub enum MdnsError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Mdns service already running")]
    AlreadyRunning,
    #[error("Packet encoding error: {0}")]
    EncodingError(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsRecord {
    Ptr {
        name: String,
        ptr_name: String,
        ttl: u32,
    },
    Srv {
        name: String,
        priority: u16,
        weight: u16,
        port: u16,
        target: String,
        ttl: u32,
    },
    Txt {
        name: String,
        entries: Vec<String>,
        ttl: u32,
    },
    A {
        name: String,
        ip: Ipv4Addr,
        ttl: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsQuestion {
    pub name: String,
    pub qtype: u16,
    pub qclass: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdnsServiceConfig {
    pub instance_name: String,
    pub service_type: String,
    pub domain: String,
    pub port: u16,
    pub hostname: String,
    pub ip_override: Option<Ipv4Addr>,
    pub txt_records: Vec<(String, String)>,
    pub announcement_interval: Duration,
    pub ttl: u32,
}

impl Default for MdnsServiceConfig {
    fn default() -> Self {
        Self {
            instance_name: "AetherStream Media Server".to_string(),
            service_type: "_aetherstream._tcp".to_string(),
            domain: "local".to_string(),
            port: 8080,
            hostname: "aetherstream-server".to_string(),
            ip_override: None,
            txt_records: vec![
                ("version".to_string(), "1.0.0".to_string()),
                ("vendor".to_string(), "AetherStream".to_string()),
                ("api_version".to_string(), "v1".to_string()),
                ("tls".to_string(), "false".to_string()),
                ("model".to_string(), "AetherStream-Daemon".to_string()),
            ],
            announcement_interval: Duration::from_secs(60),
            ttl: 120,
        }
    }
}

pub fn detect_local_ip() -> Ipv4Addr {
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(SocketAddr::V4(addr)) = socket.local_addr() {
                let ip = *addr.ip();
                if !ip.is_loopback() && !ip.is_unspecified() {
                    return ip;
                }
            }
        }
    }
    Ipv4Addr::new(127, 0, 0, 1)
}

pub fn encode_dns_name(name: &str, buf: &mut Vec<u8>) {
    let clean = name.trim_end_matches('.');
    for label in clean.split('.') {
        if label.is_empty() {
            continue;
        }
        let bytes = label.as_bytes();
        let len = bytes.len().min(63) as u8;
        buf.push(len);
        buf.extend_from_slice(&bytes[..len as usize]);
    }
    buf.push(0x00);
}

pub fn decode_dns_name(buf: &[u8], offset: &mut usize) -> Option<String> {
    let mut labels = Vec::new();
    let mut jumped = false;
    let mut initial_offset = *offset;
    let mut cur = *offset;
    let mut hops = 0;

    while cur < buf.len() && hops < 20 {
        let len = buf[cur] as usize;
        if len == 0 {
            cur += 1;
            break;
        }
        if len & 0xC0 == 0xC0 {
            if cur + 1 >= buf.len() {
                return None;
            }
            let ptr = ((len & 0x3F) << 8) | (buf[cur + 1] as usize);
            if !jumped {
                initial_offset = cur + 2;
                jumped = true;
            }
            cur = ptr;
            hops += 1;
            continue;
        }
        cur += 1;
        if cur + len > buf.len() {
            return None;
        }
        let label = std::str::from_utf8(&buf[cur..cur + len]).ok()?;
        labels.push(label.to_string());
        cur += len;
    }

    if jumped {
        *offset = initial_offset;
    } else {
        *offset = cur;
    }

    Some(labels.join("."))
}

pub fn encode_record(record: &DnsRecord, buf: &mut Vec<u8>) {
    match record {
        DnsRecord::Ptr {
            name,
            ptr_name,
            ttl,
        } => {
            encode_dns_name(name, buf);
            buf.extend_from_slice(&12u16.to_be_bytes()); // TYPE PTR
            buf.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN (shared)
            buf.extend_from_slice(&ttl.to_be_bytes());
            let mut rdata = Vec::new();
            encode_dns_name(ptr_name, &mut rdata);
            buf.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
            buf.extend_from_slice(&rdata);
        }
        DnsRecord::Srv {
            name,
            priority,
            weight,
            port,
            target,
            ttl,
        } => {
            encode_dns_name(name, buf);
            buf.extend_from_slice(&33u16.to_be_bytes()); // TYPE SRV
            buf.extend_from_slice(&0x8001u16.to_be_bytes()); // CLASS IN + CacheFlush
            buf.extend_from_slice(&ttl.to_be_bytes());
            let mut rdata = Vec::new();
            rdata.extend_from_slice(&priority.to_be_bytes());
            rdata.extend_from_slice(&weight.to_be_bytes());
            rdata.extend_from_slice(&port.to_be_bytes());
            encode_dns_name(target, &mut rdata);
            buf.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
            buf.extend_from_slice(&rdata);
        }
        DnsRecord::Txt { name, entries, ttl } => {
            encode_dns_name(name, buf);
            buf.extend_from_slice(&16u16.to_be_bytes()); // TYPE TXT
            buf.extend_from_slice(&0x8001u16.to_be_bytes()); // CLASS IN + CacheFlush
            buf.extend_from_slice(&ttl.to_be_bytes());
            let mut rdata = Vec::new();
            for entry in entries {
                let bytes = entry.as_bytes();
                let len = bytes.len().min(255) as u8;
                rdata.push(len);
                rdata.extend_from_slice(&bytes[..len as usize]);
            }
            if rdata.is_empty() {
                rdata.push(0x00);
            }
            buf.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
            buf.extend_from_slice(&rdata);
        }
        DnsRecord::A { name, ip, ttl } => {
            encode_dns_name(name, buf);
            buf.extend_from_slice(&1u16.to_be_bytes()); // TYPE A
            buf.extend_from_slice(&0x8001u16.to_be_bytes()); // CLASS IN + CacheFlush
            buf.extend_from_slice(&ttl.to_be_bytes());
            buf.extend_from_slice(&4u16.to_be_bytes());
            buf.extend_from_slice(&ip.octets());
        }
    }
}

pub fn build_mdns_response(records: &[DnsRecord]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(512);
    packet.extend_from_slice(&0u16.to_be_bytes()); // ID: 0
    packet.extend_from_slice(&0x8400u16.to_be_bytes()); // Flags: QR=1, AA=1
    packet.extend_from_slice(&0u16.to_be_bytes()); // QDCOUNT: 0
    packet.extend_from_slice(&(records.len() as u16).to_be_bytes()); // ANCOUNT
    packet.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT: 0
    packet.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT: 0

    for record in records {
        encode_record(record, &mut packet);
    }
    packet
}

pub fn parse_mdns_query(buf: &[u8]) -> Option<Vec<DnsQuestion>> {
    if buf.len() < 12 {
        return None;
    }
    let flags = u16::from_be_bytes([buf[2], buf[3]]);
    let is_query = (flags & 0x8000) == 0;
    if !is_query {
        return None;
    }
    let qdcount = u16::from_be_bytes([buf[4], buf[5]]) as usize;
    let mut offset = 12;
    let mut questions = Vec::with_capacity(qdcount);

    for _ in 0..qdcount {
        let name = decode_dns_name(buf, &mut offset)?;
        if offset + 4 > buf.len() {
            return None;
        }
        let qtype = u16::from_be_bytes([buf[offset], buf[offset + 1]]);
        let qclass = u16::from_be_bytes([buf[offset + 2], buf[offset + 3]]) & 0x7FFF;
        offset += 4;
        questions.push(DnsQuestion {
            name,
            qtype,
            qclass,
        });
    }
    Some(questions)
}

#[derive(Clone)]
pub struct MdnsPublisher {
    config: Arc<RwLock<MdnsServiceConfig>>,
    is_running: Arc<RwLock<bool>>,
    shutdown_tx: broadcast::Sender<()>,
    worker_handle: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,
}

impl MdnsPublisher {
    pub fn new(config: MdnsServiceConfig) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        Self {
            config: Arc::new(RwLock::new(config)),
            is_running: Arc::new(RwLock::new(false)),
            shutdown_tx,
            worker_handle: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn is_running(&self) -> bool {
        *self.is_running.read().await
    }

    pub async fn config(&self) -> MdnsServiceConfig {
        self.config.read().await.clone()
    }

    pub async fn service_type_fqdn(&self) -> String {
        let cfg = self.config.read().await;
        format!("{}.{}", cfg.service_type, cfg.domain)
    }

    pub async fn service_instance_fqdn(&self) -> String {
        let cfg = self.config.read().await;
        format!("{}.{}.{}", cfg.instance_name, cfg.service_type, cfg.domain)
    }

    pub async fn hostname_fqdn(&self) -> String {
        let cfg = self.config.read().await;
        format!("{}.{}", cfg.hostname, cfg.domain)
    }

    pub async fn resolved_ip(&self) -> Ipv4Addr {
        let cfg = self.config.read().await;
        cfg.ip_override.unwrap_or_else(detect_local_ip)
    }

    pub async fn build_records(&self, ttl: u32) -> Vec<DnsRecord> {
        let cfg = self.config.read().await;
        let service_type = format!("{}.{}", cfg.service_type, cfg.domain);
        let instance_fqdn = format!("{}.{}", cfg.instance_name, service_type);
        let host_fqdn = format!("{}.{}", cfg.hostname, cfg.domain);
        let ip = cfg.ip_override.unwrap_or_else(detect_local_ip);

        let txt_entries: Vec<String> = cfg
            .txt_records
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();

        vec![
            DnsRecord::Ptr {
                name: DNS_SD_SERVICE_ENUMERATION.to_string(),
                ptr_name: service_type.clone(),
                ttl: ttl.max(4500),
            },
            DnsRecord::Ptr {
                name: service_type,
                ptr_name: instance_fqdn.clone(),
                ttl: ttl.max(4500),
            },
            DnsRecord::Srv {
                name: instance_fqdn.clone(),
                priority: 0,
                weight: 0,
                port: cfg.port,
                target: host_fqdn.clone(),
                ttl,
            },
            DnsRecord::Txt {
                name: instance_fqdn,
                entries: txt_entries,
                ttl: ttl.max(4500),
            },
            DnsRecord::A {
                name: host_fqdn,
                ip,
                ttl,
            },
        ]
    }

    pub async fn start(&self) -> Result<(), MdnsError> {
        if *self.is_running.read().await {
            return Err(MdnsError::AlreadyRunning);
        }

        // Attempt binding to MDNS_PORT (5353), falling back to ephemeral port if busy
        let socket = match UdpSocket::bind(format!("0.0.0.0:{MDNS_PORT}")).await {
            Ok(s) => s,
            Err(_) => UdpSocket::bind("0.0.0.0:0").await?,
        };

        let _ = socket.set_multicast_loop_v4(true);
        let _ = socket.join_multicast_v4(MDNS_IPV4_MULTICAST, Ipv4Addr::UNSPECIFIED);

        let socket = Arc::new(socket);
        *self.is_running.write().await = true;

        let publisher_clone = self.clone();
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let socket_clone = socket.clone();

        let handle = tokio::spawn(async move {
            let target_addr: SocketAddr = SocketAddr::new(MDNS_IPV4_MULTICAST.into(), MDNS_PORT);

            // Send initial announcements (burst of 2 packets)
            let records = publisher_clone.build_records(publisher_clone.config.read().await.ttl).await;
            let packet = build_mdns_response(&records);
            let _ = socket_clone.send_to(&packet, target_addr).await;
            tokio::time::sleep(Duration::from_millis(250)).await;
            let _ = socket_clone.send_to(&packet, target_addr).await;

            let interval_dur = publisher_clone.config.read().await.announcement_interval;
            let mut ticker = tokio::time::interval(interval_dur);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            let mut buf = [0u8; 1500];

            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        // Send goodbye packet (TTL = 0)
                        let goodbye_records = publisher_clone.build_records(0).await;
                        let goodbye_packet = build_mdns_response(&goodbye_records);
                        let _ = socket_clone.send_to(&goodbye_packet, target_addr).await;
                        break;
                    }
                    _ = ticker.tick() => {
                        let records = publisher_clone.build_records(publisher_clone.config.read().await.ttl).await;
                        let packet = build_mdns_response(&records);
                        let _ = socket_clone.send_to(&packet, target_addr).await;
                    }
                    recv_res = socket_clone.recv_from(&mut buf) => {
                        if let Ok((len, src_addr)) = recv_res {
                            if let Some(questions) = parse_mdns_query(&buf[..len]) {
                                let mut should_respond = false;
                                let svc_type = publisher_clone.service_type_fqdn().await;
                                let inst_fqdn = publisher_clone.service_instance_fqdn().await;

                                for q in questions {
                                    if q.name.eq_ignore_ascii_case(&svc_type)
                                        || q.name.eq_ignore_ascii_case(&inst_fqdn)
                                        || q.name.eq_ignore_ascii_case(DNS_SD_SERVICE_ENUMERATION)
                                    {
                                        should_respond = true;
                                        break;
                                    }
                                }

                                if should_respond {
                                    let records = publisher_clone.build_records(publisher_clone.config.read().await.ttl).await;
                                    let resp_packet = build_mdns_response(&records);
                                    let _ = socket_clone.send_to(&resp_packet, src_addr).await;
                                    let _ = socket_clone.send_to(&resp_packet, target_addr).await;
                                }
                            }
                        }
                    }
                }
            }
        });

        *self.worker_handle.write().await = Some(handle);
        Ok(())
    }

    pub async fn stop(&self) -> Result<(), MdnsError> {
        if !*self.is_running.read().await {
            return Ok(());
        }

        let _ = self.shutdown_tx.send(());

        if let Some(handle) = self.worker_handle.write().await.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
        }

        *self.is_running.write().await = false;
        Ok(())
    }
}
