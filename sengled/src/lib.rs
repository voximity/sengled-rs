use std::time::Duration;

use reqwest::Response;
use rumqttc::{
    AsyncClient as MqttClient, ConnAck, ConnectReturnCode, Event as MqttEvent, Incoming,
    MqttOptions, SubscribeFilter, Transport,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use tokio::task::JoinHandle;
use url::Url;

mod device;
pub use device::*;
pub use rumqttc::QoS;

#[derive(Error, Debug)]
pub enum Error {
    #[error("request error: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("parse error: {0}")]
    Parse(#[from] url::ParseError),

    #[error("mqtt client error: {0}")]
    MqttClient(#[from] rumqttc::ClientError),

    #[error("already logged in")]
    LoggedIn,

    #[error("disconnected")]
    Disconnected,

    #[error("failed to connect to the MQTT server")]
    ConnectionFailure,
}

pub struct Client {
    http: reqwest::Client,
    username: String,
    password: String,
    preferred_qos: QoS,
    skip_server_check: bool,
    session: Option<String>,
    state: Option<ClientState>,
}

struct ClientState {
    mqtt: MqttClient,
    listener_handle: Option<JoinHandle<()>>,
}

pub enum Event {
    DeviceAttributesChanged {
        device: String,
        attributes: Vec<(String, String)>,
    },
}

impl Client {
    /// Create a new Sengled client with a given username and password.
    pub fn new(username: &str, password: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            username: String::from(username),
            password: String::from(password),
            preferred_qos: QoS::AtMostOnce,
            skip_server_check: false,
            state: None,
            session: None,
        }
    }

    // Set the preferred MQTT quality of service. Default is 0, at-most-once.
    pub fn with_preferred_qos(mut self, qos: QoS) -> Self {
        self.preferred_qos = qos;
        self
    }

    /// Skip the server check. Uses default MQTT server instead of one dynamically fetched.
    pub fn with_skip_server_check(mut self) -> Self {
        self.skip_server_check = true;
        self
    }

    async fn post<T: Serialize>(&self, url: &str, body: T) -> Result<Response, Error> {
        let mut request = self
            .http
            .post(url)
            .header("Content-Type", "application/json")
            .header("Host", "element.cloud.sengled.com:443")
            .header("Connection", "keep-alive");

        if let Some(session) = &self.session {
            request = request.header("Cookie", format!("JSESSIONID={}", session));
        }

        Ok(request.body(serde_json::to_string(&body)?).send().await?)
    }

    async fn post_with_session<T: Serialize>(
        &self,
        url: &str,
        session: &str,
        body: T,
    ) -> Result<Response, Error> {
        let request = self
            .http
            .post(url)
            .header("Content-Type", "application/json")
            .header("Host", "element.cloud.sengled.com:443")
            .header("Connection", "keep-alive")
            .header("Cookie", format!("JSESSIONID={}", session));

        Ok(request.body(serde_json::to_string(&body)?).send().await?)
    }

    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    pub fn set_session(&mut self, value: impl Into<String>) {
        self.session = Some(value.into());
    }

    pub async fn login(&mut self) -> Result<(), Error> {
        const ROUTE: &str =
            "https://ucenter.cloud.sengled.com/user/app/customer/v2/AuthenCross.json";

        if self.state.is_some() {
            return Err(Error::LoggedIn);
        }

        #[derive(Deserialize)]
        struct LoginResponse {
            #[serde(rename = "jsessionId")]
            session: String,
        }

        let data = self
            .post(
                ROUTE,
                json!({
                    "uuid": "xxxxxx",
                    "user": self.username,
                    "pwd": self.password,
                    "osType": "android",
                    "productCode": "life",
                    "appCode": "life",
                }),
            )
            .await?;

        self.session = Some(data.json::<LoginResponse>().await?.session);

        Ok(())
    }

    /// Start the client given a jsessionId.
    pub async fn start(&mut self) -> Result<EventHandler, Error> {
        let (state, handler) = self.create_client_state().await?;
        self.state = Some(state);
        Ok(handler)
    }

    async fn create_client_state(&mut self) -> Result<(ClientState, EventHandler), Error> {
        assert!(
            self.session.is_some(),
            "session has not been set! please use `login` or `set_session`"
        );

        const ROUTE: &str = "https://life2.cloud.sengled.com/life2/server/getServerInfo.json";
        const DEFAULT_SERVER_URL: &str = "wss://us-mqtt.cloud.sengled.com:443/mqtt";

        let session = self.session.as_ref().unwrap();

        let url = if self.skip_server_check {
            Url::parse(DEFAULT_SERVER_URL)?
        } else {
            #[derive(Deserialize)]
            struct ServerInfoResponse {
                #[serde(rename = "inceptionAddr")]
                addr: String,
            }

            let response = self
                .post_with_session(ROUTE, session, json!({}))
                .await?
                .json::<ServerInfoResponse>()
                .await?;

            println!("{}", response.addr);

            Url::parse(&response.addr)?
        };

        let mut mqtt_options = MqttOptions::new(
            format!("{}@lifeApp", session.to_owned()),
            format!("wss://{}{}", url.host_str().unwrap(), url.path()),
            url.port().unwrap_or(443),
        );

        let modifier_session = session.to_owned();
        mqtt_options
            .set_transport(Transport::wss_with_default_config())
            .set_keep_alive(Duration::from_secs(30))
            .set_request_modifier(move |mut request| {
                let modifier_session = modifier_session.to_owned();

                async move {
                    let headers = request.headers_mut();
                    headers.insert(
                        "Cookie",
                        format!("JSESSIONID={}", modifier_session).parse().unwrap(),
                    );
                    headers.insert("X-Requested-With", "com.sengled.life2".parse().unwrap());

                    request
                }
            });

        let (client, mut events) = MqttClient::new(mqtt_options, 10);

        match events.poll().await {
            Ok(MqttEvent::Incoming(Incoming::ConnAck(ConnAck {
                code: ConnectReturnCode::Success,
                ..
            }))) => (),
            _ => return Err(Error::ConnectionFailure),
        }

        Ok((
            ClientState {
                mqtt: client,
                listener_handle: None,
            },
            EventHandler { events },
        ))
    }

    /// Get a list of WIFI devices registered to the account.
    pub async fn wifi_devices(&self) -> Result<Vec<Device>, Error> {
        assert!(self.state.is_some(), "not logged in");

        const ROUTE: &str = "https://life2.cloud.sengled.com/life2/device/list.json";

        #[derive(Deserialize)]
        struct DevicesResponse {
            #[serde(rename = "deviceList")]
            devices: Vec<Device>,
        }

        Ok(self
            .post(ROUTE, json!({}))
            .await?
            .json::<DevicesResponse>()
            .await?
            .devices)
    }

    /// Subscribe to WIFI device events after fetching them. Returns a list of devices.
    pub async fn get_wifi_devices_and_subscribe(&self) -> Result<Vec<Device>, Error> {
        assert!(self.state.is_some(), "not logged in");

        let devices = self.wifi_devices().await?;
        self.subscribe_devices(&devices).await?;
        Ok(devices)
    }

    /// Subscribe the event listener to a single device.
    pub async fn subscribe_device(&self, device: impl AsDeviceMac) -> Result<(), Error> {
        assert!(self.state.is_some(), "not logged in");

        self.state
            .as_ref()
            .unwrap()
            .mqtt
            .subscribe(
                format!("wifielement/{}/status", device.as_device_mac()),
                QoS::AtMostOnce,
            )
            .await?;

        Ok(())
    }

    /// Subscribe the event listener to many devices.
    pub async fn subscribe_devices(&self, devices: &[impl AsDeviceMac]) -> Result<(), Error> {
        assert!(self.state.is_some(), "not logged in");

        self.state
            .as_ref()
            .unwrap()
            .mqtt
            .subscribe_many(devices.iter().map(|device| SubscribeFilter {
                path: format!("wifielement/{}/status", device.as_device_mac()),
                qos: QoS::AtMostOnce,
            }))
            .await?;

        Ok(())
    }

    /// Set an attribute on a device.
    pub async fn set_device_attribute(
        &self,
        device: impl AsDeviceMac,
        attribute: &str,
        value: &str,
    ) -> Result<(), Error> {
        assert!(self.state.is_some(), "not logged in");

        let body = json!({
            "dn": device.as_device_mac(),
            "type": attribute,
            "value": value,
            "time": chrono::Utc::now().timestamp_millis(),
        });

        self.state
            .as_ref()
            .unwrap()
            .mqtt
            .publish(
                format!("wifielement/{}/update", device.as_device_mac()),
                self.preferred_qos,
                false,
                serde_json::to_string(&body)?,
            )
            .await?;

        Ok(())
    }

    /// Set attributes on a device.
    pub async fn set_device_attributes(
        &self,
        device: impl AsDeviceMac,
        attributes: &[(impl AsRef<str>, impl AsRef<str>)],
    ) -> Result<(), Error> {
        assert!(self.state.is_some(), "not logged in");

        let mut body = vec![];
        for (key, value) in attributes.iter() {
            body.push(json!({
                "dn": device.as_device_mac(),
                "type": key.as_ref(),
                "value": value.as_ref(),
                "time": chrono::Utc::now().timestamp_millis(),
            }));
        }

        self.state
            .as_ref()
            .unwrap()
            .mqtt
            .publish(
                format!("wifielement/{}/update", device.as_device_mac()),
                self.preferred_qos,
                false,
                serde_json::to_string(&body)?,
            )
            .await?;

        Ok(())
    }

    /// Close the client, sending any remaining MQTT messages.
    pub async fn close(mut self) -> Result<(), Error> {
        if let Some(ClientState {
            listener_handle,
            mqtt,
            ..
        }) = self.state.take()
        {
            mqtt.disconnect().await?;
            if let Some(listener_handle) = listener_handle {
                let _ = listener_handle.await;
            }
        }

        Ok(())
    }
}

#[must_use = "either start the basic listener with `spawn_listener` or manually poll events for the API to function"]
pub struct EventHandler {
    events: rumqttc::EventLoop,
}

impl EventHandler {
    /// Spawn a basic listener thread that keeps the API moving forward.
    /// Use this when you do not need to receive events from the Sengled API, such as when
    /// you are just sending a few messages to the API.
    pub fn spawn_listener(mut self, client: &mut Client) {
        if let Some(ref mut state) = client.state {
            state.listener_handle = Some(tokio::spawn(async move {
                while let Ok(_event) = self.events.poll().await {
                    // ...
                }
            }))
        }
    }

    pub async fn poll(&mut self) -> Result<Event, Error> {
        loop {
            match self.events.poll().await {
                Ok(MqttEvent::Incoming(Incoming::Publish(packet))) => {
                    let status_regex = regex_macro::regex!("^wifielement/([0-9A-F:]+)/status$");
                    let status_captures = match status_regex.captures(&packet.topic) {
                        Some(captures) => captures,
                        None => continue,
                    };

                    let mac = &status_captures[1];

                    #[derive(Deserialize)]
                    struct AttributesChangedPayload {
                        #[serde(rename = "type")]
                        name: String,
                        value: String,
                    }

                    let attributes: Vec<AttributesChangedPayload> =
                        serde_json::from_slice(&packet.payload).unwrap();

                    return Ok(Event::DeviceAttributesChanged {
                        device: String::from(mac),
                        attributes: attributes
                            .into_iter()
                            .map(|AttributesChangedPayload { name, value }| (name, value))
                            .collect::<Vec<_>>(),
                    });
                }
                Ok(MqttEvent::Incoming(Incoming::Disconnect)) => return Err(Error::Disconnected),
                Err(_) => return Err(Error::Disconnected),
                Ok(_) => (),
            }
        }
    }
}
