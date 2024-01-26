use std::{fs, process, sync::Arc};

use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
    routing::{get, post},
};
use dashmap::DashMap;
use sengled::Event;
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;

mod routes;

#[derive(Deserialize, Serialize)]
pub struct Config {
    username: String,
    password: String,
    port: u16,
    require_auth: bool,
    auth_key: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            username: "".into(),
            password: "".into(),
            port: 5005,
            require_auth: true,
            auth_key: Some(String::from("key")),
        }
    }
}

struct AppState {
    config: Config,
    client: sengled::Client,
    devices: DashMap<String, sengled::Device>,
}

async fn authorization_middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    if !state.config.require_auth {
        return next.run(request).await;
    }

    let auth = match request.headers().get("Authorization") {
        Some(auth) => auth,
        None => {
            return Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(Body::empty())
                .unwrap();
        }
    };

    if auth.to_str().unwrap_or("") != state.config.auth_key.as_deref().unwrap_or_default() {
        return Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(Body::empty())
            .unwrap();
    }

    next.run(request).await
}

#[tokio::main]
async fn main() {
    // parse config
    let config: Config = if let Ok(config) = fs::read_to_string("config.yml") {
        serde_yaml::from_str(&config).expect("failed to deserialize config.yml")
    } else {
        fs::write(
            "config.yml",
            serde_yaml::to_string(&Config::default()).unwrap(),
        )
        .unwrap();
        eprintln!("no config file found! wrote the default out for you...");
        process::exit(1);
    };

    // get session ID if present
    let session = fs::read_to_string("session").ok();

    // set up the client
    let mut client = sengled::Client::new(&config.username, &config.password)
        .with_skip_server_check()
        .with_preferred_qos(sengled::QoS::AtMostOnce);

    if let Some(session) = session {
        client.set_session(session);
    } else {
        client.login().await.expect("failed to login");
    }

    let mut event_handler = client.start().await.expect("failed to start client");

    let port = config.port;
    let state = Arc::new(AppState {
        config,
        client,
        devices: DashMap::new(),
    });

    // device cache and subscription handler
    for device in state.client.get_wifi_devices_and_subscribe().await.unwrap() {
        state.devices.insert(device.mac.to_owned(), device);
    }

    let listener_state = Arc::clone(&state);
    tokio::spawn(async move {
        while let Ok(event) = event_handler.poll().await {
            match event {
                Event::DeviceAttributesChanged { device, attributes } => {
                    for (key, value) in attributes {
                        let mut device = match listener_state.devices.get_mut(&device) {
                            Some(device) => device,
                            None => continue,
                        };

                        device.attributes.insert(key, value);
                    }
                }
            }
        }
    });

    // set up webapp
    let app = axum::Router::new()
        .route("/devices", get(routes::get_devices))
        .route("/devices", post(routes::set_device_attributes))
        .route("/devices/:id", get(routes::get_device))
        .route("/devices/:id/toggle", post(routes::toggle_device))
        .route_layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            authorization_middleware,
        ))
        .layer(CorsLayer::permissive())
        .with_state(Arc::clone(&state));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();

    axum::serve(listener, app).await.unwrap();
}
