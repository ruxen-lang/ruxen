//! Thread, time, network, signal, and rand runtime symbols.

// std::thread + std::time + std::sync surface.
pub(in crate::codegen::runtime) const SYNC_TIME: &[&str] = &[
    "riven_thread_sleep_ns",
    "riven_thread_yield",
    // std::time (Phase 3): monotonic + realtime clocks, nanoseconds.
    "riven_time_now_ns",
    "riven_time_unix_ns",
    // Phase 2 stdlib (#06.5 T4): Duration / Instant scalar-wrapper classes.
    "riven_duration_from_secs",
    "riven_duration_from_millis",
    "riven_duration_from_micros",
    "riven_duration_from_nanos",
    "riven_duration_as_secs",
    "riven_duration_as_millis",
    "riven_duration_as_micros",
    "riven_duration_as_nanos",
    "riven_duration_add",
    "riven_duration_sub",
    "riven_instant_now",
    "riven_instant_elapsed",
    "riven_instant_duration_since",
    "riven_instant_sub",
    "riven_thread_sleep_duration",
];

// std::net (Phase 2 #06.5 T5): class-only surface + flat helpers.
pub(in crate::codegen::runtime) const NET: &[&str] = &[
    "riven_tcp_connect",
    "riven_tcp_listen",
    "riven_tcp_accept",
    "riven_tcp_read",
    "riven_tcp_write",
    "riven_tcp_close",
    // Class surface (#06.5 T5) — TcpListener / TcpStream wrappers.
    "riven_tcp_listener_bind",
    "riven_tcp_listener_accept",
    "riven_tcp_listener_local_addr",
    "riven_tcp_listener_set_nonblocking",
    "riven_tcp_listener_close",
    "riven_tcp_listener_drop",
    "riven_tcp_stream_connect",
    "riven_tcp_stream_read",
    "riven_tcp_stream_write",
    "riven_tcp_stream_peer_addr",
    "riven_tcp_stream_shutdown",
    "riven_tcp_stream_close",
    "riven_tcp_stream_drop",
    // #06.5 T5 additions: binary-safe read + socket timeouts.
    "riven_tcp_read_bytes",
    "riven_tcp_set_read_timeout_ns",
    "riven_tcp_set_write_timeout_ns",
    "riven_tcp_stream_set_read_timeout",
    "riven_tcp_stream_set_write_timeout",
];

// std::signal + std::rand surface.
pub(in crate::codegen::runtime) const SIGNAL_RAND: &[&str] = &[
    // std::signal (graceful-shutdown surface).
    "riven_signal_install_sigint",
    "riven_signal_received_sigint",
    // std::rand (Phase 2 #06.5 T8): kernel CSPRNG-backed entropy.
    "riven_rand_random_bytes",
    "riven_rand_random_u64",
    "riven_rand_random_fill",
];
