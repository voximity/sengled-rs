use std::time::Duration;

use reqwest::Response;
use rumqttc::{
    AsyncClient as MqttClient, ConnAck, ConnectReturnCode, Event, Incoming, MqttOptions, Transport,
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
}

pub struct Client {
    http: reqwest::Client,
    username: String,
    password: String,
    preferred_qos: QoS,
    skip_server_check: bool,
    state: Option<ClientState>,
}

struct ClientState {
    session: String,
    mqtt: MqttClient,
    listener_handle: JoinHandle<()>,
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

        if let Some(session) = &self.state.as_ref().map(|s| &s.session) {
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

    /// Login by getting the jsessionId, then starting the client.
    pub async fn login_and_start(&mut self) -> Result<String, Error> {
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

        let session = data.json::<LoginResponse>().await?.session;
        self.start(session.to_owned()).await?;

        Ok(session)
    }

    /// Start the client given a jsessionId.
    pub async fn start(&mut self, session: String) -> Result<(), Error> {
        self.state = Some(self.create_client_state(session).await?);
        Ok(())
    }

    /// Fetch the client's jsessionId.
    pub fn session(&self) -> Option<&str> {
        self.state.as_ref().map(|state| state.session.as_str())
    }

    async fn create_client_state(&mut self, session: String) -> Result<ClientState, Error> {
        const ROUTE: &str = "https://life2.cloud.sengled.com/life2/server/getServerInfo.json";
        const DEFAULT_SERVER_URL: &str = "wss://us-mqtt.cloud.sengled.com:443/mqtt";

        let url = if self.skip_server_check {
            Url::parse(DEFAULT_SERVER_URL)?
        } else {
            #[derive(Deserialize)]
            struct ServerInfoResponse {
                #[serde(rename = "inceptionAddr")]
                addr: String,
            }

            let response = self
                .post_with_session(ROUTE, &session, json!({}))
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
        let listener_handle = tokio::spawn(async move {
            match events.poll().await {
                Ok(Event::Incoming(Incoming::ConnAck(ConnAck {
                    code: ConnectReturnCode::Success,
                    ..
                }))) => (),

                _ => panic!("failed to connect in listener task"),
            }

            while events.poll().await.is_ok() {
                // ...
            }
        });

        Ok(ClientState {
            session,
            mqtt: client,
            listener_handle,
        })
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

    /// Close the client, sending any remaining MQTT messages.
    pub async fn close(mut self) -> Result<(), Error> {
        if let Some(ClientState {
            listener_handle,
            mqtt,
            ..
        }) = self.state.take()
        {
            mqtt.disconnect().await?;
            let _ = listener_handle.await;
        }

        Ok(())
    }
}
