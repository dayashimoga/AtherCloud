use crate::tunnel::mdns::detect_local_ip;
use axum::{
    extract::{ConnectInfo, Request},
    http::{HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IngressType {
    Loopback,
    Lan,
    Tunnel,
    Unknown,
}

impl IngressType {
    pub fn as_str(&self) -> &'static str {
        match self {
            IngressType::Loopback => "Loopback",
            IngressType::Lan => "Lan",
            IngressType::Tunnel => "Tunnel",
            IngressType::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientNetworkContext {
    pub peer_ip: IpAddr,
    pub real_client_ip: IpAddr,
    pub ingress_type: IngressType,
    pub host: Option<String>,
    pub cf_ray: Option<String>,
    pub forwarded_proto: Option<String>,
    pub is_tls: bool,
}

impl ClientNetworkContext {
    pub fn is_loopback(&self) -> bool {
        self.ingress_type == IngressType::Loopback
    }

    pub fn is_lan(&self) -> bool {
        self.ingress_type == IngressType::Lan
    }

    pub fn is_tunnel(&self) -> bool {
        self.ingress_type == IngressType::Tunnel
    }

    pub fn is_trusted_local(&self) -> bool {
        matches!(self.ingress_type, IngressType::Loopback | IngressType::Lan)
    }
}

pub fn is_loopback_ip(ip: &IpAddr) -> bool {
    ip.is_loopback()
}

pub fn is_private_lan_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => {
            let octets = ipv4.octets();
            // 10.0.0.0/8
            if octets[0] == 10 {
                return true;
            }
            // 172.16.0.0/12 (172.16.0.0 - 172.31.255.255)
            if octets[0] == 172 && (16..=31).contains(&octets[1]) {
                return true;
            }
            // 192.168.0.0/16
            if octets[0] == 192 && octets[1] == 168 {
                return true;
            }
            // 169.254.0.0/16 (Link-local)
            if octets[0] == 169 && octets[1] == 254 {
                return true;
            }
            false
        }
        IpAddr::V6(ipv6) => {
            let segments = ipv6.segments();
            // fc00::/7 (Unique Local Address)
            if (segments[0] & 0xFE00) == 0xFC00 {
                return true;
            }
            // fe80::/10 (Link-Local)
            if (segments[0] & 0xFFC0) == 0xFE80 {
                return true;
            }
            false
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IngressRouteDecision {
    pub recommended_url: String,
    pub route_type: IngressType,
    pub is_direct_lan: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeRoutingConfig {
    pub lan_ip: Option<Ipv4Addr>,
    pub daemon_port: u16,
    pub block_tunnel_admin: bool,
    pub custom_tunnel_domain: Option<String>,
}

impl Default for EdgeRoutingConfig {
    fn default() -> Self {
        Self {
            lan_ip: None,
            daemon_port: 8080,
            block_tunnel_admin: true,
            custom_tunnel_domain: None,
        }
    }
}

#[derive(Clone)]
pub struct EdgeRoutingClassifier {
    config: Arc<RwLock<EdgeRoutingConfig>>,
    active_tunnel_url: Arc<RwLock<Option<String>>>,
}

impl EdgeRoutingClassifier {
    pub fn new(config: EdgeRoutingConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            active_tunnel_url: Arc::new(RwLock::new(None)),
        }
    }

    pub fn set_tunnel_url(&self, url: Option<String>) {
        *self.active_tunnel_url.write().unwrap() = url;
    }

    pub fn tunnel_url(&self) -> Option<String> {
        self.active_tunnel_url.read().unwrap().clone()
    }

    pub fn config(&self) -> EdgeRoutingConfig {
        self.config.read().unwrap().clone()
    }

    pub fn classify_headers(
        &self,
        headers: &axum::http::HeaderMap,
        peer_addr: Option<SocketAddr>,
    ) -> ClientNetworkContext {
        let peer_ip = peer_addr
            .map(|s| s.ip())
            .unwrap_or_else(|| IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));

        let host = headers
            .get("host")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.to_string());

        let cf_ray = headers
            .get("cf-ray")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.to_string());

        let forwarded_proto = headers
            .get("x-forwarded-proto")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.to_string());

        let is_tls = forwarded_proto
            .as_deref()
            .map(|p| p.eq_ignore_ascii_case("https"))
            .unwrap_or(false);

        let custom_domain = self.config.read().unwrap().custom_tunnel_domain.clone();

        // Check if request arrived via Cloudflare Tunnel
        let is_cf_tunnel = cf_ray.is_some()
            || host
                .as_deref()
                .map(|h| {
                    h.contains(".trycloudflare.com")
                        || custom_domain
                            .as_deref()
                            .map(|d| h.contains(d))
                            .unwrap_or(false)
                })
                .unwrap_or(false);

        if is_cf_tunnel {
            let real_client_ip = headers
                .get("cf-connecting-ip")
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.parse::<IpAddr>().ok())
                .or_else(|| {
                    headers
                        .get("x-forwarded-for")
                        .and_then(|h| h.to_str().ok())
                        .and_then(|s| s.split(',').next())
                        .map(|s| s.trim())
                        .and_then(|s| s.parse::<IpAddr>().ok())
                })
                .unwrap_or(peer_ip);

            return ClientNetworkContext {
                peer_ip,
                real_client_ip,
                ingress_type: IngressType::Tunnel,
                host,
                cf_ray,
                forwarded_proto,
                is_tls: true,
            };
        }

        // Loopback Check
        if is_loopback_ip(&peer_ip) {
            return ClientNetworkContext {
                peer_ip,
                real_client_ip: peer_ip,
                ingress_type: IngressType::Loopback,
                host,
                cf_ray,
                forwarded_proto,
                is_tls,
            };
        }

        // Local Subnet Check
        if is_private_lan_ip(&peer_ip) {
            return ClientNetworkContext {
                peer_ip,
                real_client_ip: peer_ip,
                ingress_type: IngressType::Lan,
                host,
                cf_ray,
                forwarded_proto,
                is_tls,
            };
        }

        // External / WAN
        ClientNetworkContext {
            peer_ip,
            real_client_ip: peer_ip,
            ingress_type: IngressType::Unknown,
            host,
            cf_ray,
            forwarded_proto,
            is_tls,
        }
    }

    pub fn classify_request<B>(
        &self,
        req: &axum::http::Request<B>,
        peer_addr: Option<SocketAddr>,
    ) -> ClientNetworkContext {
        self.classify_headers(req.headers(), peer_addr)
    }

    pub fn resolve_optimal_endpoint(&self, client_ip: IpAddr) -> IngressRouteDecision {
        let config = self.config.read().unwrap().clone();
        let lan_ip = config.lan_ip.unwrap_or_else(detect_local_ip);

        if is_loopback_ip(&client_ip) {
            return IngressRouteDecision {
                recommended_url: format!("http://127.0.0.1:{}", config.daemon_port),
                route_type: IngressType::Loopback,
                is_direct_lan: true,
                reason: "Host accessing daemon directly via loopback interface".to_string(),
            };
        }

        if is_private_lan_ip(&client_ip) {
            return IngressRouteDecision {
                recommended_url: format!("http://{}:{}", lan_ip, config.daemon_port),
                route_type: IngressType::Lan,
                is_direct_lan: true,
                reason: "Client is in local subnet; routing directly to LAN interface for gigabit throughput"
                    .to_string(),
            };
        }

        // Remote WAN
        if let Some(ref tunnel_url) = *self.active_tunnel_url.read().unwrap() {
            return IngressRouteDecision {
                recommended_url: tunnel_url.clone(),
                route_type: IngressType::Tunnel,
                is_direct_lan: false,
                reason: "External client routed through zero-cost Cloudflare tunnel".to_string(),
            };
        }

        IngressRouteDecision {
            recommended_url: format!("http://{}:{}", lan_ip, config.daemon_port),
            route_type: IngressType::Unknown,
            is_direct_lan: false,
            reason: "External client detected without active tunnel; fallback to direct WAN"
                .to_string(),
        }
    }
}

pub async fn edge_routing_middleware(
    classifier: Arc<EdgeRoutingClassifier>,
    mut req: Request,
    next: Next,
) -> Response {
    let peer_addr = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|ci| ci.0);
    let ctx = classifier.classify_request(&req, peer_addr);

    let config = classifier.config();
    let path = req.uri().path();

    // Security Boundary: Prevent remote tunnel ingress from invoking admin or internal routes
    if config.block_tunnel_admin
        && ctx.ingress_type == IngressType::Tunnel
        && (path.starts_with("/api/v1/admin") || path.starts_with("/api/v1/internal"))
    {
        return (
            StatusCode::FORBIDDEN,
            [("content-type", "application/problem+json")],
            r#"{"type":"https://aetherstream.dev/errors/forbidden-ingress","title":"Forbidden Ingress Channel","status":403,"detail":"Administrative routes are restricted to trusted local network and loopback access."}"#,
        )
            .into_response();
    }

    req.extensions_mut().insert(ctx.clone());

    let mut response = next.run(req).await;

    let headers = response.headers_mut();
    headers.insert(
        "X-AetherStream-Ingress",
        HeaderValue::from_static(ctx.ingress_type.as_str()),
    );
    if let Ok(ip_header) = HeaderValue::from_str(&ctx.real_client_ip.to_string()) {
        headers.insert("X-AetherStream-Client-IP", ip_header);
    }

    response
}
