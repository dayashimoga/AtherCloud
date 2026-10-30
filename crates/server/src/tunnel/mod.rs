pub mod binary_manager;
pub mod edge_router;
pub mod heartbeat;
pub mod mdns;
pub mod pairing;
pub mod qr;
pub mod status_api;
pub mod supervisor;
pub mod url_parser;

pub use binary_manager::{
    BinaryError, BinaryManagerConfig, BinarySource, CloudflaredBinaryInfo, CloudflaredBinaryManager,
    PlatformArch,
};
pub use edge_router::{
    edge_routing_middleware, is_loopback_ip, is_private_lan_ip, ClientNetworkContext,
    EdgeRoutingClassifier, EdgeRoutingConfig, IngressRouteDecision, IngressType,
};
pub use heartbeat::{
    HeartbeatError, HeartbeatEvent, TunnelHealthState, TunnelHeartbeatConfig,
    TunnelHeartbeatMetrics, TunnelHeartbeatWorker,
};
pub use mdns::{
    build_mdns_response, decode_dns_name, detect_local_ip, encode_dns_name, parse_mdns_query,
    DnsQuestion, DnsRecord, MdnsError, MdnsPublisher, MdnsServiceConfig, MDNS_IPV4_MULTICAST,
    MDNS_PORT,
};
pub use pairing::{
    tunnel_pairing_router, CreateInvitationRequest, CreateInvitationResponse, DevicePairingAuthority,
    DevicePairingRequest, DevicePairingResponse, PairedDevice, PairingError, PasskeyRecord,
};
pub use qr::{
    format_boot_qr_banner, generate_ascii_qr, generate_png_qr, generate_svg_qr, tunnel_qr_handler,
    tunnel_qr_router, QrError, QrJsonResponse, QrQueryParams,
};
pub use status_api::{
    tunnel_status_handler, tunnel_status_router, TunnelApiState, TunnelStatusResponse,
};
pub use supervisor::{
    CloudflaredSupervisor, TunnelEvent, TunnelState, TunnelSupervisorConfig, TunnelSupervisorError,
    TunnelSupervisorStatus,
};
pub use url_parser::{
    extract_custom_tunnel_url, extract_tunnel_url, is_valid_subdomain, probe_url_reachability,
    strip_ansi,
};
