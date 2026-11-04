use std::net::SocketAddr;

mod app;
mod flows;
mod http;
mod models;
mod scripts;
mod static_files;

#[cfg(target_os = "linux")]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

// Read by jemalloc before `main`: a background thread purges freed pages back to the OS
// after ~1s (dirty) and skips the MADV_FREE stage (muzzy), even when the server is idle —
// the memory brotli takes to compress a big file must not stay in RSS.
// tikv-jemalloc-sys builds prefixed by default, hence `_rjem_malloc_conf`.
#[cfg(target_os = "linux")]
#[allow(non_upper_case_globals)]
#[export_name = "_rjem_malloc_conf"]
pub static malloc_conf: Option<&'static u8> =
    Some(&b"background_thread:true,dirty_decay_ms:1000,muzzy_decay_ms:0\0"[0]);

#[tokio::main]
async fn main() {
    #[cfg(unix)]
    let unix_socket_addr = std::env::var("UNIX_SOCKET").ok();

    crate::http::start_up::setup_server(
        SocketAddr::new([0, 0, 0, 0].into(), 8000),
        #[cfg(unix)]
        unix_socket_addr,
    )
    .await;

    crate::app::APP_CTX.app_states.wait_until_shutdown().await;
}
