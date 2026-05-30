//! Thread, time, network, signal, and rand runtime symbols.

// std::thread + std::time + std::sync surface.
pub(in crate::codegen::runtime) const SYNC_TIME: &[&str] = &[
    "ruxen_thread_sleep_ns",
    "ruxen_thread_yield",
    // std::time (Phase 3): monotonic + realtime clocks, nanoseconds.
    "ruxen_time_now_ns",
    "ruxen_time_unix_ns",
    // Phase 2 stdlib (#06.5 T4): Duration / Instant scalar-wrapper classes.
    "ruxen_duration_from_secs",
    "ruxen_duration_from_millis",
    "ruxen_duration_from_micros",
    "ruxen_duration_from_nanos",
    "ruxen_duration_as_secs",
    "ruxen_duration_as_millis",
    "ruxen_duration_as_micros",
    "ruxen_duration_as_nanos",
    "ruxen_duration_add",
    "ruxen_duration_sub",
    "ruxen_instant_now",
    "ruxen_instant_elapsed",
    "ruxen_instant_duration_since",
    "ruxen_instant_sub",
    "ruxen_thread_sleep_duration",
];

// std::net (Phase 2 #06.5 T5): class-only surface + flat helpers.
pub(in crate::codegen::runtime) const NET: &[&str] = &[
    "ruxen_tcp_connect",
    "ruxen_tcp_listen",
    "ruxen_tcp_accept",
    "ruxen_tcp_read",
    "ruxen_tcp_write",
    "ruxen_tcp_close",
    // Class surface (#06.5 T5) — TcpListener / TcpStream wrappers.
    "ruxen_tcp_listener_bind",
    "ruxen_tcp_listener_accept",
    "ruxen_tcp_listener_local_addr",
    "ruxen_tcp_listener_set_nonblocking",
    "ruxen_tcp_listener_close",
    "ruxen_tcp_listener_drop",
    "ruxen_tcp_stream_connect",
    "ruxen_tcp_stream_read",
    "ruxen_tcp_stream_write",
    "ruxen_tcp_stream_peer_addr",
    "ruxen_tcp_stream_shutdown",
    "ruxen_tcp_stream_close",
    "ruxen_tcp_stream_drop",
    // #06.5 T5 additions: binary-safe read + socket timeouts.
    "ruxen_tcp_read_bytes",
    "ruxen_tcp_set_read_timeout_ns",
    "ruxen_tcp_set_write_timeout_ns",
    "ruxen_tcp_stream_set_read_timeout",
    "ruxen_tcp_stream_set_write_timeout",
];

// std::signal + std::rand surface.
pub(in crate::codegen::runtime) const SIGNAL_RAND: &[&str] = &[
    // std::signal (graceful-shutdown surface).
    "ruxen_signal_install_sigint",
    "ruxen_signal_received_sigint",
    // std::rand (Phase 2 #06.5 T8): kernel CSPRNG-backed entropy.
    "ruxen_rand_random_bytes",
    "ruxen_rand_random_u64",
    "ruxen_rand_random_fill",
];
