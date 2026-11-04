use std::{net::SocketAddr, sync::Arc};

use is_alive_middleware::IsAliveMiddleware;
use my_http_server::MyHttpServer;

use crate::{
    models::{DESKTOP_FOLDER, MOBILE_FOLDER},
    static_files::{compile_copies_folders, CompressionAlgorithm, ContentFiles, StaticFilesReader},
};

pub async fn setup_server(
    tcp_listen_addr: SocketAddr,
    #[cfg(unix)] unix_socket_path: Option<String>,
) {
    let mut http_server = MyHttpServer::new(tcp_listen_addr);

    #[cfg(unix)]
    let mut unix_server = unix_socket_path.map(MyHttpServer::new_as_unix_socket);

    let is_alive_middleware = Arc::new(IsAliveMiddleware::new(
        crate::app::APP_CTX.app_name.as_str(),
        crate::app::APP_CTX.app_version.as_str(),
    ));

    #[cfg(unix)]
    if let Some(unix_server) = unix_server.as_mut() {
        unix_server.add_middleware(is_alive_middleware.clone());
    }

    http_server.add_middleware(is_alive_middleware);

    // robots.txt is served before the Basic authentication - so the crawlers can read it
    if crate::app::APP_CTX.no_robots {
        println!("NO_ROBOTS=true — robots.txt which forbids the crawling is served");

        let no_robots_middleware = Arc::new(super::NoRobotsMiddleware);

        #[cfg(unix)]
        if let Some(unix_server) = unix_server.as_mut() {
            unix_server.add_middleware(no_robots_middleware.clone());
        }

        http_server.add_middleware(no_robots_middleware);
    }

    if let Some(password) = crate::app::APP_CTX.basic_auth_password.as_ref() {
        println!("BASIC-AUTH is set — only the requests authenticated with the Basic authentication are served");

        let basic_auth_middleware = Arc::new(super::BasicAuthMiddleware::new(password.clone()));

        #[cfg(unix)]
        if let Some(unix_server) = unix_server.as_mut() {
            unix_server.add_middleware(basic_auth_middleware.clone());
        }

        http_server.add_middleware(basic_auth_middleware);
    }

    if crate::app::APP_CTX.with_mobile {
        println!("WITH_MOBILE=1 — serving content from the desktop and the mobile folders");
    }

    let static_files =
        StaticFilesReader::new(create_content_files().await, get_disable_cache_list().await);

    let request_flow_middleware = Arc::new(super::RequestFlowMiddleware::new(static_files));

    #[cfg(unix)]
    if let Some(unix_server) = unix_server.as_mut() {
        unix_server.add_middleware(request_flow_middleware.clone());
    }

    http_server.add_middleware(request_flow_middleware);

    http_server.start_auto(
        crate::app::APP_CTX.app_states.clone(),
        my_logger::LOGGER.clone(),
    );

    #[cfg(unix)]
    if let Some(unix_server) = unix_server.as_mut() {
        unix_server.start_auto(
            crate::app::APP_CTX.app_states.clone(),
            my_logger::LOGGER.clone(),
        );
    }
}

/// Scans the content and creates its compressed copies in the home folder. They are compressed
/// with the maximum level - it takes time, so the server is started when they are ready
async fn create_content_files() -> ContentFiles {
    let copies_folders = match get_home_folder() {
        Some(home) => compile_copies_folders(home.as_str()),
        None => {
            println!("HOME is not set — compressed copies of the content are not created, files are served uncompressed");
            vec![]
        }
    };

    for folder in copies_folders.iter() {
        println!(
            "Creating the {} copies of the content in '{}'",
            folder.algorithm.as_str(),
            folder.path
        );
    }

    let started = std::time::Instant::now();

    let content_files = tokio::task::spawn_blocking(move || {
        // in WITH_MOBILE mode only the device folders are served - each with its own copies
        let site_folders: &[&str] = if crate::app::APP_CTX.with_mobile {
            &[DESKTOP_FOLDER, MOBILE_FOLDER]
        } else {
            &[""]
        };

        ContentFiles::create(
            crate::app::APP_CTX.www_root.as_str(),
            site_folders,
            copies_folders.as_slice(),
        )
    })
    .await
    .expect("Scanning of the content has panicked");

    let copies: Vec<String> = CompressionAlgorithm::ALL
        .into_iter()
        .map(|algorithm| {
            format!(
                "{} {} copies",
                content_files.copies_count(algorithm),
                algorithm.as_str()
            )
        })
        .collect();

    println!(
        "Content of '{}' is ready in {:.1?}: {} files, {}",
        crate::app::APP_CTX.www_root,
        started.elapsed(),
        content_files.files_count(),
        copies.join(", ")
    );

    content_files
}

fn get_home_folder() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let home = home.trim();

    if home.is_empty() {
        return None;
    }

    Some(home.to_string())
}

async fn get_disable_cache_list() -> Vec<String> {
    const FILE_NAME: &str = "./www-system/.disable-cache";
    let disabled_cache = tokio::fs::read_to_string(FILE_NAME).await;

    let result = match disabled_cache {
        Ok(value) => value,
        Err(_) => {
            println!("Can not find file '{FILE_NAME}'. No Disabled cache list is used");
            return vec![];
        }
    };

    let result: Vec<String> = result
        .split('\n')
        .map(|itm| itm.trim())
        .filter(|itm| !itm.is_empty())
        .map(|itm| itm.to_string())
        .collect();

    println!("Loaded {:?} no-cache paths", result);

    result
}
