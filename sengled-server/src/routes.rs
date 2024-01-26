use std::{collections::HashMap, sync::Arc};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::AppState;

#[derive(Deserialize)]
pub(crate) struct SetDeviceAttributes {
    devices: Vec<String>,
    attributes: HashMap<String, String>,
}

// set attributes on a single device
pub(crate) async fn set_device_attributes(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Vec<SetDeviceAttributes>>,
) -> StatusCode {
    for bulk in payload {
        for device in bulk.devices {
            if state
                .client
                .set_device_attributes(device, &bulk.attributes.iter().collect::<Vec<_>>())
                .await
                .is_err()
            {
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
        }
    }

    StatusCode::OK
}

pub(crate) async fn get_devices(State(state): State<Arc<AppState>>) -> Response {
    Json(
        state
            .devices
            .iter()
            .map(|pair| pair.value().to_owned())
            .collect::<Vec<_>>(),
    )
    .into_response()
}

pub(crate) async fn get_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let device = state.devices.get(&id);

    match device {
        Some(device) => Json(device.to_owned()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub(crate) async fn toggle_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let Some(device) = state.devices.get(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let Some(switch) = device.get_attribute("switch") else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    let new_switch = if switch == "0" { "1" } else { "0" };

    if state
        .client
        .set_device_attribute(&device.mac, "switch", new_switch)
        .await
        .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    Json(json!({ "value": new_switch })).into_response()
}
